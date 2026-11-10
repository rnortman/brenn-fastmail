//! The nested calls a Fastmail tool issues: the session first, then JMAP API
//! requests, every one naming the request's `account` so each step is sent
//! with the same key.

use std::collections::BTreeMap;

use brenn_provider_kit::{ApiCall, Args, Body, Call, Failure, Next, Query};
use fastmail_jmap::session::{self, API_PATH, SESSION_PATH};
use fastmail_jmap::{Request, Session};
use serde_json::{Value, json};

use crate::failure::jmap_failure;
use crate::settings::Settings;

/// Every tool's first step: the JMAP session.
pub const SESSION_STEP: u8 = 1;

/// The `derived` key the account id the tool's calls name is kept under.
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
        body: body.map(Body::Text),
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

/// The session step's answer `body` taken on: the primary account for
/// `capability`, kept in `derived` under [`ACCOUNT_KEY`], and `step` posting
/// the request `request` builds for that account. A session without the
/// capability or its primary account is the operator's failure.
pub fn after_session(
    body: &Value,
    capability: &str,
    settings: &Settings,
    account: Option<&str>,
    step: u8,
    request: impl FnOnce(&str) -> Request,
) -> Result<Next, Failure> {
    let session = read_session(body)?;
    let account_id = session.primary_account(capability).map_err(jmap_failure)?;
    let request = request(account_id);
    Ok(Next::Call {
        call: api_step(settings, account, step, &request),
        derived: Args::from_iter([(ACCOUNT_KEY.to_owned(), json!(account_id))]),
    })
}

#[cfg(test)]
mod tests {
    use brenn_provider_kit::ErrorKind;
    use fastmail_jmap::session::{CONTACTS, MAIL, SUBMISSION};

    use super::*;
    use crate::failure::SESSION_SETUP;
    use crate::testutil::{session_with, settings};

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
        assert_eq!(call.api.body, Some(Body::Text(request.to_body())));
    }

    /// The session `session_with` answers, as the session step reads it.
    fn body(capabilities: &[&str]) -> Value {
        serde_json::from_str(
            session_with(capabilities).unwrap()["body"]
                .as_str()
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn a_session_answers_the_accounts_it_advertises() {
        let with = read_session(&body(&[MAIL, SUBMISSION])).unwrap();
        assert_eq!(with.primary_account(MAIL).unwrap(), "a1");
        assert_eq!(with.primary_account(SUBMISSION).unwrap(), "a1");
        let contacts = read_session(&body(&[CONTACTS])).unwrap();
        assert_eq!(contacts.primary_account(CONTACTS).unwrap(), "a1");
        let refused = contacts
            .primary_account(MAIL)
            .map_err(jmap_failure)
            .unwrap_err();
        assert_eq!(refused.kind, ErrorKind::Internal);
        assert_eq!(
            refused.detail,
            format!(
                "the Fastmail token for this account cannot be used here: JMAP session: the \
                 account does not advertise urn:ietf:params:jmap:mail; {SESSION_SETUP}"
            )
        );
    }

    #[test]
    fn an_advertised_capability_without_a_primary_account_is_the_operators() {
        let mut session = body(&[CONTACTS]);
        session["primaryAccounts"]
            .as_object_mut()
            .unwrap()
            .remove(CONTACTS);
        let refused = read_session(&session)
            .unwrap()
            .primary_account(CONTACTS)
            .map_err(jmap_failure)
            .unwrap_err();
        assert_eq!(refused.kind, ErrorKind::Internal);
        assert_eq!(
            refused.detail,
            format!(
                "the Fastmail token for this account cannot be used here: JMAP session: no \
                 primary account for urn:ietf:params:jmap:contacts; {SESSION_SETUP}"
            )
        );
    }

    #[test]
    fn after_the_session_the_request_is_posted_for_the_capabilitys_account() {
        let next = after_session(
            &body(&[CONTACTS]),
            CONTACTS,
            &settings(),
            Some("alice"),
            2,
            |account_id| {
                assert_eq!(account_id, "a1");
                Request::contacts()
            },
        )
        .unwrap();
        let Next::Call { call, derived } = next else {
            panic!("not a call: {next:?}");
        };
        assert_eq!(
            call,
            api_step(&settings(), Some("alice"), 2, &Request::contacts())
        );
        assert_eq!(
            derived,
            Args::from_iter([(ACCOUNT_KEY.to_owned(), json!("a1"))])
        );
        let refused = after_session(&body(&[MAIL]), CONTACTS, &settings(), None, 2, |_| {
            panic!("no request without the capability")
        })
        .unwrap_err();
        assert_eq!(refused.kind, ErrorKind::Internal);
        assert!(refused.detail.contains(CONTACTS), "{}", refused.detail);
    }

    #[test]
    fn a_session_without_core_is_the_operators() {
        let refused = read_session(&serde_json::json!({ "capabilities": {} })).unwrap_err();
        assert_eq!(refused.kind, ErrorKind::Internal);
        assert!(
            refused.detail.starts_with(
                "the Fastmail token for this account cannot be used here: JMAP session:"
            ),
            "{}",
            refused.detail
        );
    }
}
