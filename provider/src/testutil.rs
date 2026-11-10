//! Fixtures for a Fastmail component's host tests: its settings, a session,
//! a JMAP answer, a DAV multi-status or iCalendar answer, a parsed
//! calendar of events, a request's
//! in-flight row and a pending call, the patterns
//! its shared arguments are stated by and the probes that hold the parsers
//! to them.

use brenn_provider_kit::testutil::{http, payload};
use brenn_provider_kit::{Call, InFlight, NestedError, Next, Probes, Request, Tool};
use fastmail_dav::ical::{self, Component};
use fastmail_jmap::session::{API_PATH, CORE, MAIL, SESSION_PATH, SUBMISSION};
use serde_json::{Map, Value, json};

use crate::dav::CALENDAR_CONTENT_TYPE;
use crate::settings::Settings;
use crate::steps::{SESSION_STEP, api_call};

/// The most bytes a tool call's arguments may serialise to before the host
/// refuses the call. A copy of the host's `PROCESSOR_MAX_TOOL_ARGS_BYTES`,
/// which a component cannot see; a nested call's arguments are held to it.
pub const HOST_ARGS_CAP_BYTES: usize = 64 * 1024;

/// What `keyword` arguments admit. `keyword_arg` enforces exactly this;
/// [`keyword_probes`] hold it there.
pub const KEYWORD_PATTERN: &str = "^[A-Za-z0-9$_.-]{1,255}$";
/// What `account` admits. `account_arg` enforces exactly this;
/// [`account_probes`] hold it there.
pub const ACCOUNT_PATTERN: &str = "^[a-z]([a-z0-9-]{0,253}[a-z0-9])?$";
/// What a date argument admits: a date, or a UTC instant to the second.
/// `utc_date_arg` enforces exactly this; [`date_probes`] hold it there.
pub const DATE_PATTERN: &str = "^[0-9]{4}-[0-9]{2}-[0-9]{2}(T[0-9]{2}:[0-9]{2}:[0-9]{2}Z)?$";

/// An instance's settings: provider `fastmail`, host
/// `api.fastmail.example.com`.
pub fn settings() -> Settings {
    Settings::from_lookup(|key| match key {
        "provider" => Some("fastmail".to_owned()),
        "host" => Some("api.fastmail.example.com".to_owned()),
        _ => None,
    })
    .expect("the fixture's settings read")
}

/// A session answer advertising core and `capabilities`, each with primary
/// account `a1`.
pub fn session_with(capabilities: &[&str]) -> Result<Value, NestedError> {
    let advertised: Map<String, Value> = std::iter::once(CORE)
        .chain(capabilities.iter().copied())
        .map(|capability| (capability.to_owned(), json!({})))
        .collect();
    let primary: Map<String, Value> = std::iter::once(CORE)
        .chain(capabilities.iter().copied())
        .map(|capability| (capability.to_owned(), json!("a1")))
        .collect();
    let session = json!({
        "capabilities": advertised,
        "accounts": { "a1": { "name": "alice@example.com" } },
        "primaryAccounts": primary,
        "apiUrl": "https://api.fastmail.example.com/jmap/api/",
    });
    http(200, &session.to_string())
}

/// A session answer advertising mail and submission, both on account `a1`.
pub fn session_body() -> Result<Value, NestedError> {
    session_with(&[MAIL, SUBMISSION])
}

/// The in-flight row of `tool` with `args` at `step`, carrying `derived`:
/// the session step's call at [`SESSION_STEP`], an API call at any other.
pub fn row<T: Tool>(tool: T, args: Value, step: u8, derived: Value) -> InFlight<T> {
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

/// A pending provided call `id` of the tool named `tool`.
pub fn incoming(id: &str, tool: &str, args: Value) -> Request {
    Request {
        id: id.to_owned(),
        tool: tool.to_owned(),
        args,
    }
}

/// A JMAP answer carrying `responses` as its `methodResponses`.
pub fn jmap(responses: Value) -> Result<Value, NestedError> {
    http(
        200,
        &json!({ "methodResponses": responses, "sessionState": "s1" }).to_string(),
    )
}

/// A DAV `207 Multi-Status` answer carrying `xml`.
pub fn multistatus(xml: &str) -> Result<Value, NestedError> {
    payload(
        207,
        &[("content-type", "application/xml; charset=utf-8")],
        xml.as_bytes(),
    )
}

/// A `200` iCalendar answer carrying `text`.
pub fn ics(text: &str) -> Result<Value, NestedError> {
    ics_tagged(text, None)
}

/// A `200` iCalendar answer carrying `text`, with `etag` as its `ETag`
/// when given.
pub fn ics_tagged(text: &str, etag: Option<&str>) -> Result<Value, NestedError> {
    let mut headers = vec![("content-type", CALENDAR_CONTENT_TYPE)];
    headers.extend(etag.map(|etag| ("etag", etag)));
    payload(200, &headers, text.as_bytes())
}

/// A `VCALENDAR` holding one `VEVENT` per item of `events`, each item that
/// event's content lines, parsed.
pub fn calendar_of(events: &[&str]) -> Component {
    let mut text = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
    for lines in events {
        text.push_str(&format!("BEGIN:VEVENT\r\n{lines}END:VEVENT\r\n"));
    }
    text.push_str("END:VCALENDAR\r\n");
    ical::parse(text.as_bytes()).expect("the fixture parses")
}

/// A bodyless answer to a write with `status`, with `etag` as its `ETag`
/// when given.
pub fn written(status: u64, etag: Option<&str>) -> Result<Value, NestedError> {
    let headers: Vec<(&str, &str)> = etag.map(|etag| ("etag", etag)).into_iter().collect();
    payload(status, &headers, b"")
}

/// The completed request's result; panics on anything else.
pub fn completed(next: Next) -> Value {
    match next {
        Next::Complete(value) => value,
        other => panic!("not complete: {other:?}"),
    }
}

/// Probes holding `account_arg` to [`ACCOUNT_PATTERN`].
pub fn account_probes() -> Probes {
    Probes {
        admitted: vec![
            json!("alice"),
            json!("alice-work"),
            json!("a1"),
            json!("a"),
            json!("a".repeat(255)),
        ],
        refused: vec![
            json!(""),
            json!("Alice"),
            json!("alice_work"),
            json!("a b"),
            json!("1"),
            json!("-a"),
            json!("a-"),
            json!("a".repeat(256)),
            json!(1),
        ],
    }
}

/// Probes holding `utc_date_arg` to [`DATE_PATTERN`].
pub fn date_probes() -> Probes {
    Probes {
        admitted: vec![
            json!("2026-11-08"),
            json!("2026-11-08T10:00:00Z"),
            json!("0001-01-01"),
        ],
        refused: vec![
            json!("2026-11-08T10:00:00+01:00"),
            json!("2026-11-08T10:00:00"),
            json!("2026-11-08T10:00:00.5Z"),
            json!("2026-11-8"),
            json!("20261108"),
            json!("yesterday"),
            json!(""),
            json!(1),
        ],
    }
}

/// Probes holding `keyword_arg` to [`KEYWORD_PATTERN`].
pub fn keyword_probes() -> Probes {
    Probes {
        admitted: vec![json!("$seen"), json!("a.b_c-d"), json!("k".repeat(255))],
        refused: vec![
            json!("k".repeat(256)),
            json!(""),
            json!("a b"),
            json!("a/b"),
            json!("é"),
            json!(1),
        ],
    }
}

#[cfg(test)]
mod tests {
    use brenn_provider_kit::hold_probes;

    use super::*;
    use crate::args::{ACCOUNT_ARG, account_arg, keyword_arg, utc_date_arg};

    /// The probes against the parsers directly, so a component whose schema
    /// takes the patterns inherits a parser already held to them.
    #[test]
    fn the_shared_probes_hold_the_parsers_to_their_patterns() {
        let parse = |key: &'static str| {
            move |args: &Value| {
                let Value::Object(map) = args else {
                    return Err("an object".to_owned());
                };
                match key {
                    ACCOUNT_ARG => account_arg(map).map(drop),
                    "date" => utc_date_arg(map, key).map(drop),
                    _ => keyword_arg(map, key).map(drop),
                }
            }
        };
        hold_probes(
            "shared",
            ACCOUNT_ARG,
            &account_probes(),
            &json!({}),
            parse(ACCOUNT_ARG),
        );
        hold_probes(
            "shared",
            "keyword",
            &keyword_probes(),
            &json!({}),
            parse("keyword"),
        );
        hold_probes("shared", "date", &date_probes(), &json!({}), parse("date"));
    }
}
