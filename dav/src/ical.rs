//! iCalendar (RFC 5545) read into its components and properties: the
//! content-line grammar, parameter quoting and caret encoding (RFC 6868),
//! TEXT unescaping and the shape of a date or date-time. No recurrence is
//! expanded and no time is converted between zones.

use std::fmt;

use crate::error::Error;

/// A component: `VCALENDAR`, `VEVENT`, `VALARM` and the like, with its
/// properties and the components nested in it, in the order written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    /// The name, upper-cased.
    pub name: String,
    pub properties: Vec<Property>,
    pub components: Vec<Component>,
}

impl Component {
    /// The first property named `name` (upper-case).
    pub fn property(&self, name: &str) -> Option<&Property> {
        self.properties.iter().find(|p| p.name == name)
    }
}

/// One content line: `NAME;PARAM=value,value:value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// The name, upper-cased.
    pub name: String,
    /// Each parameter's name, upper-cased, and its values, unquoted and
    /// caret-decoded.
    pub params: Vec<(String, Vec<String>)>,
    /// The value as written, escapes and all.
    pub value: String,
}

impl Property {
    /// The first value of parameter `name` (upper-case).
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(param, _)| param == name)
            .and_then(|(_, values)| values.first())
            .map(String::as_str)
    }

    /// The value read as TEXT (RFC 5545 section 3.3.11): `\\`, `\;`, `\,`
    /// and `\n` or `\N` unescaped. A backslash before anything else is kept
    /// as written.
    pub fn text(&self) -> String {
        let mut out = String::with_capacity(self.value.len());
        let mut chars = self.value.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some(e @ ('\\' | ';' | ',')) => out.push(e),
                Some('n' | 'N') => out.push('\n'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        }
        out
    }
}

/// A `DTSTART`, `DTEND` or `RECURRENCE-ID` value as written: a date, or a
/// date-time that is UTC, in a named zone, or floating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum When {
    /// `YYYYMMDD`.
    Date(String),
    /// `YYYYMMDDTHHMMSS`, with `utc` when it ended in `Z` and `time_zone`
    /// when the property carried `TZID`.
    DateTime {
        local: String,
        utc: bool,
        time_zone: Option<String>,
    },
}

impl When {
    /// `property`'s value read as a date or a date-time. Any other shape, a
    /// `VALUE` parameter that names another type, or `TZID` on a UTC time
    /// (RFC 5545 section 3.2.19) is an error naming the property.
    pub fn of(property: &Property) -> Result<When, Error> {
        let malformed = |problem| Error::Value {
            property: property.name.clone(),
            problem,
        };
        let value = property.value.as_str();
        let declared = property.param("VALUE").map(str::to_ascii_uppercase);
        let digits = |text: &str| text.bytes().all(|b| b.is_ascii_digit());
        let when = match value.as_bytes() {
            v if v.len() == 8 && digits(value) => {
                if declared.as_deref().is_some_and(|d| d != "DATE") {
                    return Err(malformed("is a date under a VALUE that names another type"));
                }
                When::Date(value.to_owned())
            }
            v if (v.len() == 15 || (v.len() == 16 && v[15] == b'Z'))
                && v[8] == b'T'
                && digits(&value[..8])
                && digits(&value[9..15]) =>
            {
                if declared.as_deref().is_some_and(|d| d != "DATE-TIME") {
                    return Err(malformed(
                        "is a date-time under a VALUE that names another type",
                    ));
                }
                let utc = v.len() == 16;
                let time_zone = property.param("TZID").map(str::to_owned);
                if utc && time_zone.is_some() {
                    return Err(malformed("is a UTC time that also names a TZID"));
                }
                When::DateTime {
                    local: value[..15].to_owned(),
                    utc,
                    time_zone,
                }
            }
            _ => return Err(malformed("is neither a date nor a date-time")),
        };
        Ok(when)
    }
}

/// The date as `2026-11-02`, the date-time as `2026-11-02T14:00:00`, with
/// `Z` when UTC: punctuation inserted, no field checked, nothing converted.
impl fmt::Display for When {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let date = |f: &mut fmt::Formatter<'_>, d: &str| {
            write!(f, "{}-{}-{}", &d[..4], &d[4..6], &d[6..8])
        };
        match self {
            When::Date(d) => date(f, d),
            When::DateTime { local, utc, .. } => {
                date(f, local)?;
                write!(
                    f,
                    "T{}:{}:{}",
                    &local[9..11],
                    &local[11..13],
                    &local[13..15]
                )?;
                if *utc {
                    write!(f, "Z")?;
                }
                Ok(())
            }
        }
    }
}

/// `bytes` read as one `VCALENDAR`. Lines end in CRLF or a bare LF; a line
/// beginning with a space or tab continues the one before (RFC 5545 section
/// 3.1), joined on bytes so a fold may fall inside a character, and the
/// joined line must then be UTF-8. An error names the line, counted from 1
/// at the first physical line of the unfolded one.
pub fn parse(bytes: &[u8]) -> Result<Component, Error> {
    let mut stack: Vec<Component> = Vec::new();
    let mut root: Option<Component> = None;
    let mut last = 1;
    for (line, content) in unfold(bytes) {
        last = line;
        let error = |problem| Error::Ical { line, problem };
        let content = String::from_utf8(content).map_err(|_| error("is not UTF-8"))?;
        if root.is_some() {
            return Err(error("follows the end of the calendar"));
        }
        let property = content_line(&content).map_err(error)?;
        match property.name.as_str() {
            "BEGIN" => {
                let name = property.value.to_ascii_uppercase();
                if stack.is_empty() && name != "VCALENDAR" {
                    return Err(error("begins something other than a VCALENDAR"));
                }
                stack.push(Component {
                    name,
                    properties: Vec::new(),
                    components: Vec::new(),
                });
            }
            "END" => {
                let Some(done) = stack.pop() else {
                    return Err(error("ends a component that was never begun"));
                };
                if done.name != property.value.to_ascii_uppercase() {
                    return Err(error("ends a component other than the one open"));
                }
                match stack.last_mut() {
                    Some(parent) => parent.components.push(done),
                    None => root = Some(done),
                }
            }
            _ => match stack.last_mut() {
                Some(open) => open.properties.push(property),
                None => return Err(error("is outside any component")),
            },
        }
    }
    match (root, stack.is_empty()) {
        (Some(root), _) => Ok(root),
        (None, true) => Err(Error::Ical {
            line: 1,
            problem: "the calendar data holds no VCALENDAR",
        }),
        (None, false) => Err(Error::Ical {
            line: last,
            problem: "the calendar data ends inside a component",
        }),
    }
}

/// The logical lines of `bytes`, each with the number of its first physical
/// line, folds removed. A final line break leaves no empty line.
fn unfold(bytes: &[u8]) -> Vec<(usize, Vec<u8>)> {
    let mut physical: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
    if physical.last().is_some_and(|last| last.is_empty()) {
        physical.pop();
    }
    let mut lines: Vec<(usize, Vec<u8>)> = Vec::new();
    for (at, raw) in physical.into_iter().enumerate() {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        match (raw.first(), lines.last_mut()) {
            (Some(b' ' | b'\t'), Some((_, open))) => open.extend_from_slice(&raw[1..]),
            _ => lines.push((at + 1, raw.to_vec())),
        }
    }
    lines
}

/// Whether `c` may appear in a property or parameter name: an IANA token or
/// an `X-` name (RFC 5545 section 3.1).
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

/// One unfolded content line read into a property.
fn content_line(line: &str) -> Result<Property, &'static str> {
    let end = line
        .find([';', ':'])
        .ok_or("has no `:` between a name and a value")?;
    let name = &line[..end];
    if name.is_empty() || !name.chars().all(is_name_char) {
        return Err("has a property name that is empty or not a name");
    }
    let mut rest = &line[end..];
    let mut params = Vec::new();
    while let Some(after) = rest.strip_prefix(';') {
        let eq = after.find('=').ok_or("has a parameter with no `=`")?;
        let param = &after[..eq];
        if param.is_empty() || !param.chars().all(is_name_char) {
            return Err("has a parameter name that is empty or not a name");
        }
        let mut values = Vec::new();
        let mut tail = &after[eq + 1..];
        loop {
            let (value, next) = match tail.strip_prefix('"') {
                Some(quoted) => {
                    let close = quoted.find('"').ok_or("has an unclosed quote")?;
                    (&quoted[..close], &quoted[close + 1..])
                }
                None => {
                    let stop = tail
                        .find([',', ';', ':', '"'])
                        .ok_or("has no `:` between a name and a value")?;
                    (&tail[..stop], &tail[stop..])
                }
            };
            values.push(caret_decode(value));
            match next.strip_prefix(',') {
                Some(more) => tail = more,
                None => {
                    tail = next;
                    break;
                }
            }
        }
        if !tail.starts_with([';', ':']) {
            return Err("has a parameter value followed by something other than `;` or `:`");
        }
        params.push((param.to_ascii_uppercase(), values));
        rest = tail;
    }
    let value = rest
        .strip_prefix(':')
        .ok_or("has no `:` between a name and a value")?;
    Ok(Property {
        name: name.to_ascii_uppercase(),
        params,
        value: value.to_owned(),
    })
}

/// A parameter value with RFC 6868's caret encoding decoded: `^n` a line
/// break, `^'` a double quote, `^^` a caret. A caret before anything else
/// is kept as written.
fn caret_decode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '^' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('n') => out.push('\n'),
            Some('\'') => out.push('"'),
            Some('^') => out.push('^'),
            _ => {
                out.push('^');
                continue;
            }
        }
        chars.next();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn property(line: &str) -> Property {
        content_line(line).unwrap()
    }

    fn calendar(lines: &[&str]) -> String {
        lines.iter().map(|line| format!("{line}\r\n")).collect()
    }

    fn line_error(text: &str) -> (usize, &'static str) {
        match parse(text.as_bytes()).unwrap_err() {
            Error::Ical { line, problem } => (line, problem),
            other => panic!("not a line error: {other:?}"),
        }
    }

    #[test]
    fn components_nest_in_the_order_written() {
        let text = calendar(&[
            "BEGIN:VCALENDAR",
            "VERSION:2.0",
            "BEGIN:VEVENT",
            "UID:e1",
            "BEGIN:VALARM",
            "ACTION:DISPLAY",
            "END:VALARM",
            "END:VEVENT",
            "begin:vevent",
            "uid:e2",
            "end:VEVENT",
            "END:VCALENDAR",
        ]);
        let root = parse(text.as_bytes()).unwrap();
        assert_eq!(root.name, "VCALENDAR");
        assert_eq!(root.property("VERSION").unwrap().value, "2.0");
        assert_eq!(root.components.len(), 2);
        let first = &root.components[0];
        assert_eq!(first.property("UID").unwrap().value, "e1");
        assert_eq!(first.components[0].name, "VALARM");
        assert_eq!(root.components[1].name, "VEVENT");
        assert_eq!(root.components[1].property("UID").unwrap().value, "e2");
    }

    #[test]
    fn folded_lines_are_joined_on_bytes_and_bare_lf_is_a_line_end() {
        let mut bytes = b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:Caf".to_vec();
        let e_acute = "é".as_bytes();
        bytes.push(e_acute[0]);
        bytes.extend_from_slice(b"\r\n ");
        bytes.push(e_acute[1]);
        bytes.extend_from_slice(b" au\n\tlait\nEND:VEVENT\nEND:VCALENDAR\n");
        let root = parse(&bytes).unwrap();
        let summary = root.components[0].property("SUMMARY").unwrap();
        assert_eq!(summary.value, "Café aulait");
    }

    #[test]
    fn every_text_escape_is_read() {
        let p = property(r"DESCRIPTION:a\\b\;c\,d\ne\Nf\:g\");
        assert_eq!(p.text(), "a\\b;c,d\ne\nf\\:g\\");
    }

    #[test]
    fn parameters_are_unquoted_caret_decoded_and_split() {
        let p = property(
            r#"attendee;cn="Alice ^'A^' ^^ Example; Inc:";Partstat=ACCEPTED;member="a:b","c";x-y=^n^x:mailto:alice@example.com"#,
        );
        assert_eq!(p.name, "ATTENDEE");
        assert_eq!(p.value, "mailto:alice@example.com");
        assert_eq!(p.param("CN"), Some("Alice \"A\" ^ Example; Inc:"));
        assert_eq!(p.param("PARTSTAT"), Some("ACCEPTED"));
        assert_eq!(
            p.params[2],
            ("MEMBER".to_owned(), vec!["a:b".to_owned(), "c".to_owned()])
        );
        assert_eq!(p.param("X-Y"), Some("\n^x"));
        assert_eq!(p.param("ROLE"), None);
        assert_eq!(property("X-EMPTY:").value, "");
        assert_eq!(property("A;B=:v").param("B"), Some(""));
    }

    #[test]
    fn a_malformed_content_line_is_refused() {
        for (line, problem) in [
            ("NOVALUE", "has no `:` between a name and a value"),
            (":v", "has a property name that is empty or not a name"),
            ("A B:v", "has a property name that is empty or not a name"),
            ("A;B:v", "has a parameter with no `=`"),
            ("A;=x:v", "has a parameter name that is empty or not a name"),
            (r#"A;B="x:v"#, "has an unclosed quote"),
            (
                r#"A;B="x"y:v"#,
                "has a parameter value followed by something other than `;` or `:`",
            ),
            (
                r#"A;B=x"y":v"#,
                "has a parameter value followed by something other than `;` or `:`",
            ),
            ("A;B=x", "has no `:` between a name and a value"),
        ] {
            assert_eq!(content_line(line), Err(problem), "{line}");
        }
    }

    #[test]
    fn each_structural_error_names_its_line() {
        let cases: [(&[&str], usize, &str); 7] = [
            (
                &["BEGIN:VCALENDAR", "BEGIN:VEVENT", "END:VCALENDAR"],
                3,
                "ends a component other than the one open",
            ),
            (
                &["BEGIN:VCALENDAR", "BEGIN:VEVENT", "END:VEVENT"],
                3,
                "the calendar data ends inside a component",
            ),
            (
                &["END:VCALENDAR"],
                1,
                "ends a component that was never begun",
            ),
            (&["UID:x"], 1, "is outside any component"),
            (
                &["BEGIN:VEVENT", "END:VEVENT"],
                1,
                "begins something other than a VCALENDAR",
            ),
            (
                &["BEGIN:VCALENDAR", "END:VCALENDAR", "BEGIN:VCALENDAR"],
                3,
                "follows the end of the calendar",
            ),
            (
                &["BEGIN:VCALENDAR", "", "END:VCALENDAR"],
                2,
                "has no `:` between a name and a value",
            ),
        ];
        for (lines, line, problem) in cases {
            assert_eq!(line_error(&calendar(lines)), (line, problem), "{lines:?}");
        }
        assert_eq!(line_error(""), (1, "the calendar data holds no VCALENDAR"));
        assert_eq!(
            line_error("BEGIN:VCALENDAR\r\nDESCRIPTION:a\r\n b\r\nNO COLON\r\nEND:VCALENDAR"),
            (4, "has no `:` between a name and a value")
        );
        let mut bytes = b"BEGIN:VCALENDAR\r\nSUMMARY:".to_vec();
        bytes.extend_from_slice(&[0xff, b'\r', b'\n']);
        assert_eq!(
            parse(&bytes).unwrap_err(),
            Error::Ical {
                line: 2,
                problem: "is not UTF-8"
            }
        );
    }

    fn when(line: &str) -> Result<When, Error> {
        When::of(&property(line))
    }

    #[test]
    fn dates_and_date_times_are_read_as_written() {
        let date = when("DTSTART;VALUE=DATE:20261102").unwrap();
        assert_eq!(date, When::Date("20261102".to_owned()));
        assert_eq!(date.to_string(), "2026-11-02");
        let utc = when("DTSTART:20261102T140000Z").unwrap();
        assert_eq!(
            utc,
            When::DateTime {
                local: "20261102T140000".to_owned(),
                utc: true,
                time_zone: None
            }
        );
        assert_eq!(utc.to_string(), "2026-11-02T14:00:00Z");
        let floating = when("DTEND;VALUE=DATE-TIME:20261102T143000").unwrap();
        assert_eq!(floating.to_string(), "2026-11-02T14:30:00");
        let zoned = when("RECURRENCE-ID;TZID=Europe/Berlin:20261102T090000").unwrap();
        assert_eq!(
            zoned,
            When::DateTime {
                local: "20261102T090000".to_owned(),
                utc: false,
                time_zone: Some("Europe/Berlin".to_owned())
            }
        );
        assert_eq!(zoned.to_string(), "2026-11-02T09:00:00");
        assert_eq!(
            when("DTSTART:20261340T250000Z").unwrap().to_string(),
            "2026-13-40T25:00:00Z"
        );
    }

    #[test]
    fn a_malformed_or_contradicted_time_is_refused_by_property() {
        for (line, problem) in [
            (
                "DTSTART;VALUE=DATE:20261102T140000Z",
                "is a date-time under a VALUE that names another type",
            ),
            (
                "DTSTART;VALUE=PERIOD:20261102",
                "is a date under a VALUE that names another type",
            ),
            (
                "DTSTART;TZID=Europe/Berlin:20261102T140000Z",
                "is a UTC time that also names a TZID",
            ),
            ("DTSTART:2026-11-02", "is neither a date nor a date-time"),
            ("DTSTART:20261102T1400", "is neither a date nor a date-time"),
            (
                "DTSTART:20261102X140000",
                "is neither a date nor a date-time",
            ),
            (
                "DTSTART:20261102T140000z",
                "is neither a date nor a date-time",
            ),
            (
                "DTSTART:20261102,20261103",
                "is neither a date nor a date-time",
            ),
            ("DTSTART:", "is neither a date nor a date-time"),
            (
                "DTSTART:２０２６１１０２",
                "is neither a date nor a date-time",
            ),
        ] {
            assert_eq!(
                when(line),
                Err(Error::Value {
                    property: "DTSTART".to_owned(),
                    problem
                }),
                "{line}"
            );
        }
    }
}
