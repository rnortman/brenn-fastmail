//! The provider's decisions, in `std`, `serde_json`, the JMAP crate and
//! `brenn-provider-kit`: reading the instance's `config`, validating a tool's
//! arguments, building each JMAP request as `api-request` arguments, reading
//! `api-request`'s answer, and compacting Fastmail's JSON into the tool's
//! result. Nothing here names the guest SDK, so the whole module runs on the
//! host under test. Tools are the generated `Provided` enum, whose SDK-facing
//! parts are wasm32-only.
//!
//! Every tool asks for the JMAP session first (step 1), which names the mail
//! account, then posts one JMAP request (step 2). The thread tool posts a
//! second (step 3): `Thread/get` names the thread's emails, and only the
//! newest `max` of them are fetched.

// The host test's crate root is this file, so it declares the generated
// module and its sibling itself; in the component, `lib.rs` does.
#[cfg(test)]
mod html;
#[cfg(test)]
mod spec;

use std::collections::BTreeMap;

use brenn_provider_kit::{
    Args, COMPACT_RESULT_CEILING, Call, Failure, Logic, NestedError, Next, Tool, bounded, capped,
    cut, flag, integer, internal, no_such_step, object,
};
use fastmail_jmap::mail::{MAILBOX_PROPERTIES, READ_PROPERTIES, SUMMARY_PROPERTIES};
use fastmail_jmap::session::MAIL;
use fastmail_jmap::{
    Address, BodyPart, CallId, Email, EmailGet, EmailQuery, Filter, FilterCondition, Ids, Mailbox,
    MailboxGet, QueryResult, Request, Response, Sort, Thread, ThreadGet, result_ref,
};
use fastmail_provider::args::{
    ACCOUNT_ARG, ID_MAX_CHARS, account_arg, keyword_arg, token, utc_date_arg,
};
use fastmail_provider::failure::{
    answer_body, derived_str, derived_u64, jmap_failure, missing_derived, optional, stored_args,
};
use fastmail_provider::results::{in_order, page, the_one};
use fastmail_provider::settings::{API_NAME, Settings};
use fastmail_provider::steps::{ACCOUNT_KEY, SESSION_STEP, after_session, api_step, session_call};
use serde_json::{Value, json};

use crate::spec::Provided;

/// The most characters a free-text search argument may hold.
const TEXT_MAX_CHARS: usize = 1024;

/// A search's page size: its range and default; and the furthest it skips.
const LIMIT_MAX: u64 = 25;
const LIMIT_DEFAULT: u64 = 10;
const POSITION_MAX: u64 = 10_000;
/// The most text bytes a read fetches per part and answers: range, default.
const BODY_MIN: u64 = 200;
const BODY_MAX: u64 = 60_000;
const BODY_DEFAULT: u64 = 12_000;
/// The most messages a thread read fetches: range, default.
const THREAD_MAX: u64 = 100;
const THREAD_DEFAULT: u64 = 30;

/// The most addresses per header a summary carries, and a full read.
const SUMMARY_ADDRESSES_MAX: usize = 5;
const READ_ADDRESSES_MAX: usize = 50;
/// The most `references` a read carries: the newest.
const REFERENCES_MAX: usize = 20;
/// The most attachments a read lists.
const ATTACHMENTS_MAX: usize = 100;
// TODO(reuse-cut-width-beside-wire-width-in-wrong-crate): these bound UTF-8
// bytes, not the width a string serializes to.
/// UTF-8 byte bounds on strings copied out of Fastmail's answer.
const NAME_MAX_BYTES: usize = 1000;
const SUBJECT_MAX_BYTES: usize = 1000;
const PREVIEW_MAX_BYTES: usize = 300;
const ADDRESS_MAX_BYTES: usize = 400;
const HEADER_MAX_BYTES: usize = 1000;
const CONTENT_TYPE_MAX_BYTES: usize = 255;

/// The steps after the session: the JMAP request, and the thread's emails.
const API_STEP: u8 = 2;
const THREAD_EMAILS_STEP: u8 = 3;

/// What to narrow when a tool's answer is too large.
const LIMIT_HINT: &str = "ask for fewer with `limit`";
const BODY_HINT: &str = "ask for a smaller `max_body_bytes`";
const MAX_HINT: &str = "ask for a smaller `max`";

/// `derived` keys beside the mail account's: the thread's fetched email ids,
/// size and omitted count.
const EMAIL_IDS_KEY: &str = "email_ids";
const MESSAGE_COUNT_KEY: &str = "message_count";
const OMITTED_KEY: &str = "messages_omitted";

impl Tool for Provided {
    fn name(self) -> &'static str {
        Provided::name(self)
    }

    fn from_name(name: &str) -> Option<Provided> {
        Provided::from_name(name)
    }
}

/// A request's in-flight row, naming one of this component's tools.
pub type InFlight = brenn_provider_kit::InFlight<Provided>;

/// This component's step functions, for the kit's activation loop.
pub struct Fastmail;

impl Logic for Fastmail {
    type Tool = Provided;
    type Settings = Settings;

    fn plan(tool: Provided, args: &Value, settings: &Settings) -> Result<Call, String> {
        plan(tool, args, settings)
    }

    fn advance(state: &InFlight, outcome: Result<Value, NestedError>, settings: &Settings) -> Next {
        advance(state, outcome, settings)
    }
}

/// A request's arguments, validated.
#[derive(Clone, Debug, PartialEq)]
struct Parsed {
    /// Which of the user's accounts on the key provider every nested call
    /// names, by the operator's name for it; `None` for the user's default or
    /// only one.
    account: Option<String>,
    tool: ToolArgs,
}

/// A tool's own arguments, validated.
#[derive(Clone, Debug, PartialEq)]
enum ToolArgs {
    Mailboxes,
    Search(Box<SearchArgs>),
    Read(ReadArgs),
    Thread(ThreadArgs),
}

#[derive(Clone, Debug, PartialEq)]
struct SearchArgs {
    /// Every argument given, as one condition: Fastmail matches an email
    /// only when all its properties match, and an empty one matches every
    /// email.
    condition: FilterCondition,
    limit: u64,
    position: u64,
}

impl SearchArgs {
    const KEYS: &[&str] = &[
        "text",
        "from",
        "to",
        "subject",
        "mailbox",
        "after",
        "before",
        "keyword",
        "not_keyword",
        "has_attachment",
        "limit",
        "position",
        ACCOUNT_ARG,
    ];

    fn parse(map: &Args) -> Result<SearchArgs, String> {
        let text =
            |key: &str| capped(map, key, TEXT_MAX_CHARS).map(|value| value.map(str::to_owned));
        Ok(SearchArgs {
            condition: FilterCondition {
                text: text("text")?,
                from: text("from")?,
                to: text("to")?,
                subject: text("subject")?,
                in_mailbox: token(map, "mailbox")?,
                after: utc_date_arg(map, "after")?,
                before: utc_date_arg(map, "before")?,
                has_keyword: keyword_arg(map, "keyword")?,
                not_keyword: keyword_arg(map, "not_keyword")?,
                has_attachment: match map.get("has_attachment") {
                    None => None,
                    Some(_) => Some(flag(map, "has_attachment")?),
                },
            },
            limit: integer(map, "limit", 1, LIMIT_MAX)?.unwrap_or(LIMIT_DEFAULT),
            position: integer(map, "position", 0, POSITION_MAX)?.unwrap_or(0),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ReadArgs {
    id: String,
    max_body_bytes: u64,
}

impl ReadArgs {
    const KEYS: &[&str] = &["id", "max_body_bytes", ACCOUNT_ARG];

    fn parse(map: &Args) -> Result<ReadArgs, String> {
        Ok(ReadArgs {
            id: token(map, "id")?.ok_or("`id` is required")?,
            max_body_bytes: integer(map, "max_body_bytes", BODY_MIN, BODY_MAX)?
                .unwrap_or(BODY_DEFAULT),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ThreadArgs {
    id: String,
    max: u64,
}

impl ThreadArgs {
    const KEYS: &[&str] = &["id", "max", ACCOUNT_ARG];

    fn parse(map: &Args) -> Result<ThreadArgs, String> {
        Ok(ThreadArgs {
            id: token(map, "id")?.ok_or("`id` is required")?,
            max: integer(map, "max", 1, THREAD_MAX)?.unwrap_or(THREAD_DEFAULT),
        })
    }
}

fn parse(tool: Provided, args: &Value) -> Result<Parsed, String> {
    let keys = match tool {
        Provided::FastmailMailboxes => &[ACCOUNT_ARG],
        Provided::FastmailSearch => SearchArgs::KEYS,
        Provided::FastmailRead => ReadArgs::KEYS,
        Provided::FastmailThread => ThreadArgs::KEYS,
    };
    let map = object(args, keys)?;
    let tool = match tool {
        Provided::FastmailMailboxes => ToolArgs::Mailboxes,
        Provided::FastmailSearch => ToolArgs::Search(Box::new(SearchArgs::parse(map)?)),
        Provided::FastmailRead => ToolArgs::Read(ReadArgs::parse(map)?),
        Provided::FastmailThread => ToolArgs::Thread(ThreadArgs::parse(map)?),
    };
    Ok(Parsed {
        account: account_arg(map)?,
        tool,
    })
}

/// Validate a request's arguments and ask for the session. An error is the
/// request's `invalid_args` detail, naming the argument.
pub fn plan(tool: Provided, args: &Value, settings: &Settings) -> Result<Call, String> {
    let parsed = parse(tool, args)?;
    Ok(session_call(settings, parsed.account.as_deref()))
}

/// The step-2 request for `parsed` in `account_id`, and the call ids its
/// answer is read by, in push order.
fn api_request(parsed: &ToolArgs, account_id: &str) -> (Request, Vec<CallId>) {
    let mut request = Request::mail();
    let ids = match parsed {
        ToolArgs::Mailboxes => vec![
            MailboxGet {
                account_id,
                properties: MAILBOX_PROPERTIES,
            }
            .push(&mut request),
        ],
        ToolArgs::Search(args) => {
            let query = EmailQuery {
                account_id,
                filter: Filter::Condition(args.condition.clone()),
                sort: vec![Sort {
                    property: "receivedAt",
                    is_ascending: false,
                }],
                position: args.position,
                limit: args.limit,
                calculate_total: true,
            }
            .push(&mut request);
            let get = EmailGet {
                account_id,
                ids: Ids::Ref(result_ref(&query, "/ids")),
                properties: SUMMARY_PROPERTIES,
                fetch_text_body_values: false,
                max_body_value_bytes: None,
            }
            .push(&mut request);
            vec![query, get]
        }
        ToolArgs::Read(args) => vec![
            EmailGet {
                account_id,
                ids: Ids::Listed(vec![args.id.clone()]),
                properties: READ_PROPERTIES,
                fetch_text_body_values: true,
                max_body_value_bytes: Some(args.max_body_bytes),
            }
            .push(&mut request),
        ],
        ToolArgs::Thread(args) => vec![
            ThreadGet {
                account_id,
                ids: vec![args.id.clone()],
            }
            .push(&mut request),
        ],
    };
    (request, ids)
}

/// The thread tool's step-3 request: summaries of `ids`, listed.
fn thread_emails_request(account_id: &str, ids: &[String]) -> (Request, CallId) {
    let mut request = Request::mail();
    let get = EmailGet {
        account_id,
        ids: Ids::Listed(ids.to_vec()),
        properties: SUMMARY_PROPERTIES,
        fetch_text_body_values: false,
        max_body_value_bytes: None,
    }
    .push(&mut request);
    (request, get)
}

fn has_step(tool: Provided, step: u8) -> bool {
    matches!(
        (tool, step),
        (_, SESSION_STEP | API_STEP) | (Provided::FastmailThread, THREAD_EMAILS_STEP)
    )
}

/// What the caller narrows when `tool`'s answer is too large.
fn hint(tool: Provided) -> Option<&'static str> {
    match tool {
        Provided::FastmailMailboxes => None,
        Provided::FastmailSearch => Some(LIMIT_HINT),
        Provided::FastmailRead => Some(BODY_HINT),
        Provided::FastmailThread => Some(MAX_HINT),
    }
}

/// The hint on an unreadable answer: only the step that fetches mail has one.
fn unreadable_hint(tool: Provided, step: u8) -> Option<&'static str> {
    match (tool, step) {
        (_, SESSION_STEP) | (Provided::FastmailThread, API_STEP) => None,
        (tool, _) => hint(tool),
    }
}

/// What the request does with its nested call's answer: complete, fail, or
/// issue its next step. Every completion is held to the result ceiling.
pub fn advance(state: &InFlight, outcome: Result<Value, NestedError>, settings: &Settings) -> Next {
    if !has_step(state.tool, state.step) {
        return Next::Fail(no_such_step(state.tool, state.step));
    }
    match step(state, outcome, settings) {
        Ok(Next::Complete(value)) => bounded(value, COMPACT_RESULT_CEILING, hint(state.tool)),
        Ok(next) => next,
        Err(failure) => Next::Fail(failure),
    }
}

fn step(
    state: &InFlight,
    outcome: Result<Value, NestedError>,
    settings: &Settings,
) -> Result<Next, Failure> {
    let body = answer_body(state, outcome, unreadable_hint(state.tool, state.step))?;
    let parsed = stored_args(state, parse)?;
    if state.step == SESSION_STEP {
        return after_session(
            &body,
            MAIL,
            settings,
            parsed.account.as_deref(),
            API_STEP,
            |account_id| api_request(&parsed.tool, account_id).0,
        );
    }
    let account_id = derived_str(state, ACCOUNT_KEY)?;
    let response = Response::parse(body).map_err(jmap_failure)?;
    if state.step == THREAD_EMAILS_STEP {
        let ToolArgs::Thread(args) = parsed.tool else {
            return Err(no_such_step(state.tool, state.step));
        };
        return thread_emails(state, &args, account_id, &response);
    }
    let (_, ids) = api_request(&parsed.tool, account_id);
    let result = |at: usize| response.result(&ids[at]).map_err(jmap_failure);
    match &parsed.tool {
        ToolArgs::Mailboxes => mailboxes(account_id, result(0)?),
        ToolArgs::Search(_) => search(result(0)?, result(1)?),
        ToolArgs::Read(args) => read(args, result(0)?),
        ToolArgs::Thread(args) => thread(
            settings,
            parsed.account.as_deref(),
            account_id,
            args,
            result(0)?,
        ),
    }
}

fn mailboxes(account_id: &str, args: &Value) -> Result<Next, Failure> {
    let mut list = Mailbox::list(args).map_err(jmap_failure)?;
    list.sort_by(|a, b| (a.sort_order, &a.name).cmp(&(b.sort_order, &b.name)));
    let mailboxes: Vec<Value> = list
        .iter()
        .map(|mailbox| {
            let mut out = Args::new();
            out.insert("id".into(), json!(mailbox.id));
            out.insert("name".into(), json!(cut(&mailbox.name, NAME_MAX_BYTES).0));
            optional(&mut out, "role", &mailbox.role, NAME_MAX_BYTES);
            optional(&mut out, "parent_id", &mailbox.parent_id, ID_MAX_CHARS);
            out.insert("total_emails".into(), json!(mailbox.total_emails));
            out.insert("unread_emails".into(), json!(mailbox.unread_emails));
            out.insert("total_threads".into(), json!(mailbox.total_threads));
            out.insert("unread_threads".into(), json!(mailbox.unread_threads));
            Value::Object(out)
        })
        .collect();
    Ok(Next::Complete(
        json!({ "account_id": account_id, "mailboxes": mailboxes }),
    ))
}

fn search(query: &Value, get: &Value) -> Result<Next, Failure> {
    let query = QueryResult::read(query).map_err(jmap_failure)?;
    let emails = Email::get(get).map_err(jmap_failure)?.list;
    // In the query's order; an email deleted since the query is absent.
    let messages: Vec<Value> = in_order(&query.ids, emails, email_id)
        .iter()
        .map(|email| Value::Object(summary(email)))
        .collect();
    Ok(Next::Complete(page(&query, "messages", messages)))
}

fn email_id(email: &Email) -> &str {
    &email.id
}

fn read(args: &ReadArgs, get: &Value) -> Result<Next, Failure> {
    let got = Email::get(get).map_err(jmap_failure)?;
    let email = the_one(got, &args.id, email_id, EmailGet::METHOD, "email")?;
    let (mut out, mut omitted) = headers(&email, READ_ADDRESSES_MAX);
    for (key, list) in [
        ("cc", &email.cc),
        ("bcc", &email.bcc),
        ("reply_to", &email.reply_to),
    ] {
        addresses(&mut out, key, list, READ_ADDRESSES_MAX, &mut omitted);
    }
    if omitted > 0 {
        out.insert("recipients_omitted".into(), json!(omitted));
    }
    optional(&mut out, "sent_at", &email.sent_at, HEADER_MAX_BYTES);
    header_ids(&mut out, "message_id", &email.message_id);
    header_ids(&mut out, "in_reply_to", &email.in_reply_to);
    let skipped = email.references.len().saturating_sub(REFERENCES_MAX);
    header_ids(&mut out, "references", &email.references[skipped..]);
    if skipped > 0 {
        out.insert("references_omitted".into(), json!(skipped));
    }
    let body = body_text(&email, args.max_body_bytes)?;
    out.insert("text".into(), json!(body.text));
    out.insert("text_source".into(), json!(body.source));
    out.insert("body_truncated".into(), json!(body.truncated));
    out.insert("encoding_problem".into(), json!(body.encoding_problem));
    let listed: Vec<(&BodyPart, bool)> = email
        .attachments
        .iter()
        .map(|part| (part, part.cid.is_some()))
        .chain(
            body.media
                .iter()
                .filter(|part| {
                    !email
                        .attachments
                        .iter()
                        .any(|listed| listed.part_id.is_some() && listed.part_id == part.part_id)
                })
                .map(|part| (*part, true)),
        )
        .collect();
    // TODO(blob-transfer): attachments are listed, never fetched.
    let attachments: Vec<Value> = listed
        .iter()
        .take(ATTACHMENTS_MAX)
        .map(|(part, inline)| {
            let mut out = Args::new();
            optional(&mut out, "name", &part.name, NAME_MAX_BYTES);
            out.insert(
                "content_type".into(),
                json!(cut(&part.content_type, CONTENT_TYPE_MAX_BYTES).0),
            );
            out.insert("size".into(), json!(part.size));
            optional(&mut out, "disposition", &part.disposition, NAME_MAX_BYTES);
            optional(&mut out, "blob_id", &part.blob_id, ID_MAX_CHARS);
            out.insert("inline".into(), json!(inline));
            Value::Object(out)
        })
        .collect();
    out.insert("attachments".into(), Value::Array(attachments));
    if listed.len() > ATTACHMENTS_MAX {
        out.insert(
            "attachments_omitted".into(),
            json!(listed.len() - ATTACHMENTS_MAX),
        );
    }
    Ok(Next::Complete(Value::Object(out)))
}

/// A read's text: its text parts' values in order, HTML rendered to text,
/// an inline image, audio or video part as a bracketed placeholder, joined
/// by a blank line and cut at `max_bytes`; and those media parts.
struct BodyText<'a> {
    text: String,
    source: &'static str,
    truncated: bool,
    encoding_problem: bool,
    media: Vec<&'a BodyPart>,
}

/// `textBody` holds the text parts and, displayed between them, inline
/// media parts (RFC 8621 section 4.1.4), which carry no body value.
fn body_text(email: &Email, max_bytes: u64) -> Result<BodyText<'_>, Failure> {
    let (mut plain, mut html, mut truncated, mut encoding_problem) = (false, false, false, false);
    let mut parts = Vec::with_capacity(email.text_body.len());
    let mut media = Vec::new();
    for part in &email.text_body {
        let is_text = part
            .content_type
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("text/"));
        if !is_text {
            let name = part
                .name
                .as_deref()
                .map(|name| format!(" \"{}\"", cut(name, NAME_MAX_BYTES).0))
                .unwrap_or_default();
            parts.push(format!(
                "[inline {}{name}, {} bytes; listed in attachments]",
                cut(&part.content_type, CONTENT_TYPE_MAX_BYTES).0,
                part.size
            ));
            media.push(part);
            continue;
        }
        let value = part
            .part_id
            .as_ref()
            .and_then(|id| email.body_values.get(id))
            .ok_or_else(|| {
                internal(format!(
                    "{API_NAME}'s Email/get gave a text part of email `{}` no body value",
                    email.id
                ))
            })?;
        truncated |= value.is_truncated;
        encoding_problem |= value.is_encoding_problem;
        if part.content_type.eq_ignore_ascii_case("text/html") {
            html = true;
            parts.push(crate::html::to_text(&value.value));
        } else {
            plain = true;
            parts.push(value.value.clone());
        }
    }
    let source = match (plain, html) {
        (false, false) => "none",
        (true, false) => "text/plain",
        (false, true) => "text/html",
        (true, true) => "mixed",
    };
    let max_bytes = usize::try_from(max_bytes).expect("BODY_MAX fits usize");
    let (text, cut_here) = cut(&parts.join("\n\n"), max_bytes);
    Ok(BodyText {
        text,
        source,
        truncated: truncated || cut_here,
        encoding_problem,
        media,
    })
}

/// The thread's ids: complete an empty thread, or fetch its newest `max`.
fn thread(
    settings: &Settings,
    account: Option<&str>,
    account_id: &str,
    args: &ThreadArgs,
    get: &Value,
) -> Result<Next, Failure> {
    let got = Thread::get(get).map_err(jmap_failure)?;
    let thread = the_one(
        got,
        &args.id,
        |thread| &thread.id,
        ThreadGet::METHOD,
        "thread",
    )?;
    let count = thread.email_ids.len();
    if count == 0 {
        return Ok(Next::Complete(thread_result(&args.id, 0, 0, Vec::new())));
    }
    let max = usize::try_from(args.max).expect("THREAD_MAX fits usize");
    let newest = &thread.email_ids[count.saturating_sub(max)..];
    let (request, _) = thread_emails_request(account_id, newest);
    let derived = Args::from_iter([
        (ACCOUNT_KEY.to_owned(), json!(account_id)),
        (EMAIL_IDS_KEY.to_owned(), json!(newest)),
        (MESSAGE_COUNT_KEY.to_owned(), json!(count)),
        (OMITTED_KEY.to_owned(), json!(count - newest.len())),
    ]);
    Ok(Next::Call {
        call: api_step(settings, account, THREAD_EMAILS_STEP, &request),
        derived,
    })
}

/// The thread's fetched summaries, in the thread's order; an email deleted
/// since `Thread/get` answered is counted as omitted.
fn thread_emails(
    state: &InFlight,
    args: &ThreadArgs,
    account_id: &str,
    response: &Response,
) -> Result<Next, Failure> {
    let ids: Vec<String> = state
        .derived
        .get(EMAIL_IDS_KEY)
        .and_then(|ids| serde_json::from_value(ids.clone()).ok())
        .ok_or_else(|| missing_derived(state, EMAIL_IDS_KEY))?;
    let count = derived_u64(state, MESSAGE_COUNT_KEY)?;
    let omitted = derived_u64(state, OMITTED_KEY)?;
    let (_, get) = thread_emails_request(account_id, &ids);
    let emails = Email::get(response.result(&get).map_err(jmap_failure)?)
        .map_err(jmap_failure)?
        .list;
    let found = in_order(&ids, emails, email_id);
    let gone = (ids.len() - found.len()) as u64;
    let messages = found
        .iter()
        .map(|email| Value::Object(summary(email)))
        .collect();
    Ok(Next::Complete(thread_result(
        &args.id,
        count,
        omitted + gone,
        messages,
    )))
}

fn thread_result(id: &str, count: u64, omitted: u64, messages: Vec<Value>) -> Value {
    json!({
        "id": id,
        "message_count": count,
        "messages_omitted": omitted,
        "messages": messages,
    })
}

/// A search or thread entry: the summary headers, at most
/// `SUMMARY_ADDRESSES_MAX` addresses each.
fn summary(email: &Email) -> Args {
    let (mut out, omitted) = headers(email, SUMMARY_ADDRESSES_MAX);
    if omitted > 0 {
        out.insert("recipients_omitted".into(), json!(omitted));
    }
    out
}

/// The headers a summary and a read share, and how many addresses were
/// left out of `from` and `to`.
fn headers(email: &Email, max_addresses: usize) -> (Args, usize) {
    let mut out = Args::new();
    let mut omitted = 0;
    out.insert("id".into(), json!(email.id));
    optional(&mut out, "thread_id", &email.thread_id, ID_MAX_CHARS);
    addresses(&mut out, "from", &email.from, max_addresses, &mut omitted);
    addresses(&mut out, "to", &email.to, max_addresses, &mut omitted);
    optional(&mut out, "subject", &email.subject, SUBJECT_MAX_BYTES);
    optional(
        &mut out,
        "received_at",
        &email.received_at,
        HEADER_MAX_BYTES,
    );
    if let Some(size) = email.size {
        out.insert("size".into(), json!(size));
    }
    if let Some(has_attachment) = email.has_attachment {
        out.insert("has_attachment".into(), json!(has_attachment));
    }
    out.insert("keywords".into(), json!(set_keys(&email.keywords)));
    out.insert("mailbox_ids".into(), json!(set_keys(&email.mailbox_ids)));
    optional(&mut out, "preview", &email.preview, PREVIEW_MAX_BYTES);
    (out, omitted)
}

/// The keys a JMAP set holds `true` for, sorted.
fn set_keys(set: &BTreeMap<String, bool>) -> Vec<&str> {
    set.iter()
        .filter(|(_, held)| **held)
        .map(|(key, _)| key.as_str())
        .collect()
}

/// `list` as at most `max` displayed addresses at `key`, counting the rest
/// in `omitted`; nothing when the list is empty.
fn addresses(out: &mut Args, key: &str, list: &[Address], max: usize, omitted: &mut usize) {
    if list.is_empty() {
        return;
    }
    let shown: Vec<String> = list
        .iter()
        .take(max)
        .map(|address| cut(&address.display(), ADDRESS_MAX_BYTES).0)
        .collect();
    *omitted += list.len().saturating_sub(max);
    out.insert(key.into(), json!(shown));
}

/// Message-id header values at `key`, each cut; nothing when there are none.
fn header_ids(out: &mut Args, key: &str, ids: &[String]) {
    if !ids.is_empty() {
        let ids: Vec<String> = ids.iter().map(|id| cut(id, HEADER_MAX_BYTES).0).collect();
        out.insert(key.into(), json!(ids));
    }
}

#[cfg(test)]
mod tests {
    use brenn_provider_kit::activation::BACKLOG_NS;
    use brenn_provider_kit::testutil::{Answered, MemoryHost, failure, http, nested, truncated};
    use brenn_provider_kit::{ApiCall, ErrorKind, Query};
    use brenn_provider_kit::{
        Bound, CALL_BUDGET, Delivered, Input, ToolBounds, Unreadable, check, hold_probes,
        hold_required, input_schema, probes,
    };
    use fastmail_jmap::session::CORE;
    use fastmail_provider::failure::{CALLER_ERRORS, LIMIT_ERROR, SESSION_SETUP};
    use fastmail_provider::testutil::{
        ACCOUNT_PATTERN, DATE_PATTERN, KEYWORD_PATTERN, account_probes, completed, date_probes,
        incoming, jmap, keyword_probes, row, session_body, settings,
    };
    use serde_json::Map;

    use super::*;

    fn at_api(tool: Provided, args: Value) -> InFlight {
        row(tool, args, API_STEP, json!({ ACCOUNT_KEY: "a1" }))
    }

    fn answered(state: &InFlight, outcome: Result<Value, NestedError>) -> Next {
        advance(state, outcome, &settings())
    }

    fn email(id: &str, received_at: &str) -> Value {
        json!({
            "id": id, "threadId": "t1", "mailboxIds": { "m1": true },
            "keywords": { "$seen": true, "$flagged": false }, "size": 2048,
            "receivedAt": received_at, "from": [{ "name": "Alice", "email": "alice@example.com" }],
            "to": [{ "name": null, "email": "bob@example.com" }], "subject": "Lunch",
            "preview": "Shall we", "hasAttachment": false,
        })
    }

    fn summary_of(id: &str, received_at: &str) -> Value {
        json!({
            "id": id, "thread_id": "t1", "from": ["Alice <alice@example.com>"],
            "to": ["bob@example.com"], "subject": "Lunch", "received_at": received_at,
            "size": 2048, "has_attachment": false, "keywords": ["$seen"],
            "mailbox_ids": ["m1"], "preview": "Shall we",
        })
    }

    fn bounds(tool: Provided) -> ToolBounds<'static> {
        let (keys, required): (&'static [&'static str], &'static [&'static str]) = match tool {
            Provided::FastmailMailboxes => (&[ACCOUNT_ARG], &[]),
            Provided::FastmailSearch => (SearchArgs::KEYS, &[]),
            Provided::FastmailRead => (ReadArgs::KEYS, &["id"]),
            Provided::FastmailThread => (ThreadArgs::KEYS, &["id"]),
        };
        let property = |key: &'static str| {
            let bound = match (tool, key) {
                (Provided::FastmailSearch, "text" | "from" | "to" | "subject") => Bound::Text {
                    max_chars: TEXT_MAX_CHARS,
                    format: None,
                },
                (_, "mailbox" | "id") => Bound::Token {
                    max_chars: ID_MAX_CHARS,
                },
                (_, "after" | "before") => Bound::Pattern(DATE_PATTERN.to_owned()),
                (_, "keyword" | "not_keyword") => Bound::Pattern(KEYWORD_PATTERN.to_owned()),
                (_, "has_attachment") => Bound::Bool,
                (_, ACCOUNT_ARG) => Bound::Pattern(ACCOUNT_PATTERN.to_owned()),
                (_, "limit") => Bound::Integer {
                    min: 1,
                    max: LIMIT_MAX,
                },
                (_, "position") => Bound::Integer {
                    min: 0,
                    max: POSITION_MAX,
                },
                (_, "max_body_bytes") => Bound::Integer {
                    min: BODY_MIN,
                    max: BODY_MAX,
                },
                (_, "max") => Bound::Integer {
                    min: 1,
                    max: THREAD_MAX,
                },
                _ => panic!("{tool:?} takes `{key}`, which no bound here states"),
            };
            (key, bound)
        };
        ToolBounds {
            properties: keys.iter().copied().map(property).collect(),
            required,
        }
    }

    fn admitted_args(tool: Provided) -> Value {
        match tool {
            Provided::FastmailMailboxes | Provided::FastmailSearch => json!({}),
            Provided::FastmailRead | Provided::FastmailThread => json!({ "id": "e1" }),
        }
    }

    fn spec_text() -> String {
        let path = std::env::var("FASTMAIL_MAIL_READ_SPEC").expect("FASTMAIL_MAIL_READ_SPEC");
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
    }

    #[test]
    fn every_tools_schema_states_what_its_parser_enforces() {
        let spec = spec_text();
        let dates = date_probes();
        for tool in Provided::ALL {
            let name = tool.name();
            let bounds = bounds(tool);
            if let Err(error) = check(&input_schema(&spec, name), &bounds) {
                panic!("`{name}`'s input_schema: {error}");
            }
            let plan_with = |args: &Value| plan(tool, args, &settings()).map(drop);
            let base = admitted_args(tool);
            for key in bounds.required {
                hold_required(name, key, &base, plan_with);
            }
            for (key, bound) in &bounds.properties {
                let probes = match *key {
                    "after" | "before" => dates.clone(),
                    "keyword" | "not_keyword" => keyword_probes(),
                    ACCOUNT_ARG => account_probes(),
                    _ => {
                        probes(bound).unwrap_or_else(|| panic!("`{name}`'s `{key}` has no probes"))
                    }
                };
                hold_probes(name, key, &probes, &base, plan_with);
            }
        }
    }

    #[test]
    fn every_tool_asks_for_the_session_first() {
        for tool in Provided::ALL {
            let call = plan(tool, &admitted_args(tool), &settings()).unwrap();
            assert_eq!(call.step, SESSION_STEP);
            assert_eq!(
                call.api,
                ApiCall {
                    provider: "fastmail".to_owned(),
                    account: None,
                    method: "GET".to_owned(),
                    host: "api.fastmail.example.com".to_owned(),
                    path: "/jmap/session".to_owned(),
                    query: Query::new(),
                    headers: BTreeMap::new(),
                    body: None,
                }
            );
        }
        let refused = |tool, args: Value| plan(tool, &args, &settings()).unwrap_err();
        assert_eq!(
            refused(Provided::FastmailSearch, json!({ "after": "last week" })),
            "`after` must be a date (2026-11-08) or a UTC instant (2026-11-08T10:00:00Z)"
        );
        assert_eq!(
            refused(Provided::FastmailRead, json!({})),
            "`id` is required"
        );
        assert!(refused(Provided::FastmailMailboxes, json!({ "x": 1 })).contains("`x`"));
    }

    /// Every nested call of a request names the request's `account`: the
    /// session, the JMAP request and the thread's emails, so every step is
    /// sent with the same key. Omitted, every call names none.
    #[test]
    fn every_nested_call_carries_the_requests_account() {
        let thread_args = |account: Option<&str>| {
            let mut args = json!({ "id": "t1" });
            if let Some(account) = account {
                args["account"] = json!(account);
            }
            args
        };
        for account in [Some("alice-work"), None] {
            let expected = account.map(str::to_owned);
            for tool in Provided::ALL {
                let mut args = admitted_args(tool);
                if let Some(account) = account {
                    args["account"] = json!(account);
                }
                let call = plan(tool, &args, &settings()).unwrap();
                assert_eq!(call.api.account, expected, "{tool:?} session");
                let state = row(tool, args, SESSION_STEP, json!({}));
                let Next::Call { call, .. } = answered(&state, session_body()) else {
                    panic!("{tool:?}: the session step issues the request");
                };
                assert_eq!(call.api.account, expected, "{tool:?} request");
            }
            let state = at_api(Provided::FastmailThread, thread_args(account));
            let got = jmap(json!([["Thread/get", {
                "accountId": "a1", "state": "s", "notFound": [],
                "list": [{ "id": "t1", "emailIds": ["e1", "e2"] }],
            }, "c0"]]));
            let Next::Call { call, .. } = answered(&state, got) else {
                panic!("the thread step fetches its emails");
            };
            assert_eq!(call.step, THREAD_EMAILS_STEP);
            assert_eq!(call.api.account, expected, "thread emails");
        }
        assert_eq!(
            plan(
                Provided::FastmailMailboxes,
                &json!({ "account": "Alice" }),
                &settings()
            )
            .unwrap_err(),
            "`account` must be an account name: lowercase letters, digits and `-`, starting \
             with a letter, not ending in `-`, at most 255 characters"
        );
    }

    #[test]
    fn the_session_step_posts_the_tools_request_to_the_mail_account() {
        let state = row(
            Provided::FastmailSearch,
            json!({ "from": "alice@example.com", "after": "2026-11-01", "keyword": "$Seen",
                    "has_attachment": false, "limit": 5 }),
            SESSION_STEP,
            json!({}),
        );
        let Next::Call { call, derived } = answered(&state, session_body()) else {
            panic!("the session step issues the request");
        };
        assert_eq!(
            derived,
            Map::from_iter([(ACCOUNT_KEY.to_owned(), json!("a1"))])
        );
        assert_eq!(call.step, API_STEP);
        assert_eq!(
            (call.api.method.as_str(), call.api.path.as_str()),
            ("POST", "/jmap/api")
        );
        let body: Value =
            serde_json::from_str(brenn_provider_kit::testutil::text_body(&call.api)).unwrap();
        assert_eq!(
            body,
            json!({
                "using": [CORE, MAIL],
                "methodCalls": [
                    ["Email/query", {
                        "accountId": "a1",
                        "filter": {
                            "from": "alice@example.com",
                            "after": "2026-11-01T00:00:00Z",
                            "hasKeyword": "$seen",
                            "hasAttachment": false,
                        },
                        "sort": [{ "property": "receivedAt", "isAscending": false }],
                        "position": 0, "limit": 5, "calculateTotal": true,
                    }, "c0"],
                    ["Email/get", {
                        "accountId": "a1",
                        "#ids": { "resultOf": "c0", "name": "Email/query", "path": "/ids" },
                        "properties": SUMMARY_PROPERTIES,
                        "fetchTextBodyValues": false,
                    }, "c1"],
                ],
            })
        );
        // One argument is a one-property condition; none is `{}`.
        for (args, filter) in [
            (json!({ "text": "lunch" }), json!({ "text": "lunch" })),
            (json!({}), json!({})),
        ] {
            let state = row(Provided::FastmailSearch, args, SESSION_STEP, json!({}));
            let Next::Call { call, .. } = answered(&state, session_body()) else {
                panic!("issues");
            };
            let body: Value =
                serde_json::from_str(brenn_provider_kit::testutil::text_body(&call.api)).unwrap();
            assert_eq!(body["methodCalls"][0][1]["filter"], filter);
        }
    }

    #[test]
    fn a_session_that_cannot_serve_mail_or_a_dead_token_is_the_operators() {
        let state = row(
            Provided::FastmailMailboxes,
            json!({}),
            SESSION_STEP,
            json!({}),
        );
        let no_mail = json!({ "capabilities": { CORE: {} }, "primaryAccounts": {} }).to_string();
        let Next::Fail(failed) = answered(&state, http(200, &no_mail)) else {
            panic!("fails");
        };
        assert_eq!(failed.kind, ErrorKind::Internal);
        assert_eq!(
            failed.detail,
            format!(
                "the Fastmail token for this account cannot be used here: JMAP session: the \
                 account does not advertise urn:ietf:params:jmap:mail; {SESSION_SETUP}"
            )
        );
        for status in [401, 403] {
            assert_eq!(
                answered(&state, http(status, "{\"secret\":\"x\"}")),
                Next::Fail(internal(format!(
                    "Fastmail refused the token for this account (HTTP {status}): the \
                     operator must replace its key file and restart Brenn"
                )))
            );
        }
    }

    #[test]
    fn mailboxes_are_listed_in_display_order() {
        let state = at_api(Provided::FastmailMailboxes, json!({}));
        let list = json!([
            { "id": "m2", "name": "Receipts", "parentId": "m3", "role": null, "sortOrder": 5,
              "totalEmails": 4, "unreadEmails": 1, "totalThreads": 3, "unreadThreads": 1 },
            { "id": "m1", "name": "Inbox", "role": "inbox", "sortOrder": 1,
              "totalEmails": 10, "unreadEmails": 2, "totalThreads": 8, "unreadThreads": 2 },
        ]);
        let result = completed(answered(
            &state,
            jmap(
                json!([["Mailbox/get", { "accountId": "a1", "list": list, "notFound": [] }, "c0"]]),
            ),
        ));
        assert_eq!(
            result,
            json!({ "account_id": "a1", "mailboxes": [
                { "id": "m1", "name": "Inbox", "role": "inbox", "total_emails": 10,
                  "unread_emails": 2, "total_threads": 8, "unread_threads": 2 },
                { "id": "m2", "name": "Receipts", "parent_id": "m3", "total_emails": 4,
                  "unread_emails": 1, "total_threads": 3, "unread_threads": 1 },
            ] })
        );
    }

    #[test]
    fn a_search_answers_summaries_in_query_order_and_the_next_position() {
        let state = at_api(Provided::FastmailSearch, json!({ "limit": 2 }));
        let result = completed(answered(
            &state,
            jmap(json!([
                ["Email/get", { "accountId": "a1", "notFound": [],
                    "list": [email("e2", "2026-11-07T09:00:00Z"), email("e1", "2026-11-08T09:00:00Z")] }, "c1"],
                ["Email/query", { "accountId": "a1", "ids": ["e1", "e2"], "position": 0, "total": 7 }, "c0"],
            ])),
        ));
        assert_eq!(
            result,
            json!({ "position": 0, "total": 7, "next_position": 2, "messages": [
                summary_of("e1", "2026-11-08T09:00:00Z"),
                summary_of("e2", "2026-11-07T09:00:00Z"),
            ] })
        );
    }

    #[test]
    fn a_read_answers_headers_text_and_attachments() {
        let state = at_api(
            Provided::FastmailRead,
            json!({ "id": "e1", "max_body_bytes": 200 }),
        );
        let mut full = email("e1", "2026-11-08T09:00:00Z");
        let extra = json!({
            "cc": [{ "email": "carol@example.com" }], "bcc": null, "replyTo": [],
            "sentAt": "2026-11-08T10:00:00+01:00", "messageId": ["m1@example.com"],
            "inReplyTo": null, "references": ["r0@example.com"],
            "textBody": [
                { "partId": "1", "blobId": "b1", "size": 10, "type": "text/plain" },
                { "partId": "2", "blobId": "b2", "size": 20, "type": "text/html" },
            ],
            "attachments": [{ "partId": "3", "blobId": "b3", "size": 99, "name": "a.pdf",
                              "type": "application/pdf", "disposition": "attachment" }],
            "bodyValues": {
                "1": { "value": "Plain part.", "isTruncated": false },
                "2": { "value": "<p>Html &amp; part</p>", "isEncodingProblem": true },
            },
        });
        full.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let get =
            json!([["Email/get", { "accountId": "a1", "list": [full], "notFound": [] }, "c0"]]);
        let mut expected = summary_of("e1", "2026-11-08T09:00:00Z");
        expected.as_object_mut().unwrap().extend(
            json!({
                "cc": ["carol@example.com"], "sent_at": "2026-11-08T10:00:00+01:00",
                "message_id": ["m1@example.com"], "references": ["r0@example.com"],
                "text": "Plain part.\n\nHtml & part", "text_source": "mixed",
                "body_truncated": false, "encoding_problem": true,
                "attachments": [{ "name": "a.pdf", "content_type": "application/pdf",
                    "size": 99, "disposition": "attachment", "blob_id": "b3", "inline": false }],
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        assert_eq!(completed(answered(&state, jmap(get))), expected);

        let missing =
            json!([["Email/get", { "accountId": "a1", "list": [], "notFound": ["e1"] }, "c0"]]);
        assert_eq!(
            answered(&state, jmap(missing)),
            Next::Fail(failure(ErrorKind::InvalidArgs, "no email with id `e1`"))
        );
    }

    #[test]
    fn a_read_cuts_its_text_at_max_body_bytes() {
        let mut long = email("e1", "2026-11-08T09:00:00Z");
        long["textBody"] = json!([{ "partId": "1", "size": 500, "type": "text/plain" }]);
        long["bodyValues"] = json!({ "1": { "value": "x".repeat(500) } });
        let state = at_api(
            Provided::FastmailRead,
            json!({ "id": "e1", "max_body_bytes": 200 }),
        );
        let get =
            json!([["Email/get", { "accountId": "a1", "list": [long], "notFound": [] }, "c0"]]);
        let result = completed(answered(&state, jmap(get)));
        assert_eq!(result["text"], json!(format!("{}…", "x".repeat(200))));
        assert_eq!(result["body_truncated"], json!(true));
        assert_eq!(result["text_source"], json!("text/plain"));
    }

    /// A read of `e1` answered with `email`'s properties plus `extra`.
    fn read_of(extra: Value) -> Next {
        let mut full = email("e1", "2026-11-08T09:00:00Z");
        full.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let state = at_api(Provided::FastmailRead, json!({ "id": "e1" }));
        answered(
            &state,
            jmap(
                json!([["Email/get", { "accountId": "a1", "list": [full], "notFound": [] }, "c0"]]),
            ),
        )
    }

    #[test]
    fn a_read_bounds_its_recipients_references_and_attachments() {
        let addresses = |n: usize| -> Vec<Value> {
            (0..n)
                .map(|i| json!({ "email": format!("u{i}@example.com") }))
                .collect()
        };
        let references: Vec<String> = (0..25).map(|i| format!("r{i}")).collect();
        let attachments: Vec<Value> = (0..101)
            .map(|i| {
                let mut part = json!({ "partId": format!("p{i}"), "size": 1,
                                       "type": "application/pdf" });
                if i == 0 {
                    part["cid"] = json!("c0@example.com");
                }
                part
            })
            .collect();
        let result = completed(read_of(json!({
            "cc": addresses(51), "references": references, "attachments": attachments,
            "textBody": [], "bodyValues": {},
        })));
        assert_eq!(result["cc"].as_array().unwrap().len(), 50);
        assert_eq!(result["recipients_omitted"], json!(1));
        assert_eq!(result["references"], json!(references[5..]));
        assert_eq!(result["references_omitted"], json!(5));
        assert_eq!(result["attachments"].as_array().unwrap().len(), 100);
        assert_eq!(result["attachments_omitted"], json!(1));
        assert_eq!(result["attachments"][0]["inline"], json!(true));
        assert_eq!(result["attachments"][1]["inline"], json!(false));

        let state = at_api(Provided::FastmailSearch, json!({}));
        let mut many = email("e1", "2026-11-08T09:00:00Z");
        many["to"] = json!(addresses(7));
        let result = completed(answered(
            &state,
            jmap(json!([
                ["Email/query", { "accountId": "a1", "ids": ["e1"], "position": 0 }, "c0"],
                ["Email/get", { "accountId": "a1", "list": [many], "notFound": [] }, "c1"],
            ])),
        ));
        assert_eq!(result["messages"][0]["to"].as_array().unwrap().len(), 5);
        assert_eq!(result["messages"][0]["recipients_omitted"], json!(2));
    }

    #[test]
    fn a_read_says_where_its_text_came_from_and_what_was_cut() {
        let text = |extra: Value| {
            let result = completed(read_of(extra));
            (
                result["text"].clone(),
                result["text_source"].clone(),
                result["body_truncated"].clone(),
            )
        };
        assert_eq!(
            text(json!({ "textBody": [], "bodyValues": {} })),
            (json!(""), json!("none"), json!(false))
        );
        assert_eq!(
            text(json!({
                "textBody": [{ "partId": "1", "size": 9, "type": "TEXT/HTML" }],
                "bodyValues": { "1": { "value": "<p>Hi</p>" } },
            })),
            (json!("Hi"), json!("text/html"), json!(false))
        );
        assert_eq!(
            text(json!({
                "textBody": [{ "partId": "1", "size": 9000, "type": "text/plain" }],
                "bodyValues": { "1": { "value": "short", "isTruncated": true } },
            })),
            (json!("short"), json!("text/plain"), json!(true))
        );
        assert_eq!(
            read_of(json!({
                "textBody": [{ "partId": "1", "size": 9, "type": "text/plain" }],
                "bodyValues": {},
            })),
            Next::Fail(internal(
                "Fastmail's Email/get gave a text part of email `e1` no body value"
            ))
        );
    }

    #[test]
    fn inline_media_between_text_parts_is_shown_in_place_and_listed() {
        let result = completed(read_of(json!({
            "textBody": [
                { "partId": "1", "size": 5, "type": "text/plain" },
                { "partId": "2", "blobId": "b2", "size": 4096, "type": "image/jpeg",
                  "name": "photo.jpg", "disposition": "inline" },
                { "partId": "3", "size": 6, "type": "text/plain" },
            ],
            "bodyValues": { "1": { "value": "Look:" }, "3": { "value": "Nice?" } },
            "attachments": [{ "partId": "4", "blobId": "b4", "size": 9, "type": "text/csv",
                              "name": "a.csv", "disposition": "attachment" }],
        })));
        assert_eq!(
            result["text"],
            json!(
                "Look:\n\n[inline image/jpeg \"photo.jpg\", 4096 bytes; listed in attachments]\
                 \n\nNice?"
            )
        );
        assert_eq!(result["text_source"], json!("text/plain"));
        assert_eq!(
            result["attachments"],
            json!([
                { "name": "a.csv", "content_type": "text/csv", "size": 9,
                  "disposition": "attachment", "blob_id": "b4", "inline": false },
                { "name": "photo.jpg", "content_type": "image/jpeg", "size": 4096,
                  "disposition": "inline", "blob_id": "b2", "inline": true },
            ])
        );
        // A media part Fastmail also lists as an attachment is listed once.
        let result = completed(read_of(json!({
            "textBody": [{ "partId": "2", "size": 7, "type": "video/mp4" }],
            "bodyValues": {},
            "attachments": [{ "partId": "2", "size": 7, "type": "video/mp4", "cid": "v" }],
        })));
        assert_eq!(
            result["text"],
            json!("[inline video/mp4, 7 bytes; listed in attachments]")
        );
        assert_eq!(result["text_source"], json!("none"));
        assert_eq!(
            result["attachments"],
            json!([{ "content_type": "video/mp4", "size": 7, "inline": true }])
        );
    }

    #[test]
    fn next_position_is_offered_only_while_more_remain() {
        let state = at_api(Provided::FastmailSearch, json!({}));
        for (position, ids, total, expected) in [
            (
                5,
                json!(["e1", "e2"]),
                json!(7),
                json!({ "position": 5, "total": 7 }),
            ),
            (
                0,
                json!(["e1", "e2"]),
                Value::Null,
                json!({ "position": 0 }),
            ),
            (
                10,
                json!([]),
                json!(7),
                json!({ "position": 10, "total": 7 }),
            ),
            (
                3,
                json!(["e1"]),
                json!(9),
                json!({ "position": 3, "total": 9, "next_position": 4 }),
            ),
        ] {
            let mut query = json!({ "accountId": "a1", "ids": ids, "position": position });
            if !total.is_null() {
                query["total"] = total;
            }
            let mut result = completed(answered(
                &state,
                jmap(json!([
                    ["Email/query", query, "c0"],
                    ["Email/get", { "accountId": "a1", "list": [], "notFound": [] }, "c1"],
                ])),
            ));
            assert_eq!(result["messages"], json!([]));
            result.as_object_mut().unwrap().remove("messages");
            assert_eq!(result, expected, "position {position}, ids {ids}");
        }
    }

    #[test]
    fn an_impossible_answer_or_a_corrupt_row_fails_loudly() {
        let empty_get = |name: &str| {
            jmap(json!([[name, { "accountId": "a1", "list": [], "notFound": [] }, "c0"]]))
        };
        let cases = [
            (
                at_api(Provided::FastmailRead, json!({ "id": "e1" })),
                empty_get("Email/get"),
                "neither email `e1` nor its absence",
            ),
            (
                at_api(Provided::FastmailThread, json!({ "id": "t1" })),
                empty_get("Thread/get"),
                "neither thread `t1` nor its absence",
            ),
            (
                row(Provided::FastmailMailboxes, json!({}), API_STEP, json!({})),
                empty_get("Mailbox/get"),
                "carries no `account_id`",
            ),
            (
                row(
                    Provided::FastmailThread,
                    json!({ "id": "t1" }),
                    THREAD_EMAILS_STEP,
                    json!({ ACCOUNT_KEY: "a1", EMAIL_IDS_KEY: "x", MESSAGE_COUNT_KEY: 1,
                            OMITTED_KEY: 0 }),
                ),
                empty_get("Email/get"),
                "carries no `email_ids`",
            ),
            (
                row(
                    Provided::FastmailThread,
                    json!({ "id": "t1" }),
                    THREAD_EMAILS_STEP,
                    json!({ ACCOUNT_KEY: "a1", EMAIL_IDS_KEY: ["e1"], MESSAGE_COUNT_KEY: 1 }),
                ),
                empty_get("Email/get"),
                "carries no `messages_omitted`",
            ),
            (
                at_api(Provided::FastmailRead, json!({ "bogus": 1 })),
                empty_get("Email/get"),
                "stored fastmail-read arguments no longer parse",
            ),
            (
                at_api(Provided::FastmailRead, json!({ "id": "e1" })),
                Ok(json!({ "error": "credential_invalid", "detail": "no account" })),
                "credential relay",
            ),
        ];
        for (state, outcome, says) in cases {
            let Next::Fail(failed) = answered(&state, outcome) else {
                panic!("{says}: does not fail");
            };
            assert_eq!(failed.kind, ErrorKind::Internal, "{says}: {failed:?}");
            assert!(failed.detail.contains(says), "{says}: {failed:?}");
        }
    }

    #[test]
    fn fastmails_errors_map_to_the_callers_or_the_operators() {
        let state = at_api(Provided::FastmailSearch, json!({}));
        let method_error =
            |kind: &str| jmap(json!([["error", { "type": kind, "description": "why" }, "c0"]]));
        for kind in CALLER_ERRORS {
            assert_eq!(
                answered(&state, method_error(kind)),
                Next::Fail(failure(
                    ErrorKind::InvalidArgs,
                    &format!("Fastmail: Email/query failed: {kind}: why")
                ))
            );
        }
        for kind in [
            "accountNotFound",
            "unknownMethod",
            "serverFail",
            "somethingNew",
        ] {
            assert_eq!(
                answered(&state, method_error(kind)),
                Next::Fail(internal(format!(
                    "Fastmail: Email/query failed: {kind}: why"
                )))
            );
        }
        let problem = |kind: &str| {
            json!({ "type": kind, "status": 400, "detail": "too many",
                    "limit": "maxConcurrentRequests" })
            .to_string()
        };
        assert_eq!(
            answered(&state, http(400, &problem(LIMIT_ERROR))),
            Next::Fail(internal(
                "Fastmail: JMAP request refused: urn:ietf:params:jmap:error:limit (HTTP 400): \
                 too many; try again later"
            ))
        );
        let Next::Fail(not_json) = answered(
            &state,
            http(400, &problem("urn:ietf:params:jmap:error:notJSON")),
        ) else {
            panic!("fails");
        };
        assert_eq!(not_json.kind, ErrorKind::Internal);
        for (status, detail) in [
            (400, "Fastmail answered HTTP 400"),
            (404, "Fastmail answered HTTP 404"),
            (500, "Fastmail answered HTTP 500"),
            (
                429,
                "Fastmail is rate-limiting this deployment (HTTP 429); try again later",
            ),
        ] {
            assert_eq!(
                answered(&state, http(status, "<html>secret</html>")),
                Next::Fail(internal(detail))
            );
        }
        assert_eq!(
            answered(&state, http(204, "")),
            Next::Fail(internal(
                "Fastmail answered HTTP 204 with no body, which JMAP never does"
            ))
        );
        let Next::Fail(shape) = answered(&state, jmap(json!([]))) else {
            panic!("fails");
        };
        assert!(
            shape
                .detail
                .starts_with("Fastmail: malformed JMAP response"),
            "{shape:?}"
        );
        let Next::Fail(denied) = answered(&state, nested("denied", "no clause admits it")) else {
            panic!("fails");
        };
        assert_eq!(denied.kind, ErrorKind::Denied);
    }

    #[test]
    fn every_step_answers_an_unreadable_read_by_its_shape() {
        let refusal = "result exceeds [messaging] max_body_bytes";
        let cases = [
            (
                row(Provided::FastmailSearch, json!({}), SESSION_STEP, json!({})),
                None,
            ),
            (at_api(Provided::FastmailMailboxes, json!({})), None),
            (
                at_api(Provided::FastmailSearch, json!({})),
                Some(LIMIT_HINT),
            ),
            (
                at_api(Provided::FastmailRead, json!({ "id": "e1" })),
                Some(BODY_HINT),
            ),
            (
                at_api(Provided::FastmailThread, json!({ "id": "t1" })),
                None,
            ),
            (
                row(
                    Provided::FastmailThread,
                    json!({ "id": "t1" }),
                    THREAD_EMAILS_STEP,
                    json!({}),
                ),
                Some(MAX_HINT),
            ),
        ];
        for (state, hint) in cases {
            for (outcome, why) in [
                (
                    truncated(200, json!(300000), "{"),
                    Unreadable::Cut("300000 bytes".to_owned()),
                ),
                (
                    nested("internal", refusal),
                    Unreadable::Refused(refusal.to_owned()),
                ),
            ] {
                assert_eq!(
                    answered(&state, outcome),
                    Next::Fail(why.failure(API_NAME, hint)),
                    "{} step {}",
                    state.tool.name(),
                    state.step
                );
            }
        }
    }

    #[test]
    fn a_step_a_tool_does_not_have_is_refused_as_such() {
        for (tool, args, step) in [
            (Provided::FastmailMailboxes, json!({}), 3),
            (Provided::FastmailSearch, json!({}), 3),
            (Provided::FastmailRead, json!({ "id": "e1" }), 3),
            (Provided::FastmailThread, json!({ "id": "t1" }), 4),
        ] {
            let state = row(tool, args, step, json!({ ACCOUNT_KEY: "a1" }));
            assert_eq!(
                answered(&state, jmap(json!([]))),
                Next::Fail(no_such_step(tool, step))
            );
        }
    }

    #[test]
    fn a_result_over_the_ceiling_is_refused_with_the_tools_hint() {
        let state = at_api(Provided::FastmailSearch, json!({ "limit": 25 }));
        let ids: Vec<String> = (0..25).map(|i| format!("e{i}")).collect();
        let list: Vec<Value> = ids
            .iter()
            .map(|id| {
                let mut email = email(id, "2026-11-08T09:00:00Z");
                email["to"] = json!(vec![json!({ "email": "x".repeat(399) }); 5]);
                email["subject"] = json!("s".repeat(1000));
                email["keywords"] = json!(
                    (0..100)
                        .map(|k| (format!("k{k:0>250}"), json!(true)))
                        .collect::<Map<_, _>>()
                );
                email
            })
            .collect();
        let Next::Fail(refused) = answered(
            &state,
            jmap(json!([
                ["Email/query", { "accountId": "a1", "ids": ids, "position": 0 }, "c0"],
                ["Email/get", { "accountId": "a1", "list": list, "notFound": [] }, "c1"],
            ])),
        ) else {
            panic!("refused");
        };
        assert!(
            refused.detail.ends_with(&format!("; {LIMIT_HINT}")),
            "{refused:?}"
        );
    }

    #[test]
    fn a_thread_fetches_only_its_newest_max_emails() {
        let state = at_api(Provided::FastmailThread, json!({ "id": "t1", "max": 30 }));
        let ids: Vec<String> = (0..50).map(|i| format!("e{i:02}")).collect();
        let Next::Call { call, derived } = answered(
            &state,
            jmap(json!([["Thread/get", { "accountId": "a1", "notFound": [],
                "list": [{ "id": "t1", "emailIds": ids }] }, "c0"]])),
        ) else {
            panic!("issues step 3");
        };
        let newest = &ids[20..];
        assert_eq!(call.step, THREAD_EMAILS_STEP);
        assert_eq!(call.api.path, "/jmap/api");
        let body: Value =
            serde_json::from_str(brenn_provider_kit::testutil::text_body(&call.api)).unwrap();
        assert_eq!(
            body["methodCalls"],
            json!([["Email/get", { "accountId": "a1", "ids": newest,
                "properties": SUMMARY_PROPERTIES, "fetchTextBodyValues": false }, "c0"]])
        );
        assert_eq!(
            Value::Object(derived),
            json!({ ACCOUNT_KEY: "a1", EMAIL_IDS_KEY: newest, MESSAGE_COUNT_KEY: 50,
                    OMITTED_KEY: 20 })
        );
    }

    #[test]
    fn an_unknown_or_empty_thread_answers_at_once() {
        let state = at_api(Provided::FastmailThread, json!({ "id": "t1" }));
        assert_eq!(
            answered(
                &state,
                jmap(
                    json!([["Thread/get", { "accountId": "a1", "list": [], "notFound": ["t1"] }, "c0"]])
                )
            ),
            Next::Fail(failure(ErrorKind::InvalidArgs, "no thread with id `t1`"))
        );
        assert_eq!(
            completed(answered(
                &state,
                jmap(json!([["Thread/get", { "accountId": "a1", "notFound": [],
                    "list": [{ "id": "t1", "emailIds": [] }] }, "c0"]]))
            )),
            json!({ "id": "t1", "message_count": 0, "messages_omitted": 0, "messages": [] })
        );
    }

    #[test]
    fn a_threads_emails_come_in_its_order_and_deleted_ones_count_as_omitted() {
        let state = row(
            Provided::FastmailThread,
            json!({ "id": "t1", "max": 3 }),
            THREAD_EMAILS_STEP,
            json!({ ACCOUNT_KEY: "a1", EMAIL_IDS_KEY: ["e1", "e2", "e3"],
                    MESSAGE_COUNT_KEY: 5, OMITTED_KEY: 2 }),
        );
        let result = completed(answered(
            &state,
            jmap(
                json!([["Email/get", { "accountId": "a1", "notFound": ["e2"], "list": [
                email("e3", "2026-11-08T09:00:00Z"), email("e1", "2026-11-06T09:00:00Z"),
            ] }, "c0"]]),
            ),
        ));
        assert_eq!(
            result,
            json!({ "id": "t1", "message_count": 5, "messages_omitted": 3, "messages": [
                summary_of("e1", "2026-11-06T09:00:00Z"),
                summary_of("e3", "2026-11-08T09:00:00Z"),
            ] })
        );
    }

    #[test]
    fn a_request_runs_session_then_api_then_completes_and_excess_waits() {
        let host = MemoryHost::new();
        let id = "q".repeat(32);
        let activate = |requests, results| {
            host.activate::<Fastmail>(
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
        activate(vec![incoming(&id, "fastmail-mailboxes", json!({}))], vec![]);
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call.path, "/jmap/session");
        activate(vec![], vec![deliver(&calls[0].call_id, session_body())]);
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            (calls[0].call.method.as_str(), calls[0].call.path.as_str()),
            ("POST", "/jmap/api")
        );
        let list = json!([{ "id": "m1", "name": "Inbox", "role": "inbox" }]);
        activate(
            vec![],
            vec![deliver(
                &calls[0].call_id,
                jmap(
                    json!([["Mailbox/get", { "accountId": "a1", "list": list, "notFound": [] }, "c0"]]),
                ),
            )],
        );
        let completions = host.take_completions();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, id);
        let Answered::Value(value) = &completions[0].answer else {
            panic!("{completions:?}");
        };
        assert_eq!(value["mailboxes"][0]["name"], json!("Inbox"));
        assert!(host.take_calls().is_empty());

        let requests = (0..=CALL_BUDGET)
            .map(|i| incoming(&format!("{i:032}"), "fastmail-read", json!({ "id": "e1" })))
            .collect();
        activate(requests, vec![]);
        assert_eq!(host.take_calls().len(), CALL_BUDGET);
        assert_eq!(host.rows(BACKLOG_NS).len(), 1);
    }
}
