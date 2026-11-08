use serde_json::Value;

use crate::error::{Error, RequestError};
use crate::request::CallId;

/// A response envelope's method responses, as `(name, args, call id)`
/// (RFC 8620 section 3.4).
#[derive(Debug, Clone)]
pub struct Response {
    responses: Vec<(String, Value, String)>,
}

impl Response {
    /// Reads `methodResponses`; any other shape is refused.
    pub fn parse(mut body: Value) -> Result<Response, Error> {
        let Some(Value::Array(list)) = body
            .as_object_mut()
            .and_then(|envelope| envelope.remove("methodResponses"))
        else {
            return Err(Error::Shape("no methodResponses array".into()));
        };
        let mut responses = Vec::with_capacity(list.len());
        for entry in list {
            let Value::Array(triple) = entry else {
                return Err(Error::Shape("a method response is not a triple".into()));
            };
            let Ok([name, args, id]) = <[Value; 3]>::try_from(triple) else {
                return Err(Error::Shape("a method response is not a triple".into()));
            };
            let (Value::String(name), Value::String(id)) = (name, id) else {
                return Err(Error::Shape(
                    "a method response's name or call id is not a string".into(),
                ));
            };
            if !args.is_object() {
                return Err(Error::Shape(format!(
                    "the arguments of {name} ({id}) are not an object"
                )));
            }
            responses.push((name, args, id));
        }
        Ok(Response { responses })
    }

    /// The arguments of the response to call `id`, which must answer the
    /// method `id` names or be an `error` in its place. Found by call id,
    /// never by position. The first response carrying the id is the call's
    /// own: an implicit call's response follows it under the same id
    /// (RFC 8620 section 5.4) and is not read here.
    // TODO(correctness-implicit-method-responses-unreachable): a reader for
    // the implicit call's response, for the first caller that triggers one.
    pub fn result(&self, id: &CallId) -> Result<&Value, Error> {
        let name = id.name();
        let (found, args, _) = self
            .responses
            .iter()
            .find(|(_, _, call)| call == id.as_str())
            .ok_or_else(|| Error::Shape(format!("no response to {name} ({})", id.as_str())))?;
        if found == "error" {
            let kind = args.get("type").and_then(Value::as_str).ok_or_else(|| {
                Error::Shape(format!(
                    "error response to {name} ({}) has no string type",
                    id.as_str()
                ))
            })?;
            return Err(Error::Method {
                name: name.to_owned(),
                kind: kind.to_owned(),
                description: args
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
        if found != name {
            return Err(Error::Shape(format!(
                "call {} answered as {found}, not {name}",
                id.as_str()
            )));
        }
        Ok(args)
    }
}

/// Reads a request-level problem object (RFC 8620 section 3.6.1), the body of
/// a 4xx answer. `None` when the body is not one: no string `type`, or a
/// `status` or `detail` present with the wrong type.
pub fn request_error(body: &Value) -> Option<RequestError> {
    let kind = body.get("type")?.as_str()?.to_owned();
    let status = match body.get("status") {
        None => None,
        Some(status) => Some(status.as_u64()?),
    };
    let detail = match body.get("detail") {
        None => None,
        Some(detail) => Some(detail.as_str()?.to_owned()),
    };
    Some(RequestError {
        kind,
        status,
        detail,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::request::Request;

    fn two_calls() -> (CallId, CallId) {
        let mut request = Request::mail();
        let query = request.push("Email/query", json!({}));
        let get = request.push("Email/get", json!({}));
        (query, get)
    }

    #[test]
    fn results_are_found_by_call_id_whatever_their_order() {
        let (query, get) = two_calls();
        let response = Response::parse(json!({
            "methodResponses": [
                ["Email/get", {"list": []}, "c1"],
                ["Email/query", {"ids": ["e1"]}, "c0"]
            ],
            "sessionState": "s1"
        }))
        .unwrap();
        assert_eq!(response.result(&query).unwrap()["ids"], json!(["e1"]));
        assert_eq!(response.result(&get).unwrap()["list"], json!([]));
    }

    #[test]
    fn a_method_error_names_its_type_and_description() {
        let (query, _) = two_calls();
        let response = Response::parse(json!({
            "methodResponses": [
                ["error", {"type": "invalidArguments", "description": "bad date"}, "c0"]
            ]
        }))
        .unwrap();
        assert_eq!(
            response.result(&query),
            Err(Error::Method {
                name: "Email/query".into(),
                kind: "invalidArguments".into(),
                description: "bad date".into(),
            })
        );
    }

    #[test]
    fn a_method_error_without_a_string_type_is_malformed() {
        let (query, _) = two_calls();
        for args in [json!({"description": "bad date"}), json!({"type": 7})] {
            let response = Response::parse(json!({
                "methodResponses": [["error", args, "c0"]]
            }))
            .unwrap();
            assert!(
                matches!(response.result(&query), Err(Error::Shape(_))),
                "{args}"
            );
        }
    }

    #[test]
    fn a_method_error_without_a_description_has_an_empty_one() {
        let (query, _) = two_calls();
        let response = Response::parse(json!({
            "methodResponses": [["error", {"type": "serverFail"}, "c0"]]
        }))
        .unwrap();
        assert_eq!(
            response.result(&query),
            Err(Error::Method {
                name: "Email/query".into(),
                kind: "serverFail".into(),
                description: String::new(),
            })
        );
    }

    #[test]
    fn a_missing_or_mismatched_response_is_malformed() {
        let (query, get) = two_calls();
        let response = Response::parse(json!({
            "methodResponses": [["Mailbox/get", {"list": []}, "c0"]]
        }))
        .unwrap();
        assert!(matches!(response.result(&query), Err(Error::Shape(_))));
        assert!(matches!(response.result(&get), Err(Error::Shape(_))));
    }

    #[test]
    fn the_first_response_carrying_the_id_is_the_calls_own() {
        let mut request = Request::mail();
        let copy = request.push("Email/copy", json!({}));
        let implicit = json!(["Email/set", {"notDestroyed": {"m1": {}}}, "c0"]);
        let answered = Response::parse(json!({
            "methodResponses": [["Email/copy", {"created": {}}, "c0"], implicit.clone()]
        }))
        .unwrap();
        assert_eq!(answered.result(&copy).unwrap(), &json!({"created": {}}));
        let failed = Response::parse(json!({
            "methodResponses": [["error", {"type": "serverFail"}, "c0"], implicit.clone()]
        }))
        .unwrap();
        assert_eq!(
            failed.result(&copy),
            Err(Error::Method {
                name: "Email/copy".into(),
                kind: "serverFail".into(),
                description: String::new(),
            })
        );
        let misordered = Response::parse(json!({
            "methodResponses": [implicit, ["Email/copy", {"created": {}}, "c0"]]
        }))
        .unwrap();
        assert_eq!(
            misordered.result(&copy),
            Err(Error::Shape(
                "call c0 answered as Email/set, not Email/copy".into()
            ))
        );
    }

    #[test]
    fn anything_but_a_list_of_triples_is_refused() {
        for body in [
            json!({}),
            json!({"methodResponses": {}}),
            json!({"methodResponses": [["Email/get", {}]]}),
            json!({"methodResponses": [[1, {}, "c0"]]}),
            json!({"methodResponses": [["Email/get", [], "c0"]]}),
        ] {
            assert!(
                matches!(Response::parse(body.clone()), Err(Error::Shape(_))),
                "{body}"
            );
        }
    }

    #[test]
    fn a_problem_object_is_read_and_anything_else_is_not() {
        assert_eq!(
            request_error(&json!({
                "type": "urn:ietf:params:jmap:error:limit",
                "status": 400,
                "detail": "too many calls",
                "limit": "maxCallsInRequest"
            })),
            Some(RequestError {
                kind: "urn:ietf:params:jmap:error:limit".into(),
                status: Some(400),
                detail: Some("too many calls".into()),
            })
        );
        assert_eq!(
            request_error(&json!({"type": "urn:ietf:params:jmap:error:notJSON"})),
            Some(RequestError {
                kind: "urn:ietf:params:jmap:error:notJSON".into(),
                status: None,
                detail: None,
            })
        );
        for body in [
            json!({"error": "nope"}),
            json!("text"),
            json!({"type": 7}),
            json!({"type": "urn:ietf:params:jmap:error:limit", "status": "400"}),
            json!({"type": "urn:ietf:params:jmap:error:limit", "detail": ["too many"]}),
        ] {
            assert_eq!(request_error(&body), None, "{body}");
        }
    }
}
