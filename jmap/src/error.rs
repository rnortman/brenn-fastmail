use std::fmt;

/// What went wrong reading a JMAP answer. Each variant carries Fastmail's own
/// words where there are any; deciding what the caller is told is not this
/// crate's business.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The session object lacks something every call needs.
    Session(String),
    /// A request-level problem object (RFC 8620 section 3.6.1).
    Request(RequestError),
    /// A method-level `error` response (RFC 8620 section 3.6.2).
    Method {
        name: String,
        kind: String,
        description: String,
    },
    /// The answer is not shaped as JMAP says it must be.
    Shape(String),
}

/// A request-level problem object: `type`, and the optional `status` and
/// `detail` RFC 7807 gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestError {
    pub kind: String,
    pub status: Option<u64>,
    pub detail: Option<String>,
}

/// A string from the answer, written with each control character escaped
/// (`\n`, `\u{1b}`), so a rendering stays on one line whatever the server
/// sent.
pub(crate) struct OneLine<'a>(pub(crate) &'a str);

impl fmt::Display for OneLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for c in self.0.chars() {
            if c.is_control() {
                write!(f, "{}", c.escape_default())?;
            } else {
                write!(f, "{c}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Session(what) => write!(f, "JMAP session: {}", OneLine(what)),
            Error::Request(e) => write!(f, "{e}"),
            Error::Method {
                name,
                kind,
                description,
            } => {
                write!(f, "{} failed: {}", OneLine(name), OneLine(kind))?;
                if !description.is_empty() {
                    write!(f, ": {}", OneLine(description))?;
                }
                Ok(())
            }
            Error::Shape(what) => write!(f, "malformed JMAP response: {}", OneLine(what)),
        }
    }
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JMAP request refused: {}", OneLine(&self.kind))?;
        if let Some(status) = self.status {
            write!(f, " (HTTP {status})")?;
        }
        if let Some(detail) = &self.detail {
            write!(f, ": {}", OneLine(detail))?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_variant_renders_in_fastmails_words() {
        let method = Error::Method {
            name: "Email/query".into(),
            kind: "invalidArguments".into(),
            description: "bad date".into(),
        };
        assert_eq!(
            method.to_string(),
            "Email/query failed: invalidArguments: bad date"
        );
        let bare = Error::Method {
            name: "Email/get".into(),
            kind: "serverFail".into(),
            description: String::new(),
        };
        assert_eq!(bare.to_string(), "Email/get failed: serverFail");
        let request = Error::Request(RequestError {
            kind: "urn:ietf:params:jmap:error:limit".into(),
            status: Some(400),
            detail: Some("too many calls".into()),
        });
        assert_eq!(
            request.to_string(),
            "JMAP request refused: urn:ietf:params:jmap:error:limit (HTTP 400): too many calls"
        );
    }

    #[test]
    fn control_characters_from_the_server_are_escaped() {
        let injected = "bad\nINJECTED\r\u{1b}[31m line";
        let escaped = r"bad\nINJECTED\r\u{1b}[31m line";
        let cases = [
            (
                Error::Session(injected.into()),
                format!("JMAP session: {escaped}"),
            ),
            (
                Error::Method {
                    name: "Email/query".into(),
                    kind: injected.into(),
                    description: injected.into(),
                },
                format!("Email/query failed: {escaped}: {escaped}"),
            ),
            (
                Error::Request(RequestError {
                    kind: injected.into(),
                    status: Some(400),
                    detail: Some(injected.into()),
                }),
                format!("JMAP request refused: {escaped} (HTTP 400): {escaped}"),
            ),
            (
                Error::Shape(injected.into()),
                format!("malformed JMAP response: {escaped}"),
            ),
        ];
        for (error, rendered) in cases {
            let text = error.to_string();
            assert!(!text.chars().any(char::is_control), "{text}");
            assert_eq!(text, rendered);
        }
    }
}
