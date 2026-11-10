//! Fastmail's answer as the request's body or its failure: HTTP errors, JMAP
//! method errors, per-object `/set` refusals, and the hint a write step adds
//! to a failure that leaves the outcome unknown.

use brenn_provider_kit::{
    Answer, Args, ErrorKind, Failure, InFlight, NestedError, Tool, cut, internal, read_api_result,
};
use fastmail_jmap::{Error as JmapError, SetError, request_error};
use serde_json::{Value, json};

use crate::settings::API_NAME;

/// The request-level error for a request over one of the server's limits.
/// Of those (RFC 8620 section 3.6.1), a provider's few small calls can reach
/// only the concurrent-request count, which a retry clears.
pub const LIMIT_ERROR: &str = "urn:ietf:params:jmap:error:limit";

/// Method-level error types caused by what the caller asked for.
/// `requestTooLarge` is a `/set` naming more objects than the server takes
/// at once.
pub const CALLER_ERRORS: &[&str] = &[
    "invalidArguments",
    "unsupportedFilter",
    "unsupportedSort",
    "anchorNotFound",
    "requestTooLarge",
];

// TODO(restart-wording-after-key-reload): say "reload" once a Brenn release
// converges key files on reload, naming that version.
/// What the operator does when the account's token cannot write `data`
/// ("mail", "contacts").
pub fn read_only_token(data: &str) -> String {
    format!(
        "the token for this account cannot write {data}: the operator must replace its key \
         file with a token that has read-write access to {data} and restart Brenn"
    )
}

/// Where the operator looks when the token's session lacks a capability or
/// a primary account the tool needs.
pub const SESSION_SETUP: &str = "the operator checks the token's session as the README's \
     Fastmail setup step 3 describes";

/// Why Fastmail refuses one object `forbidden`, in the caller's terms.
pub const FORBIDDEN_CHANGE: &str = "the account's permissions do not allow this change to it";

/// What a message's mailbox membership requires, in the caller's terms.
pub const MAILBOX_MEMBERSHIP_HINT: &str = "a message must be in at least one mailbox and every \
     mailbox id must exist: when removing a message's last mailbox, add another";

/// A step's failure, by what Fastmail is known to have done.
#[derive(Clone, Debug, PartialEq)]
pub enum StepFailure {
    /// Fastmail answered that it did nothing: an HTTP 4xx rejects the request
    /// as a whole (RFC 8620 section 3.6.1), and a method error this crate
    /// maps to the caller or the operator means the method was not run.
    Refused(Failure),
    /// Fastmail's answer was lost, cut or unreadable, or says the server
    /// failed: at a write step, the write may have been applied.
    Unknown(Failure),
}

impl From<Failure> for StepFailure {
    fn from(failure: Failure) -> StepFailure {
        StepFailure::Unknown(failure)
    }
}

impl StepFailure {
    /// The request's failure.
    pub fn failure(self) -> Failure {
        match self {
            StepFailure::Refused(failure) | StepFailure::Unknown(failure) => failure,
        }
    }

    /// The request's failure at a step whose write hint is `hint`: an
    /// unknown outcome gets the hint as [`with_write_hint`] adds it.
    pub fn with_hint(self, hint: Option<&str>) -> Failure {
        match self {
            StepFailure::Refused(failure) => failure,
            StepFailure::Unknown(failure) => with_write_hint(hint, failure),
        }
    }
}

/// The JMAP JSON of a 2xx answer; every other answer is the request's
/// failure. An unreadable answer carries `unreadable_hint`, what the caller
/// narrows.
pub fn answer_body<T: Tool>(
    state: &InFlight<T>,
    outcome: Result<Value, NestedError>,
    unreadable_hint: Option<&str>,
) -> Result<Value, Failure> {
    step_answer(state, outcome, unreadable_hint).map_err(StepFailure::failure)
}

/// [`answer_body`], its failure telling an HTTP 4xx, which Fastmail answers
/// without acting, from every other.
pub fn step_answer<T: Tool>(
    state: &InFlight<T>,
    outcome: Result<Value, NestedError>,
    unreadable_hint: Option<&str>,
) -> Result<Value, StepFailure> {
    match read_api_result(outcome, &state.target, API_NAME)? {
        Answer::Body(body) => Ok(body),
        Answer::NoBody { status } => Err(internal(format!(
            "{API_NAME} answered HTTP {status} with no body, which JMAP never does"
        ))
        .into()),
        Answer::Relay(_) => Err(internal(
            "api-request answered with a credential relay, which an api_key_provider never sends",
        )
        .into()),
        Answer::Unreadable(why) => Err(why.failure(API_NAME, unreadable_hint).into()),
        Answer::Error { status, body } if (400..500).contains(&status) => {
            Err(StepFailure::Refused(http_failure(status, &body)))
        }
        Answer::Error { status, body } => Err(http_failure(status, &body).into()),
    }
}

/// Fastmail's HTTP error as the request's failure. The body is read only as
/// a JMAP problem object and never copied otherwise.
pub fn http_failure(status: u64, body: &str) -> Failure {
    match status {
        // TODO(restart-wording-after-key-reload): say "reload" once a Brenn
        // release converges key files on reload, naming that version.
        401 | 403 => internal(format!(
            "{API_NAME} refused the token for this account (HTTP {status}): the operator \
             must replace its key file and restart Brenn"
        )),
        429 => internal(format!(
            "{API_NAME} is rate-limiting this deployment (HTTP 429); try again later"
        )),
        400..=499 => {
            let problem = serde_json::from_str::<Value>(body)
                .ok()
                .as_ref()
                .and_then(request_error);
            match problem {
                Some(problem) if problem.kind == LIMIT_ERROR => internal(format!(
                    "{API_NAME}: {}; try again later",
                    JmapError::Request(problem)
                )),
                Some(problem) => internal(format!("{API_NAME}: {}", JmapError::Request(problem))),
                None => internal(format!("{API_NAME} answered HTTP {status}")),
            }
        }
        _ => internal(format!("{API_NAME} answered HTTP {status}")),
    }
}

/// A JMAP reading error as the request's failure: a method error the
/// caller's arguments caused is `invalid_args`, everything else `internal`.
pub fn jmap_failure(error: JmapError) -> Failure {
    match &error {
        JmapError::Session(_) => internal(format!(
            "the {API_NAME} token for this account cannot be used here: {error}; {SESSION_SETUP}"
        )),
        JmapError::Method { kind, .. } if CALLER_ERRORS.contains(&kind.as_str()) => {
            invalid(format!("{API_NAME}: {error}"))
        }
        _ => internal(format!("{API_NAME}: {error}")),
    }
}

/// A JMAP reading error at a write method: the method refused because the
/// token cannot write to the account (`accountReadOnly`, or `forbidden`,
/// which refuses the whole method rather than one object) is the operator's,
/// with [`read_only_token`] for `data`; everything else as [`jmap_failure`].
/// Those two and the caller's errors are [`StepFailure::Refused`]; every
/// other error, a server failure or a malformed answer among them, leaves
/// the write unknown.
pub fn write_method_failure(data: &str, error: JmapError) -> StepFailure {
    match &error {
        JmapError::Method { kind, .. } if kind == "accountReadOnly" || kind == "forbidden" => {
            StepFailure::Refused(internal(format!(
                "{API_NAME}: {error}; {}",
                read_only_token(data)
            )))
        }
        JmapError::Method { kind, .. } if CALLER_ERRORS.contains(&kind.as_str()) => {
            StepFailure::Refused(jmap_failure(error))
        }
        _ => StepFailure::Unknown(jmap_failure(error)),
    }
}

/// Whether a message's refusal is about its mailbox membership: it names
/// `mailboxIds`, which covers both a patch that leaves no mailbox and one
/// naming a mailbox the account does not have (RFC 8621 section 4.6).
pub fn concerns_mailbox_membership(error: &SetError) -> bool {
    error.properties.iter().any(|p| p == "mailboxIds")
}

/// Why Fastmail refused one object of a `/set` (a `what`, as [`set_failure`]
/// takes it): its `SetError` rendered, and for a message (`what` is
/// "message") whose refusal
/// [`concerns_mailbox_membership`], [`MAILBOX_MEMBERSHIP_HINT`] appended.
pub fn set_error_reason(what: &str, error: &SetError) -> String {
    let mut reason = error.to_string();
    if what == "message" && concerns_mailbox_membership(error) {
        reason.push_str("; ");
        reason.push_str(MAILBOX_MEMBERSHIP_HINT);
    }
    reason
}

/// A `/set`'s refusal of object `id` (a `what`, the object's kind in the
/// caller's terms: "message", "mailbox", "contact") as the request's failure:
/// a refusal the caller's arguments caused is `invalid_args`, one the server
/// caused `internal`. `forbidden` is the account's permissions refusing this
/// change to the object, such as a mailbox the user may not rename (RFC 8620
/// section 5.3), not the token's access, which the method's own error
/// reports.
pub fn set_failure(what: &str, id: &str, error: &SetError) -> Failure {
    let reason = set_error_reason(what, error);
    match error.kind.as_str() {
        "notFound" => invalid(format!("no {what} with id `{id}`")),
        "invalidProperties" | "invalidPatch" | "tooManyKeywords" | "tooManyMailboxes"
        | "singleton" => invalid(format!("{API_NAME} refused `{id}`: {reason}")),
        "forbidden" => invalid(format!(
            "{API_NAME} refused `{id}`: {reason}; {FORBIDDEN_CHANGE}"
        )),
        "rateLimit" => internal(format!("{reason}; try again later")),
        _ => internal(format!("{API_NAME} refused `{id}`: {reason}")),
    }
}

/// `failure` as a step answers it: at a write step (`hint` is `Some`), an
/// `internal` failure gets the hint appended, since Fastmail may have acted
/// before its answer was lost. Every other failure, and every failure at a
/// read step (`None`), passes through unchanged.
pub fn with_write_hint(hint: Option<&str>, failure: Failure) -> Failure {
    match hint {
        Some(hint) if failure.kind == ErrorKind::Internal => {
            internal(format!("{}; {hint}", failure.detail))
        }
        _ => failure,
    }
}

/// An `invalid_args` failure: the caller's arguments are at fault.
pub fn invalid(detail: String) -> Failure {
    Failure {
        kind: ErrorKind::InvalidArgs,
        detail,
    }
}

/// The request's arguments as `parse` reads them; arguments a row stored
/// that no longer parse are the request's internal failure.
pub fn stored_args<T: Tool, P>(
    state: &InFlight<T>,
    parse: fn(T, &Value) -> Result<P, String>,
) -> Result<P, Failure> {
    parse(state.tool, &state.args).map_err(|reason| {
        internal(format!(
            "stored {} arguments no longer parse: {reason}",
            state.tool.name()
        ))
    })
}

/// The string `state` carries in `derived` at `key`.
pub fn derived_str<'a, T: Tool>(state: &'a InFlight<T>, key: &str) -> Result<&'a str, Failure> {
    state
        .derived
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| missing_derived(state, key))
}

/// The count `state` carries in `derived` at `key`.
pub fn derived_u64<T: Tool>(state: &InFlight<T>, key: &str) -> Result<u64, Failure> {
    state
        .derived
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| missing_derived(state, key))
}

/// A row whose `derived` lacks `key`: a store fault.
pub fn missing_derived<T: Tool>(state: &InFlight<T>, key: &str) -> Failure {
    internal(format!(
        "in-flight row at {} step {} carries no `{key}`",
        state.tool.name(),
        state.step
    ))
}

/// `value` cut at `max_bytes` at `key`; nothing when absent.
pub fn optional(out: &mut Args, key: &str, value: &Option<String>, max_bytes: usize) {
    if let Some(value) = value {
        out.insert(key.into(), json!(cut(value, max_bytes).0));
    }
}

#[cfg(test)]
mod tests {
    use brenn_provider_kit::testutil::{failure, http, nested};

    use super::*;
    use crate::steps::SESSION_STEP;
    use crate::testutil;

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Probe;

    impl Tool for Probe {
        fn name(self) -> &'static str {
            "probe"
        }

        fn from_name(name: &str) -> Option<Probe> {
            (name == "probe").then_some(Probe)
        }
    }

    fn row(derived: Value) -> InFlight<Probe> {
        testutil::row(Probe, json!({}), SESSION_STEP, derived)
    }

    fn method(kind: &str) -> JmapError {
        JmapError::Method {
            name: "Email/set".to_owned(),
            kind: kind.to_owned(),
            description: "no".to_owned(),
        }
    }

    fn set_error(kind: &str, description: Option<&str>, properties: &[&str]) -> SetError {
        SetError {
            kind: kind.to_owned(),
            description: description.map(str::to_owned),
            properties: properties.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    #[test]
    fn an_answer_is_its_body_or_the_requests_failure() {
        let state = row(json!({}));
        assert_eq!(
            answer_body(&state, http(200, r#"{"a":1}"#), None),
            Ok(json!({ "a": 1 }))
        );
        assert_eq!(
            answer_body(&state, http(401, ""), None).unwrap_err(),
            http_failure(401, "")
        );
        let detail = "`api-request` refused the call";
        assert_eq!(
            answer_body(&state, nested("internal", detail), None)
                .unwrap_err()
                .kind,
            ErrorKind::Internal
        );
        let cut = answer_body(
            &state,
            brenn_provider_kit::testutil::truncated(200, json!(9_000_000), "{"),
            Some("ask for less"),
        )
        .unwrap_err();
        assert!(cut.detail.contains("ask for less"), "{}", cut.detail);
    }

    #[test]
    fn http_errors_are_the_operators() {
        for status in [401, 403] {
            assert_eq!(
                http_failure(status, ""),
                failure(
                    ErrorKind::Internal,
                    &format!(
                        "Fastmail refused the token for this account (HTTP {status}): the \
                         operator must replace its key file and restart Brenn"
                    )
                )
            );
        }
        assert_eq!(
            http_failure(429, "").detail,
            "Fastmail is rate-limiting this deployment (HTTP 429); try again later"
        );
        let limit = json!({ "type": LIMIT_ERROR, "status": 400, "detail": "too many" });
        assert!(
            http_failure(400, &limit.to_string())
                .detail
                .ends_with("; try again later")
        );
        assert_eq!(
            http_failure(400, "not json").detail,
            "Fastmail answered HTTP 400"
        );
        assert_eq!(http_failure(500, "").detail, "Fastmail answered HTTP 500");
    }

    #[test]
    fn the_read_only_token_text_names_the_data_the_token_cannot_write() {
        assert_eq!(
            read_only_token("mail"),
            "the token for this account cannot write mail: the operator must replace its key \
             file with a token that has read-write access to mail and restart Brenn"
        );
        assert_eq!(
            read_only_token("contacts"),
            "the token for this account cannot write contacts: the operator must replace its \
             key file with a token that has read-write access to contacts and restart Brenn"
        );
    }

    #[test]
    fn method_errors_map_to_the_callers_or_the_operators() {
        for kind in CALLER_ERRORS {
            assert_eq!(jmap_failure(method(kind)).kind, ErrorKind::InvalidArgs);
        }
        assert_eq!(
            jmap_failure(method("requestTooLarge")).detail,
            "Fastmail: Email/set failed: requestTooLarge: no"
        );
        assert_eq!(jmap_failure(method("serverFail")).kind, ErrorKind::Internal);
        assert_eq!(
            jmap_failure(JmapError::Session("x".to_owned())).detail,
            format!(
                "the Fastmail token for this account cannot be used here: JMAP session: x; \
                 {SESSION_SETUP}"
            )
        );
        for kind in ["accountReadOnly", "forbidden"] {
            for data in ["mail", "contacts"] {
                assert_eq!(
                    write_method_failure(data, method(kind)),
                    StepFailure::Refused(failure(
                        ErrorKind::Internal,
                        &format!(
                            "Fastmail: Email/set failed: {kind}: no; {}",
                            read_only_token(data)
                        )
                    ))
                );
            }
            assert_eq!(jmap_failure(method(kind)).kind, ErrorKind::Internal);
            assert!(!jmap_failure(method(kind)).detail.contains("cannot write"));
        }
        assert_eq!(
            write_method_failure("mail", method("requestTooLarge")),
            StepFailure::Refused(jmap_failure(method("requestTooLarge")))
        );
        for kind in [
            "serverFail",
            "serverUnavailable",
            "serverPartialFail",
            "somethingNew",
        ] {
            assert_eq!(
                write_method_failure("mail", method(kind)),
                StepFailure::Unknown(jmap_failure(method(kind)))
            );
        }
        assert_eq!(
            write_method_failure("mail", JmapError::Shape("x".to_owned())),
            StepFailure::Unknown(jmap_failure(JmapError::Shape("x".to_owned())))
        );
    }

    #[test]
    fn only_an_http_4xx_says_fastmail_did_nothing() {
        let state = row(json!({}));
        for status in [400, 401, 403, 429] {
            assert_eq!(
                step_answer(&state, http(status, ""), None),
                Err(StepFailure::Refused(http_failure(status, "")))
            );
        }
        assert_eq!(
            step_answer(&state, http(500, ""), None),
            Err(StepFailure::Unknown(http_failure(500, "")))
        );
        assert!(matches!(
            step_answer(&state, http(200, ""), None),
            Err(StepFailure::Unknown(_))
        ));
        assert!(matches!(
            step_answer(&state, nested("internal", "lost"), None),
            Err(StepFailure::Unknown(_))
        ));
        let refused = StepFailure::Refused(internal("refused"));
        assert_eq!(refused.with_hint(Some("look first")), internal("refused"));
        let lost = StepFailure::Unknown(internal("lost"));
        assert_eq!(
            lost.clone().with_hint(Some("look first")),
            internal("lost; look first")
        );
        assert_eq!(lost.failure(), internal("lost"));
    }

    #[test]
    fn only_a_messages_mailbox_ids_gain_the_membership_rule() {
        let mailboxes = set_error("invalidProperties", Some("no mailbox"), &["mailboxIds"]);
        assert_eq!(
            set_error_reason("message", &mailboxes),
            format!("invalidProperties: no mailbox (mailboxIds); {MAILBOX_MEMBERSHIP_HINT}")
        );
        assert_eq!(
            set_error_reason("mailbox", &mailboxes),
            "invalidProperties: no mailbox (mailboxIds)"
        );
        let parent = set_error("invalidProperties", None, &["parentId"]);
        assert_eq!(
            set_error_reason("mailbox", &parent),
            "invalidProperties (parentId)"
        );
        let keywords = set_error("invalidProperties", None, &["keywords"]);
        assert_eq!(
            set_error_reason("message", &keywords),
            "invalidProperties (keywords)"
        );
    }

    #[test]
    fn a_set_refusal_is_the_callers_or_the_operators_by_kind() {
        let refused = |kind: &str| set_failure("message", "m1", &set_error(kind, None, &[]));
        assert_eq!(
            refused("notFound"),
            failure(ErrorKind::InvalidArgs, "no message with id `m1`")
        );
        assert_eq!(
            set_failure("mailbox", "x1", &set_error("notFound", None, &[])).detail,
            "no mailbox with id `x1`"
        );
        for kind in [
            "invalidProperties",
            "invalidPatch",
            "tooManyKeywords",
            "tooManyMailboxes",
            "singleton",
        ] {
            assert_eq!(
                refused(kind),
                failure(
                    ErrorKind::InvalidArgs,
                    &format!("Fastmail refused `m1`: {kind}")
                )
            );
        }
        assert_eq!(
            refused("forbidden"),
            failure(
                ErrorKind::InvalidArgs,
                &format!("Fastmail refused `m1`: forbidden; {FORBIDDEN_CHANGE}")
            )
        );
        assert_eq!(
            refused("rateLimit"),
            failure(ErrorKind::Internal, "rateLimit; try again later")
        );
        for kind in [
            "overQuota",
            "tooLarge",
            "blobNotFound",
            "mailboxHasChild",
            "mailboxHasEmail",
            "somethingNew",
        ] {
            assert_eq!(
                refused(kind),
                failure(
                    ErrorKind::Internal,
                    &format!("Fastmail refused `m1`: {kind}")
                )
            );
        }
        let membership = set_failure(
            "message",
            "m1",
            &set_error("invalidProperties", Some("x"), &["mailboxIds"]),
        );
        assert!(
            membership.detail.ends_with(MAILBOX_MEMBERSHIP_HINT),
            "{}",
            membership.detail
        );
    }

    #[test]
    fn a_write_hint_is_added_to_internal_failures_only() {
        let lost = internal("the answer was lost");
        assert_eq!(
            with_write_hint(Some("look first"), lost.clone()),
            internal("the answer was lost; look first")
        );
        assert_eq!(with_write_hint(None, lost.clone()), lost);
        let bad = invalid("`id` is required".to_owned());
        assert_eq!(with_write_hint(Some("look first"), bad.clone()), bad);
    }

    #[test]
    fn a_rows_derived_values_are_read_or_refused_as_a_store_fault() {
        let state = row(json!({ "account_id": "a1", "count": 3 }));
        assert_eq!(derived_str(&state, "account_id"), Ok("a1"));
        assert_eq!(derived_u64(&state, "count"), Ok(3));
        assert_eq!(
            derived_str(&state, "count").unwrap_err(),
            internal("in-flight row at probe step 1 carries no `count`")
        );
        assert_eq!(
            derived_u64(&state, "absent").unwrap_err(),
            internal("in-flight row at probe step 1 carries no `absent`")
        );
    }

    #[test]
    fn an_optional_value_is_cut_or_left_out() {
        let mut out = Args::new();
        optional(&mut out, "a", &Some("hello".to_owned()), 3);
        optional(&mut out, "b", &None, 3);
        assert_eq!(Value::Object(out), json!({ "a": cut("hello", 3).0 }));
    }
}
