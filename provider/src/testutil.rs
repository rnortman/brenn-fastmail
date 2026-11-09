//! Fixtures for a Fastmail component's host tests: its settings, a session,
//! a JMAP answer, the patterns its shared arguments are stated by and the
//! probes that hold the parsers to them.

use brenn_provider_kit::testutil::http;
use brenn_provider_kit::{NestedError, Next, Probes};
use fastmail_jmap::session::{CORE, MAIL, SUBMISSION};
use serde_json::{Value, json};

use crate::settings::Settings;

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

/// A session answer advertising mail and submission, both on account `a1`.
pub fn session_body() -> Result<Value, NestedError> {
    let session = json!({
        "capabilities": { CORE: {}, MAIL: {}, SUBMISSION: {} },
        "accounts": { "a1": { "name": "alice@example.com" } },
        "primaryAccounts": { MAIL: "a1", SUBMISSION: "a1" },
        "apiUrl": "https://api.fastmail.example.com/jmap/api/",
    });
    http(200, &session.to_string())
}

/// A JMAP answer carrying `responses` as its `methodResponses`.
pub fn jmap(responses: Value) -> Result<Value, NestedError> {
    http(
        200,
        &json!({ "methodResponses": responses, "sessionState": "s1" }).to_string(),
    )
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
    use crate::args::{ACCOUNT_ARG, account_arg, keyword_arg};

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
    }
}
