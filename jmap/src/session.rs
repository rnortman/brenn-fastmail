use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::error::Error;

/// The session resource. Fetched before every API call: the account id it
/// names is per-token.
pub const SESSION_PATH: &str = "/jmap/session";
/// The API endpoint. No trailing slash: a path with an empty segment is not
/// one `api-request` will send.
pub const API_PATH: &str = "/jmap/api";

/// The JMAP core capability (RFC 8620).
pub const CORE: &str = "urn:ietf:params:jmap:core";
/// The JMAP mail capability (RFC 8621).
pub const MAIL: &str = "urn:ietf:params:jmap:mail";
/// The JMAP submission capability (RFC 8621 section 1.3.4), under which
/// sending identities are read.
pub const SUBMISSION: &str = "urn:ietf:params:jmap:submission";
/// The JMAP contacts capability (RFC 9610 section 1.4.1).
pub const CONTACTS: &str = "urn:ietf:params:jmap:contacts";
// TODO(sieve-filters-without-jmap-sieve): Fastmail offers no JMAP Sieve
// capability (RFC 9661); filter authoring has no supported route.

/// What a call needs from the session object (RFC 8620 section 2): the
/// capabilities it advertises and the primary account of each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    capabilities: BTreeSet<String>,
    primary_accounts: BTreeMap<String, String>,
}

impl Session {
    /// The primary account for `capability`. A capability the session does
    /// not advertise, or one with no string-valued primary account, is
    /// `Error::Session`.
    pub fn primary_account(&self, capability: &str) -> Result<&str, Error> {
        if !self.capabilities.contains(capability) {
            return Err(Error::Session(format!(
                "the account does not advertise {capability}"
            )));
        }
        self.primary_accounts
            .get(capability)
            .map(String::as_str)
            .ok_or_else(|| Error::Session(format!("no primary account for {capability}")))
    }
}

/// Checks that the server advertises core and reads the capabilities and
/// primary accounts. No data-type capability is required here: a caller asks
/// `Session::primary_account` for the one it uses. A `primaryAccounts` entry
/// that is not a string is treated as absent. `apiUrl` is not read: the
/// endpoint is fixed by `API_PATH`, and the deployment's clause admits that
/// path and no other.
pub fn parse(session: &Value) -> Result<Session, Error> {
    let capabilities = session
        .get("capabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Shape("session has no capabilities object".into()))?;
    if !capabilities.contains_key(CORE) {
        return Err(Error::Session(format!(
            "the account does not advertise {CORE}"
        )));
    }
    let primary_accounts = session
        .get("primaryAccounts")
        .and_then(Value::as_object)
        .map(|accounts| {
            accounts
                .iter()
                .filter_map(|(capability, account)| {
                    account
                        .as_str()
                        .map(|account| (capability.clone(), account.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Session {
        capabilities: capabilities.keys().cloned().collect(),
        primary_accounts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../fixtures/session.json")).unwrap()
    }

    fn contacts_fixture() -> Value {
        serde_json::from_str(include_str!("../fixtures/session_contacts.json")).unwrap()
    }

    fn refusal(session: &Session, capability: &str) -> String {
        match session.primary_account(capability) {
            Err(Error::Session(message)) => message,
            other => panic!("expected a session error, got {other:?}"),
        }
    }

    #[test]
    fn reads_the_primary_mail_and_submission_accounts() {
        let session = parse(&fixture()).unwrap();
        assert_eq!(session.primary_account(MAIL).unwrap(), "a1");
        assert_eq!(session.primary_account(SUBMISSION).unwrap(), "a1");
    }

    #[test]
    fn a_contacts_only_session_reads_its_contacts_account_and_no_mail() {
        let session = parse(&contacts_fixture()).unwrap();
        assert_eq!(session.primary_account(CONTACTS).unwrap(), "a1");
        assert_eq!(
            refusal(&session, MAIL),
            "the account does not advertise urn:ietf:params:jmap:mail"
        );
    }

    #[test]
    fn an_unadvertised_capability_is_refused() {
        let mut session = fixture();
        session["capabilities"]
            .as_object_mut()
            .unwrap()
            .remove(SUBMISSION);
        assert_eq!(
            refusal(&parse(&session).unwrap(), SUBMISSION),
            "the account does not advertise urn:ietf:params:jmap:submission"
        );
        assert_eq!(
            refusal(&parse(&fixture()).unwrap(), CONTACTS),
            "the account does not advertise urn:ietf:params:jmap:contacts"
        );
    }

    #[test]
    fn an_advertised_capability_without_a_string_primary_account_is_refused() {
        let mut session = fixture();
        session["primaryAccounts"]
            .as_object_mut()
            .unwrap()
            .remove(MAIL);
        assert_eq!(
            refusal(&parse(&session).unwrap(), MAIL),
            "no primary account for urn:ietf:params:jmap:mail"
        );
        session["primaryAccounts"][MAIL] = serde_json::json!(7);
        assert_eq!(
            refusal(&parse(&session).unwrap(), MAIL),
            "no primary account for urn:ietf:params:jmap:mail"
        );
        session["primaryAccounts"][MAIL] = Value::Null;
        assert_eq!(
            refusal(&parse(&session).unwrap(), MAIL),
            "no primary account for urn:ietf:params:jmap:mail"
        );
    }

    #[test]
    fn a_session_without_primary_accounts_parses_and_names_none() {
        let mut session = fixture();
        session.as_object_mut().unwrap().remove("primaryAccounts");
        assert_eq!(
            refusal(&parse(&session).unwrap(), MAIL),
            "no primary account for urn:ietf:params:jmap:mail"
        );
    }

    #[test]
    fn a_session_without_core_is_refused() {
        let mut session = fixture();
        session["capabilities"]
            .as_object_mut()
            .unwrap()
            .remove(CORE);
        assert!(matches!(
            parse(&session),
            Err(Error::Session(message))
                if message == "the account does not advertise urn:ietf:params:jmap:core"
        ));
    }

    #[test]
    fn a_session_without_capabilities_is_malformed() {
        let session = serde_json::json!({"primaryAccounts": {MAIL: "a1"}});
        assert!(matches!(parse(&session), Err(Error::Shape(_))));
    }
}
