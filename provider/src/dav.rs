//! The nested calls a DAV component issues and its answers read: a WebDAV
//! (RFC 4918) or CalDAV (RFC 4791) call with the headers its method needs,
//! and an answer taken as its 2xx payload or mapped to the request's failure,
//! told apart by whether Fastmail is known to have done nothing.

use brenn_provider_kit::{
    ApiCall, Failure, InFlight, NestedError, Payload, PayloadAnswer, Tool, internal,
    read_api_payload,
};
use serde_json::Value;

use crate::failure::{RELAY_REFUSAL, StepFailure, invalid, rate_limited};
use crate::results::cut_width;
use crate::settings::{API_NAME, Settings};
use crate::steps::api_call;

/// The content type of every `PROPFIND` and `REPORT` body this crate sends.
pub const XML_CONTENT_TYPE: &str = "application/xml; charset=utf-8";

/// The content type of an iCalendar body.
pub const CALENDAR_CONTENT_TYPE: &str = "text/calendar; charset=utf-8";

/// The media type an event is read in.
pub const CALENDAR_ACCEPT: &str = "text/calendar";

/// The widest a path is quoted in a failure.
pub const QUOTED_PATH_MAX: usize = 200;

/// The widest entity tag [`etag`] takes.
pub const ETAG_MAX: usize = 256;

/// The method and path `state`'s call asked, for a failure.
pub fn asked<T: Tool>(state: &InFlight<T>) -> String {
    format!(
        "{} `{}`",
        state.target.method,
        cut_width(&state.target.path, QUOTED_PATH_MAX).0
    )
}

/// How far below its path a `PROPFIND` or `REPORT` reaches. A `PROPFIND`
/// without `Depth` means `infinity` (RFC 4918 section 9.1), so every one
/// states it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Depth {
    /// The resource at the path alone.
    Zero,
    /// The resource and its members.
    One,
}

impl Depth {
    fn header(self) -> &'static str {
        match self {
            Depth::Zero => "0",
            Depth::One => "1",
        }
    }
}

/// What must hold on the server for a `PUT` to be applied (RFC 7232
/// section 3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Precondition<'a> {
    /// `If-None-Match: *`: nothing is stored at the path.
    Absent,
    /// `If-Match: <etag>`: the resource is the one that was read.
    Unchanged(&'a str),
}

/// One DAV request: its method and what that method carries.
#[derive(Clone, Debug, PartialEq)]
pub enum DavRequest<'a> {
    /// A `PROPFIND` asking `body`.
    Propfind { depth: Depth, body: &'a str },
    /// A `REPORT` asking `body`.
    Report { depth: Depth, body: String },
    /// A `GET` answered in `accept`.
    Get { accept: &'a str },
    /// A `PUT` of `body`, applied only when `when` holds.
    Put {
        content_type: &'a str,
        body: String,
        when: Precondition<'a>,
    },
    /// A `DELETE`, applied only when the resource's entity tag is still
    /// `unchanged`.
    Delete { unchanged: &'a str },
}

/// One nested DAV call to `path` naming the request's `account`: the method
/// `request` names, with `Depth` and `Content-Type` [`XML_CONTENT_TYPE`] for
/// `PROPFIND` and `REPORT`, `Accept` for `GET`, the given `Content-Type` and
/// `If-None-Match: *` or `If-Match` for `PUT`, and `If-Match` for `DELETE`.
pub fn dav_call(
    settings: &Settings,
    account: Option<&str>,
    path: &str,
    request: DavRequest<'_>,
) -> ApiCall {
    let (method, body, headers): (&str, Option<String>, Vec<(&str, &str)>) = match &request {
        DavRequest::Propfind { depth, body } => (
            "PROPFIND",
            Some((*body).to_owned()),
            vec![
                ("Depth", depth.header()),
                ("Content-Type", XML_CONTENT_TYPE),
            ],
        ),
        DavRequest::Report { depth, body } => (
            "REPORT",
            Some(body.clone()),
            vec![
                ("Depth", depth.header()),
                ("Content-Type", XML_CONTENT_TYPE),
            ],
        ),
        DavRequest::Get { accept } => ("GET", None, vec![("Accept", accept)]),
        DavRequest::Put {
            content_type,
            body,
            when,
        } => (
            "PUT",
            Some(body.clone()),
            vec![
                ("Content-Type", content_type),
                match when {
                    Precondition::Absent => ("If-None-Match", "*"),
                    Precondition::Unchanged(etag) => ("If-Match", etag),
                },
            ],
        ),
        DavRequest::Delete { unchanged } => ("DELETE", None, vec![("If-Match", unchanged)]),
    };
    let mut call = api_call(settings, account, method, path, body);
    for (name, value) in headers {
        call.headers.insert(name.to_owned(), value.to_owned());
    }
    call
}

/// The answer's `ETag` when it is a strong entity tag: quoted, not `W/`, at
/// most [`ETAG_MAX`] bytes, with no control character. `If-Match` compares
/// strongly (RFC 7232 section 3.1), so a weak tag could never match.
pub fn etag(payload: &Payload) -> Option<&str> {
    payload.header("ETag").filter(|tag| {
        tag.len() >= 2
            && tag.len() <= ETAG_MAX
            && tag.starts_with('"')
            && tag.ends_with('"')
            && !tag.chars().any(char::is_control)
    })
}

/// What a step expects of its answer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Expect<'a> {
    /// The 2xx statuses that are this call's success.
    pub statuses: &'a [u64],
    /// What a 404 means, when the call asked for a handle the caller passed:
    /// the failure is "no {missing}".
    pub missing: Option<&'a str>,
    /// What a 412 means, when the call carried a precondition: the sentence,
    /// to which " (HTTP 412)" is appended.
    pub stale: Option<&'a str>,
    /// What the caller narrows after an unreadable answer or a 507.
    pub hint: Option<&'a str>,
}

/// The step's answer as its payload when Fastmail answered one of
/// `expect.statuses`; every other answer is the request's failure, told
/// [`StepFailure::Refused`] when Fastmail is known to have done nothing (a
/// 4xx, a relay) from [`StepFailure::Unknown`] (another 2xx, a 5xx, an answer
/// lost or unreadable). A 404 is the caller's only with `expect.missing`, a
/// 412 has its own text only with `expect.stale`; otherwise each is
/// internal, as every other status is. An error body is read only for the
/// precondition a `{DAV:}error` names, and never copied otherwise.
pub fn step_payload<T: Tool>(
    state: &InFlight<T>,
    outcome: Result<Value, NestedError>,
    expect: &Expect<'_>,
) -> Result<Payload, StepFailure> {
    match read_api_payload(outcome, &state.target, API_NAME)? {
        PayloadAnswer::Payload(payload) if expect.statuses.contains(&payload.status) => Ok(payload),
        PayloadAnswer::Payload(payload) => Err(StepFailure::Unknown(internal(format!(
            "{API_NAME} answered HTTP {} to {}, where {} was expected",
            payload.status,
            asked(state),
            expect
                .statuses
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(" or ")
        )))),
        PayloadAnswer::Relay(_) => Err(StepFailure::Refused(internal(RELAY_REFUSAL))),
        PayloadAnswer::Unreadable(why) => {
            Err(StepFailure::Unknown(why.failure(API_NAME, expect.hint)))
        }
        PayloadAnswer::Error { status, body } => Err(match (status, expect) {
            // TODO(restart-wording-after-key-reload): say "reload" once a Brenn
            // release converges key files on reload, naming that version.
            (401, _) => StepFailure::Refused(internal(format!(
                "{API_NAME} refused the app password for this account (HTTP 401): the operator \
                 must replace its key file and restart Brenn"
            ))),
            (
                404,
                Expect {
                    missing: Some(missing),
                    ..
                },
            ) => StepFailure::Refused(invalid(format!("no {missing}"))),
            (
                412,
                Expect {
                    stale: Some(stale), ..
                },
            ) => StepFailure::Refused(internal(format!("{stale} (HTTP 412)"))),
            (429, _) => StepFailure::Refused(rate_limited()),
            (507, _) => {
                let found = format!(
                    "{API_NAME} found more than it will return for {} (HTTP 507)",
                    asked(state)
                );
                StepFailure::Unknown(internal(match expect.hint {
                    Some(hint) => format!("{found}; {hint}"),
                    None => found,
                }))
            }
            _ => {
                let answered = format!("{API_NAME} answered HTTP {status} to {}", asked(state));
                let failure = internal(match fastmail_dav::error_condition(&body) {
                    Some(condition) => format!("{answered}: precondition `{condition}` failed"),
                    None => answered,
                });
                if (400..500).contains(&status) {
                    StepFailure::Refused(failure)
                } else {
                    StepFailure::Unknown(failure)
                }
            }
        }),
    }
}

/// [`step_payload`], its failure the request's whatever Fastmail did.
pub fn payload<T: Tool>(
    state: &InFlight<T>,
    outcome: Result<Value, NestedError>,
    expect: &Expect<'_>,
) -> Result<Payload, Failure> {
    step_payload(state, outcome, expect).map_err(StepFailure::failure)
}

#[cfg(test)]
mod tests {
    use brenn_provider_kit::testutil::{binary, failure, http, nested, truncated};
    use brenn_provider_kit::{Body, Call, ErrorKind};
    use serde_json::json;

    use super::*;
    use crate::testutil::{ics, ics_tagged, multistatus, settings, written};

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

    const MULTI: Expect<'static> = Expect {
        statuses: &[207],
        missing: None,
        stale: None,
        hint: None,
    };

    const GOT: Expect<'static> = Expect {
        statuses: &[200],
        missing: None,
        stale: None,
        hint: None,
    };

    fn at(method: &str, path: &str) -> InFlight<Probe> {
        let call = Call {
            step: 1,
            api: api_call(&settings(), None, method, path, None),
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

    fn put() -> InFlight<Probe> {
        at("PUT", "/dav/calendars/user/alice@example.com/work/e1.ics")
    }

    fn headers(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn each_request_carries_its_method_headers_and_body() {
        let path = "/dav/calendars/user/alice@example.com/work/e1.ics";
        let call = dav_call(
            &settings(),
            Some("alice"),
            "/dav/principals/",
            DavRequest::Propfind {
                depth: Depth::Zero,
                body: "<x/>",
            },
        );
        assert_eq!(call.method, "PROPFIND");
        assert_eq!(call.path, "/dav/principals/");
        assert_eq!(call.account.as_deref(), Some("alice"));
        assert_eq!(call.body, Some(Body::Text("<x/>".to_owned())));
        assert_eq!(
            call.headers,
            headers(&[("Content-Type", XML_CONTENT_TYPE), ("Depth", "0")])
        );
        let call = dav_call(
            &settings(),
            None,
            "/c/",
            DavRequest::Report {
                depth: Depth::One,
                body: "<r/>".to_owned(),
            },
        );
        assert_eq!(
            (call.method.as_str(), call.body),
            ("REPORT", Some(Body::Text("<r/>".to_owned())))
        );
        assert_eq!(call.account, None);
        assert_eq!(
            call.headers,
            headers(&[("Content-Type", XML_CONTENT_TYPE), ("Depth", "1")])
        );
        let call = dav_call(
            &settings(),
            None,
            path,
            DavRequest::Get {
                accept: "text/calendar",
            },
        );
        assert_eq!((call.method.as_str(), call.body), ("GET", None));
        assert_eq!(call.headers, headers(&[("Accept", "text/calendar")]));
        let call = dav_call(
            &settings(),
            None,
            path,
            DavRequest::Put {
                content_type: CALENDAR_CONTENT_TYPE,
                body: "BEGIN:VCALENDAR".to_owned(),
                when: Precondition::Absent,
            },
        );
        assert_eq!(
            (call.method.as_str(), call.body),
            ("PUT", Some(Body::Text("BEGIN:VCALENDAR".to_owned())))
        );
        assert_eq!(
            call.headers,
            headers(&[
                ("Content-Type", CALENDAR_CONTENT_TYPE),
                ("If-None-Match", "*")
            ])
        );
        let call = dav_call(
            &settings(),
            None,
            path,
            DavRequest::Put {
                content_type: CALENDAR_CONTENT_TYPE,
                body: String::new(),
                when: Precondition::Unchanged("\"t1\""),
            },
        );
        assert_eq!(
            call.headers,
            headers(&[
                ("Content-Type", CALENDAR_CONTENT_TYPE),
                ("If-Match", "\"t1\"")
            ])
        );
        let call = dav_call(
            &settings(),
            None,
            path,
            DavRequest::Delete {
                unchanged: "\"t1\"",
            },
        );
        assert_eq!((call.method.as_str(), call.body), ("DELETE", None));
        assert_eq!(call.headers, headers(&[("If-Match", "\"t1\"")]));
    }

    #[test]
    fn only_a_strong_entity_tag_is_taken() {
        let read = |etag: Option<&str>| {
            let answer = payload(&at("GET", "/e.ics"), ics_tagged("x", etag), &GOT).unwrap();
            super::etag(&answer).map(str::to_owned)
        };
        assert_eq!(read(Some("\"abc\"")).as_deref(), Some("\"abc\""));
        assert_eq!(read(Some("\"\"")).as_deref(), Some("\"\""));
        let widest = format!("\"{}\"", "a".repeat(ETAG_MAX - 2));
        assert_eq!(read(Some(&widest)), Some(widest.clone()));
        for refused in [
            "W/\"abc\"",
            "abc",
            "\"abc",
            "\"",
            "\"a\tb\"",
            &format!("\"{}\"", "a".repeat(ETAG_MAX - 1)),
        ] {
            assert_eq!(read(Some(refused)), None, "{refused}");
        }
        assert_eq!(read(None), None);
        let answer = payload(&at("GET", "/e.ics"), ics("x"), &GOT).unwrap();
        assert_eq!(etag(&answer), None);
    }

    #[test]
    fn an_expected_success_is_the_payload() {
        let read = payload(&report(), multistatus("<x/>"), &MULTI).unwrap();
        assert_eq!((read.status, read.text()), (207, Some("<x/>")));
        let read = payload(&at("GET", "/e.ics"), ics("BEGIN"), &GOT).unwrap();
        assert_eq!(read.header("Content-Type"), Some(CALENDAR_CONTENT_TYPE));
        let bytes = payload(&at("GET", "/e.ics"), binary(200, &[0xff]), &GOT).unwrap();
        assert_eq!(bytes.body, vec![0xff]);
        let writes = Expect {
            statuses: &[200, 201, 204],
            ..Expect::default()
        };
        for status in [200, 201, 204] {
            let read = step_payload(&put(), written(status, Some("\"t2\"")), &writes).unwrap();
            assert_eq!(read.status, status);
            assert!(read.body.is_empty());
        }
    }

    #[test]
    fn another_success_status_is_internal_and_unknown() {
        assert_eq!(
            step_payload(&report(), ics("x"), &MULTI).unwrap_err(),
            StepFailure::Unknown(failure(
                ErrorKind::Internal,
                "Fastmail answered HTTP 200 to REPORT \
                 `/dav/calendars/user/alice@example.com/work/`, where 207 was expected"
            ))
        );
        let writes = Expect {
            statuses: &[201, 204],
            ..Expect::default()
        };
        assert_eq!(
            step_payload(&put(), written(200, None), &writes).unwrap_err(),
            StepFailure::Unknown(failure(
                ErrorKind::Internal,
                "Fastmail answered HTTP 200 to PUT \
                 `/dav/calendars/user/alice@example.com/work/e1.ics`, where 201 or 204 was expected"
            ))
        );
    }

    #[test]
    fn a_refused_app_password_is_the_operators_and_refused() {
        let expect = Expect {
            missing: Some("calendar"),
            ..MULTI
        };
        assert_eq!(
            step_payload(&report(), http(401, "no"), &expect).unwrap_err(),
            StepFailure::Refused(failure(
                ErrorKind::Internal,
                "Fastmail refused the app password for this account (HTTP 401): the operator \
                 must replace its key file and restart Brenn"
            ))
        );
    }

    #[test]
    fn a_404_is_the_callers_only_for_a_handle_they_passed() {
        let expect = Expect {
            missing: Some("calendar at `/c/`"),
            ..MULTI
        };
        assert_eq!(
            step_payload(&report(), http(404, ""), &expect).unwrap_err(),
            StepFailure::Refused(failure(ErrorKind::InvalidArgs, "no calendar at `/c/`"))
        );
        assert_eq!(
            step_payload(&at("PROPFIND", "/dav/principals/"), http(404, ""), &MULTI).unwrap_err(),
            StepFailure::Refused(failure(
                ErrorKind::Internal,
                "Fastmail answered HTTP 404 to PROPFIND `/dav/principals/`"
            ))
        );
    }

    #[test]
    fn a_412_says_what_changed_only_for_a_call_with_a_precondition() {
        let expect = Expect {
            statuses: &[204],
            stale: Some("the event changed since it was read, so nothing was changed"),
            ..Expect::default()
        };
        assert_eq!(
            step_payload(&put(), http(412, "<x/>"), &expect).unwrap_err(),
            StepFailure::Refused(failure(
                ErrorKind::Internal,
                "the event changed since it was read, so nothing was changed (HTTP 412)"
            ))
        );
        assert_eq!(
            step_payload(&put(), http(412, ""), &GOT).unwrap_err(),
            StepFailure::Refused(failure(
                ErrorKind::Internal,
                "Fastmail answered HTTP 412 to PUT \
                 `/dav/calendars/user/alice@example.com/work/e1.ics`"
            ))
        );
    }

    #[test]
    fn rate_limits_and_oversized_answers_say_what_to_do() {
        assert_eq!(
            step_payload(&report(), http(429, ""), &MULTI).unwrap_err(),
            StepFailure::Refused(failure(
                ErrorKind::Internal,
                "Fastmail is rate-limiting this deployment (HTTP 429); try again later"
            ))
        );
        let hinted = Expect {
            hint: Some("narrow it"),
            ..MULTI
        };
        assert_eq!(
            step_payload(&report(), http(507, ""), &hinted).unwrap_err(),
            StepFailure::Unknown(failure(
                ErrorKind::Internal,
                "Fastmail found more than it will return for REPORT \
                 `/dav/calendars/user/alice@example.com/work/` (HTTP 507); narrow it"
            ))
        );
        assert_eq!(
            payload(&at("PROPFIND", "/h/"), http(507, ""), &MULTI)
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
        let expect = Expect {
            missing: Some("calendar"),
            ..MULTI
        };
        assert_eq!(
            step_payload(&report(), http(403, body), &expect).unwrap_err(),
            StepFailure::Refused(failure(
                ErrorKind::Internal,
                "Fastmail answered HTTP 403 to REPORT \
                 `/dav/calendars/user/alice@example.com/work/`: precondition `valid-filter` failed"
            ))
        );
        for (status, body, unknown) in [
            (400, "secret", false),
            (405, "", false),
            (409, "", false),
            (500, "<secret/>", true),
            (503, "", true),
        ] {
            let refused = step_payload(&report(), http(status, body), &MULTI).unwrap_err();
            assert_eq!(
                matches!(refused, StepFailure::Unknown(_)),
                unknown,
                "{status}"
            );
            assert_eq!(
                refused.failure().detail,
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
            step_payload(&report(), relay, &MULTI).unwrap_err(),
            StepFailure::Refused(failure(ErrorKind::Internal, RELAY_REFUSAL))
        );
        let hinted = Expect {
            hint: Some("narrow the window"),
            ..MULTI
        };
        let cut =
            step_payload(&report(), truncated(207, json!(9_000_000), "<"), &hinted).unwrap_err();
        assert!(matches!(cut, StepFailure::Unknown(_)), "{cut:?}");
        let cut = cut.failure();
        assert_eq!(cut.kind, ErrorKind::Internal);
        assert!(
            cut.detail
                .ends_with("(max_response_bytes); narrow the window"),
            "{}",
            cut.detail
        );
        let denied = payload(&report(), nested("denied", "no clause"), &MULTI).unwrap_err();
        assert_eq!(denied.kind, ErrorKind::Denied);
        assert!(denied.detail.contains("REPORT"), "{}", denied.detail);
    }

    #[test]
    fn payload_flattens_either_failure() {
        assert_eq!(
            payload(&report(), http(429, ""), &MULTI).unwrap_err(),
            step_payload(&report(), http(429, ""), &MULTI)
                .unwrap_err()
                .failure()
        );
        assert_eq!(
            payload(&report(), http(500, ""), &MULTI).unwrap_err(),
            step_payload(&report(), http(500, ""), &MULTI)
                .unwrap_err()
                .failure()
        );
    }
}
