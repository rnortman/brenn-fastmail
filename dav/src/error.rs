use std::fmt;

/// The longest server-supplied text an error quotes: an href, or the XML
/// parser's reason. Longer text is cut at a character boundary.
const QUOTED_MAX: usize = 200;

/// What went wrong reading a DAV answer, iCalendar text or a handle. An
/// error names a kind and, where it applies, an href, a property or a line
/// number; it never copies a body or a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The answer is not a multi-status this crate can read: not XML, a
    /// document with a DTD, or a root that is not `{DAV:}multistatus`. The
    /// reason is the parser's, or the root's local name.
    Multistatus(String),
    /// A `{DAV:}response` with no `{DAV:}href`.
    ResponseWithoutHref,
    /// An href that is not a path on the instance's host that brenn will
    /// send. `reason` says which rule it breaks.
    Href { href: String, reason: &'static str },
    /// A property the answer must carry one href in carries none or several,
    /// or is absent.
    Property {
        name: &'static str,
        problem: &'static str,
    },
    /// A response of a `calendar-query` answer with no `calendar-data`.
    NoCalendarData { href: String },
    /// iCalendar text that cannot be read: `problem` says what is wrong
    /// with the content line that begins at physical line `line`, counted
    /// from 1, or with the text as a whole.
    Ical { line: usize, problem: &'static str },
    /// A property whose value has the wrong shape for its type.
    Value {
        property: String,
        problem: &'static str,
    },
}

/// `text` cut to [`QUOTED_MAX`] bytes with each control character escaped,
/// so a rendering stays on one line and short whatever the server sent.
struct Quoted<'a>(&'a str);

impl fmt::Display for Quoted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = self.0.floor_char_boundary(QUOTED_MAX);
        for c in self.0[..at].chars() {
            if c.is_control() {
                write!(f, "{}", c.escape_default())?;
            } else {
                write!(f, "{c}")?;
            }
        }
        if at < self.0.len() {
            write!(f, "…")?;
        }
        Ok(())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Multistatus(reason) => write!(
                f,
                "the answer is not a multi-status this component can read: {}",
                Quoted(reason)
            ),
            Error::ResponseWithoutHref => {
                write!(f, "the multi-status answer has a response with no href")
            }
            Error::Href { href, reason } => write!(f, "href `{}` {reason}", Quoted(href)),
            Error::Property { name, problem } => write!(f, "the answer's {name} {problem}"),
            Error::NoCalendarData { href } => {
                write!(f, "the answer for `{}` has no calendar-data", Quoted(href))
            }
            Error::Ical { line, problem } => {
                write!(f, "the calendar data's line {line} {problem}")
            }
            Error::Value { property, problem } => {
                write!(f, "the {} value {problem}", Quoted(property))
            }
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_text_is_cut_and_kept_on_one_line() {
        let long = format!("/{}", "é".repeat(150));
        let shown = Error::Href {
            href: long,
            reason: "is too long",
        }
        .to_string();
        assert!(shown.starts_with("href `/éé"), "{shown}");
        assert!(shown.ends_with("…` is too long"), "{shown}");
        assert!(shown.len() < 230, "{}", shown.len());
        assert_eq!(
            Error::Multistatus("a\nb".to_owned()).to_string(),
            "the answer is not a multi-status this component can read: a\\nb"
        );
        assert_eq!(
            Error::Ical {
                line: 3,
                problem: "has an unclosed quote"
            }
            .to_string(),
            "the calendar data's line 3 has an unclosed quote"
        );
        assert_eq!(
            Error::Value {
                property: "DTSTART".to_owned(),
                problem: "is neither a date nor a date-time"
            }
            .to_string(),
            "the DTSTART value is neither a date nor a date-time"
        );
    }
}
