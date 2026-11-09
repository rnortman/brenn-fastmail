//! `FastmailDraft`: a new email saved in the account's Drafts mailbox from
//! one of the account's sending identities, never sent. Three steps: the
//! session, which must name a submission account; a lookup of the sending
//! identities, the `drafts`-role mailbox and, for a reply, the original; then
//! the `Email/set` that creates the draft. Only the last writes.
//!
//! Every string the draft carries but `subject` and `body` is wire-safe (no
//! control character, `"` or `\`), so its width inside the nested call's
//! arguments is its byte length and a byte bound on it is a bound on the
//! call.

use brenn_provider_kit::{Args, Failure, Next, capped, cut, internal};
use fastmail_jmap::mail::DRAFT_LOOKUP_PROPERTIES;
use fastmail_jmap::{
    Address, CallId, Email, EmailCreate, EmailGet, EmailSet, Error as JmapError, Identity,
    IdentityGet, Ids, MailboxQuery, QueryResult, Request, Response,
};
use fastmail_provider::args::{ACCOUNT_ARG, is_token, token, wire_safe, wire_width};
use fastmail_provider::failure::{
    StepFailure, derived_str, invalid, jmap_failure, missing_derived, set_failure,
    write_method_failure,
};
use fastmail_provider::settings::{API_NAME, Settings};
use fastmail_provider::steps::{ACCOUNT_KEY, SESSION_STEP, api_step, read_session};
use serde_json::{Map, Value, json};

use super::{API_STEP, InFlight, one_line};

/// The draft's write: the `Email/set` that creates it.
pub(super) const DRAFT_SET_STEP: u8 = 3;

/// What a lost answer at the draft's write leaves the caller to know.
pub(super) const DRAFT_UNKNOWN_HINT: &str = "the draft may have been saved before its answer was \
     lost: look for it with FastmailSearch (`keyword: $draft`) before asking again, or the user \
     has two";

/// The most recipients in each of `to`, `cc` and `bcc`.
pub(super) const RECIPIENTS_MAX: usize = 10;
/// The most characters a recipient's or the sender's display name may hold.
pub(super) const RECIPIENT_NAME_MAX_CHARS: usize = 100;
/// The most bytes an address may hold: RFC 5321 section 4.5.3.1.3's 256-byte
/// path, less its angle brackets.
pub(super) const RECIPIENT_EMAIL_MAX_BYTES: usize = 254;
/// The most characters a draft's `subject` may hold.
pub(super) const SUBJECT_MAX_CHARS: usize = 500;
/// The most characters a draft's `body` may hold, as its schema states it.
pub(super) const BODY_MAX_CHARS: usize = 20_000;
/// The most bytes a draft's `body` may take inside the nested call's
/// arguments, after its two JSON escapings: a line break is three bytes
/// there, a quote or backslash four, a C0 control seven, and non-ASCII stays
/// UTF-8. With 30 recipients at about 690 bytes each (a 100-character name
/// of four-byte characters, a 254-byte address and the object's escaped
/// punctuation), a 500-character subject of quotes at 2 000, eleven
/// 255-byte threading ids at about 2 900, the From at about 700, and the
/// mailbox id, keywords, `accountId`, `account` and the call's own fields at
/// about 2 000, the widest draft is about 57 600 bytes, under the host's
/// 65 536.
pub(super) const BODY_MAX_WIRE_BYTES: usize = 30_000;
/// The most ids a reply's `References` carries: the newest.
pub(super) const DRAFT_REFERENCES_MAX: usize = 10;
/// The most bytes a threading id taken from the original may hold.
pub(super) const HEADER_ID_MAX_BYTES: usize = 255;
/// The most identities a refusal asking for `from` lists.
pub(super) const IDENTITIES_LISTED_MAX: usize = 20;

/// The role of the mailbox a draft is saved in (RFC 8621 section 2).
const DRAFTS_ROLE: &str = "drafts";
/// The creation id of the draft's one new email.
const DRAFT_CREATE_ID: &str = "draft";
/// The keywords a new draft carries: a draft, and not unread.
const DRAFT_KEYWORDS: [&str; 2] = ["$draft", "$seen"];

/// `derived` keys beside the mail account's.
const SUBMISSION_KEY: &str = "submission_account_id";
const MAILBOX_KEY: &str = "mailbox_id";
const FROM_KEY: &str = "from_address";
const TO_KEY: &str = "to";
const SUBJECT_KEY: &str = "subject";
const IN_REPLY_TO_KEY: &str = "in_reply_to";
const REFERENCES_KEY: &str = "references";
const THREAD_KEY: &str = "thread_id";

/// Where the operator looks when the account cannot name a sending identity.
const SUBMISSION_SETUP: &str = "the operator checks the token's session as the README's Fastmail \
     setup step 3 describes";

/// `FastmailDraft`'s arguments, validated.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DraftArgs {
    /// Empty only for a reply, which addresses the original's sender.
    to: Vec<Address>,
    cc: Vec<Address>,
    bcc: Vec<Address>,
    /// `None` only for a reply, which derives its subject from the
    /// original's.
    subject: Option<String>,
    body: String,
    reply_to_id: Option<String>,
    /// The sending identity's address, when the caller picks one.
    from: Option<String>,
}

impl DraftArgs {
    pub(super) const KEYS: &[&str] = &[
        "to",
        "cc",
        "bcc",
        "subject",
        "body",
        "reply_to_id",
        "from",
        ACCOUNT_ARG,
    ];

    pub(super) fn parse(map: &Args) -> Result<DraftArgs, String> {
        let to = recipients(map, "to")?;
        let cc = recipients(map, "cc")?;
        let bcc = recipients(map, "bcc")?;
        let subject = one_line(map, "subject", SUBJECT_MAX_CHARS)?;
        let body = body(map)?.ok_or("`body` is required")?;
        let reply_to_id = token(map, "reply_to_id")?;
        let from = match map.get("from") {
            None => None,
            Some(Value::String(from)) if is_address(from) => Some(from.clone()),
            Some(_) => {
                return Err(format!(
                    "`from` must be the address of one of the account's sending identities: \
                     {ADDRESS_RULE}"
                ));
            }
        };
        if reply_to_id.is_none() {
            if to.is_empty() {
                return Err("`to` is required unless `reply_to_id` is given".to_owned());
            }
            if subject.is_none() {
                return Err("`subject` is required unless `reply_to_id` is given".to_owned());
            }
        }
        Ok(DraftArgs {
            to,
            cc,
            bcc,
            subject,
            body,
            reply_to_id,
            from,
        })
    }
}

/// What an address admits, in the caller's terms.
const ADDRESS_RULE: &str = "a local part of 1 to 64 characters, `@`, and a domain of 1 to 253, \
     at most 254 bytes in all, with no whitespace, control character, `<`, `>`, `\"`, `\\`, `@` \
     or `,`";

/// What a recipient admits, in the caller's terms.
fn recipient_rule() -> String {
    format!(
        "`user@example.com` or `Name <user@example.com>`: a name of 1 to \
         {RECIPIENT_NAME_MAX_CHARS} characters with no control character, `<`, `>`, `\"` or \
         `\\`, and an address of {ADDRESS_RULE}"
    )
}

/// Whether `c` may appear in an address.
fn is_address_char(c: char) -> bool {
    !c.is_whitespace() && !c.is_control() && !"<>\"\\@,".contains(c)
}

/// Whether `text` is one address: a local part of 1 to 64 characters, `@`,
/// a domain of 1 to 253, each of [`is_address_char`], at most
/// `RECIPIENT_EMAIL_MAX_BYTES` in all.
fn is_address(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    text.len() <= RECIPIENT_EMAIL_MAX_BYTES
        && (1..=64).contains(&local.chars().count())
        && (1..=253).contains(&domain.chars().count())
        && local.chars().chain(domain.chars()).all(is_address_char)
}

/// Whether `name` is a display name a recipient may carry: 1 to
/// `RECIPIENT_NAME_MAX_CHARS` characters, no control character, `<`, `>`,
/// `"` or `\`.
fn is_display_name(name: &str) -> bool {
    (1..=RECIPIENT_NAME_MAX_CHARS).contains(&name.chars().count())
        && !name.chars().any(|c| c.is_control() || "<>\"\\".contains(c))
}

/// One recipient: `user@example.com`, or `Name <user@example.com>`.
fn recipient(text: &str) -> Option<Address> {
    if let Some(inner) = text.strip_suffix('>')
        && let Some((name, email)) = inner.split_once(" <")
    {
        return (is_display_name(name) && is_address(email)).then(|| Address {
            name: Some(name.to_owned()),
            email: email.to_owned(),
        });
    }
    is_address(text).then(|| Address {
        name: None,
        email: text.to_owned(),
    })
}

/// A recipient list at `key`: 1 to `RECIPIENTS_MAX` distinct recipients;
/// absent is empty.
fn recipients(map: &Args, key: &str) -> Result<Vec<Address>, String> {
    let Some(value) = map.get(key) else {
        return Ok(Vec::new());
    };
    let items = match value {
        Value::Array(items) if (1..=RECIPIENTS_MAX).contains(&items.len()) => items,
        _ => {
            return Err(format!(
                "`{key}` must be an array of 1 to {RECIPIENTS_MAX} recipients"
            ));
        }
    };
    let mut out: Vec<Address> = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let Some(address) = item.as_str().and_then(recipient) else {
            return Err(format!("each of `{key}` must be {}", recipient_rule()));
        };
        if items[..i].contains(item) {
            return Err(format!("`{key}` names {item} twice"));
        }
        out.push(address);
    }
    Ok(out)
}

/// A draft's `body`: as `capped` admits it at `BODY_MAX_CHARS`, and at most
/// `BODY_MAX_WIRE_BYTES` as sent.
fn body(map: &Args) -> Result<Option<String>, String> {
    let Some(body) = capped(map, "body", BODY_MAX_CHARS)? else {
        return Ok(None);
    };
    if wire_width(body) > BODY_MAX_WIRE_BYTES {
        return Err(format!(
            "`body` is too long as sent: at most {BODY_MAX_WIRE_BYTES} bytes of UTF-8 where a \
             line break counts as three and a quote or backslash as four"
        ));
    }
    Ok(Some(body.to_owned()))
}

/// The calls of the lookup step, by what each reads.
struct LookupCalls {
    identities: CallId,
    drafts: CallId,
    original: Option<CallId>,
}

/// The lookup: every sending identity, the `drafts`-role mailbox, and the
/// original a reply answers.
fn lookup_request(
    args: &DraftArgs,
    account_id: &str,
    submission_id: &str,
) -> (Request, LookupCalls) {
    let mut request = Request::mail_with_submission();
    let identities = IdentityGet {
        account_id: submission_id,
    }
    .push(&mut request);
    let drafts = MailboxQuery {
        account_id,
        role: DRAFTS_ROLE,
    }
    .push(&mut request);
    let original = args.reply_to_id.as_ref().map(|id| {
        EmailGet {
            account_id,
            ids: Ids::Listed(vec![id.clone()]),
            properties: DRAFT_LOOKUP_PROPERTIES,
            fetch_text_body_values: false,
            max_body_value_bytes: None,
        }
        .push(&mut request)
    });
    (
        request,
        LookupCalls {
            identities,
            drafts,
            original,
        },
    )
}

/// What the write step needs beyond the arguments, found by the lookup and
/// carried in `derived`.
#[derive(Clone, Debug, PartialEq)]
struct Found {
    mailbox_id: String,
    from: Address,
    /// The original's sender, when the caller gave no `to`.
    to: Option<Vec<Address>>,
    /// Derived from the original's, when the caller gave no `subject`.
    subject: Option<String>,
    in_reply_to: Option<String>,
    references: Vec<String>,
    /// The original's thread.
    thread_id: Option<String>,
}

impl Found {
    fn to_derived(&self, account_id: &str) -> Args {
        let mut derived = Args::new();
        derived.insert(ACCOUNT_KEY.into(), json!(account_id));
        derived.insert(MAILBOX_KEY.into(), json!(self.mailbox_id));
        derived.insert(FROM_KEY.into(), self.from.to_value());
        if let Some(to) = &self.to {
            let to: Vec<Value> = to.iter().map(Address::to_value).collect();
            derived.insert(TO_KEY.into(), Value::Array(to));
        }
        for (key, value) in [
            (SUBJECT_KEY, &self.subject),
            (IN_REPLY_TO_KEY, &self.in_reply_to),
            (THREAD_KEY, &self.thread_id),
        ] {
            if let Some(value) = value {
                derived.insert(key.into(), json!(value));
            }
        }
        if !self.references.is_empty() {
            derived.insert(REFERENCES_KEY.into(), json!(self.references));
        }
        derived
    }

    fn from_derived(state: &InFlight) -> Result<Found, Failure> {
        let missing = |key| missing_derived(state, key);
        let address = |value: &Value| Address::from_value(value).ok();
        let text = |key| match state.derived.get(key) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(missing(key)),
        };
        let to = match state.derived.get(TO_KEY) {
            None => None,
            Some(Value::Array(to)) => Some(
                to.iter()
                    .map(address)
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| missing(TO_KEY))?,
            ),
            Some(_) => return Err(missing(TO_KEY)),
        };
        let references = match state.derived.get(REFERENCES_KEY) {
            None => Vec::new(),
            Some(Value::Array(ids)) => ids
                .iter()
                .map(|id| id.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| missing(REFERENCES_KEY))?,
            Some(_) => return Err(missing(REFERENCES_KEY)),
        };
        Ok(Found {
            mailbox_id: derived_str(state, MAILBOX_KEY)?.to_owned(),
            from: state
                .derived
                .get(FROM_KEY)
                .and_then(address)
                .ok_or_else(|| missing(FROM_KEY))?,
            to,
            subject: text(SUBJECT_KEY)?,
            in_reply_to: text(IN_REPLY_TO_KEY)?,
            references,
            thread_id: text(THREAD_KEY)?,
        })
    }
}

/// The write: one `Email/set` creating the draft in the Drafts mailbox.
fn set_request(args: &DraftArgs, account_id: &str, found: &Found) -> (Request, CallId) {
    let mut request = Request::mail();
    let email = EmailCreate {
        mailbox_ids: vec![found.mailbox_id.clone()],
        keywords: DRAFT_KEYWORDS.map(str::to_owned).to_vec(),
        from: vec![found.from.clone()],
        to: found.to.clone().unwrap_or_else(|| args.to.clone()),
        cc: args.cc.clone(),
        bcc: args.bcc.clone(),
        reply_to: Vec::new(),
        subject: args.subject.clone().or_else(|| found.subject.clone()),
        in_reply_to: found.in_reply_to.iter().cloned().collect(),
        references: found.references.clone(),
        text: args.body.clone(),
    };
    let call = EmailSet {
        account_id,
        create: vec![(DRAFT_CREATE_ID.to_owned(), email)],
        update: Vec::new(),
    }
    .push(&mut request);
    (request, call)
}

/// One step of a draft. Fastmail's refusal of the draft is
/// `Ok(Next::Fail)`; an `Err` at the write says whether Fastmail refused the
/// method or left the draft's outcome unknown.
pub(super) fn step(
    state: &InFlight,
    body: Value,
    settings: &Settings,
    account: Option<&str>,
    args: &DraftArgs,
) -> Result<Next, StepFailure> {
    match state.step {
        SESSION_STEP => {
            let session = read_session(&body)?;
            let Some(submission_id) = session.submission_account_id else {
                return Err(internal(format!(
                    "this account's session does not advertise \
                     `urn:ietf:params:jmap:submission` with a primary account, so no sending \
                     identity can be read for the draft's From; {SUBMISSION_SETUP}"
                ))
                .into());
            };
            let (request, _) = lookup_request(args, &session.mail_account_id, &submission_id);
            let derived = Args::from_iter([
                (ACCOUNT_KEY.to_owned(), json!(session.mail_account_id)),
                (SUBMISSION_KEY.to_owned(), json!(submission_id)),
            ]);
            Ok(Next::Call {
                call: api_step(settings, account, API_STEP, &request),
                derived,
            })
        }
        API_STEP => {
            let account_id = derived_str(state, ACCOUNT_KEY)?;
            let found = looked_up(state, body, args)?;
            let (request, _) = set_request(args, account_id, &found);
            Ok(Next::Call {
                call: api_step(settings, account, DRAFT_SET_STEP, &request),
                derived: found.to_derived(account_id),
            })
        }
        _ => {
            let account_id = derived_str(state, ACCOUNT_KEY)?;
            let found = Found::from_derived(state)?;
            let response = Response::parse(body).map_err(jmap_failure)?;
            let (_, call) = set_request(args, account_id, &found);
            let set = response.result(&call).map_err(write_method_failure)?;
            Ok(created(&found, set)?)
        }
    }
}

/// The lookup's answer read into what the write needs.
fn looked_up(state: &InFlight, body: Value, args: &DraftArgs) -> Result<Found, Failure> {
    let account_id = derived_str(state, ACCOUNT_KEY)?;
    let submission_id = derived_str(state, SUBMISSION_KEY)?;
    let response = Response::parse(body).map_err(jmap_failure)?;
    let (_, calls) = lookup_request(args, account_id, submission_id);
    let identities = response
        .result(&calls.identities)
        .map_err(identity_failure)?;
    let identities = Identity::get(identities).map_err(jmap_failure)?.list;
    let from = sender(&identities, args.from.as_deref())?;
    let drafts = response.result(&calls.drafts).map_err(jmap_failure)?;
    let mailbox_id = drafts_mailbox(QueryResult::read(drafts).map_err(jmap_failure)?.ids)?;
    let mut found = Found {
        mailbox_id,
        from,
        to: None,
        subject: None,
        in_reply_to: None,
        references: Vec::new(),
        thread_id: None,
    };
    if let (Some(call), Some(id)) = (&calls.original, &args.reply_to_id) {
        let original = original(&response, call, id)?;
        if args.to.is_empty() {
            found.to = Some(original_sender(&original)?);
        }
        if args.subject.is_none() {
            found.subject = Some(reply_subject(original.subject.as_deref())?);
        }
        if let Some(message_id) = original.message_id.first().filter(|id| usable_id(id)) {
            let mut references: Vec<String> = original
                .references
                .iter()
                .filter(|id| usable_id(id))
                .cloned()
                .collect();
            references.push(message_id.clone());
            let newest = references.len().saturating_sub(DRAFT_REFERENCES_MAX);
            found.references = references.split_off(newest);
            found.in_reply_to = Some(message_id.clone());
        }
        found.thread_id = original.thread_id;
    }
    Ok(found)
}

/// The method errors on `Identity/get` that say the token cannot read the
/// account's identities.
const IDENTITY_ACCESS_ERRORS: &[&str] = &[
    "unknownMethod",
    "accountNotSupportedByMethod",
    "forbidden",
    "accountReadOnly",
];

/// An error on `Identity/get`: one saying the token cannot read identities
/// is the operator's to fix; every other as [`jmap_failure`].
fn identity_failure(error: JmapError) -> Failure {
    match &error {
        JmapError::Method { kind, .. } if IDENTITY_ACCESS_ERRORS.contains(&kind.as_str()) => {
            internal(format!(
                "{API_NAME}: {error}; no sending identity can be read for the draft's \
                 From; {SUBMISSION_SETUP}"
            ))
        }
        _ => jmap_failure(error),
    }
}

/// The From a draft is sent with, chosen among the candidates: the
/// identities whose address contains no `*`, so neither RFC 8621 section 6's
/// `*@<domain>` form nor any other address with `*` in it is ever sent from.
/// With `from`, the candidate whose address is `from` ignoring ASCII case;
/// without it, the only candidate. Where several remain, the one the account
/// cannot delete (RFC 8621 section 6), the user's main address; otherwise the
/// caller is refused, or, when they all share one address, told so; a `from`
/// containing `*` is refused saying so. The From
/// carries the identity's own display name, and an address the draft cannot
/// carry is Fastmail's.
fn sender(identities: &[Identity], from: Option<&str>) -> Result<Address, Failure> {
    let candidates: Vec<&Identity> = identities
        .iter()
        .filter(|identity| !identity.email.contains('*'))
        .collect();
    if candidates.is_empty() {
        let excluded = match identities.len() {
            0 => String::new(),
            n => format!(
                " that this tool sends from ({n} of its identities have an address containing `*`)"
            ),
        };
        return Err(internal(format!(
            "{API_NAME}'s Identity/get lists no sending identity for this account{excluded}, so \
             the draft has no From"
        )));
    }
    let identity = match from {
        Some(from) => {
            let matched: Vec<&Identity> = candidates
                .iter()
                .copied()
                .filter(|identity| identity.email.eq_ignore_ascii_case(from))
                .collect();
            match matched.as_slice() {
                [] => {
                    let reason = if from.contains('*') {
                        format!(
                            "`{from}` contains `*`, and this tool never sends from such an address"
                        )
                    } else {
                        format!("no sending identity is `{from}`")
                    };
                    return Err(invalid(format!(
                        "{reason}: give `from` as one of {}",
                        listed(&candidates)
                    )));
                }
                [one] => *one,
                several => {
                    main_identity(several).ok_or_else(|| shared_address(several.len(), from))?
                }
            }
        }
        None => match candidates.as_slice() {
            [one] => *one,
            several => main_identity(several).ok_or_else(|| {
                if distinct_addresses(several).len() == 1 {
                    shared_address(several.len(), &several[0].email)
                } else {
                    invalid(format!(
                        "the account has {} sending identities: give `from` as one of {}",
                        several.len(),
                        listed(several)
                    ))
                }
            })?,
        },
    };
    if !wire_safe(&identity.email) || identity.email.len() > RECIPIENT_EMAIL_MAX_BYTES {
        return Err(internal(format!(
            "the sending identity `{}` has an address the draft's From cannot carry: a control \
             character, `\"` or `\\`, or over {RECIPIENT_EMAIL_MAX_BYTES} bytes",
            identity.id
        )));
    }
    Ok(Address {
        name: kept_name(identity.name.as_deref()),
        email: identity.email.clone(),
    })
}

/// The one of `identities` the account cannot delete, if exactly one is.
fn main_identity<'a>(identities: &[&'a Identity]) -> Option<&'a Identity> {
    match identities
        .iter()
        .filter(|identity| identity.may_delete == Some(false))
        .collect::<Vec<_>>()
        .as_slice()
    {
        [main] => Some(**main),
        _ => None,
    }
}

/// The refusal for `count` identities sharing `address`, none of them main,
/// the address cut at `RECIPIENT_EMAIL_MAX_BYTES`.
fn shared_address(count: usize, address: &str) -> Failure {
    invalid(format!(
        "{count} sending identities share `{}` and none is the account's main one, so the draft \
         cannot choose its display name: the user removes the duplicate in {API_NAME}'s settings",
        cut(address, RECIPIENT_EMAIL_MAX_BYTES).0
    ))
}

/// The distinct addresses of `identities`, ignoring ASCII case, each as the
/// first identity carrying it spells it.
fn distinct_addresses<'a>(identities: &[&'a Identity]) -> Vec<&'a str> {
    let mut addresses: Vec<&str> = Vec::new();
    for identity in identities {
        if !addresses
            .iter()
            .any(|seen| seen.eq_ignore_ascii_case(&identity.email))
        {
            addresses.push(&identity.email);
        }
    }
    addresses
}

// TODO(correctness-mixed-identity-listing-offers-from-certain-to-fail): an
// address two or more candidates share, none of them main, is listed as a
// `from` to give, and giving it meets the shared-address refusal.
/// Up to `IDENTITIES_LISTED_MAX` distinct addresses of `identities`, ignoring
/// ASCII case, as `from` takes them, each cut at `RECIPIENT_EMAIL_MAX_BYTES`.
fn listed(identities: &[&Identity]) -> String {
    let addresses = distinct_addresses(identities);
    let more = addresses.len().saturating_sub(IDENTITIES_LISTED_MAX);
    let mut shown: Vec<String> = addresses
        .iter()
        .take(IDENTITIES_LISTED_MAX)
        .map(|address| cut(address, RECIPIENT_EMAIL_MAX_BYTES).0)
        .collect();
    if more > 0 {
        shown.push(format!("and {more} more"));
    }
    shown.join(", ")
}

/// A display name taken from Fastmail: kept when wire-safe and not empty, cut
/// at `RECIPIENT_NAME_MAX_CHARS`.
fn kept_name(name: Option<&str>) -> Option<String> {
    name.filter(|name| !name.is_empty() && wire_safe(name))
        .map(|name| name.chars().take(RECIPIENT_NAME_MAX_CHARS).collect())
}

/// The one mailbox whose role is `drafts`.
fn drafts_mailbox(ids: Vec<String>) -> Result<String, Failure> {
    match <[String; 1]>::try_from(ids) {
        Ok([id]) if is_token(&id) => Ok(id),
        Ok([id]) => Err(internal(format!(
            "{API_NAME}'s Mailbox/query answered a drafts mailbox id that is not a JMAP id: {}",
            cut(&id, HEADER_ID_MAX_BYTES).0
        ))),
        Err(ids) if ids.is_empty() => Err(internal(format!(
            "the account has no mailbox with role `{DRAFTS_ROLE}`, so the draft has nowhere to go"
        ))),
        Err(ids) => Err(internal(format!(
            "{API_NAME}'s Mailbox/query answered {} mailboxes with role `{DRAFTS_ROLE}`, which \
             RFC 8621 gives to at most one",
            ids.len()
        ))),
    }
}

/// The email a reply answers, or the caller's error when it does not exist.
fn original(response: &Response, call: &CallId, id: &str) -> Result<Email, Failure> {
    let got = Email::get(response.result(call).map_err(jmap_failure)?).map_err(jmap_failure)?;
    if let Some(email) = got.list.into_iter().find(|email| email.id == id) {
        return Ok(email);
    }
    if got.not_found.iter().any(|missing| missing == id) {
        return Err(invalid(format!("no email with id `{id}`")));
    }
    Err(internal(format!(
        "{API_NAME}'s Email/get answered neither email `{id}` nor its absence"
    )))
}

/// Whom a reply goes to: the original's `replyTo` when it names anyone, else
/// its `from`. An address the draft cannot carry is the caller's to replace.
fn original_sender(original: &Email) -> Result<Vec<Address>, Failure> {
    let senders = if original.reply_to.is_empty() {
        &original.from
    } else {
        &original.reply_to
    };
    let give_to = |why: &str| invalid(format!("the original's sender {why}: give `to`"));
    if senders.is_empty() {
        return Err(give_to("is not named"));
    }
    if senders.len() > RECIPIENTS_MAX {
        return Err(give_to(&format!(
            "names more than {RECIPIENTS_MAX} addresses"
        )));
    }
    senders
        .iter()
        .map(|sender| {
            if sender.email.is_empty()
                || !wire_safe(&sender.email)
                || sender.email.len() > RECIPIENT_EMAIL_MAX_BYTES
            {
                return Err(give_to(&format!(
                    "has an address the draft cannot carry: a control character, `\"` or `\\`, \
                     or over {RECIPIENT_EMAIL_MAX_BYTES} bytes"
                )));
            }
            Ok(Address {
                name: kept_name(sender.name.as_deref()),
                email: sender.email.clone(),
            })
        })
        .collect()
}

/// A reply's subject from its original's: every control character a space,
/// then `Re: ` before it unless it already starts with `re:` in any case
/// after leading whitespace; an absent or blank original gives `Re:`. Over
/// `SUBJECT_MAX_CHARS` is refused, the caller to give `subject`.
fn reply_subject(original: Option<&str>) -> Result<String, Failure> {
    let original: String = original
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let start = original.trim_start();
    let subject = if start.is_empty() {
        "Re:".to_owned()
    } else if start
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("re:"))
    {
        original
    } else {
        format!("Re: {original}")
    };
    if subject.chars().count() > SUBJECT_MAX_CHARS {
        return Err(invalid(format!(
            "the original's subject is longer than {SUBJECT_MAX_CHARS} characters as a reply's: \
             give `subject`"
        )));
    }
    Ok(subject)
}

/// Whether a threading id from the original may be written as is: not
/// empty, wire-safe, no whitespace, at most `HEADER_ID_MAX_BYTES`.
fn usable_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= HEADER_ID_MAX_BYTES
        && wire_safe(id)
        && !id.chars().any(char::is_whitespace)
}

/// The write's answer: the new draft's ids, where it went and from whom,
/// and what the tool chose for the caller. Nothing the caller wrote is
/// echoed.
fn created(found: &Found, set: &Value) -> Result<Next, Failure> {
    let result = EmailSet::result(set).map_err(jmap_failure)?;
    if let Some(error) = result.not_created.get(DRAFT_CREATE_ID) {
        return Ok(Next::Fail(set_failure("draft", DRAFT_CREATE_ID, error)));
    }
    let Some(draft) = result.created.get(DRAFT_CREATE_ID) else {
        return Err(internal(format!(
            "{API_NAME}'s Email/set answered neither the new draft nor its refusal"
        )));
    };
    let mut out = Map::new();
    out.insert("id".into(), json!(draft.id));
    if let Some(thread_id) = &draft.thread_id {
        out.insert("thread_id".into(), json!(thread_id));
        if found
            .thread_id
            .as_ref()
            .is_some_and(|original| original != thread_id)
        {
            out.insert("in_thread".into(), json!(false));
        }
    }
    out.insert("mailbox_id".into(), json!(found.mailbox_id));
    out.insert("from".into(), json!(found.from.display()));
    if let Some(to) = &found.to {
        let to: Vec<String> = to.iter().map(Address::display).collect();
        out.insert("to".into(), json!(to));
    }
    if let Some(subject) = &found.subject {
        out.insert("subject".into(), json!(subject));
    }
    if let Some(in_reply_to) = &found.in_reply_to {
        out.insert("in_reply_to".into(), json!(in_reply_to));
    }
    Ok(Next::Complete(Value::Object(out)))
}

#[cfg(test)]
mod tests {
    use brenn_provider_kit::testutil::{Answered, MemoryHost, failure};
    use brenn_provider_kit::{Delivered, ErrorKind, Input, NestedError};
    use fastmail_jmap::session::{CORE, MAIL, SUBMISSION};
    use fastmail_provider::failure::{FORBIDDEN_CHANGE, READ_ONLY_TOKEN};
    use fastmail_provider::testutil::{
        HOST_ARGS_CAP_BYTES, completed, jmap, session_body, settings,
    };

    use super::super::FastmailWrite;
    use super::super::tests::{admitted_args, answered, incoming, row, set_answer};
    use super::*;
    use crate::spec::Provided;

    const DRAFT: Provided = Provided::FastmailDraft;

    /// The draft's row at the lookup step, for `args`.
    fn at_lookup(args: Value) -> InFlight {
        row(
            DRAFT,
            args,
            API_STEP,
            json!({ ACCOUNT_KEY: "a1", SUBMISSION_KEY: "s1" }),
        )
    }

    fn identity(id: &str, email: &str, name: Option<&str>, may_delete: bool) -> Value {
        json!({ "id": id, "email": email, "name": name, "mayDelete": may_delete })
    }

    /// Alice's one identity.
    fn alice() -> Value {
        json!([identity(
            "i1",
            "alice@example.com",
            Some("Alice Example"),
            false
        )])
    }

    /// The lookup's answer: `identities`, the drafts mailbox query's `drafts`
    /// ids, and the original's `Email/get` when there is one.
    fn lookup_answer(
        identities: Value,
        drafts: Value,
        original: Option<Value>,
    ) -> Result<Value, NestedError> {
        let mut responses = vec![
            json!(["Identity/get", { "accountId": "s1", "list": identities, "notFound": [] }, "c0"]),
            json!(["Mailbox/query", { "accountId": "a1", "ids": drafts, "position": 0 }, "c1"]),
        ];
        if let Some(original) = original {
            responses.push(json!(["Email/get", original, "c2"]));
        }
        jmap(Value::Array(responses))
    }

    /// An `Email/get` answer listing `email`.
    fn got(email: Value) -> Value {
        json!({ "accountId": "a1", "list": [email], "notFound": [] })
    }

    /// The original a reply answers, `e1` in thread `t1`.
    fn original(fields: Value) -> Value {
        let mut email = json!({
            "id": "e1", "threadId": "t1",
            "from": [{ "name": "Bob", "email": "bob@example.org" }],
            "subject": "Lunch", "messageId": ["m1@example.org"],
        });
        for (key, value) in fields.as_object().unwrap() {
            email[key] = value.clone();
        }
        got(email)
    }

    /// The write the lookup plans, as the `Email/set`'s `create.draft`, and
    /// the `derived` it carries.
    fn planned(state: &InFlight, answer: Result<Value, NestedError>) -> (Value, Args) {
        let Next::Call { call, derived } = answered(state, answer) else {
            panic!("the lookup plans the write");
        };
        assert_eq!(call.step, DRAFT_SET_STEP);
        let body: Value = serde_json::from_str(call.api.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["using"], json!([CORE, MAIL]));
        assert_eq!(body["methodCalls"].as_array().unwrap().len(), 1);
        assert_eq!(body["methodCalls"][0][0], json!("Email/set"));
        let set = &body["methodCalls"][0][1];
        assert_eq!(set["accountId"], json!("a1"));
        assert_eq!(set.get("update"), None);
        (set["create"]["draft"].clone(), derived)
    }

    fn failed(next: Next) -> Failure {
        match next {
            Next::Fail(failure) => failure,
            other => panic!("not failed: {other:?}"),
        }
    }

    #[test]
    fn the_session_step_looks_up_the_sender_the_drafts_mailbox_and_the_original() {
        let lookup = |args: Value| {
            let state = row(DRAFT, args, SESSION_STEP, json!({}));
            let Next::Call { call, derived } = answered(&state, session_body()) else {
                panic!("the session step plans the lookup");
            };
            assert_eq!(call.step, API_STEP);
            assert_eq!(
                Value::Object(derived),
                json!({ ACCOUNT_KEY: "a1", SUBMISSION_KEY: "a1" })
            );
            serde_json::from_str::<Value>(call.api.body.as_deref().unwrap()).unwrap()
        };
        let identities = json!(["Identity/get", { "accountId": "a1", "ids": null }, "c0"]);
        let drafts = json!(["Mailbox/query",
                            { "accountId": "a1", "filter": { "role": "drafts" } }, "c1"]);
        assert_eq!(
            lookup(admitted_args(DRAFT)),
            json!({ "using": [CORE, MAIL, SUBMISSION], "methodCalls": [identities, drafts] })
        );
        assert_eq!(
            lookup(json!({ "reply_to_id": "e1", "body": "Hi" }))["methodCalls"],
            json!([identities, drafts, ["Email/get", {
                "accountId": "a1", "ids": ["e1"], "properties": DRAFT_LOOKUP_PROPERTIES,
                "fetchTextBodyValues": false,
            }, "c2"]])
        );
    }

    #[test]
    fn a_session_without_submission_refuses_every_draft_before_any_write() {
        let refusal = format!(
            "this account's session does not advertise `urn:ietf:params:jmap:submission` with a \
             primary account, so no sending identity can be read for the draft's From; \
             {SUBMISSION_SETUP}"
        );
        let state = row(DRAFT, admitted_args(DRAFT), SESSION_STEP, json!({}));
        for session in [
            json!({ "capabilities": { CORE: {}, MAIL: {} },
                    "primaryAccounts": { MAIL: "a1" } }),
            json!({ "capabilities": { CORE: {}, MAIL: {}, SUBMISSION: {} },
                    "primaryAccounts": { MAIL: "a1" } }),
        ] {
            assert_eq!(
                answered(&state, jmap_session(session)),
                Next::Fail(internal(refusal.clone()))
            );
        }
    }

    fn jmap_session(session: Value) -> Result<Value, NestedError> {
        brenn_provider_kit::testutil::http(200, &session.to_string())
    }

    #[test]
    fn a_draft_is_created_in_drafts_from_the_identity_with_the_callers_fields() {
        let args = json!({
            "to": ["Bob Example <bob@example.org>"], "cc": ["carol@example.org"],
            "bcc": ["dave@example.org"], "subject": "Hello \"there\"", "body": "Hi\nBob",
        });
        let (draft, derived) = planned(
            &at_lookup(args),
            lookup_answer(alice(), json!(["d1"]), None),
        );
        assert_eq!(
            draft,
            json!({
                "mailboxIds": { "d1": true },
                "keywords": { "$draft": true, "$seen": true },
                "from": [{ "name": "Alice Example", "email": "alice@example.com" }],
                "to": [{ "name": "Bob Example", "email": "bob@example.org" }],
                "cc": [{ "name": null, "email": "carol@example.org" }],
                "bcc": [{ "name": null, "email": "dave@example.org" }],
                "subject": "Hello \"there\"",
                "textBody": [{ "partId": "1", "type": "text/plain" }],
                "bodyValues": { "1": { "value": "Hi\nBob" } },
            })
        );
        assert_eq!(
            Value::Object(derived),
            json!({
                ACCOUNT_KEY: "a1", MAILBOX_KEY: "d1",
                FROM_KEY: { "name": "Alice Example", "email": "alice@example.com" },
            })
        );
    }

    #[test]
    fn the_sender_is_the_named_the_only_or_the_main_identity() {
        let several = json!([
            identity("i1", "alice@example.com", None, true),
            identity("i2", "bob@example.org", Some("Bob"), true),
        ]);
        assert_eq!(
            sender_of(Some("BOB@example.org"), several.clone()),
            Ok(json!({ "name": "Bob", "email": "bob@example.org" }))
        );
        assert_eq!(
            sender_of(
                None,
                json!([identity("i9", "solo@example.com", None, true)])
            ),
            Ok(json!({ "name": null, "email": "solo@example.com" }))
        );
        let mut marked = several.clone();
        marked[1]["mayDelete"] = json!(false);
        assert_eq!(
            sender_of(None, marked),
            Ok(json!({ "name": "Bob", "email": "bob@example.org" }))
        );
        assert_eq!(
            sender_of(None, several.clone()),
            Err(failure(
                ErrorKind::InvalidArgs,
                "the account has 2 sending identities: give `from` as one of \
                 alice@example.com, bob@example.org"
            ))
        );
        assert_eq!(
            sender_of(Some("eve@example.net"), several),
            Err(failure(
                ErrorKind::InvalidArgs,
                "no sending identity is `eve@example.net`: give `from` as one of \
                 alice@example.com, bob@example.org"
            ))
        );
        let many: Vec<Value> = (0..25)
            .map(|i| identity(&format!("i{i}"), &format!("a{i}@example.com"), None, true))
            .collect();
        let refusal = sender_of(None, json!(many)).unwrap_err().detail;
        assert!(
            refusal.starts_with("the account has 25 sending identities"),
            "{refusal}"
        );
        assert!(
            refusal.ends_with("a19@example.com, and 5 more"),
            "{refusal}"
        );
        // A name the draft cannot carry is dropped; an address it cannot
        // carry is the operator's.
        assert_eq!(
            sender_of(
                None,
                json!([identity("i1", "alice@example.com", Some("A \"B\""), false)])
            ),
            Ok(json!({ "name": null, "email": "alice@example.com" }))
        );
        assert_eq!(
            sender_of(
                None,
                json!([identity(
                    "i1",
                    "alice@example.com",
                    Some("😀".repeat(101).as_str()),
                    false
                )])
            ),
            Ok(json!({ "name": "😀".repeat(100), "email": "alice@example.com" }))
        );
        assert_eq!(
            sender_of(
                None,
                json!([identity("i1", "a\\b@example.com", None, false)])
            ),
            Err(internal(
                "the sending identity `i1` has an address the draft's From cannot carry: a \
                 control character, `\"` or `\\`, or over 254 bytes"
            ))
        );
    }

    /// The From the lookup chooses for a draft with `from`, among
    /// `identities`, or its refusal.
    fn sender_of(from: Option<&str>, identities: Value) -> Result<Value, Failure> {
        let mut args = admitted_args(DRAFT);
        if let Some(from) = from {
            args["from"] = json!(from);
        }
        match answered(
            &at_lookup(args),
            lookup_answer(identities, json!(["d1"]), None),
        ) {
            Next::Call { derived, .. } => Ok(derived[FROM_KEY].clone()),
            Next::Fail(failure) => Err(failure),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_named_from_is_refused_with_the_bare_address_rule() {
        let mut args = admitted_args(DRAFT);
        args["from"] = json!("Alice <alice@example.com>");
        assert_eq!(
            super::super::plan(DRAFT, &args, &settings()).map(drop),
            Err(
                "`from` must be the address of one of the account's sending identities: a local \
                 part of 1 to 64 characters, `@`, and a domain of 1 to 253, at most 254 bytes in \
                 all, with no whitespace, control character, `<`, `>`, `\"`, `\\`, `@` or `,`"
                    .to_owned()
            )
        );
    }

    #[test]
    fn identities_sharing_an_address_are_the_main_one_or_refused() {
        let shared = json!([
            identity("i1", "alice@example.com", Some("Alice Example"), true),
            identity("i2", "ALICE@example.com", Some("Alice (Board)"), true),
        ]);
        let mut marked = shared.clone();
        marked[1]["mayDelete"] = json!(false);
        assert_eq!(
            sender_of(Some("alice@example.com"), marked),
            Ok(json!({ "name": "Alice (Board)", "email": "ALICE@example.com" }))
        );
        let shared_refusal = Err(failure(
            ErrorKind::InvalidArgs,
            "2 sending identities share `alice@example.com` and none is the account's main one, \
             so the draft cannot choose its display name: the user removes the duplicate in \
             Fastmail's settings",
        ));
        assert_eq!(
            sender_of(Some("alice@example.com"), shared.clone()),
            shared_refusal
        );
        // Without `from`, addresses differing only in case are one address,
        // named as the first identity spells it.
        assert_eq!(sender_of(None, shared.clone()), shared_refusal);
        // The main identity is the one marked, only when exactly one is: two
        // marked are refused as none marked is.
        let mut both_marked = shared;
        both_marked[0]["mayDelete"] = json!(false);
        both_marked[1]["mayDelete"] = json!(false);
        assert_eq!(
            sender_of(Some("alice@example.com"), both_marked.clone()),
            shared_refusal
        );
        assert_eq!(sender_of(None, both_marked), shared_refusal);
        // The shared address is bounded as the listing bounds every address.
        let long = format!("{}@example.org", "a".repeat(250));
        let long_shared = json!([
            identity("i1", &long, Some("Alice"), true),
            identity("i2", &long, Some("Alice (Board)"), true),
        ]);
        assert_eq!(
            sender_of(None, long_shared),
            Err(failure(
                ErrorKind::InvalidArgs,
                &format!(
                    "2 sending identities share `{}…` and none is the account's main one, so the \
                     draft cannot choose its display name: the user removes the duplicate in \
                     Fastmail's settings",
                    &long[..RECIPIENT_EMAIL_MAX_BYTES]
                )
            ))
        );
        let two_main = json!([
            identity("i1", "alice@example.com", Some("Alice"), false),
            identity("i2", "bob@example.org", Some("Bob"), false),
        ]);
        assert_eq!(
            sender_of(None, two_main),
            Err(failure(
                ErrorKind::InvalidArgs,
                "the account has 2 sending identities: give `from` as one of \
                 alice@example.com, bob@example.org"
            ))
        );
    }

    #[test]
    fn wildcard_identities_are_never_candidates() {
        let mixed = json!([
            identity("i1", "*@example.com", Some("Sales"), false),
            identity("i2", "alice@example.com", Some("Alice"), true),
        ]);
        assert_eq!(
            sender_of(None, mixed.clone()),
            Ok(json!({ "name": "Alice", "email": "alice@example.com" }))
        );
        assert_eq!(
            sender_of(Some("orders@example.com"), mixed.clone()),
            Err(failure(
                ErrorKind::InvalidArgs,
                "no sending identity is `orders@example.com`: give `from` as one of \
                 alice@example.com"
            ))
        );
        // A `from` naming an address with `*` is told why it is refused, whether
        // or not an identity has that address.
        let mut literal = mixed.clone();
        literal[0]["email"] = json!("first*last@example.com");
        for (from, identities) in [
            ("*@example.com", mixed.clone()),
            ("first*last@example.com", literal),
            ("first*last@example.com", mixed.clone()),
        ] {
            assert_eq!(
                sender_of(Some(from), identities),
                Err(failure(
                    ErrorKind::InvalidArgs,
                    &format!(
                        "`{from}` contains `*`, and this tool never sends from such an address: \
                         give `from` as one of alice@example.com"
                    )
                ))
            );
        }
        let excluded = internal(
            "Fastmail's Identity/get lists no sending identity for this account that this tool \
             sends from (1 of its identities have an address containing `*`), so the draft has \
             no From",
        );
        for only in ["*@example.com", "first*last@example.com"] {
            for from in [None, Some("alice@example.com")] {
                assert_eq!(
                    sender_of(from, json!([identity("i1", only, None, false)])),
                    Err(excluded.clone()),
                    "{only} {from:?}"
                );
            }
        }
        assert_eq!(
            sender_of(None, json!([])),
            Err(internal(
                "Fastmail's Identity/get lists no sending identity for this account, so the \
                 draft has no From"
            ))
        );
    }

    #[test]
    fn identities_the_token_cannot_read_are_the_operators() {
        let state = at_lookup(admitted_args(DRAFT));
        let identity_error = |kind: &str| {
            jmap(json!([
                ["error", { "type": kind }, "c0"],
                ["Mailbox/query", { "accountId": "a1", "ids": ["d1"], "position": 0 }, "c1"],
            ]))
        };
        for kind in IDENTITY_ACCESS_ERRORS {
            assert_eq!(
                answered(&state, identity_error(kind)),
                Next::Fail(internal(format!(
                    "Fastmail: Identity/get failed: {kind}; no sending identity can be read for \
                     the draft's From; {SUBMISSION_SETUP}"
                )))
            );
        }
        // Any other error is Fastmail's, as the lookup's other calls report
        // theirs.
        assert_eq!(
            answered(&state, identity_error("serverUnavailable")),
            Next::Fail(internal("Fastmail: Identity/get failed: serverUnavailable"))
        );
    }

    #[test]
    fn the_drafts_mailbox_is_exactly_one() {
        let state = at_lookup(admitted_args(DRAFT));
        assert_eq!(
            answered(&state, lookup_answer(alice(), json!([]), None)),
            Next::Fail(internal(
                "the account has no mailbox with role `drafts`, so the draft has nowhere to go"
            ))
        );
        assert_eq!(
            answered(&state, lookup_answer(alice(), json!(["d1", "d2"]), None)),
            Next::Fail(internal(
                "Fastmail's Mailbox/query answered 2 mailboxes with role `drafts`, which RFC \
                 8621 gives to at most one"
            ))
        );
        assert_eq!(
            failed(answered(
                &state,
                lookup_answer(alice(), json!(["d 1"]), None)
            ))
            .kind,
            ErrorKind::Internal
        );
    }

    /// The write a reply plans for `args`, answering `original`: its
    /// `create.draft` and `derived`.
    fn reply(args: Value, original: Value) -> Result<(Value, Args), Failure> {
        let state = at_lookup(args);
        let answer = || lookup_answer(alice(), json!(["d1"]), Some(original.clone()));
        match answered(&state, answer()) {
            Next::Call { .. } => Ok(planned(&state, answer())),
            Next::Fail(failure) => Err(failure),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_reply_is_addressed_to_the_originals_reply_to_or_sender() {
        let args = json!({ "reply_to_id": "e1", "body": "Sure" });
        let (draft, derived) = reply(args.clone(), original(json!({}))).unwrap();
        assert_eq!(
            draft["to"],
            json!([{ "name": "Bob", "email": "bob@example.org" }])
        );
        assert_eq!(draft["subject"], json!("Re: Lunch"));
        assert_eq!(draft["inReplyTo"], json!(["m1@example.org"]));
        assert_eq!(draft["references"], json!(["m1@example.org"]));
        assert_eq!(
            derived[TO_KEY],
            json!([{ "name": "Bob", "email": "bob@example.org" }])
        );
        assert_eq!(derived[SUBJECT_KEY], json!("Re: Lunch"));
        assert_eq!(derived[THREAD_KEY], json!("t1"));
        let (draft, _) = reply(
            args.clone(),
            original(json!({ "replyTo": [
                { "name": "List \"Ops\"", "email": "ops@example.org" },
                { "name": null, "email": "ops2@example.org" },
            ] })),
        )
        .unwrap();
        assert_eq!(
            draft["to"],
            json!([
                { "name": null, "email": "ops@example.org" },
                { "name": null, "email": "ops2@example.org" },
            ])
        );
        // The caller's `to` and `subject` win, and are not reported back.
        let (draft, derived) = reply(
            json!({ "reply_to_id": "e1", "body": "Sure", "to": ["carol@example.org"],
                    "subject": "Lunch?" }),
            original(json!({})),
        )
        .unwrap();
        assert_eq!(
            draft["to"],
            json!([{ "name": null, "email": "carol@example.org" }])
        );
        assert_eq!(draft["subject"], json!("Lunch?"));
        assert_eq!(derived.get(TO_KEY), None);
        assert_eq!(derived.get(SUBJECT_KEY), None);
        let give_to = |original: Value| reply(args.clone(), original).unwrap_err();
        for sender in [
            json!({ "from": [{ "email": "bo\\b@example.org" }] }),
            json!({ "from": [{ "email": format!("{}@example.org", "b".repeat(243)) }] }),
            json!({ "from": [] }),
            json!({ "from": null }),
            json!({ "replyTo": (0..11).map(|i| json!({ "email": format!("r{i}@example.org") }))
                .collect::<Vec<_>>() }),
        ] {
            let refusal = give_to(original(sender.clone()));
            assert_eq!(refusal.kind, ErrorKind::InvalidArgs, "{sender}");
            assert!(
                refusal.detail.ends_with(": give `to`"),
                "{sender}: {refusal:?}"
            );
        }
        // The caller's `to` makes an unaddressable original harmless.
        assert!(
            reply(
                json!({ "reply_to_id": "e1", "body": "Sure", "to": ["carol@example.org"] }),
                original(json!({ "from": [{ "email": "bo\\b@example.org" }] })),
            )
            .is_ok()
        );
    }

    #[test]
    fn a_replys_subject_is_derived_from_the_originals() {
        let args = json!({ "reply_to_id": "e1", "body": "Sure" });
        let subject = |original_subject: Value| {
            reply(
                args.clone(),
                original(json!({ "subject": original_subject })),
            )
            .map(|(draft, _)| draft["subject"].clone())
        };
        assert_eq!(subject(json!("Lunch")), Ok(json!("Re: Lunch")));
        assert_eq!(subject(json!("RE: Lunch")), Ok(json!("RE: Lunch")));
        assert_eq!(subject(json!("  re:Lunch")), Ok(json!("  re:Lunch")));
        assert_eq!(subject(json!("a\r\nb")), Ok(json!("Re: a  b")));
        assert_eq!(subject(json!("   ")), Ok(json!("Re:")));
        assert_eq!(subject(json!(null)), Ok(json!("Re:")));
        assert_eq!(
            subject(json!("x".repeat(SUBJECT_MAX_CHARS - 4))),
            Ok(json!(format!("Re: {}", "x".repeat(SUBJECT_MAX_CHARS - 4))))
        );
        assert_eq!(
            subject(json!("x".repeat(SUBJECT_MAX_CHARS - 3))),
            Err(failure(
                ErrorKind::InvalidArgs,
                "the original's subject is longer than 500 characters as a reply's: give \
                 `subject`"
            ))
        );
    }

    #[test]
    fn a_reply_threads_only_on_ids_it_can_write_as_they_are() {
        let args = json!({ "reply_to_id": "e1", "body": "Sure" });
        let threading = |fields: Value| {
            let (draft, derived) = reply(args.clone(), original(fields)).unwrap();
            (
                draft.get("inReplyTo").cloned(),
                draft.get("references").cloned(),
                derived.get(IN_REPLY_TO_KEY).cloned(),
            )
        };
        for unusable in [
            json!("m 1@example.org"),
            json!("m\u{1}1@example.org"),
            json!("m\"1@example.org"),
            json!(format!("{}@example.org", "m".repeat(244))),
        ] {
            assert_eq!(
                threading(json!({ "messageId": [unusable.clone()] })),
                (None, None, None),
                "{unusable}"
            );
        }
        assert_eq!(threading(json!({ "messageId": [] })), (None, None, None));
        let widest = format!("{}@example.org", "m".repeat(243));
        assert_eq!(
            threading(json!({ "messageId": [widest] })).0,
            Some(json!([widest]))
        );
        let references: Vec<String> = (0..12).map(|i| format!("r{i}@example.org")).collect();
        let mut with_unusable = references.clone();
        with_unusable.insert(5, "bad id".to_owned());
        let mut expected: Vec<String> = references[3..].to_vec();
        expected.push("m1@example.org".to_owned());
        assert_eq!(
            threading(json!({ "references": with_unusable })),
            (
                Some(json!(["m1@example.org"])),
                Some(json!(expected)),
                Some(json!("m1@example.org"))
            )
        );
    }

    #[test]
    fn a_reply_to_an_email_that_is_gone_is_the_callers() {
        let args = json!({ "reply_to_id": "e1", "body": "Sure" });
        assert_eq!(
            reply(
                args.clone(),
                json!({ "accountId": "a1", "list": [], "notFound": ["e1"] })
            ),
            Err(failure(ErrorKind::InvalidArgs, "no email with id `e1`"))
        );
        assert_eq!(
            reply(
                args,
                json!({ "accountId": "a1", "list": [], "notFound": [] })
            )
            .unwrap_err()
            .kind,
            ErrorKind::Internal
        );
    }

    /// The draft's row at its write, carrying what a reply's lookup found.
    fn at_write() -> InFlight {
        row(
            DRAFT,
            json!({ "reply_to_id": "e1", "body": "Sure" }),
            DRAFT_SET_STEP,
            json!({
                ACCOUNT_KEY: "a1", MAILBOX_KEY: "d1",
                FROM_KEY: { "name": "Alice Example", "email": "alice@example.com" },
                TO_KEY: [{ "name": "Bob", "email": "bob@example.org" }],
                SUBJECT_KEY: "Re: Lunch", IN_REPLY_TO_KEY: "m1@example.org",
                REFERENCES_KEY: ["m1@example.org"], THREAD_KEY: "t1",
            }),
        )
    }

    fn created_answer(created: Value) -> Result<Value, NestedError> {
        set_answer(
            "Email/set",
            json!({ "accountId": "a1", "created": { "draft": created } }),
        )
    }

    #[test]
    fn the_write_answers_the_new_draft_and_what_the_tool_chose() {
        let state = at_write();
        let expected = json!({
            "id": "e9", "thread_id": "t1", "mailbox_id": "d1",
            "from": "Alice Example <alice@example.com>", "to": ["Bob <bob@example.org>"],
            "subject": "Re: Lunch", "in_reply_to": "m1@example.org",
        });
        assert_eq!(
            completed(answered(
                &state,
                created_answer(json!({ "id": "e9", "threadId": "t1" }))
            )),
            expected
        );
        let mut elsewhere = expected.clone();
        elsewhere["thread_id"] = json!("t2");
        elsewhere["in_thread"] = json!(false);
        assert_eq!(
            completed(answered(
                &state,
                created_answer(json!({ "id": "e9", "threadId": "t2" }))
            )),
            elsewhere
        );
        let fresh = row(
            DRAFT,
            admitted_args(DRAFT),
            DRAFT_SET_STEP,
            json!({ ACCOUNT_KEY: "a1", MAILBOX_KEY: "d1",
                    FROM_KEY: { "name": null, "email": "alice@example.com" } }),
        );
        assert_eq!(
            completed(answered(
                &fresh,
                created_answer(json!({ "id": "e9", "threadId": "t5" }))
            )),
            json!({ "id": "e9", "thread_id": "t5", "mailbox_id": "d1",
                    "from": "alice@example.com" })
        );
    }

    #[test]
    fn a_refused_draft_is_fastmails_reason_and_a_lost_one_carries_the_hint() {
        let state = at_write();
        let refused = |error: Value| {
            answered(
                &state,
                set_answer(
                    "Email/set",
                    json!({ "accountId": "a1", "notCreated": { "draft": error } }),
                ),
            )
        };
        assert_eq!(
            refused(
                json!({ "type": "invalidProperties", "description": "bad subject",
                            "properties": ["subject"] })
            ),
            Next::Fail(failure(
                ErrorKind::InvalidArgs,
                "Fastmail refused `draft`: invalidProperties: bad subject (subject)"
            ))
        );
        assert_eq!(
            refused(json!({ "type": "overQuota" })),
            Next::Fail(internal("Fastmail refused `draft`: overQuota"))
        );
        assert_eq!(
            refused(json!({ "type": "forbidden" })),
            Next::Fail(failure(
                ErrorKind::InvalidArgs,
                &format!("Fastmail refused `draft`: forbidden; {FORBIDDEN_CHANGE}")
            ))
        );
        assert_eq!(
            answered(
                &state,
                set_answer("Email/set", json!({ "accountId": "a1" }))
            ),
            Next::Fail(internal(format!(
                "Fastmail's Email/set answered neither the new draft nor its refusal; \
                 {DRAFT_UNKNOWN_HINT}"
            )))
        );
        assert_eq!(
            answered(
                &state,
                jmap(json!([["error", { "type": "accountReadOnly" }, "c0"]]))
            ),
            Next::Fail(internal(format!(
                "Fastmail: Email/set failed: accountReadOnly; {READ_ONLY_TOKEN}"
            )))
        );
        // The lookup reads: a lost answer there carries no hint.
        let lookup = failed(answered(
            &at_lookup(admitted_args(DRAFT)),
            set_answer("Email/set", json!({})),
        ));
        assert_eq!(lookup.kind, ErrorKind::Internal);
        assert!(!lookup.detail.contains(DRAFT_UNKNOWN_HINT), "{lookup:?}");
    }

    #[test]
    fn every_step_of_a_draft_carries_the_requests_account() {
        for account in [Some("alice-work"), None] {
            let mut args = json!({ "reply_to_id": "e1", "body": "Sure" });
            if let Some(account) = account {
                args["account"] = json!(account);
            }
            let expected = account.map(str::to_owned);
            let call = super::super::plan(DRAFT, &args, &settings()).unwrap();
            assert_eq!(call.api.account, expected);
            let state = row(DRAFT, args.clone(), SESSION_STEP, json!({}));
            let Next::Call { call, .. } = answered(&state, session_body()) else {
                panic!("the session step plans the lookup");
            };
            assert_eq!(call.api.account, expected);
            let state = at_lookup(args);
            let Next::Call { call, .. } = answered(
                &state,
                lookup_answer(alice(), json!(["d1"]), Some(original(json!({})))),
            ) else {
                panic!("the lookup plans the write");
            };
            assert_eq!(call.api.account, expected);
            assert_eq!(
                (call.api.method.as_str(), call.api.path.as_str()),
                ("POST", "/jmap/api")
            );
        }
    }

    /// Every field at its bound and its widest escaping: the body of quotes,
    /// thirty recipients with four-byte names and 254-byte addresses, a
    /// subject of quotes, eleven 255-byte threading ids, the From at its
    /// widest, and `account` at 255.
    #[test]
    fn the_widest_draft_fits_the_args_cap() {
        let address = |i: usize| format!("{i:0>64}@{}", "d".repeat(189));
        let recipient = |i: usize| {
            format!(
                "{} <{}>",
                "\u{1F600}".repeat(RECIPIENT_NAME_MAX_CHARS),
                address(i)
            )
        };
        let list =
            |from: usize| -> Vec<String> { (from..from + RECIPIENTS_MAX).map(recipient).collect() };
        let args = json!({
            "to": list(0), "cc": list(10), "bcc": list(20),
            "subject": "\"".repeat(SUBJECT_MAX_CHARS),
            "body": "\"".repeat(BODY_MAX_WIRE_BYTES / 4),
            "reply_to_id": "e".repeat(255),
            "account": "a".repeat(255),
        });
        assert!(super::super::plan(DRAFT, &args, &settings()).is_ok());
        let id = |i: usize| format!("{i:0>243}@example.org");
        let references: Vec<String> = (1..=DRAFT_REFERENCES_MAX).map(id).collect();
        let original = got(json!({
            "id": "e".repeat(255), "threadId": "t".repeat(255),
            "from": [{ "email": "bob@example.org" }],
            "messageId": [id(0)], "references": references,
        }));
        let identities = json!([identity(
            "i1",
            &address(99),
            Some("\u{1F600}".repeat(RECIPIENT_NAME_MAX_CHARS).as_str()),
            false
        )]);
        let state = row(
            DRAFT,
            args,
            API_STEP,
            json!({ ACCOUNT_KEY: "a".repeat(255), SUBMISSION_KEY: "s1" }),
        );
        let answer = jmap(json!([
            ["Identity/get", { "accountId": "s1", "list": identities, "notFound": [] }, "c0"],
            ["Mailbox/query", { "accountId": "a1", "ids": ["d".repeat(255)], "position": 0 }, "c1"],
            ["Email/get", original, "c2"],
        ]));
        let Next::Call { call, .. } = answered(&state, answer) else {
            panic!("the lookup plans the write");
        };
        let body: Value = serde_json::from_str(call.api.body.as_deref().unwrap()).unwrap();
        let draft = &body["methodCalls"][0][1]["create"]["draft"];
        assert_eq!(
            draft["references"].as_array().unwrap().len(),
            DRAFT_REFERENCES_MAX
        );
        assert_eq!(draft["inReplyTo"], json!([id(0)]));
        let width = serde_json::to_vec(&call.api).unwrap().len();
        assert!(width <= HOST_ARGS_CAP_BYTES, "{width} bytes");
    }

    #[test]
    fn a_draft_runs_session_lookup_write_then_completes() {
        let host = MemoryHost::new();
        let id = "q".repeat(32);
        let activate = |requests, results| {
            host.activate::<FastmailWrite>(
                &settings(),
                Input {
                    now_ms: 1_000,
                    requests,
                    results,
                },
            )
            .unwrap();
        };
        let deliver = |call_id: &str, outcome| Delivered {
            call_id: call_id.to_owned(),
            serving: Some(id.clone()),
            outcome,
        };
        activate(
            vec![incoming(&id, "fastmail-draft", admitted_args(DRAFT))],
            vec![],
        );
        let calls = host.take_calls();
        assert_eq!(calls[0].call.path, "/jmap/session");
        activate(vec![], vec![deliver(&calls[0].call_id, session_body())]);
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0]
                .call
                .body
                .as_deref()
                .unwrap()
                .contains("Identity/get")
        );
        let lookup = jmap(json!([
            ["Identity/get", { "accountId": "a1", "list": alice(), "notFound": [] }, "c0"],
            ["Mailbox/query", { "accountId": "a1", "ids": ["d1"], "position": 0 }, "c1"],
        ]));
        activate(vec![], vec![deliver(&calls[0].call_id, lookup)]);
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].call.body.as_deref().unwrap().contains("Email/set"));
        activate(
            vec![],
            vec![deliver(
                &calls[0].call_id,
                created_answer(json!({ "id": "e9", "threadId": "t9" })),
            )],
        );
        let completions = host.take_completions();
        assert_eq!(completions.len(), 1);
        let Answered::Value(value) = &completions[0].answer else {
            panic!("{completions:?}");
        };
        assert_eq!(
            value,
            &json!({ "id": "e9", "thread_id": "t9", "mailbox_id": "d1",
                     "from": "Alice Example <alice@example.com>" })
        );
        assert!(host.take_calls().is_empty());
    }
}
