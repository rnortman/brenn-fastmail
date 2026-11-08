use serde_json::{Value, json};

use crate::session::{CORE, MAIL};

/// The id a method call carries, which its response echoes (RFC 8620
/// section 3.2), and the name of the method it calls. Assigned by
/// `Request::push`; responses are found by the id and checked against the
/// name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallId {
    id: String,
    name: &'static str,
}

impl CallId {
    /// The id as it appears on the wire.
    pub fn as_str(&self) -> &str {
        &self.id
    }

    /// The method the call names.
    pub fn name(&self) -> &'static str {
        self.name
    }
}

#[derive(Debug, Clone)]
struct MethodCall {
    args: Value,
    id: CallId,
}

/// One request envelope: the capabilities it uses and its method calls, in
/// order (RFC 8620 section 3.3).
#[derive(Debug, Clone)]
pub struct Request {
    using: Vec<&'static str>,
    calls: Vec<MethodCall>,
}

impl Request {
    /// An envelope using core and mail.
    pub fn mail() -> Request {
        Request {
            using: vec![CORE, MAIL],
            calls: Vec::new(),
        }
    }

    /// Appends a call and returns its id: `c0`, `c1`, … in push order.
    pub fn push(&mut self, name: &'static str, args: Value) -> CallId {
        let id = CallId {
            id: format!("c{}", self.calls.len()),
            name,
        };
        self.calls.push(MethodCall {
            args,
            id: id.clone(),
        });
        id
    }

    /// The envelope as the body of the API POST.
    pub fn to_body(&self) -> String {
        let calls: Vec<Value> = self
            .calls
            .iter()
            .map(|call| json!([call.id.name, call.args, call.id.id]))
            .collect();
        json!({"using": self.using, "methodCalls": calls}).to_string()
    }
}

/// A result reference (RFC 8620 section 3.7): the value of a `#`-prefixed
/// argument that takes `path` from the response of call `of`.
pub fn result_ref(of: &CallId, path: &str) -> Value {
    json!({"resultOf": of.id, "name": of.name, "path": path})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_envelope_names_core_and_mail() {
        let body: Value = serde_json::from_str(&Request::mail().to_body()).unwrap();
        assert_eq!(
            body,
            json!({
                "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
                "methodCalls": []
            })
        );
    }

    #[test]
    fn calls_carry_sequential_ids_and_references_name_them() {
        let mut request = Request::mail();
        let query = request.push("Email/query", json!({"accountId": "a1"}));
        let get = request.push(
            "Email/get",
            json!({"accountId": "a1", "#ids": result_ref(&query, "/ids")}),
        );
        assert_eq!((query.as_str(), query.name()), ("c0", "Email/query"));
        assert_eq!((get.as_str(), get.name()), ("c1", "Email/get"));
        let body: Value = serde_json::from_str(&request.to_body()).unwrap();
        assert_eq!(
            body["methodCalls"],
            json!([
                ["Email/query", {"accountId": "a1"}, "c0"],
                ["Email/get", {
                    "accountId": "a1",
                    "#ids": {"resultOf": "c0", "name": "Email/query", "path": "/ids"}
                }, "c1"]
            ])
        );
    }
}
