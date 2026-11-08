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
// TODO(sieve-capability-check): whether Fastmail's session advertises
// urn:ietf:params:jmap:sieve (RFC 9661) decides how filters are authored.

/// What a mail call needs from the session object (RFC 8620 section 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub mail_account_id: String,
}

/// Checks that the server advertises mail and reads the primary account for
/// mail. `apiUrl` is not read: the endpoint is fixed by `API_PATH`, and the
/// deployment's clause admits that path and no other.
pub fn parse(session: &Value) -> Result<Session, Error> {
    let capabilities = session
        .get("capabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Shape("session has no capabilities object".into()))?;
    if !capabilities.contains_key(MAIL) {
        return Err(Error::Session(format!(
            "the account does not advertise {MAIL}"
        )));
    }
    let mail_account_id = session
        .get("primaryAccounts")
        .and_then(|accounts| accounts.get(MAIL))
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Session(format!("no primary account for {MAIL}")))?
        .to_owned();
    Ok(Session { mail_account_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../fixtures/session.json")).unwrap()
    }

    #[test]
    fn reads_the_primary_mail_account() {
        assert_eq!(
            parse(&fixture()).unwrap(),
            Session {
                mail_account_id: "a1".into()
            }
        );
    }

    #[test]
    fn a_session_without_mail_is_refused() {
        let mut session = fixture();
        session["capabilities"]
            .as_object_mut()
            .unwrap()
            .remove(MAIL);
        assert!(matches!(parse(&session), Err(Error::Session(_))));
    }

    #[test]
    fn a_session_without_a_primary_mail_account_is_refused() {
        let mut session = fixture();
        session["primaryAccounts"]
            .as_object_mut()
            .unwrap()
            .remove(MAIL);
        assert!(matches!(parse(&session), Err(Error::Session(_))));
    }

    #[test]
    fn a_session_without_capabilities_is_malformed() {
        let session = serde_json::json!({"primaryAccounts": {MAIL: "a1"}});
        assert!(matches!(parse(&session), Err(Error::Shape(_))));
    }
}
