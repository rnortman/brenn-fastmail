//! The nested calls a DAV component issues and its answers read: a WebDAV
//! (RFC 4918) or CalDAV (RFC 4791) call with its `Depth`, body and `Accept`
//! headers, and an answer taken as its 2xx payload or mapped to the
//! request's failure.

use brenn_provider_kit::{
    ApiCall, Failure, InFlight, NestedError, Payload, PayloadAnswer, Tool, internal,
    read_api_payload,
};
use serde_json::Value;

use crate::failure::{RELAY_REFUSAL, invalid, rate_limited};
use crate::results::cut_width;
use crate::settings::{API_NAME, Settings};
use crate::steps::api_call;

/// The content type of every DAV request body this crate sends.
pub const XML_CONTENT_TYPE: &str = "application/xml; charset=utf-8";

/// The widest a path is quoted in a failure.
pub const QUOTED_PATH_MAX: usize = 200;

/// The method and path `state`'s call asked, for a failure.
pub fn asked<T: Tool>(state: &InFlight<T>) -> String {
    format!(
        "{} `{}`",
        state.target.method,
        cut_width(&state.target.path, QUOTED_PATH_MAX).0
    )
}

/// One nested DAV call naming the request's `account`: [`api_call`] with
/// `Depth` when `depth` is given, `Content-Type` [`XML_CONTENT_TYPE`] when
/// there is a body, and `Accept` when `accept` is given. A `PROPFIND`
/// without `Depth` means `infinity` (RFC 4918 section 9.1), so every
/// `PROPFIND` and `REPORT` passes one.
// TODO(quality-dav-call-stringly-depth-and-option-sprawl): state the
// method-dependent `Depth`, body and `Accept` rules as types once a second
// DAV caller shows which methods it needs.
pub fn dav_call(
    settings: &Settings,
    account: Option<&str>,
    method: &str,
    path: &str,
    depth: Option<&str>,
    body: Option<String>,
    accept: Option<&str>,
) -> ApiCall {
    let has_body = body.is_some();
    let mut call = api_call(settings, account, method, path, body);
    if let Some(depth) = depth {
        call.headers.insert("Depth".to_owned(), depth.to_owned());
    }
    if has_body {
        call.headers
            .insert("Content-Type".to_owned(), XML_CONTENT_TYPE.to_owned());
    }
    if let Some(accept) = accept {
        call.headers.insert("Accept".to_owned(), accept.to_owned());
    }
    call
}

/// The step's answer as its payload when Fastmail answered HTTP `expect`;
/// every other answer is the request's failure. A 404 is the caller's,
/// "no `missing`", when the step asked for a handle the caller passed
/// (`missing` is given); otherwise it is internal, as every other status is.
/// `hint`, what the caller narrows, follows an unreadable answer and a 507.
/// An error body is read only for the precondition a `{DAV:}error` names,
/// and never copied otherwise.
pub fn payload<T: Tool>(
    state: &InFlight<T>,
    outcome: Result<Value, NestedError>,
    expect: u64,
    missing: Option<&str>,
    hint: Option<&str>,
) -> Result<Payload, Failure> {
    match read_api_payload(outcome, &state.target, API_NAME)? {
        PayloadAnswer::Payload(payload) if payload.status == expect => Ok(payload),
        PayloadAnswer::Payload(payload) => Err(internal(format!(
            "{API_NAME} answered HTTP {} to {}, where {expect} was expected",
            payload.status,
            asked(state)
        ))),
        PayloadAnswer::Relay(_) => Err(internal(RELAY_REFUSAL)),
        PayloadAnswer::Unreadable(why) => Err(why.failure(API_NAME, hint)),
        PayloadAnswer::Error { status, body } => Err(match (status, missing) {
            // TODO(restart-wording-after-key-reload): say "reload" once a Brenn
            // release converges key files on reload, naming that version.
            (401, _) => internal(format!(
                "{API_NAME} refused the app password for this account (HTTP 401): the operator \
                 must replace its key file and restart Brenn"
            )),
            (404, Some(missing)) => invalid(format!("no {missing}")),
            (429, _) => rate_limited(),
            (507, _) => {
                let found = format!(
                    "{API_NAME} found more than it will return for {} (HTTP 507)",
                    asked(state)
                );
                internal(match hint {
                    Some(hint) => format!("{found}; {hint}"),
                    None => found,
                })
            }
            _ => {
                let answered = format!("{API_NAME} answered HTTP {status} to {}", asked(state));
                internal(match fastmail_dav::error_condition(&body) {
                    Some(condition) => format!("{answered}: precondition `{condition}` failed"),
                    None => answered,
                })
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use brenn_provider_kit::testutil::{binary, failure, http, nested, truncated};
    use brenn_provider_kit::{Body, Call, ErrorKind};
    use serde_json::json;

    use super::*;
    use crate::testutil::{ics, multistatus, settings};

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

    fn at(method: &str, path: &str) -> InFlight<Probe> {
        let call = Call {
            step: 1,
            api: dav_call(&settings(), None, method, path, None, None, None),
        };
        InFlight::new(
            &"r".repeat(32),
            Probe,
            &json!({}),
            &call,
            serde_json::Map::new(),
            1_000,
            1_000,
        )
    }

    fn report() -> InFlight<Probe> {
        at("REPORT", "/dav/calendars/user/alice@example.com/work/")
    }

    #[test]
    fn a_dav_call_carries_its_depth_body_type_and_accept() {
        let call = dav_call(
            &settings(),
            Some("alice"),
            "PROPFIND",
            "/dav/principals/",
            Some("0"),
            Some("<x/>".to_owned()),
            None,
        );
        assert_eq!(call.method, "PROPFIND");
        assert_eq!(call.path, "/dav/principals/");
        assert_eq!(call.account.as_deref(), Some("alice"));
        assert_eq!(call.body, Some(Body::Text("<x/>".to_owned())));
        assert_eq!(
            call.headers,
            [
                ("Content-Type".to_owned(), XML_CONTENT_TYPE.to_owned()),
                ("Depth".to_owned(), "0".to_owned()),
            ]
            .into()
        );
        let call = dav_call(
            &settings(),
            None,
            "GET",
            "/dav/calendars/user/alice@example.com/work/e1.ics",
            None,
            None,
            Some("text/calendar"),
        );
        assert_eq!(call.body, None);
        assert_eq!(
            call.headers,
            [("Accept".to_owned(), "text/calendar".to_owned())].into()
        );
    }

    #[test]
    fn the_expected_success_is_the_payload() {
        let read = payload(&report(), multistatus("<x/>"), 207, None, None).unwrap();
        assert_eq!((read.status, read.text()), (207, Some("<x/>")));
        let read = payload(&at("GET", "/e.ics"), ics("BEGIN"), 200, None, None).unwrap();
        assert_eq!(
            read.header("Content-Type"),
            Some("text/calendar; charset=utf-8")
        );
        let bytes = payload(&at("GET", "/e.ics"), binary(200, &[0xff]), 200, None, None).unwrap();
        assert_eq!(bytes.body, vec![0xff]);
    }

    #[test]
    fn another_success_status_is_internal() {
        assert_eq!(
            payload(&report(), ics("x"), 207, None, None).unwrap_err(),
            failure(
                ErrorKind::Internal,
                "Fastmail answered HTTP 200 to REPORT \
                 `/dav/calendars/user/alice@example.com/work/`, where 207 was expected"
            )
        );
    }

    #[test]
    fn a_refused_app_password_is_the_operators() {
        assert_eq!(
            payload(&report(), http(401, "no"), 207, Some("calendar"), None).unwrap_err(),
            failure(
                ErrorKind::Internal,
                "Fastmail refused the app password for this account (HTTP 401): the operator \
                 must replace its key file and restart Brenn"
            )
        );
    }

    #[test]
    fn a_404_is_the_callers_only_for_a_handle_they_passed() {
        assert_eq!(
            payload(
                &report(),
                http(404, ""),
                207,
                Some("calendar at `/c/`"),
                None
            )
            .unwrap_err(),
            failure(ErrorKind::InvalidArgs, "no calendar at `/c/`")
        );
        assert_eq!(
            payload(
                &at("PROPFIND", "/dav/principals/"),
                http(404, ""),
                207,
                None,
                None
            )
            .unwrap_err(),
            failure(
                ErrorKind::Internal,
                "Fastmail answered HTTP 404 to PROPFIND `/dav/principals/`"
            )
        );
    }

    #[test]
    fn rate_limits_and_oversized_answers_say_what_to_do() {
        assert_eq!(
            payload(&report(), http(429, ""), 207, None, None)
                .unwrap_err()
                .detail,
            "Fastmail is rate-limiting this deployment (HTTP 429); try again later"
        );
        assert_eq!(
            payload(&report(), http(507, ""), 207, None, Some("narrow it"))
                .unwrap_err()
                .detail,
            "Fastmail found more than it will return for REPORT \
             `/dav/calendars/user/alice@example.com/work/` (HTTP 507); narrow it"
        );
        assert_eq!(
            payload(&at("PROPFIND", "/h/"), http(507, ""), 207, None, None)
                .unwrap_err()
                .detail,
            "Fastmail found more than it will return for PROPFIND `/h/` (HTTP 507)"
        );
    }

    #[test]
    fn another_error_names_its_precondition_and_copies_nothing_else() {
        let body = r#"<?xml version="1.0"?>
            <D:error xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
              <C:valid-filter/><D:responsedescription>secret</D:responsedescription>
            </D:error>"#;
        let refused = payload(&report(), http(403, body), 207, Some("calendar"), None).unwrap_err();
        assert_eq!(
            refused,
            failure(
                ErrorKind::Internal,
                "Fastmail answered HTTP 403 to REPORT \
                 `/dav/calendars/user/alice@example.com/work/`: precondition `valid-filter` failed"
            )
        );
        for (status, body) in [(400, "secret"), (500, "<secret/>"), (405, "")] {
            let refused = payload(&report(), http(status, body), 207, None, None).unwrap_err();
            assert_eq!(
                refused.detail,
                format!(
                    "Fastmail answered HTTP {status} to REPORT \
                     `/dav/calendars/user/alice@example.com/work/`"
                )
            );
        }
    }

    #[test]
    fn a_relay_a_cut_answer_and_a_nested_refusal_are_mapped() {
        let relay = Ok(json!({ "error": "credential_invalid" }));
        assert_eq!(
            payload(&report(), relay, 207, None, None).unwrap_err().kind,
            ErrorKind::Internal
        );
        let cut = payload(
            &report(),
            truncated(207, json!(9_000_000), "<"),
            207,
            None,
            Some("narrow the window"),
        )
        .unwrap_err();
        assert_eq!(cut.kind, ErrorKind::Internal);
        assert!(
            cut.detail
                .ends_with("(max_response_bytes); narrow the window"),
            "{}",
            cut.detail
        );
        let denied =
            payload(&report(), nested("denied", "no clause"), 207, None, None).unwrap_err();
        assert_eq!(denied.kind, ErrorKind::Denied);
        assert!(denied.detail.contains("REPORT"), "{}", denied.detail);
    }
}
