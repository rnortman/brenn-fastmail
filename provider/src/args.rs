//! Reading the arguments every Fastmail tool shares, and the width a string
//! has once it is inside a nested call's body.

use std::collections::BTreeSet;

use brenn_provider_kit::{Args, capped};
use serde_json::Value;

/// The argument every tool takes beside its own.
pub const ACCOUNT_ARG: &str = "account";

/// The most characters an account name may hold.
pub const ACCOUNT_MAX_CHARS: usize = 255;
/// The most characters a JMAP id may hold (RFC 8620 section 1.2).
pub const ID_MAX_CHARS: usize = 255;
/// The most characters a keyword may hold.
pub const KEYWORD_MAX_CHARS: usize = 255;

/// An account name argument: a lowercase letter, then lowercase letters,
/// digits or `-`, not ending in `-`, at most `ACCOUNT_MAX_CHARS` in all.
pub fn account_arg(args: &Args) -> Result<Option<String>, String> {
    let is_name = |name: &str| {
        let bytes = name.as_bytes();
        (1..=ACCOUNT_MAX_CHARS).contains(&bytes.len())
            && bytes[0].is_ascii_lowercase()
            && bytes[bytes.len() - 1] != b'-'
            && bytes
                .iter()
                .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    match args.get(ACCOUNT_ARG) {
        None => Ok(None),
        Some(Value::String(name)) if is_name(name) => Ok(Some(name.clone())),
        Some(_) => Err(format!(
            "`{ACCOUNT_ARG}` must be an account name: lowercase letters, digits and `-`, \
             starting with a letter, not ending in `-`, at most {ACCOUNT_MAX_CHARS} characters"
        )),
    }
}

/// Whether `id` is 1 to `ID_MAX_CHARS` letters, digits, `_` or `-`: a JMAP
/// id as RFC 8620 section 1.2 shapes it.
pub fn is_token(id: &str) -> bool {
    (1..=ID_MAX_CHARS).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn token_rule() -> String {
    format!("an id of 1 to {ID_MAX_CHARS} letters, digits, `_` or `-`")
}

/// A JMAP id argument: 1 to `ID_MAX_CHARS` letters, digits, `_` or `-`.
pub fn token(args: &Args, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::String(id)) if is_token(id) => Ok(Some(id.clone())),
        Some(_) => Err(format!("`{key}` must be {}", token_rule())),
    }
}

/// Whether `keyword` is 1 to `KEYWORD_MAX_CHARS` letters, digits, `$`, `_`,
/// `.` or `-`.
pub fn is_keyword(keyword: &str) -> bool {
    (1..=KEYWORD_MAX_CHARS).contains(&keyword.len())
        && keyword
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"$_.-".contains(&b))
}

/// What [`is_keyword`] admits, in the caller's terms.
pub fn keyword_rule() -> String {
    format!("a keyword of 1 to {KEYWORD_MAX_CHARS} letters, digits, `$`, `_`, `.` or `-`")
}

/// A keyword argument, as [`is_keyword`] admits it, lower-cased, since JMAP
/// keywords are case-insensitive.
pub fn keyword_arg(args: &Args, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::String(keyword)) if is_keyword(keyword) => {
            Ok(Some(keyword.to_ascii_lowercase()))
        }
        Some(_) => Err(format!("`{key}` must be {}", keyword_rule())),
    }
}

/// An argument that is 1 to `max` distinct items, each as `item` reads it;
/// absent is `None`. Refusals say "`key` must be an array of 1 to `max`"
/// `what`, give `item`'s refusal, or say that `key` names an item twice.
pub fn distinct_list(
    args: &Args,
    key: &str,
    max: usize,
    what: &str,
    item: impl Fn(&Value) -> Result<String, String>,
) -> Result<Option<Vec<String>>, String> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let items = match value {
        Value::Array(items) if (1..=max).contains(&items.len()) => items,
        _ => return Err(format!("`{key}` must be an array of 1 to {max} {what}")),
    };
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    let mut seen = BTreeSet::new();
    for value in items {
        let read = item(value)?;
        if !seen.insert(read.clone()) {
            return Err(format!("`{key}` names {read:?} twice"));
        }
        out.push(read);
    }
    Ok(Some(out))
}

/// An argument that is 1 to `max` distinct JMAP ids, each as [`token`]
/// admits it, as [`distinct_list`] reads it; each refusal of an item says
/// that each of `key` must be an id.
pub fn id_list(
    args: &Args,
    key: &str,
    max: usize,
    what: &str,
) -> Result<Option<Vec<String>>, String> {
    distinct_list(args, key, max, what, |id| match id {
        Value::String(id) if is_token(id) => Ok(id.clone()),
        _ => Err(format!("each of `{key}` must be {}", token_rule())),
    })
}

/// Whether a text may hold line breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lines {
    One,
    Many,
}

/// Refuses a control character in `text`, named `what` in the caller's
/// terms, but, for `Lines::Many`, a line break (`\n`, `\r`).
fn hold_lines(what: &str, text: &str, lines: Lines) -> Result<(), String> {
    let line_break = |c: char| lines == Lines::Many && (c == '\n' || c == '\r');
    if text.chars().any(|c| c.is_control() && !line_break(c)) {
        return Err(match lines {
            Lines::One => format!("{what} must not hold a control character"),
            Lines::Many => format!("{what} must not hold a control character but a line break"),
        });
    }
    Ok(())
}

/// `text`, named `what` in the caller's terms, held as [`one_line`] holds an
/// argument, line breaks admitted for `Lines::Many`: at most `max_chars`
/// characters, not empty or whitespace only, with no other control
/// character. For a text that is not an argument of its own, such as an
/// array's item; the length and emptiness refusals word `capped`'s.
pub fn held_text(what: &str, text: &str, max_chars: usize, lines: Lines) -> Result<(), String> {
    if text.chars().count() > max_chars {
        return Err(format!("{what} is longer than {max_chars} characters"));
    }
    if text.trim().is_empty() {
        return Err(format!("{what} is empty"));
    }
    hold_lines(what, text, lines)
}

/// A one-line text argument at `key`: as `capped` admits it at `max_chars`,
/// with no control character.
pub fn one_line(args: &Args, key: &str, max_chars: usize) -> Result<Option<String>, String> {
    match capped(args, key, max_chars)? {
        None => Ok(None),
        Some(text) => {
            hold_lines(&format!("`{key}`"), text, Lines::One)?;
            Ok(Some(text.to_owned()))
        }
    }
}

/// The bytes `text` takes once escaped as a JSON string twice — inside a
/// JMAP request body, itself a string inside the nested call's arguments —
/// without the quotes either escaping adds around it. JSON escaping widens
/// only control characters, `"` and `\`: a line break is three bytes on the
/// wire, a quote or backslash four, a C0 control seven; non-ASCII stays
/// UTF-8.
pub fn wire_width(text: &str) -> usize {
    let once = serde_json::to_string(text).expect("a string serialises");
    let twice = serde_json::to_string(&once[1..once.len() - 1]).expect("a string serialises");
    twice.len() - 2
}

/// Whether `text` holds no character JSON escaping widens — no control
/// character, `"` or `\` — so its width on the wire is its byte length.
pub fn wire_safe(text: &str) -> bool {
    !text
        .chars()
        .any(|c| c.is_control() || c == '"' || c == '\\')
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn map(value: Value) -> Args {
        let Value::Object(map) = value else {
            panic!("an object");
        };
        map
    }

    #[test]
    fn an_id_list_is_one_to_max_distinct_tokens() {
        let list = |value: Value| id_list(&map(json!({ "ids": value })), "ids", 3, "message ids");
        assert_eq!(id_list(&map(json!({})), "ids", 3, "message ids"), Ok(None));
        assert_eq!(
            list(json!(["a", "b-c", "D_1"])),
            Ok(Some(vec![
                "a".to_owned(),
                "b-c".to_owned(),
                "D_1".to_owned()
            ]))
        );
        assert_eq!(
            list(json!(["x".repeat(255)])),
            Ok(Some(vec!["x".repeat(255)]))
        );
        let count = "`ids` must be an array of 1 to 3 message ids";
        for refused in [
            json!([]),
            json!(["a", "b", "c", "d"]),
            json!("a"),
            json!(null),
        ] {
            assert_eq!(list(refused), Err(count.to_owned()));
        }
        let each = "each of `ids` must be an id of 1 to 255 letters, digits, `_` or `-`";
        for refused in [
            json!([""]),
            json!(["a b"]),
            json!(["a/b"]),
            json!(["é"]),
            json!([1]),
            json!(["x".repeat(256)]),
        ] {
            assert_eq!(list(refused), Err(each.to_owned()));
        }
        assert_eq!(
            list(json!(["a", "b", "a"])),
            Err("`ids` names \"a\" twice".to_owned())
        );
    }

    #[test]
    fn a_one_line_text_is_capped_and_holds_no_control_character() {
        let line = |value: Value| one_line(&map(json!({ "name": value })), "name", 5);
        assert_eq!(one_line(&map(json!({})), "name", 5), Ok(None));
        assert_eq!(line(json!("Alice")), Ok(Some("Alice".to_owned())));
        assert_eq!(line(json!("é😀 ab")), Ok(Some("é😀 ab".to_owned())));
        assert_eq!(
            line(json!("Alice!")),
            Err("`name` is longer than 5 characters".to_owned())
        );
        assert_eq!(line(json!(" ")), Err("`name` is empty".to_owned()));
        assert_eq!(line(json!(7)), Err("`name` must be a string".to_owned()));
        for refused in ["a\nb", "a\tb", "a\rb", "a\u{1}b", "a\u{7f}b", "a\u{85}b"] {
            assert_eq!(
                line(json!(refused)),
                Err("`name` must not hold a control character".to_owned()),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn a_distinct_list_reads_each_item_and_refuses_a_repeat() {
        let list = |value: Value| {
            distinct_list(
                &map(json!({ "xs": value })),
                "xs",
                2,
                "small numbers",
                |item| {
                    item.as_u64()
                        .filter(|n| *n < 10)
                        .map(|n| n.to_string())
                        .ok_or_else(|| "each of `xs` must be a small number".to_owned())
                },
            )
        };
        assert_eq!(
            list(json!([1, 2])),
            Ok(Some(vec!["1".to_owned(), "2".to_owned()]))
        );
        for refused in [json!([]), json!([1, 2, 3]), json!(1)] {
            assert_eq!(
                list(refused),
                Err("`xs` must be an array of 1 to 2 small numbers".to_owned())
            );
        }
        assert_eq!(
            list(json!([1, 10])),
            Err("each of `xs` must be a small number".to_owned())
        );
        assert_eq!(
            list(json!([3, 3])),
            Err("`xs` names \"3\" twice".to_owned())
        );
    }

    #[test]
    fn a_held_text_is_capped_not_blank_and_holds_line_breaks_only_when_allowed() {
        let held = |text: &str, lines| held_text("each of `xs`", text, 5, lines);
        for lines in [Lines::One, Lines::Many] {
            assert_eq!(held("Alice", lines), Ok(()));
            assert_eq!(
                held("Alice!", lines),
                Err("each of `xs` is longer than 5 characters".to_owned())
            );
            assert_eq!(held(" \n", lines), Err("each of `xs` is empty".to_owned()));
        }
        assert_eq!(held("a\r\nb", Lines::Many), Ok(()));
        for refused in ["a\nb", "a\tb", "a\u{1}b"] {
            assert_eq!(
                held(refused, Lines::One),
                Err("each of `xs` must not hold a control character".to_owned()),
                "{refused:?}"
            );
        }
        for refused in ["a\tb", "a\u{1}b", "a\u{85}b"] {
            assert_eq!(
                held(refused, Lines::Many),
                Err("each of `xs` must not hold a control character but a line break".to_owned()),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn wire_width_counts_both_escapings() {
        assert_eq!(wire_width(""), 0);
        assert_eq!(wire_width("hello"), 5);
        assert_eq!(wire_width("\""), 4);
        assert_eq!(wire_width("\\"), 4);
        assert_eq!(wire_width("\n"), 3);
        assert_eq!(wire_width("\r\t"), 6);
        assert_eq!(wire_width("\u{1}"), 7);
        assert_eq!(wire_width("é"), 2);
        assert_eq!(wire_width("😀"), 4);
        assert_eq!(wire_width(&"é".repeat(15_000)), 30_000);
        assert_eq!(wire_width(&format!("x{}", "\n".repeat(9_999))), 29_998);
        assert_eq!(wire_width(&format!("xxx{}", "\n".repeat(9_999))), 30_000);
        assert_eq!(wire_width(&"\"".repeat(7_500)), 30_000);
    }

    #[test]
    fn a_wire_safe_string_is_as_wide_on_the_wire_as_in_bytes() {
        for safe in ["alice@example.com", "Alice Example", "é😀 ", "<a@b>", ""] {
            assert!(wire_safe(safe), "{safe:?}");
            assert_eq!(wire_width(safe), safe.len(), "{safe:?}");
        }
        for unsafe_ in [
            "a\"b", "a\\b", "a\u{1}b", "a\u{7f}b", "a\u{85}b", "a\nb", "a\tb",
        ] {
            assert!(!wire_safe(unsafe_), "{unsafe_:?}");
        }
    }
}
