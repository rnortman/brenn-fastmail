//! The nested calls a Fastmail tool issues: the session first, then JMAP API
//! requests, every one naming the request's `account` so each step is sent
//! with the same key.

use std::collections::BTreeMap;

use brenn_provider_kit::{ApiCall, Call, Failure, Query};
use fastmail_jmap::session::{self, API_PATH, SESSION_PATH};
use fastmail_jmap::{Request, Session};
use serde_json::Value;

use crate::failure::jmap_failure;
use crate::settings::Settings;

/// Every tool's first step: the JMAP session.
pub const SESSION_STEP: u8 = 1;

/// The `derived` key the mail account's id is kept under.
pub const ACCOUNT_KEY: &str = "account_id";

/// One nested call of a request, naming the request's `account`.
pub fn api_call(
    settings: &Settings,
    account: Option<&str>,
    method: &str,
    path: &str,
    body: Option<String>,
) -> ApiCall {
    ApiCall {
        provider: settings.provider.clone(),
        account: account.map(str::to_owned),
        method: method.to_owned(),
        host: settings.host.clone(),
        path: path.to_owned(),
        query: Query::new(),
        headers: BTreeMap::new(),
        body,
    }
}

/// The session step: `GET` the JMAP session for `account`.
pub fn session_call(settings: &Settings, account: Option<&str>) -> Call {
    Call {
        step: SESSION_STEP,
        api: api_call(settings, account, "GET", SESSION_PATH, None),
    }
}

/// The session step's answer read as a session, or the request's failure.
pub fn read_session(body: &Value) -> Result<Session, Failure> {
    session::parse(body).map_err(jmap_failure)
}

/// A JMAP API step: `request` posted for `account`.
pub fn api_step(settings: &Settings, account: Option<&str>, step: u8, request: &Request) -> Call {
    Call {
        step,
        api: api_call(settings, account, "POST", API_PATH, Some(request.to_body())),
    }
}

#[cfg(test)]
mod tests {
    use brenn_provider_kit::ErrorKind;
    use fastmail_jmap::session::{CORE, MAIL, SUBMISSION};
    use serde_json::json;

    use super::*;
    use crate::testutil::settings;

    #[test]
    fn the_session_and_api_steps_name_the_requests_account() {
        let call = session_call(&settings(), Some("alice"));
        assert_eq!(call.step, SESSION_STEP);
        assert_eq!(
            call.api,
            ApiCall {
                provider: "fastmail".to_owned(),
                account: Some("alice".to_owned()),
                method: "GET".to_owned(),
                host: "api.fastmail.example.com".to_owned(),
                path: "/jmap/session".to_owned(),
                query: Query::new(),
                headers: BTreeMap::new(),
                body: None,
            }
        );
        let request = Request::mail();
        let call = api_step(&settings(), None, 2, &request);
        assert_eq!(call.step, 2);
        assert_eq!(call.api.account, None);
        assert_eq!(
            (call.api.method.as_str(), call.api.path.as_str()),
            ("POST", "/jmap/api")
        );
        assert_eq!(call.api.body, Some(request.to_body()));
    }

    #[test]
    fn a_session_reads_with_or_without_submission() {
        let session = |capabilities: Value, primary: Value| {
            json!({
                "capabilities": capabilities,
                "accounts": { "a1": { "name": "alice@example.com" } },
                "primaryAccounts": primary,
                "apiUrl": "https://api.fastmail.example.com/jmap/api/",
            })
        };
        let with = read_session(&session(
            json!({ CORE: {}, MAIL: {}, SUBMISSION: {} }),
            json!({ MAIL: "a1", SUBMISSION: "a1" }),
        ))
        .unwrap();
        assert_eq!(with.mail_account_id, "a1");
        assert_eq!(with.submission_account_id.as_deref(), Some("a1"));
        let without = read_session(&session(
            json!({ CORE: {}, MAIL: {} }),
            json!({ MAIL: "a1" }),
        ))
        .unwrap();
        assert_eq!(without.submission_account_id, None);
        let refused = read_session(&session(json!({ CORE: {} }), json!({}))).unwrap_err();
        assert_eq!(refused.kind, ErrorKind::Internal);
        assert!(
            refused
                .detail
                .starts_with("the Fastmail account cannot be read as mail: JMAP session:"),
            "{}",
            refused.detail
        );
    }
}
