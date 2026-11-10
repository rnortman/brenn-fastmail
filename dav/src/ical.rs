//! iCalendar (RFC 5545) read into its components and properties, and written
//! back: the content-line grammar, parameter quoting and caret encoding (RFC
//! 6868), TEXT escaping and unescaping, line folding and the shape of a date
//! or date-time. A property that was read and not changed is written as the
//! line it was read from. No recurrence is expanded and no time is converted
//! between zones.

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
    /// The unfolded content line the property was read from; `None` for a
    /// property built here.
    written: Option<String>,
}

impl Property {
    /// A property built from its parts, to be serialized by [`write`]. The
    /// names are upper-cased. A name or parameter name that is empty or not
    /// a name, a control character in `value`, or a control character other
    /// than a line break in a parameter value is an error naming the
    /// property. A TEXT value is escaped by the caller ([`escape_text`]).
    pub fn new(
        name: &str,
        params: Vec<(String, Vec<String>)>,
        value: String,
    ) -> Result<Property, Error> {
        let refuse = |problem| {
            Err(Error::Value {
                property: name.to_owned(),
                problem,
            })
        };
        let is_name = |text: &str| !text.is_empty() && text.chars().all(is_name_char);
        if !is_name(name) {
            return refuse("has a name that is empty or not a name");
        }
        if value.chars().any(char::is_control) {
            return refuse("holds a control character");
        }
        let mut upper = Vec::with_capacity(params.len());
        for (param, values) in params {
            if !is_name(&param) {
                return refuse("has a parameter name that is empty or not a name");
            }
            let breaks = |c: char| c == '\r' || c == '\n';
            if values
                .iter()
                .any(|v| v.chars().any(|c| c.is_control() && !breaks(c)))
            {
                return refuse("has a parameter value that holds a control character");
            }
            upper.push((param.to_ascii_uppercase(), values));
        }
        Ok(Property {
            name: name.to_ascii_uppercase(),
            params: upper,
            value,
            written: None,
        })
    }

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

    /// A property named `name` holding this time: a date as
    /// `NAME;VALUE=DATE:YYYYMMDD`, a UTC date-time as `NAME:…Z`, one in a
    /// zone as `NAME;TZID=<zone>:…`, a floating one as `NAME:…`.
    /// [`When::of`] reads it back.
    pub fn property(&self, name: &str) -> Property {
        let (params, value) = match self {
            When::Date(date) => (
                vec![("VALUE".to_owned(), vec!["DATE".to_owned()])],
                date.clone(),
            ),
            When::DateTime {
                local,
                utc,
                time_zone,
            } => {
                let params = time_zone
                    .iter()
                    .map(|zone| ("TZID".to_owned(), vec![zone.clone()]))
                    .collect();
                let suffix = if *utc { "Z" } else { "" };
                (params, format!("{local}{suffix}"))
            }
        };
        Property {
            name: name.to_owned(),
            params,
            value,
            written: None,
        }
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
        written: Some(line.to_owned()),
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

/// `text` escaped as a TEXT value (RFC 5545 section 3.3.11): `\` as `\\`,
/// `;` as `\;`, `,` as `\,`, and each line break (CRLF, a lone CR or a LF)
/// as `\n`.
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\r' => {
                chars.next_if_eq(&'\n');
                out.push_str("\\n");
            }
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

/// `epoch_ms`, milliseconds since 1970-01-01T00:00:00Z, as a UTC date-time
/// value, `YYYYMMDDTHHMMSSZ`, the milliseconds dropped.
pub fn utc_stamp(epoch_ms: u64) -> String {
    let seconds = epoch_ms / 1000;
    let (days, of_day) = (seconds / 86_400, seconds % 86_400);
    // Days to a proleptic Gregorian date, counting eras of 400 years from
    // 0000-03-01 so a leap day ends its year.
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

/// The longest physical line, in octets before its CRLF (RFC 5545 section
/// 3.1).
const FOLD_OCTETS: usize = 75;

/// `calendar` as iCalendar text: `BEGIN`, the properties, the nested
/// components and `END`, recursively, each logical line folded and ended in
/// CRLF. A property is written as the line it was read from while that line
/// still reads as the property's name, parameters and value; otherwise it is
/// serialized.
pub fn write(calendar: &Component) -> String {
    let mut out = String::new();
    write_component(calendar, &mut out);
    out
}

fn write_component(component: &Component, out: &mut String) {
    fold(&format!("BEGIN:{}", component.name), out);
    for property in &component.properties {
        fold(&property_line(property), out);
    }
    for nested in &component.components {
        write_component(nested, out);
    }
    fold(&format!("END:{}", component.name), out);
}

/// The unfolded line for `property`: the one it was read from when that
/// still matches it, else its serialization. Each parameter value is quoted
/// when it holds `:`, `;` or `,` and caret-encoded (RFC 6868 section 3).
fn property_line(property: &Property) -> String {
    if let Some(written) = &property.written
        && let Ok(again) = content_line(written)
        && again.name == property.name
        && again.params == property.params
        && again.value == property.value
    {
        return written.clone();
    }
    let mut line = property.name.clone();
    for (name, values) in &property.params {
        line.push(';');
        line.push_str(name);
        line.push('=');
        for (at, value) in values.iter().enumerate() {
            if at > 0 {
                line.push(',');
            }
            let quote = value.contains([':', ';', ',']);
            if quote {
                line.push('"');
            }
            line.push_str(&caret_encode(value));
            if quote {
                line.push('"');
            }
        }
    }
    line.push(':');
    line.push_str(&property.value);
    line
}

/// A parameter value with RFC 6868's caret encoding applied: a caret as
/// `^^`, a double quote as `^'`, and each line break (CRLF, a lone CR or a
/// LF) as `^n`.
fn caret_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '^' => out.push_str("^^"),
            '"' => out.push_str("^'"),
            '\r' => {
                chars.next_if_eq(&'\n');
                out.push_str("^n");
            }
            '\n' => out.push_str("^n"),
            other => out.push(other),
        }
    }
    out
}

/// `line` appended to `out` folded (RFC 5545 section 3.1): no physical line
/// longer than [`FOLD_OCTETS`] before its CRLF, each continuation beginning
/// with one space that counts toward it, and no break inside a character.
fn fold(line: &str, out: &mut String) {
    let mut rest = line;
    let mut room = FOLD_OCTETS;
    while rest.len() > room {
        let at = rest.floor_char_boundary(room);
        out.push_str(&rest[..at]);
        out.push_str("\r\n ");
        rest = &rest[at..];
        room = FOLD_OCTETS - 1;
    }
    out.push_str(rest);
    out.push_str("\r\n");
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

    /// An event with a time zone, an alarm, attendees with quoted and
    /// caret-encoded parameters, `X-` properties, lower-case names, a lone
    /// caret, a property after a nested component, bare LF line ends and a
    /// fold inside a two-byte character.
    fn rich_event() -> Vec<u8> {
        let mut bytes = b"BEGIN:VCALENDAR\r\n\
            VERSION:2.0\r\n\
            PRODID:-//Example Corp//Example Client//EN\r\n\
            BEGIN:VTIMEZONE\r\n\
            TZID:Europe/Berlin\r\n\
            BEGIN:STANDARD\r\n\
            DTSTART:19701025T030000\r\n\
            RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\n\
            TZOFFSETFROM:+0200\r\n\
            TZOFFSETTO:+0100\r\n\
            END:STANDARD\r\n\
            END:VTIMEZONE\r\n\
            begin:vevent\n\
            uid:event-1@example.com\n\
            DTSTAMP:20261101T120000Z\n\
            DTSTART;TZID=Europe/Berlin:20261102T090000\n\
            DTEND;TZID=Europe/Berlin:20261102T100000\n\
            SUMMARY:Caf"
            .to_vec();
        let e_acute = "é".as_bytes();
        bytes.push(e_acute[0]);
        bytes.extend_from_slice(b"\r\n ");
        bytes.push(e_acute[1]);
        bytes.extend_from_slice(
            "  with Alice\\, Bob\\; and a backslash \\\\ in a summary long enough to fold\r\n\
            ORGANIZER;CN=\"Alice ^'A^' Example\":mailto:alice@example.com\r\n\
            ATTENDEE;CN=\"Bob; Example, Inc:\";PARTSTAT=ACCEPTED;x-note=^n^x:mailto:bob@example.com\r\n\
            X-EXAMPLE-FLAG;x-param=\"a:b\",c:value with ^ a lone caret\r\n\
            BEGIN:VALARM\r\n\
            ACTION:DISPLAY\r\n\
            TRIGGER:-PT15M\r\n\
            DESCRIPTION:Reminder\r\n\
            END:VALARM\r\n\
            LOCATION:Room 1\r\n\
            end:VEVENT\r\n\
            END:VCALENDAR\r\n"
                .as_bytes(),
        );
        bytes
    }

    /// The logical lines of `text`, folds removed.
    fn unfolded(text: impl AsRef<[u8]>) -> Vec<String> {
        unfold(text.as_ref())
            .into_iter()
            .map(|(_, line)| String::from_utf8(line).unwrap())
            .collect()
    }

    fn assert_well_formed(text: &str) {
        assert!(text.ends_with("\r\n"), "{text:?}");
        for line in text.split_terminator("\r\n") {
            assert!(!line.contains(['\r', '\n']), "{line:?}");
            assert!(line.len() <= FOLD_OCTETS, "{} {line:?}", line.len());
        }
    }

    #[test]
    fn an_event_read_and_written_unchanged_reads_the_same() {
        let read = parse(&rich_event()).unwrap();
        let text = write(&read);
        assert_well_formed(&text);
        assert_eq!(parse(text.as_bytes()).unwrap(), read);
        let lines = unfolded(&text);
        for kept in [
            "begin:vevent",
            "uid:event-1@example.com",
            "ORGANIZER;CN=\"Alice ^'A^' Example\":mailto:alice@example.com",
            "ATTENDEE;CN=\"Bob; Example, Inc:\";PARTSTAT=ACCEPTED;x-note=^n^x:mailto:bob@example.com",
            "X-EXAMPLE-FLAG;x-param=\"a:b\",c:value with ^ a lone caret",
        ] {
            let normalized = kept.starts_with("begin");
            assert_eq!(lines.iter().any(|line| line == kept), !normalized, "{kept}");
        }
        assert!(lines.iter().any(|line| line == "BEGIN:VEVENT"));
        assert!(lines.iter().any(|line| line == "END:VEVENT"));
        let event = &read.components[1];
        assert_eq!(
            event.property("SUMMARY").unwrap().text(),
            "Café  with Alice, Bob; and a backslash \\ in a summary long enough to fold"
        );
        let location = lines.iter().position(|l| l == "LOCATION:Room 1").unwrap();
        let alarm = lines.iter().position(|l| l == "BEGIN:VALARM").unwrap();
        assert!(
            location < alarm,
            "a property is written before nested components"
        );
    }

    #[test]
    fn replacing_one_property_leaves_every_other_line_as_read() {
        let original = rich_event();
        let mut calendar = parse(&original).unwrap();
        let event = &mut calendar.components[1];
        let at = event
            .properties
            .iter()
            .position(|p| p.name == "SUMMARY")
            .unwrap();
        event.properties[at] =
            Property::new("SUMMARY", Vec::new(), escape_text("Lunch; then, coffee")).unwrap();
        let text = write(&calendar);
        assert_well_formed(&text);
        // Expected: the lines as read, the summary replaced, the lower-case
        // `BEGIN` and `END` upper-cased, and the `LOCATION` that followed the
        // alarm moved before it.
        let mut expected = unfolded(&original);
        for line in expected.iter_mut() {
            if line.starts_with("SUMMARY") {
                *line = r"SUMMARY:Lunch\; then\, coffee".to_owned();
            } else if line.eq_ignore_ascii_case("begin:vevent")
                || line.eq_ignore_ascii_case("end:vevent")
            {
                *line = line.to_ascii_uppercase();
            }
        }
        let location = expected
            .iter()
            .position(|l| l == "LOCATION:Room 1")
            .unwrap();
        let moved = expected.remove(location);
        let alarm = expected.iter().position(|l| l == "BEGIN:VALARM").unwrap();
        expected.insert(alarm, moved);
        assert_eq!(unfolded(&text), expected);
    }

    #[test]
    fn a_property_changed_after_reading_is_serialized() {
        let mut calendar = parse(&rich_event()).unwrap();
        let event = &mut calendar.components[1];
        let attendee = event
            .properties
            .iter_mut()
            .find(|p| p.name == "ATTENDEE")
            .unwrap();
        attendee.value = "mailto:carol@example.com".to_owned();
        let uid = event
            .properties
            .iter_mut()
            .find(|p| p.name == "UID")
            .unwrap();
        uid.params
            .push(("X-TAG".to_owned(), vec!["one".to_owned()]));
        let lines = unfolded(write(&calendar));
        assert!(lines.contains(
            &"ATTENDEE;CN=\"Bob; Example, Inc:\";PARTSTAT=ACCEPTED;X-NOTE=^n^^x:mailto:carol@example.com"
                .to_owned()
        ));
        assert!(lines.contains(&"UID;X-TAG=one:event-1@example.com".to_owned()));
        assert!(!lines.iter().any(|l| l.contains("bob@example.com")));
    }

    #[test]
    fn a_built_property_is_quoted_and_caret_encoded() {
        let property = Property::new(
            "attendee",
            vec![
                ("cn".to_owned(), vec!["Alice \"A\" ^ Example".to_owned()]),
                (
                    "X-LIST".to_owned(),
                    vec![
                        "a:b".to_owned(),
                        "c;d".to_owned(),
                        "e,f".to_owned(),
                        "g".to_owned(),
                    ],
                ),
                (
                    "X-NOTE".to_owned(),
                    vec!["one\r\ntwo\rthree\nfour".to_owned()],
                ),
                ("X-EMPTY".to_owned(), vec![String::new()]),
            ],
            "mailto:alice@example.com".to_owned(),
        )
        .unwrap();
        assert_eq!(property.name, "ATTENDEE");
        let line = property_line(&property);
        assert_eq!(
            line,
            "ATTENDEE;CN=Alice ^'A^' ^^ Example;X-LIST=\"a:b\",\"c;d\",\"e,f\",g;\
             X-NOTE=one^ntwo^nthree^nfour;X-EMPTY=:mailto:alice@example.com"
        );
        let again = content_line(&line).unwrap();
        assert_eq!(again.param("CN"), Some("Alice \"A\" ^ Example"));
        assert_eq!(again.params[1].1, ["a:b", "c;d", "e,f", "g"]);
        assert_eq!(again.param("X-NOTE"), Some("one\ntwo\nthree\nfour"));
        assert_eq!(again.value, "mailto:alice@example.com");
    }

    #[test]
    fn a_property_that_cannot_be_written_is_refused() {
        let refused =
            |name: &str, params: Vec<(String, Vec<String>)>, value: &str| match Property::new(
                name,
                params,
                value.to_owned(),
            )
            .unwrap_err()
            {
                Error::Value { property, problem } => {
                    assert_eq!(property, name);
                    problem
                }
                other => panic!("not a value error: {other:?}"),
            };
        let param = |name: &str, value: &str| vec![(name.to_owned(), vec![value.to_owned()])];
        assert_eq!(
            refused("", Vec::new(), "v"),
            "has a name that is empty or not a name"
        );
        assert_eq!(
            refused("X_NAME", Vec::new(), "v"),
            "has a name that is empty or not a name"
        );
        assert_eq!(
            refused("SUMMARY", Vec::new(), "a\nb"),
            "holds a control character"
        );
        assert_eq!(
            refused("SUMMARY", Vec::new(), "a\tb"),
            "holds a control character"
        );
        assert_eq!(
            refused("SUMMARY", param("", "x"), "v"),
            "has a parameter name that is empty or not a name"
        );
        assert_eq!(
            refused("SUMMARY", param("A=B", "x"), "v"),
            "has a parameter name that is empty or not a name"
        );
        assert_eq!(
            refused("SUMMARY", param("X-A", "a\u{7}b"), "v"),
            "has a parameter value that holds a control character"
        );
        assert!(Property::new("SUMMARY", param("X-A", "a\r\nb"), "v".to_owned()).is_ok());
    }

    #[test]
    fn text_is_escaped_and_read_back() {
        for (text, escaped, read) in [
            (r"a\b", r"a\\b", r"a\b"),
            ("a;b", r"a\;b", "a;b"),
            ("a,b", r"a\,b", "a,b"),
            ("a\r\nb", r"a\nb", "a\nb"),
            ("a\rb", r"a\nb", "a\nb"),
            ("a\nb", r"a\nb", "a\nb"),
            ("a\r\n\r\nb", r"a\n\nb", "a\n\nb"),
            ("é: plain", "é: plain", "é: plain"),
        ] {
            assert_eq!(escape_text(text), escaped, "{text:?}");
            let property = Property::new("DESCRIPTION", Vec::new(), escape_text(text)).unwrap();
            assert_eq!(property.text(), read, "{text:?}");
        }
    }

    #[test]
    fn folding_holds_lines_to_75_octets_on_character_boundaries() {
        let mut out = String::new();
        fold(&"a".repeat(75), &mut out);
        assert_eq!(out, format!("{}\r\n", "a".repeat(75)));
        let mut out = String::new();
        fold(&"a".repeat(76), &mut out);
        assert_eq!(out, format!("{}\r\n a\r\n", "a".repeat(75)));
        for wide in ["é", "€", "𝄞"] {
            for lead in 0..4 {
                let line = format!("{}{}", "a".repeat(lead), wide.repeat(100));
                let mut out = String::new();
                fold(&line, &mut out);
                assert_well_formed(&out);
                for physical in out.split_terminator("\r\n") {
                    assert!(std::str::from_utf8(physical.as_bytes()).is_ok());
                    let body = physical.strip_prefix(' ').unwrap_or(physical);
                    assert!(body.chars().all(|c| c == 'a' || c.to_string() == wide));
                }
                assert_eq!(unfolded(&out), [line]);
            }
        }
    }

    #[test]
    fn times_are_written_and_read_back() {
        for (when, line) in [
            (
                When::Date("20261102".to_owned()),
                "DTSTART;VALUE=DATE:20261102",
            ),
            (
                When::DateTime {
                    local: "20261102T090000".to_owned(),
                    utc: true,
                    time_zone: None,
                },
                "DTSTART:20261102T090000Z",
            ),
            (
                When::DateTime {
                    local: "20261102T090000".to_owned(),
                    utc: false,
                    time_zone: Some("America/New_York".to_owned()),
                },
                "DTSTART;TZID=America/New_York:20261102T090000",
            ),
            (
                When::DateTime {
                    local: "20261102T090000".to_owned(),
                    utc: false,
                    time_zone: None,
                },
                "DTSTART:20261102T090000",
            ),
        ] {
            let property = when.property("DTSTART");
            assert_eq!(property_line(&property), line);
            assert_eq!(When::of(&property).unwrap(), when);
            assert_eq!(When::of(&content_line(line).unwrap()).unwrap(), when);
        }
    }

    #[test]
    fn stamps_are_utc_date_times() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        assert_eq!(utc_stamp(999), "19700101T000000Z");
        assert_eq!(utc_stamp(951_782_400_000), "20000229T000000Z");
        assert_eq!(utc_stamp(1_709_210_096_000), "20240229T123456Z");
        assert_eq!(utc_stamp(1_767_225_599_999), "20251231T235959Z");
        assert_eq!(utc_stamp(1_767_225_600_000), "20260101T000000Z");
    }
}
