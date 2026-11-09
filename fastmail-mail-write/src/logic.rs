//! The write provider's decisions, in `std`, `serde_json`, the JMAP crate,
//! the shared provider crate and `brenn-provider-kit`: validating a tool's
//! arguments, building each JMAP `/set` as `api-request` arguments, and
//! reading Fastmail's answer into the tool's result. Nothing here names the
//! guest SDK, so the whole module runs on the host under test. Tools are the
//! generated `Provided` enum, whose SDK-facing parts are wasm32-only.
//!
//! Every tool asks for the JMAP session first (step 1), which names the mail
//! account. Organize and the mailbox tools then post one `/set` (step 2), the
//! write; a draft looks up its sender and mailbox (step 2) and then writes
//! (step 3), in `draft`. A failure that leaves the write's outcome unknown —
//! a lost, cut or malformed answer, or a server failure — carries the tool's
//! hint saying what may already have happened; Fastmail's refusal, of the
//! request, the method or the change, does not.

// The host test's crate root is this file, so it declares the generated
// module itself; in the component, `lib.rs` does.
#[cfg(test)]
mod spec;

// `FastmailDraft` is this module's child in the component and the host test
// alike: the path resolves beside this file whether it is `crate::logic` or
// the crate root.
#[path = "draft.rs"]
mod draft;

use std::collections::BTreeSet;

use brenn_provider_kit::{
    Args, COMPACT_RESULT_CEILING, Call, Failure, Logic, NestedError, Next, Tool, bounded, capped,
    cut, flag, internal, no_such_step, object, serialized_len,
};
use fastmail_jmap::{
    CallId, EmailSet, MailboxCreate, MailboxSet, Patch, Request, Response, SetError,
};
use fastmail_provider::args::{ACCOUNT_ARG, account_arg, id_list, is_keyword, keyword_rule, token};
use fastmail_provider::failure::{
    MAILBOX_MEMBERSHIP_HINT, StepFailure, concerns_mailbox_membership, derived_str, jmap_failure,
    set_failure, step_answer, write_method_failure,
};
use fastmail_provider::settings::{API_NAME, Settings};
use fastmail_provider::steps::{ACCOUNT_KEY, SESSION_STEP, api_step, read_session, session_call};
use serde_json::{Map, Value, json};

use crate::spec::Provided;
use draft::{DRAFT_SET_STEP, DRAFT_UNKNOWN_HINT, DraftArgs};

/// The most messages one organize changes.
const ORGANIZE_IDS_MAX: usize = 500;
/// The most entries in each of organize's mailbox and keyword lists.
const ORGANIZE_LIST_MAX: usize = 20;
/// The most bytes organize's `update` object may serialise to. No per-field
/// bound fits the host's 64 KiB cap on a nested call's arguments, so the
/// whole object is bounded. The JMAP body is a string in the call's
/// arguments, so the object is escaped once more there; every id and keyword
/// is a token, so only the object's quotes widen, each to `\"`. The densest
/// quoting an `update` reaches is about one quote in seven bytes (short ids
/// each carrying a three-keyword patch: `"ab":{"keywords/x":true,…}` is 8
/// quotes in about 60 bytes), so 30 000 bytes escape to about 34 300; with
/// `account` at 255, the envelope and the call's own fields the call stays
/// well under 65 536.
const ORGANIZE_UPDATE_MAX_BYTES: usize = 30_000;
/// The most raw bytes of Fastmail's reason kept per refused message. The
/// reason has passed through the JMAP crate's one-line rendering, so it holds
/// no control character, and every byte serialises to itself except `"` and
/// `\`, which double: at most 240 bytes per reason in the result.
const NOT_UPDATED_REASON_MAX_BYTES: usize = 120;
/// The most characters a mailbox name may hold.
const MAILBOX_NAME_MAX_CHARS: usize = 255;

/// The step after the session: the `/set`, or a draft's lookup.
const API_STEP: u8 = 2;

/// What a lost answer at each write step leaves the caller to know.
const ORGANIZE_UNKNOWN_HINT: &str =
    "the change may already have been applied; asking again with the same arguments is safe";
const MAILBOX_UPDATE_UNKNOWN_HINT: &str = ORGANIZE_UNKNOWN_HINT;
const MAILBOX_CREATE_UNKNOWN_HINT: &str = "the mailbox may have been created before its answer \
     was lost: list the mailboxes with FastmailMailboxes before asking again, or the user has two";

/// The creation id of mailbox create's one new mailbox.
const MAILBOX_CREATE_ID: &str = "mailbox";

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
pub struct FastmailWrite;

impl Logic for FastmailWrite {
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
    Set(SetArgs),
    Draft(DraftArgs),
}

/// The arguments of a tool whose write is one `/set` after the session.
#[derive(Clone, Debug, PartialEq)]
enum SetArgs {
    Organize(OrganizeArgs),
    MailboxCreate(MailboxCreateArgs),
    MailboxUpdate(MailboxUpdateArgs),
}

#[derive(Clone, Debug, PartialEq)]
struct OrganizeArgs {
    ids: Vec<String>,
    /// The one change every message gets.
    patch: Patch,
}

impl OrganizeArgs {
    const KEYS: &[&str] = &[
        "ids",
        "add_mailboxes",
        "remove_mailboxes",
        "add_keywords",
        "remove_keywords",
        ACCOUNT_ARG,
    ];

    fn parse(map: &Args) -> Result<OrganizeArgs, String> {
        let ids =
            id_list(map, "ids", ORGANIZE_IDS_MAX, "message ids")?.ok_or("`ids` is required")?;
        let mailboxes = |key| id_list(map, key, ORGANIZE_LIST_MAX, "mailbox ids");
        let add_mailboxes = mailboxes("add_mailboxes")?.unwrap_or_default();
        let remove_mailboxes = mailboxes("remove_mailboxes")?.unwrap_or_default();
        let add_keywords = keyword_list(map, "add_keywords")?;
        let remove_keywords = keyword_list(map, "remove_keywords")?;
        if add_mailboxes.is_empty()
            && remove_mailboxes.is_empty()
            && add_keywords.is_empty()
            && remove_keywords.is_empty()
        {
            return Err("give at least one of `add_mailboxes`, `remove_mailboxes`, \
                        `add_keywords` and `remove_keywords`"
                .to_owned());
        }
        disjoint(
            ("add_mailboxes", &add_mailboxes),
            ("remove_mailboxes", &remove_mailboxes),
        )?;
        disjoint(
            ("add_keywords", &add_keywords),
            ("remove_keywords", &remove_keywords),
        )?;
        let mut patch = Patch::new();
        for (list, member) in [(&add_mailboxes, true), (&remove_mailboxes, false)] {
            for id in list {
                patch = patch.mailbox(id, member);
            }
        }
        for (list, set) in [(&add_keywords, true), (&remove_keywords, false)] {
            for keyword in list {
                patch = patch.keyword(keyword, set);
            }
        }
        let args = OrganizeArgs { ids, patch };
        if serialized_len(&args.update_value()) > ORGANIZE_UPDATE_MAX_BYTES {
            return Err(
                "`ids` and the changes together are too wide for one request; ask for \
                        fewer `ids`"
                    .to_owned(),
            );
        }
        Ok(args)
    }

    /// The `/set`'s `update`: the patch under every id.
    fn update(&self) -> Vec<(String, Patch)> {
        self.ids
            .iter()
            .map(|id| (id.clone(), self.patch.clone()))
            .collect()
    }

    fn update_value(&self) -> Value {
        Value::Object(
            self.ids
                .iter()
                .map(|id| (id.clone(), self.patch.to_value()))
                .collect(),
        )
    }
}

/// A list of 1 to `ORGANIZE_LIST_MAX` distinct keywords at `key`, each as
/// `is_keyword` admits it, lower-cased; absent is empty.
fn keyword_list(map: &Args, key: &str) -> Result<Vec<String>, String> {
    let Some(value) = map.get(key) else {
        return Ok(Vec::new());
    };
    let items = match value {
        Value::Array(items) if (1..=ORGANIZE_LIST_MAX).contains(&items.len()) => items,
        _ => {
            return Err(format!(
                "`{key}` must be an array of 1 to {ORGANIZE_LIST_MAX} keywords"
            ));
        }
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let keyword = match item {
            Value::String(keyword) if is_keyword(keyword) => keyword,
            _ => return Err(format!("each of `{key}` must be {}", keyword_rule())),
        };
        if !seen.insert(keyword.as_str()) {
            return Err(format!("`{key}` names {keyword:?} twice"));
        }
        out.push(keyword.to_ascii_lowercase());
    }
    Ok(out)
}

/// Refuses an entry both an `add` list and its `remove` list name.
fn disjoint(add: (&str, &[String]), remove: (&str, &[String])) -> Result<(), String> {
    match add.1.iter().find(|entry| remove.1.contains(entry)) {
        Some(both) => Err(format!("`{}` and `{}` both name {both:?}", add.0, remove.0)),
        None => Ok(()),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct MailboxCreateArgs {
    name: String,
    parent_id: Option<String>,
}

impl MailboxCreateArgs {
    const KEYS: &[&str] = &["name", "parent_id", ACCOUNT_ARG];

    fn parse(map: &Args) -> Result<MailboxCreateArgs, String> {
        Ok(MailboxCreateArgs {
            name: one_line(map, "name", MAILBOX_NAME_MAX_CHARS)?.ok_or("`name` is required")?,
            parent_id: token(map, "parent_id")?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct MailboxUpdateArgs {
    id: String,
    name: Option<String>,
    /// The new parent: `Some(None)` is the top level; `None` leaves it.
    parent: Option<Option<String>>,
}

impl MailboxUpdateArgs {
    const KEYS: &[&str] = &["id", "name", "parent_id", "top_level", ACCOUNT_ARG];

    fn parse(map: &Args) -> Result<MailboxUpdateArgs, String> {
        let id = token(map, "id")?.ok_or("`id` is required")?;
        let name = one_line(map, "name", MAILBOX_NAME_MAX_CHARS)?;
        let parent_id = token(map, "parent_id")?;
        let top_level = flag(map, "top_level")?;
        let parent = match (parent_id, top_level) {
            (Some(_), true) => {
                return Err("`parent_id` and `top_level` cannot be given together".to_owned());
            }
            (Some(parent_id), false) => Some(Some(parent_id)),
            (None, true) => Some(None),
            (None, false) => None,
        };
        if name.is_none() && parent.is_none() {
            return Err(
                "give at least one of `name`, `parent_id` and `top_level` (true)".to_owned(),
            );
        }
        Ok(MailboxUpdateArgs { id, name, parent })
    }

    fn patch(&self) -> Patch {
        let mut patch = Patch::new();
        if let Some(name) = &self.name {
            patch = patch.property("name", json!(name));
        }
        if let Some(parent) = &self.parent {
            patch = patch.property("parentId", json!(parent));
        }
        patch
    }

    /// The properties the update changes, in the caller's terms.
    fn changed(&self) -> Vec<&'static str> {
        let mut changed = Vec::new();
        if self.name.is_some() {
            changed.push("name");
        }
        if self.parent.is_some() {
            changed.push("parent_id");
        }
        changed
    }
}

/// A one-line text argument at `key`: as `capped` admits it at `max_chars`,
/// with no control character.
fn one_line(map: &Args, key: &str, max_chars: usize) -> Result<Option<String>, String> {
    match capped(map, key, max_chars)? {
        None => Ok(None),
        Some(text) if text.chars().any(char::is_control) => {
            Err(format!("`{key}` must not hold a control character"))
        }
        Some(text) => Ok(Some(text.to_owned())),
    }
}

fn keys(tool: Provided) -> &'static [&'static str] {
    match tool {
        Provided::FastmailOrganize => OrganizeArgs::KEYS,
        Provided::FastmailMailboxCreate => MailboxCreateArgs::KEYS,
        Provided::FastmailMailboxUpdate => MailboxUpdateArgs::KEYS,
        Provided::FastmailDraft => DraftArgs::KEYS,
    }
}

fn parse(tool: Provided, args: &Value) -> Result<Parsed, String> {
    let map = object(args, keys(tool))?;
    let tool = match tool {
        Provided::FastmailOrganize => ToolArgs::Set(SetArgs::Organize(OrganizeArgs::parse(map)?)),
        Provided::FastmailMailboxCreate => {
            ToolArgs::Set(SetArgs::MailboxCreate(MailboxCreateArgs::parse(map)?))
        }
        Provided::FastmailMailboxUpdate => {
            ToolArgs::Set(SetArgs::MailboxUpdate(MailboxUpdateArgs::parse(map)?))
        }
        Provided::FastmailDraft => ToolArgs::Draft(DraftArgs::parse(map)?),
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

/// The step-2 request for `parsed` in `account_id`, and the call id its
/// answer is read by.
fn api_request(parsed: &SetArgs, account_id: &str) -> (Request, CallId) {
    let mut request = Request::mail();
    let call = match parsed {
        SetArgs::Organize(args) => EmailSet {
            account_id,
            create: Vec::new(),
            update: args.update(),
        }
        .push(&mut request),
        SetArgs::MailboxCreate(args) => MailboxSet {
            account_id,
            create: vec![(
                MAILBOX_CREATE_ID.to_owned(),
                MailboxCreate {
                    name: args.name.clone(),
                    parent_id: args.parent_id.clone(),
                },
            )],
            update: Vec::new(),
        }
        .push(&mut request),
        SetArgs::MailboxUpdate(args) => MailboxSet {
            account_id,
            create: Vec::new(),
            update: vec![(args.id.clone(), args.patch())],
        }
        .push(&mut request),
    };
    (request, call)
}

fn has_step(tool: Provided, step: u8) -> bool {
    matches!(
        (tool, step),
        (_, SESSION_STEP | API_STEP) | (Provided::FastmailDraft, DRAFT_SET_STEP)
    )
}

/// The hint on a failure that leaves `tool`'s write at `step` unknown; `None`
/// at a step that writes nothing.
fn write_hint(tool: Provided, step: u8) -> Option<&'static str> {
    match (tool, step) {
        (Provided::FastmailOrganize, API_STEP) => Some(ORGANIZE_UNKNOWN_HINT),
        (Provided::FastmailMailboxCreate, API_STEP) => Some(MAILBOX_CREATE_UNKNOWN_HINT),
        (Provided::FastmailMailboxUpdate, API_STEP) => Some(MAILBOX_UPDATE_UNKNOWN_HINT),
        (Provided::FastmailDraft, DRAFT_SET_STEP) => Some(DRAFT_UNKNOWN_HINT),
        _ => None,
    }
}

/// What the request does with its nested call's answer: complete, fail, or
/// issue its next step. Every completion is held to the result ceiling; every
/// failure `step` leaves unknown carries the step's write hint.
pub fn advance(state: &InFlight, outcome: Result<Value, NestedError>, settings: &Settings) -> Next {
    if !has_step(state.tool, state.step) {
        return Next::Fail(no_such_step(state.tool, state.step));
    }
    match step(state, outcome, settings) {
        Ok(Next::Complete(value)) => bounded(value, COMPACT_RESULT_CEILING, None),
        Ok(next) => next,
        Err(failure) => Next::Fail(failure.with_hint(write_hint(state.tool, state.step))),
    }
}

/// One step. Fastmail's refusal of one object, which says nothing was
/// changed, is `Ok(Next::Fail)`; an `Err` says whether Fastmail refused the
/// request or the method or left the write's outcome unknown.
fn step(
    state: &InFlight,
    outcome: Result<Value, NestedError>,
    settings: &Settings,
) -> Result<Next, StepFailure> {
    let body = step_answer(state, outcome, None)?;
    let parsed = parse(state.tool, &state.args).map_err(|reason| {
        internal(format!(
            "stored {} arguments no longer parse: {reason}",
            state.tool.name()
        ))
    })?;
    let account = parsed.account.as_deref();
    let args = match &parsed.tool {
        ToolArgs::Draft(args) => return draft::step(state, body, settings, account, args),
        ToolArgs::Set(args) => args,
    };
    if state.step == SESSION_STEP {
        let session = read_session(&body)?;
        let (request, _) = api_request(args, &session.mail_account_id);
        let derived = Args::from_iter([(ACCOUNT_KEY.to_owned(), json!(session.mail_account_id))]);
        return Ok(Next::Call {
            call: api_step(settings, account, API_STEP, &request),
            derived,
        });
    }
    let account_id = derived_str(state, ACCOUNT_KEY)?;
    let response = Response::parse(body).map_err(jmap_failure)?;
    let (_, call) = api_request(args, account_id);
    let result = response.result(&call).map_err(write_method_failure)?;
    Ok(match args {
        SetArgs::Organize(args) => organized(args, result),
        SetArgs::MailboxCreate(args) => mailbox_created(args, result),
        SetArgs::MailboxUpdate(args) => mailbox_updated(args, result),
    }?)
}

/// Organize's answer: how many messages changed, and why Fastmail refused
/// the rest. Every requested id must be answered one way or the other.
fn organized(args: &OrganizeArgs, set: &Value) -> Result<Next, Failure> {
    let result = EmailSet::result(set).map_err(jmap_failure)?;
    let updated: BTreeSet<&str> = result.updated.iter().map(String::as_str).collect();
    let mut count = 0_usize;
    let mut refused: Vec<(&str, &SetError)> = Vec::new();
    for id in &args.ids {
        if updated.contains(id.as_str()) {
            count += 1;
        } else if let Some(error) = result.not_updated.get(id) {
            refused.push((id, error));
        } else {
            return Err(internal(format!(
                "{API_NAME}'s Email/set answered neither message `{id}`'s update nor its refusal"
            )));
        }
    }
    if count == 0 {
        let (id, error) = refused[0];
        let failure = set_failure("message", id, error);
        return Ok(Next::Fail(Failure {
            kind: failure.kind,
            detail: format!(
                "none of the {} messages was changed: {}",
                args.ids.len(),
                failure.detail
            ),
        }));
    }
    let mut out = Args::new();
    out.insert("updated".into(), json!(count));
    if !refused.is_empty() {
        let reasons: Map<String, Value> = refused
            .iter()
            .map(|(id, error)| ((*id).to_owned(), json!(not_updated_reason(error))))
            .collect();
        out.insert("not_updated".into(), Value::Object(reasons));
        if refused
            .iter()
            .any(|(_, error)| concerns_mailbox_membership(error))
        {
            out.insert("hint".into(), json!(MAILBOX_MEMBERSHIP_HINT));
        }
    }
    Ok(Next::Complete(Value::Object(out)))
}

/// Fastmail's reason for one refused message, at most
/// `NOT_UPDATED_REASON_MAX_BYTES` raw bytes with the cut's ellipsis.
fn not_updated_reason(error: &SetError) -> String {
    cut(
        &error.to_string(),
        NOT_UPDATED_REASON_MAX_BYTES - '…'.len_utf8(),
    )
    .0
}

fn mailbox_created(args: &MailboxCreateArgs, set: &Value) -> Result<Next, Failure> {
    let result = MailboxSet::result(set).map_err(jmap_failure)?;
    if let Some(created) = result.created.get(MAILBOX_CREATE_ID) {
        return Ok(Next::Complete(json!({ "id": created.id })));
    }
    if let Some(error) = result.not_created.get(MAILBOX_CREATE_ID) {
        return Ok(Next::Fail(set_failure("mailbox", &args.name, error)));
    }
    Err(internal(format!(
        "{API_NAME}'s Mailbox/set answered neither the new mailbox nor its refusal"
    )))
}

fn mailbox_updated(args: &MailboxUpdateArgs, set: &Value) -> Result<Next, Failure> {
    let result = MailboxSet::result(set).map_err(jmap_failure)?;
    if result.updated.contains(&args.id) {
        return Ok(Next::Complete(
            json!({ "id": args.id, "updated": args.changed() }),
        ));
    }
    if let Some(error) = result.not_updated.get(&args.id) {
        return Ok(Next::Fail(set_failure("mailbox", &args.id, error)));
    }
    Err(internal(format!(
        "{API_NAME}'s Mailbox/set answered neither mailbox `{}`'s update nor its refusal",
        args.id
    )))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use brenn_provider_kit::activation::BACKLOG_NS;
    use brenn_provider_kit::testutil::{Answered, MemoryHost, failure, http, nested, truncated};
    use brenn_provider_kit::{
        ApiCall, Bound, CALL_BUDGET, Delivered, ErrorKind, Input, Probes, Query,
        Request as Incoming, ToolBounds, Unreadable, check, hold_probes, hold_required,
        input_schema, probes,
    };
    use fastmail_jmap::session::{API_PATH, CORE, MAIL, SESSION_PATH};
    use fastmail_provider::args::{ID_MAX_CHARS, KEYWORD_MAX_CHARS};
    use fastmail_provider::failure::{FORBIDDEN_CHANGE, READ_ONLY_TOKEN};
    use fastmail_provider::steps::api_call;
    use fastmail_provider::testutil::{
        ACCOUNT_PATTERN, HOST_ARGS_CAP_BYTES, KEYWORD_PATTERN, account_probes, completed, jmap,
        session_body, settings,
    };

    use super::*;

    /// The unanchored pattern of a bare address, the one body both patterns
    /// below are built from.
    macro_rules! address {
        () => {
            r#"[^\s\u0000-\u001f\u007f-\u009f<>"\\@,]{1,64}@[^\s\u0000-\u001f\u007f-\u009f<>"\\@,]{1,253}"#
        };
    }

    /// What a recipient admits: `draft`'s recipient parser enforces exactly
    /// this, but for an address's byte bound, which a pattern cannot state.
    /// [`recipient_probes`] holds it there.
    const RECIPIENT_PATTERN: &str = concat!(
        r#"^(?:[^\u0000-\u001f\u007f-\u009f<>"\\]{1,100} <"#,
        address!(),
        ">|",
        address!(),
        ")$"
    );

    /// What `from` admits: the bare-address alternative of
    /// `RECIPIENT_PATTERN`, held to the byte bound by [`from_probes`].
    const ADDRESS_PATTERN: &str = concat!("^", address!(), "$");

    /// The row of `tool` with `args` at `step`, carrying `derived`.
    pub(super) fn row(tool: Provided, args: Value, step: u8, derived: Value) -> InFlight {
        let (method, path) = match step {
            SESSION_STEP => ("GET", SESSION_PATH),
            _ => ("POST", API_PATH),
        };
        let call = Call {
            step,
            api: api_call(&settings(), None, method, path, None),
        };
        let Value::Object(derived) = derived else {
            panic!("derived is an object");
        };
        InFlight::new(&"r".repeat(32), tool, &args, &call, derived, 1_000, 1_000)
    }

    pub(super) fn at_api(tool: Provided, args: Value) -> InFlight {
        row(tool, args, API_STEP, json!({ ACCOUNT_KEY: "a1" }))
    }

    pub(super) fn answered(state: &InFlight, outcome: Result<Value, NestedError>) -> Next {
        advance(state, outcome, &settings())
    }

    /// A request's arguments every tool's parser admits.
    pub(super) fn admitted_args(tool: Provided) -> Value {
        match tool {
            Provided::FastmailDraft => {
                json!({ "to": ["alice@example.com"], "subject": "Hello", "body": "Hi" })
            }
            Provided::FastmailOrganize => json!({ "ids": ["e1"], "add_mailboxes": ["m1"] }),
            Provided::FastmailMailboxCreate => json!({ "name": "Receipts" }),
            Provided::FastmailMailboxUpdate => json!({ "id": "m1", "name": "Receipts" }),
        }
    }

    /// The JMAP request the session step issues for `tool` with `args`.
    fn api_body(tool: Provided, args: Value) -> Value {
        let state = row(tool, args, SESSION_STEP, json!({}));
        let Next::Call { call, .. } = answered(&state, session_body()) else {
            panic!("{tool:?}: the session step issues the request");
        };
        assert_eq!(call.step, API_STEP);
        serde_json::from_str(call.api.body.as_deref().unwrap()).unwrap()
    }

    /// A one-method JMAP answer: `name`'s `args` as call `c0`.
    pub(super) fn set_answer(name: &str, args: Value) -> Result<Value, NestedError> {
        jmap(json!([[name, args, "c0"]]))
    }

    fn bounds(tool: Provided) -> ToolBounds<'static> {
        let token = || Bound::Token {
            max_chars: ID_MAX_CHARS,
        };
        let ids = |max| Bound::Array {
            min: 1,
            max,
            unique: true,
            item: Box::new(token()),
        };
        let keywords = || Bound::Array {
            min: 1,
            max: ORGANIZE_LIST_MAX,
            unique: true,
            item: Box::new(Bound::Pattern(KEYWORD_PATTERN.to_owned())),
        };
        let name = || Bound::Text {
            max_chars: MAILBOX_NAME_MAX_CHARS,
            format: None,
        };
        let account = (ACCOUNT_ARG, Bound::Pattern(ACCOUNT_PATTERN.to_owned()));
        match tool {
            Provided::FastmailOrganize => ToolBounds {
                properties: vec![
                    ("ids", ids(ORGANIZE_IDS_MAX)),
                    ("add_mailboxes", ids(ORGANIZE_LIST_MAX)),
                    ("remove_mailboxes", ids(ORGANIZE_LIST_MAX)),
                    ("add_keywords", keywords()),
                    ("remove_keywords", keywords()),
                    account,
                ],
                required: &["ids"],
            },
            Provided::FastmailMailboxCreate => ToolBounds {
                properties: vec![("name", name()), ("parent_id", token()), account],
                required: &["name"],
            },
            Provided::FastmailMailboxUpdate => ToolBounds {
                properties: vec![
                    ("id", token()),
                    ("name", name()),
                    ("parent_id", token()),
                    ("top_level", Bound::Bool),
                    account,
                ],
                required: &["id"],
            },
            Provided::FastmailDraft => {
                let recipients = || Bound::Array {
                    min: 1,
                    max: draft::RECIPIENTS_MAX,
                    unique: true,
                    item: Box::new(Bound::Pattern(RECIPIENT_PATTERN.to_owned())),
                };
                let text = |max_chars| Bound::Text {
                    max_chars,
                    format: None,
                };
                ToolBounds {
                    properties: vec![
                        ("to", recipients()),
                        ("cc", recipients()),
                        ("bcc", recipients()),
                        ("subject", text(draft::SUBJECT_MAX_CHARS)),
                        ("body", text(draft::BODY_MAX_CHARS)),
                        ("reply_to_id", token()),
                        ("from", Bound::Pattern(ADDRESS_PATTERN.to_owned())),
                        account,
                    ],
                    required: &["body"],
                }
            }
        }
    }

    /// `n` distinct tokens of at least `len` characters: `prefix`, then the
    /// index zero-padded on the left.
    fn padded(prefix: &str, n: usize, len: usize) -> Vec<String> {
        let width = len - prefix.len();
        (0..n).map(|i| format!("{prefix}{i:0>width$}")).collect()
    }

    /// Probes for an id list of at most `max`, prefixed `prefix`.
    fn id_list_probes(prefix: &str, max: usize) -> Probes {
        let ids = |n| json!(padded(prefix, n, 2 + prefix.len()));
        Probes {
            admitted: vec![ids(1), ids(max), json!(padded(prefix, 1, ID_MAX_CHARS))],
            refused: vec![
                json!([]),
                ids(max + 1),
                json!([format!("{prefix}1"), format!("{prefix}1")]),
                json!(["a b"]),
                json!(["a.b"]),
                json!(["é"]),
                json!(padded(prefix, 1, ID_MAX_CHARS + 1)),
                json!([1]),
                json!(format!("{prefix}1")),
                json!(1),
            ],
        }
    }

    /// A 254-byte address and a 255-byte one: the parser's byte bound.
    fn addresses_at_the_byte_bound() -> (String, String) {
        let local = "l".repeat(64);
        (
            format!("{local}@{}", "d".repeat(189)),
            format!("{local}@{}", "d".repeat(190)),
        )
    }

    /// Probes holding a recipient list to `RECIPIENT_PATTERN`, its count and
    /// distinctness, and the byte bound on an address.
    fn recipient_probes() -> Probes {
        let (widest, too_wide) = addresses_at_the_byte_bound();
        let many = |n: usize| {
            json!(
                (0..n)
                    .map(|i| format!("r{i}@example.com"))
                    .collect::<Vec<_>>()
            )
        };
        let one = |text: &str| json!([text]);
        Probes {
            admitted: vec![
                one("alice@example.com"),
                one("Alice Example <alice@example.com>"),
                one(&widest),
                one(&format!("{} <a@example.com>", "\u{1F600}".repeat(100))),
                one("A, B @ C <ab@example.com>"),
                one("é@例え.example"),
                many(draft::RECIPIENTS_MAX),
            ],
            refused: vec![
                json!([]),
                many(draft::RECIPIENTS_MAX + 1),
                json!(["a@example.com", "a@example.com"]),
                one("a\\b@example.com"),
                one("a\u{1}b@example.com"),
                one("a\"b@example.com"),
                one("a b@example.com"),
                one("a@b@example.com"),
                one("a,b@example.com"),
                one(&too_wide),
                one(&format!("{}@example.com", "l".repeat(65))),
                one("a@"),
                one("@example.com"),
                one("A<b <a@example.com>"),
                one("A\"b <a@example.com>"),
                one(" <a@example.com>"),
                one(&format!("{} <a@example.com>", "x".repeat(101))),
                one("Alice <alice@example.com"),
                one("Alice<alice@example.com>"),
                json!([1]),
                json!("alice@example.com"),
                json!(1),
            ],
        }
    }

    /// Probes holding `from`, a bare address, to `ADDRESS_PATTERN` and the
    /// byte bound.
    fn from_probes() -> Probes {
        let (widest, too_wide) = addresses_at_the_byte_bound();
        Probes {
            admitted: vec![
                json!("alice@example.com"),
                json!(widest),
                json!("é@example.com"),
            ],
            refused: vec![
                json!("Alice <alice@example.com>"),
                json!("<alice@example.com>"),
                json!("a b@example.com"),
                json!("a\"b@example.com"),
                json!("a\\b@example.com"),
                json!("a\u{85}b@example.com"),
                json!(too_wide),
                json!("a@b@example.com"),
                json!(""),
                json!(["alice@example.com"]),
                json!(1),
            ],
        }
    }

    /// Probes holding `body` to its wire width as well as its characters.
    fn body_probes() -> Probes {
        let max = draft::BODY_MAX_CHARS;
        Probes {
            admitted: vec![
                json!("x".repeat(max)),
                json!("é".repeat(15_000)),
                json!(format!("x{}", "\n".repeat(9_999))),
                json!(format!("xxx{}", "\n".repeat(9_999))),
                json!("\"".repeat(7_500)),
                json!("a\u{1}\r\n\tb"),
            ],
            refused: vec![
                json!("x".repeat(max + 1)),
                json!("é".repeat(15_001)),
                json!(format!("x{}", "\n".repeat(10_000))),
                json!("\"".repeat(7_501)),
                json!(""),
                json!(" \t\n\u{85}\u{3000}"),
                json!(1),
            ],
        }
    }

    /// The kit's probes of a one-line text bound, and the control characters
    /// its parser refuses beside them.
    fn one_line_probes(bound: &Bound) -> Probes {
        let mut probes = probes(bound).expect("a text bound has probes");
        probes
            .refused
            .extend(["a\u{1}b", "a\r\nb", "a\tb", "a\u{85}b"].map(|probe| json!(probe)));
        probes
    }

    fn keyword_list_probes() -> Probes {
        let keywords = |n: usize| json!((0..n).map(|i| format!("k{i}")).collect::<Vec<_>>());
        Probes {
            admitted: vec![
                keywords(1),
                keywords(ORGANIZE_LIST_MAX),
                json!(["$Seen"]),
                json!(["k".repeat(KEYWORD_MAX_CHARS)]),
            ],
            refused: vec![
                json!([]),
                keywords(ORGANIZE_LIST_MAX + 1),
                json!(["k1", "k1"]),
                json!(["a b"]),
                json!(["é"]),
                json!(["k".repeat(KEYWORD_MAX_CHARS + 1)]),
                json!([""]),
                json!([1]),
                json!("k1"),
                json!(1),
            ],
        }
    }

    fn spec_text() -> String {
        let path = std::env::var("FASTMAIL_MAIL_WRITE_SPEC").expect("FASTMAIL_MAIL_WRITE_SPEC");
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
    }

    #[test]
    fn every_tools_schema_states_what_its_parser_enforces() {
        let spec = spec_text();
        for tool in Provided::ALL {
            let name = tool.name();
            let bounds = bounds(tool);
            let stated: Vec<&str> = bounds.properties.iter().map(|(key, _)| *key).collect();
            assert_eq!(stated, keys(tool), "`{name}`'s bounds and parser keys");
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
                    "ids" => id_list_probes("e", ORGANIZE_IDS_MAX),
                    "add_mailboxes" => id_list_probes("a", ORGANIZE_LIST_MAX),
                    "remove_mailboxes" => id_list_probes("r", ORGANIZE_LIST_MAX),
                    "add_keywords" | "remove_keywords" => keyword_list_probes(),
                    "name" | "subject" => one_line_probes(bound),
                    "to" | "cc" | "bcc" => recipient_probes(),
                    "from" => from_probes(),
                    "body" => body_probes(),
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
            refused(Provided::FastmailOrganize, json!({ "ids": ["e1"] })),
            "give at least one of `add_mailboxes`, `remove_mailboxes`, `add_keywords` and \
             `remove_keywords`"
        );
        assert_eq!(
            refused(
                Provided::FastmailOrganize,
                json!({ "ids": ["e1"], "add_mailboxes": ["m1", "m2"], "remove_mailboxes": ["m2"] })
            ),
            "`add_mailboxes` and `remove_mailboxes` both name \"m2\""
        );
        // Keywords are compared lower-cased, as JMAP compares them.
        assert_eq!(
            refused(
                Provided::FastmailOrganize,
                json!({ "ids": ["e1"], "add_keywords": ["$Seen"], "remove_keywords": ["$seen"] })
            ),
            "`add_keywords` and `remove_keywords` both name \"$seen\""
        );
        assert_eq!(
            refused(Provided::FastmailMailboxUpdate, json!({ "id": "m1" })),
            "give at least one of `name`, `parent_id` and `top_level` (true)"
        );
        assert_eq!(
            refused(
                Provided::FastmailMailboxUpdate,
                json!({ "id": "m1", "top_level": false })
            ),
            "give at least one of `name`, `parent_id` and `top_level` (true)"
        );
        assert_eq!(
            refused(
                Provided::FastmailMailboxUpdate,
                json!({ "id": "m1", "parent_id": "m2", "top_level": true })
            ),
            "`parent_id` and `top_level` cannot be given together"
        );
        assert!(
            refused(
                Provided::FastmailMailboxCreate,
                json!({ "name": "a", "x": 1 })
            )
            .contains("`x`")
        );
    }

    /// Every nested call of a request names the request's `account`: the
    /// session and the `/set`. Omitted, every call names none.
    #[test]
    fn every_nested_call_carries_the_requests_account() {
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
                let Next::Call { call, derived } = answered(&state, session_body()) else {
                    panic!("{tool:?}: the session step issues the request");
                };
                assert_eq!(call.api.account, expected, "{tool:?} request");
                assert_eq!(
                    (call.api.method.as_str(), call.api.path.as_str()),
                    ("POST", "/jmap/api")
                );
                let expected_derived = match tool {
                    Provided::FastmailDraft => {
                        json!({ ACCOUNT_KEY: "a1", "submission_account_id": "a1" })
                    }
                    _ => json!({ ACCOUNT_KEY: "a1" }),
                };
                assert_eq!(Value::Object(derived), expected_derived);
            }
        }
    }

    #[test]
    fn organize_sends_one_patch_under_every_id() {
        let body = api_body(
            Provided::FastmailOrganize,
            json!({
                "ids": ["e1", "e2"],
                "add_mailboxes": ["m2"], "remove_mailboxes": ["m1"],
                "add_keywords": ["$Flagged"], "remove_keywords": ["$seen"],
            }),
        );
        let patch = json!({
            "mailboxIds/m2": true, "mailboxIds/m1": null,
            "keywords/$flagged": true, "keywords/$seen": null,
        });
        assert_eq!(
            body,
            json!({
                "using": [CORE, MAIL],
                "methodCalls": [["Email/set", {
                    "accountId": "a1",
                    "update": { "e1": patch, "e2": patch },
                }, "c0"]],
            })
        );
    }

    #[test]
    fn organize_answers_the_count_and_each_refusal() {
        let state = at_api(
            Provided::FastmailOrganize,
            json!({ "ids": ["e1", "e2", "e3"], "remove_mailboxes": ["m1"] }),
        );
        let result = completed(answered(
            &state,
            set_answer(
                "Email/set",
                json!({
                    "accountId": "a1",
                    "updated": { "e1": null },
                    "notUpdated": {
                        "e2": { "type": "invalidProperties", "description": "no mailbox left",
                                "properties": ["mailboxIds"] },
                        "e3": { "type": "notFound" },
                    },
                }),
            ),
        ));
        assert_eq!(
            result,
            json!({
                "updated": 1,
                "not_updated": {
                    "e2": "invalidProperties: no mailbox left (mailboxIds)",
                    "e3": "notFound",
                },
                "hint": MAILBOX_MEMBERSHIP_HINT,
            })
        );
        // A refusal on `keywords` carries no membership hint.
        let result = completed(answered(
            &state,
            set_answer(
                "Email/set",
                json!({
                    "accountId": "a1",
                    "updated": { "e1": null, "e3": {} },
                    "notUpdated": {
                        "e2": { "type": "tooManyKeywords", "properties": ["keywords"] },
                    },
                }),
            ),
        ));
        assert_eq!(
            result,
            json!({ "updated": 2, "not_updated": { "e2": "tooManyKeywords (keywords)" } })
        );
        assert_eq!(
            completed(answered(
                &state,
                set_answer(
                    "Email/set",
                    json!({ "accountId": "a1", "updated": { "e1": null, "e2": null, "e3": null } }),
                ),
            )),
            json!({ "updated": 3 })
        );
    }

    #[test]
    fn organize_fails_when_no_message_changed_and_never_reports_an_unanswered_id() {
        let state = at_api(
            Provided::FastmailOrganize,
            json!({ "ids": ["e1", "e2"], "remove_mailboxes": ["m1"] }),
        );
        let refusal = json!({ "type": "invalidProperties", "properties": ["mailboxIds"] });
        assert_eq!(
            answered(
                &state,
                set_answer(
                    "Email/set",
                    json!({ "accountId": "a1",
                            "notUpdated": { "e1": refusal, "e2": { "type": "notFound" } } }),
                ),
            ),
            Next::Fail(failure(
                ErrorKind::InvalidArgs,
                &format!(
                    "none of the 2 messages was changed: Fastmail refused `e1`: \
                     invalidProperties (mailboxIds); {MAILBOX_MEMBERSHIP_HINT}"
                )
            ))
        );
        assert_eq!(
            answered(
                &state,
                set_answer(
                    "Email/set",
                    json!({ "accountId": "a1", "updated": { "e1": null } }),
                ),
            ),
            Next::Fail(internal(format!(
                "Fastmail's Email/set answered neither message `e2`'s update nor its refusal; \
                 {ORGANIZE_UNKNOWN_HINT}"
            )))
        );
    }

    #[test]
    fn organize_refuses_ids_and_changes_too_wide_for_one_request() {
        let ids = padded("e", ORGANIZE_IDS_MAX, ID_MAX_CHARS);
        assert_eq!(
            plan(
                Provided::FastmailOrganize,
                &json!({ "ids": ids, "add_mailboxes": ["m1"] }),
                &settings()
            )
            .unwrap_err(),
            "`ids` and the changes together are too wide for one request; ask for fewer `ids`"
        );
    }

    /// The result organize builds for `ids`, each refused with `error`.
    fn organize_result(ids: Vec<String>, updated: &[String], error: &SetError) -> Value {
        let args = OrganizeArgs {
            ids,
            patch: Patch::new().mailbox("m1", false),
        };
        let set = json!({
            "accountId": "a1",
            "updated": updated.iter().map(|id| (id.clone(), Value::Null)).collect::<Map<_, _>>(),
            "notUpdated": args.ids.iter().filter(|id| !updated.contains(id))
                .map(|id| (id.clone(), json!({
                    "type": error.kind, "description": error.description,
                    "properties": error.properties,
                })))
                .collect::<Map<_, _>>(),
        });
        completed(organized(&args, &set).unwrap())
    }

    /// The parser bounds organize's ids by the width of `update`, so the
    /// widest result here, 499 refused 255-character ids, is wider than any
    /// admitted request can draw; it still fits the ceiling.
    #[test]
    fn the_widest_organize_result_fits_the_ceiling() {
        let ids = padded("e", ORGANIZE_IDS_MAX, ID_MAX_CHARS);
        let error = SetError {
            kind: "invalidProperties".to_owned(),
            description: Some(format!("\u{1}{}", "\"\\".repeat(200))),
            properties: vec!["mailboxIds".to_owned()],
        };
        let result = organize_result(ids.clone(), &ids[..1], &error);
        assert_eq!(result["updated"], json!(1));
        assert_eq!(result["hint"], json!(MAILBOX_MEMBERSHIP_HINT));
        let reasons = result["not_updated"].as_object().unwrap();
        assert_eq!(reasons.len(), ORGANIZE_IDS_MAX - 1);
        for reason in reasons.values() {
            let reason = reason.as_str().unwrap();
            assert!(
                reason.starts_with("invalidProperties: \\u{1}\"\\"),
                "{reason}"
            );
            assert!(reason.len() <= NOT_UPDATED_REASON_MAX_BYTES, "{reason}");
            assert!(serialized_len(&json!(reason)) - 2 <= 2 * NOT_UPDATED_REASON_MAX_BYTES);
        }
        assert!(
            serialized_len(&result) < COMPACT_RESULT_CEILING,
            "{} bytes",
            serialized_len(&result)
        );
    }

    #[test]
    fn a_refusal_reason_is_cut_on_a_character_boundary() {
        // "ab: " is four bytes, so the 117th byte falls inside an "é".
        let error = SetError {
            kind: "ab".to_owned(),
            description: Some("é".repeat(100)),
            properties: Vec::new(),
        };
        let reason = not_updated_reason(&error);
        assert_eq!(reason, format!("ab: {}…", "é".repeat(56)));
        assert!(reason.len() <= NOT_UPDATED_REASON_MAX_BYTES);
    }

    #[test]
    fn mailbox_create_answers_the_new_id_or_fastmails_refusal() {
        assert_eq!(
            api_body(
                Provided::FastmailMailboxCreate,
                json!({ "name": "Receipts", "parent_id": "m1" })
            )["methodCalls"],
            json!([["Mailbox/set", {
                "accountId": "a1",
                "create": { "mailbox": { "name": "Receipts", "parentId": "m1" } },
            }, "c0"]])
        );
        assert_eq!(
            api_body(
                Provided::FastmailMailboxCreate,
                json!({ "name": "Receipts" })
            )["methodCalls"][0][1]["create"],
            json!({ "mailbox": { "name": "Receipts" } })
        );
        let state = at_api(
            Provided::FastmailMailboxCreate,
            json!({ "name": "Receipts" }),
        );
        assert_eq!(
            completed(answered(
                &state,
                set_answer(
                    "Mailbox/set",
                    json!({ "accountId": "a1", "created": { "mailbox": { "id": "m9" } } }),
                ),
            )),
            json!({ "id": "m9" })
        );
        assert_eq!(
            answered(
                &state,
                set_answer(
                    "Mailbox/set",
                    json!({ "accountId": "a1", "notCreated": { "mailbox": {
                        "type": "invalidProperties", "description": "name taken",
                        "properties": ["name"] } } }),
                ),
            ),
            Next::Fail(failure(
                ErrorKind::InvalidArgs,
                "Fastmail refused `Receipts`: invalidProperties: name taken (name)"
            ))
        );
        assert_eq!(
            answered(
                &state,
                set_answer("Mailbox/set", json!({ "accountId": "a1" })),
            ),
            Next::Fail(internal(format!(
                "Fastmail's Mailbox/set answered neither the new mailbox nor its refusal; \
                 {MAILBOX_CREATE_UNKNOWN_HINT}"
            )))
        );
    }

    #[test]
    fn mailbox_update_renames_moves_or_lifts_to_the_top_level() {
        for (args, patch, changed) in [
            (
                json!({ "id": "m1", "name": "Bills" }),
                json!({ "name": "Bills" }),
                json!(["name"]),
            ),
            (
                json!({ "id": "m1", "parent_id": "m2" }),
                json!({ "parentId": "m2" }),
                json!(["parent_id"]),
            ),
            (
                json!({ "id": "m1", "top_level": true }),
                json!({ "parentId": null }),
                json!(["parent_id"]),
            ),
            (
                json!({ "id": "m1", "name": "Bills", "parent_id": "m2", "top_level": false }),
                json!({ "name": "Bills", "parentId": "m2" }),
                json!(["name", "parent_id"]),
            ),
        ] {
            assert_eq!(
                api_body(Provided::FastmailMailboxUpdate, args.clone())["methodCalls"],
                json!([["Mailbox/set", { "accountId": "a1", "update": { "m1": patch } }, "c0"]]),
                "{args}"
            );
            let state = at_api(Provided::FastmailMailboxUpdate, args.clone());
            assert_eq!(
                completed(answered(
                    &state,
                    set_answer(
                        "Mailbox/set",
                        json!({ "accountId": "a1", "updated": { "m1": null } }),
                    ),
                )),
                json!({ "id": "m1", "updated": changed }),
                "{args}"
            );
        }
        let state = at_api(
            Provided::FastmailMailboxUpdate,
            json!({ "id": "m1", "parent_id": "m2" }),
        );
        assert_eq!(
            answered(
                &state,
                set_answer(
                    "Mailbox/set",
                    json!({ "accountId": "a1", "notUpdated": { "m1": {
                        "type": "invalidProperties", "properties": ["parentId"] } } }),
                ),
            ),
            Next::Fail(failure(
                ErrorKind::InvalidArgs,
                "Fastmail refused `m1`: invalidProperties (parentId)"
            ))
        );
        assert_eq!(
            answered(
                &state,
                set_answer(
                    "Mailbox/set",
                    json!({ "accountId": "a1", "notUpdated": { "m1": { "type": "notFound" } } }),
                ),
            ),
            Next::Fail(failure(ErrorKind::InvalidArgs, "no mailbox with id `m1`"))
        );
        assert_eq!(
            answered(
                &state,
                set_answer("Mailbox/set", json!({ "accountId": "a1", "updated": {} })),
            ),
            Next::Fail(internal(format!(
                "Fastmail's Mailbox/set answered neither mailbox `m1`'s update nor its refusal; \
                 {MAILBOX_UPDATE_UNKNOWN_HINT}"
            )))
        );
    }

    /// The serialised width of the `/set` call the session step issues for
    /// `tool` with `args`.
    fn api_call_width(tool: Provided, args: Value) -> usize {
        let state = row(tool, args, SESSION_STEP, json!({}));
        let Next::Call { call, .. } = answered(&state, session_body()) else {
            panic!("{tool:?}: the session step issues the request");
        };
        serde_json::to_vec(&call.api).unwrap().len()
    }

    #[test]
    fn the_widest_call_of_every_tool_fits_the_args_cap() {
        let account = "a".repeat(255);
        // Organize, three shapes: short ids under a three-keyword patch (the
        // densest quoting), long ids under a one-mailbox patch, and every list
        // full of the longest entries. Each takes the most ids the byte cap
        // admits.
        let shapes = [
            (
                2,
                json!({ "add_keywords": ["a", "b"], "remove_keywords": ["c"] }),
            ),
            (ID_MAX_CHARS, json!({ "add_mailboxes": ["m"] })),
            (
                ID_MAX_CHARS,
                json!({
                    "add_mailboxes": padded("a", ORGANIZE_LIST_MAX, ID_MAX_CHARS),
                    "remove_mailboxes": padded("r", ORGANIZE_LIST_MAX, ID_MAX_CHARS),
                    "add_keywords": padded("k", ORGANIZE_LIST_MAX, KEYWORD_MAX_CHARS),
                    "remove_keywords": padded("x", ORGANIZE_LIST_MAX, KEYWORD_MAX_CHARS),
                }),
            ),
        ];
        for (len, lists) in shapes {
            let with = |n: usize| {
                let mut args = lists.clone();
                // Two-character ids are drawn from the 64 token characters.
                let ids: Vec<String> = if len == 2 {
                    let chars: Vec<char> =
                        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-"
                            .chars()
                            .collect();
                    (0..n)
                        .map(|i| format!("{}{}", chars[i / 64], chars[i % 64]))
                        .collect()
                } else {
                    padded("e", n, len)
                };
                args["ids"] = json!(ids);
                args["account"] = json!(account);
                args
            };
            // The width grows with the id count, so the admitted counts are a
            // prefix of 1..=ORGANIZE_IDS_MAX.
            let counts: Vec<usize> = (1..=ORGANIZE_IDS_MAX).collect();
            let n = counts.partition_point(|&n| {
                plan(Provided::FastmailOrganize, &with(n), &settings()).is_ok()
            });
            assert!(n >= 1, "one id fits");
            if n < ORGANIZE_IDS_MAX {
                assert_eq!(
                    plan(Provided::FastmailOrganize, &with(n + 1), &settings()).unwrap_err(),
                    "`ids` and the changes together are too wide for one request; ask for \
                     fewer `ids`"
                );
            }
            let width = api_call_width(Provided::FastmailOrganize, with(n));
            assert!(
                width <= HOST_ARGS_CAP_BYTES,
                "{n} ids of {len}: {width} bytes"
            );
        }
        // A mailbox name at its bound, of the characters that widen most.
        for name in [
            "\"".repeat(MAILBOX_NAME_MAX_CHARS),
            "\u{1F600}".repeat(MAILBOX_NAME_MAX_CHARS),
        ] {
            let parent = "p".repeat(ID_MAX_CHARS);
            for (tool, args) in [
                (
                    Provided::FastmailMailboxCreate,
                    json!({ "name": name, "parent_id": parent, "account": account }),
                ),
                (
                    Provided::FastmailMailboxUpdate,
                    json!({ "id": "i".repeat(ID_MAX_CHARS), "name": name, "parent_id": parent,
                            "account": account }),
                ),
            ] {
                let width = api_call_width(tool, args);
                assert!(width <= HOST_ARGS_CAP_BYTES, "{tool:?}: {width} bytes");
            }
        }
    }

    /// The step `tool` writes at.
    fn write_step(tool: Provided) -> u8 {
        match tool {
            Provided::FastmailDraft => DRAFT_SET_STEP,
            _ => API_STEP,
        }
    }

    /// The row of `tool` at its write step, as `write_hint` names it.
    fn write_step_state(tool: Provided) -> InFlight {
        match tool {
            Provided::FastmailDraft => row(
                tool,
                admitted_args(tool),
                DRAFT_SET_STEP,
                json!({
                    ACCOUNT_KEY: "a1",
                    "mailbox_id": "d1",
                    "from_address": { "name": null, "email": "alice@example.com" },
                }),
            ),
            _ => at_api(tool, admitted_args(tool)),
        }
    }

    #[test]
    fn every_write_step_answers_a_lost_answer_with_its_hint() {
        let refusal = "result exceeds [messaging] max_body_bytes";
        let lost = || {
            [
                http(500, "<html>secret</html>"),
                nested("internal", refusal),
                truncated(200, json!(300000), "{"),
                http(204, ""),
                set_answer("Email/set", json!({ "accountId": "a1", "updated": 1 })),
                set_answer("Mailbox/set", json!({ "accountId": "a1", "created": [] })),
                jmap(json!([["error", { "type": "serverFail" }, "c0"]])),
                jmap(json!([["error", { "type": "serverUnavailable" }, "c0"]])),
            ]
        };
        // Answers saying Fastmail did nothing: the request or the method
        // refused.
        let refused = || {
            [
                http(400, "{}"),
                http(401, ""),
                http(403, ""),
                http(429, ""),
                jmap(json!([["error", { "type": "accountReadOnly" }, "c0"]])),
                jmap(json!([["error", { "type": "forbidden" }, "c0"]])),
            ]
        };
        for tool in Provided::ALL {
            let hint = write_hint(tool, write_step(tool)).expect("every tool has a write step");
            for step in [SESSION_STEP, API_STEP, DRAFT_SET_STEP] {
                if step != write_step(tool) {
                    assert_eq!(write_hint(tool, step), None, "{tool:?} step {step}");
                }
            }
            let state = write_step_state(tool);
            for outcome in lost() {
                let shown = format!("{outcome:?}");
                let Next::Fail(failed) = answered(&state, outcome) else {
                    panic!("{tool:?}: {shown} does not fail");
                };
                assert_eq!(failed.kind, ErrorKind::Internal, "{tool:?}: {failed:?}");
                assert!(
                    failed.detail.ends_with(&format!("; {hint}")),
                    "{tool:?}: {failed:?}"
                );
            }
            for outcome in refused() {
                let shown = format!("{outcome:?}");
                let Next::Fail(failed) = answered(&state, outcome) else {
                    panic!("{tool:?}: {shown} does not fail");
                };
                assert!(!failed.detail.contains(hint), "{tool:?}: {failed:?}");
            }
            let session = row(tool, admitted_args(tool), SESSION_STEP, json!({}));
            for outcome in [
                truncated(200, json!(300000), "{"),
                nested("internal", refusal),
            ] {
                let why = match &outcome {
                    Err(_) => Unreadable::Refused(refusal.to_owned()),
                    Ok(_) => Unreadable::Cut("300000 bytes".to_owned()),
                };
                assert_eq!(
                    answered(&session, outcome),
                    Next::Fail(why.failure(API_NAME, None)),
                    "{tool:?} session"
                );
            }
        }
    }

    #[test]
    fn fastmails_errors_map_to_the_callers_or_the_operators() {
        let state = write_step_state(Provided::FastmailOrganize);
        let method_error =
            |kind: &str| jmap(json!([["error", { "type": kind, "description": "why" }, "c0"]]));
        for kind in ["accountReadOnly", "forbidden"] {
            assert_eq!(
                answered(&state, method_error(kind)),
                Next::Fail(internal(format!(
                    "Fastmail: Email/set failed: {kind}: why; {READ_ONLY_TOKEN}"
                )))
            );
        }
        assert_eq!(
            answered(&state, method_error("requestTooLarge")),
            Next::Fail(failure(
                ErrorKind::InvalidArgs,
                "Fastmail: Email/set failed: requestTooLarge: why"
            ))
        );
        let refused = |error: Value| {
            answered(
                &state,
                set_answer(
                    "Email/set",
                    json!({ "accountId": "a1", "notUpdated": { "e1": error } }),
                ),
            )
        };
        assert_eq!(
            refused(json!({ "type": "forbidden" })),
            Next::Fail(failure(
                ErrorKind::InvalidArgs,
                &format!(
                    "none of the 1 messages was changed: Fastmail refused `e1`: forbidden; \
                     {FORBIDDEN_CHANGE}"
                )
            ))
        );
        assert_eq!(
            refused(json!({ "type": "rateLimit", "description": "slow down" })),
            Next::Fail(internal(
                "none of the 1 messages was changed: rateLimit: slow down; try again later"
            ))
        );
        assert_eq!(
            refused(json!({ "type": "notFound" })),
            Next::Fail(failure(
                ErrorKind::InvalidArgs,
                "none of the 1 messages was changed: no message with id `e1`"
            ))
        );
        let session = row(
            Provided::FastmailOrganize,
            admitted_args(Provided::FastmailOrganize),
            SESSION_STEP,
            json!({}),
        );
        for status in [401, 403] {
            assert_eq!(
                answered(&session, http(status, "{\"secret\":\"x\"}")),
                Next::Fail(internal(format!(
                    "Fastmail refused the token for this account (HTTP {status}): the \
                     operator must replace its key file and restart Brenn"
                )))
            );
            assert_eq!(
                answered(&state, http(status, "{}")),
                Next::Fail(internal(format!(
                    "Fastmail refused the token for this account (HTTP {status}): the \
                     operator must replace its key file and restart Brenn"
                )))
            );
        }
        assert_eq!(
            answered(&session, http(429, "")),
            Next::Fail(internal(
                "Fastmail is rate-limiting this deployment (HTTP 429); try again later"
            ))
        );
        let limit = json!({ "type": "urn:ietf:params:jmap:error:limit", "status": 400,
                            "detail": "too many", "limit": "maxConcurrentRequests" });
        assert_eq!(
            answered(&session, http(400, &limit.to_string())),
            Next::Fail(internal(
                "Fastmail: JMAP request refused: urn:ietf:params:jmap:error:limit (HTTP 400): \
                 too many; try again later"
            ))
        );
        let Next::Fail(denied) = answered(&state, nested("denied", "no clause admits it")) else {
            panic!("fails");
        };
        assert_eq!(denied.kind, ErrorKind::Denied);
        assert!(!denied.detail.contains(ORGANIZE_UNKNOWN_HINT), "{denied:?}");
    }

    #[test]
    fn a_step_a_tool_does_not_have_is_refused_as_such() {
        for tool in Provided::ALL {
            let step = write_step(tool) + 1;
            let state = row(
                tool,
                admitted_args(tool),
                step,
                json!({ ACCOUNT_KEY: "a1" }),
            );
            assert_eq!(
                answered(&state, jmap(json!([]))),
                Next::Fail(no_such_step(tool, step))
            );
        }
    }

    pub(super) fn incoming(id: &str, tool: &str, args: Value) -> Incoming {
        Incoming {
            id: id.to_owned(),
            tool: tool.to_owned(),
            args,
        }
    }

    #[test]
    fn a_request_runs_session_then_api_then_completes_and_excess_waits() {
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
        let args = admitted_args(Provided::FastmailOrganize);
        activate(
            vec![incoming(&id, "fastmail-organize", args.clone())],
            vec![],
        );
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
        activate(
            vec![],
            vec![deliver(
                &calls[0].call_id,
                set_answer(
                    "Email/set",
                    json!({ "accountId": "a1", "updated": { "e1": null } }),
                ),
            )],
        );
        let completions = host.take_completions();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, id);
        let Answered::Value(value) = &completions[0].answer else {
            panic!("{completions:?}");
        };
        assert_eq!(value, &json!({ "updated": 1 }));
        assert!(host.take_calls().is_empty());

        let requests = (0..=CALL_BUDGET)
            .map(|i| incoming(&format!("{i:032}"), "fastmail-organize", args.clone()))
            .collect();
        activate(requests, vec![]);
        assert_eq!(host.take_calls().len(), CALL_BUDGET);
        assert_eq!(host.rows(BACKLOG_NS).len(), 1);
    }
}
