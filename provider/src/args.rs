//! Reading the arguments every Fastmail tool shares, and the width a string
//! has once it is inside a nested call's body.

use std::collections::BTreeSet;

use brenn_provider_kit::Args;
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

/// An argument that is 1 to `max` distinct JMAP ids, each as [`token`]
/// admits it; absent is `None`. Refusals say "`key` must be an array of 1 to
/// `max`" `what`, that each of `key` must be an id, or that it names an id
/// twice.
pub fn id_list(
    args: &Args,
    key: &str,
    max: usize,
    what: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let ids = match value {
        Value::Array(ids) if (1..=max).contains(&ids.len()) => ids,
        _ => return Err(format!("`{key}` must be an array of 1 to {max} {what}")),
    };
    let mut out: Vec<String> = Vec::with_capacity(ids.len());
    let mut seen = BTreeSet::new();
    for id in ids {
        match id {
            Value::String(id) if is_token(id) => {
                if !seen.insert(id.as_str()) {
                    return Err(format!("`{key}` names {id:?} twice"));
                }
                out.push(id.clone());
            }
            _ => return Err(format!("each of `{key}` must be {}", token_rule())),
        }
    }
    Ok(Some(out))
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
