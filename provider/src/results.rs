//! A `/query` and `/get` pair (RFC 8620 sections 5.1 and 5.5) read into a
//! tool's answer: the objects in the query's order, the page's position,
//! total and the position of the next page, and the one object a get by id
//! names; a string cut to the width it takes in a serialized answer; and a
//! list capped with a count of what it leaves out.

use std::collections::BTreeMap;

use brenn_provider_kit::{Args, Failure, internal};
use fastmail_jmap::{GetResult, QueryResult};
use serde_json::{Value, json};

use crate::failure::invalid;
use crate::settings::API_NAME;

/// `items` in the order of `ids`; an id with no item, such as an object
/// deleted between the query and the get, is skipped.
pub fn in_order<T>(ids: &[String], items: Vec<T>, id_of: fn(&T) -> &str) -> Vec<T> {
    let mut by_id: BTreeMap<String, T> = items
        .into_iter()
        .map(|item| (id_of(&item).to_owned(), item))
        .collect();
    ids.iter().filter_map(|id| by_id.remove(id)).collect()
}

/// A page of `query`'s results answered under `key`: `position`, `total`
/// when the query counted, and `next_position` while more match after this
/// page.
pub fn page(query: &QueryResult, key: &str, items: Vec<Value>) -> Value {
    let mut out = Args::new();
    out.insert("position".into(), json!(query.position));
    if let Some(total) = query.total {
        out.insert("total".into(), json!(total));
    }
    out.insert(key.into(), Value::Array(items));
    let next = query.position + query.ids.len() as u64;
    if query.total.is_some_and(|total| total > next) && !query.ids.is_empty() {
        out.insert("next_position".into(), json!(next));
    }
    Value::Object(out)
}

/// The `noun` with id `id` from `got`, `method`'s answer: the caller's
/// failure when the server says it does not exist, an internal one when the
/// server answers neither it nor its absence.
pub fn the_one<T>(
    got: GetResult<T>,
    id: &str,
    id_of: fn(&T) -> &str,
    method: &str,
    noun: &str,
) -> Result<T, Failure> {
    if got.not_found.iter().any(|missing| missing == id) {
        return Err(invalid(format!("no {noun} with id `{id}`")));
    }
    got.list
        .into_iter()
        .find(|item| id_of(item) == id)
        .ok_or_else(|| {
            internal(format!(
                "{API_NAME}'s {method} answered neither {noun} `{id}` nor its absence"
            ))
        })
}

/// The bytes `c` occupies inside a JSON string as `serde_json` writes it.
fn char_width(c: char) -> usize {
    match c {
        '"' | '\\' | '\u{8}' | '\t' | '\n' | '\u{c}' | '\r' => 2,
        c if u32::from(c) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// `text` cut to the longest prefix of whole characters whose serialized
/// widths sum to at most `max`, ending in `…` when cut, and whether it was
/// cut.
pub fn cut_width(text: &str, max: usize) -> (String, bool) {
    let mut width = 0;
    for (at, c) in text.char_indices() {
        width += char_width(c);
        if width > max {
            return (format!("{}…", &text[..at]), true);
        }
    }
    (text.to_owned(), false)
}

/// [`cut_width`]'s text as a JSON string.
pub fn cut_value(text: &str, max: usize) -> Value {
    json!(cut_width(text, max).0)
}

/// At most `cap` of `items`, each through `show`, at `key`, and
/// `<key>_omitted` counting the rest; nothing when `items` is empty.
pub fn capped_list<I>(
    out: &mut Args,
    key: &str,
    items: I,
    cap: usize,
    show: impl FnMut(I::Item) -> Value,
) where
    I: IntoIterator,
    I::IntoIter: ExactSizeIterator,
{
    let items = items.into_iter();
    let total = items.len();
    if total == 0 {
        return;
    }
    out.insert(key.into(), items.take(cap).map(show).collect());
    if total > cap {
        out.insert(format!("{key}_omitted"), json!(total - cap));
    }
}

#[cfg(test)]
mod tests {
    use brenn_provider_kit::ErrorKind;

    use super::*;

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    fn id_of(item: &String) -> &str {
        item
    }

    #[test]
    fn items_follow_the_query_and_a_missing_one_is_skipped() {
        assert_eq!(
            in_order(&ids(&["c", "a", "b"]), ids(&["a", "b", "x"]), id_of),
            ids(&["a", "b"])
        );
    }

    #[test]
    fn a_page_names_the_next_while_more_match() {
        let query = |position, found: &[&str], total| QueryResult {
            ids: ids(found),
            position,
            total,
        };
        assert_eq!(
            page(&query(0, &["a", "b"], Some(5)), "items", vec![json!(1)]),
            json!({ "position": 0, "total": 5, "items": [1], "next_position": 2 })
        );
        assert_eq!(
            page(&query(3, &["a", "b"], Some(5)), "items", vec![]),
            json!({ "position": 3, "total": 5, "items": [] })
        );
        assert_eq!(
            page(&query(9, &[], Some(5)), "items", vec![]),
            json!({ "position": 9, "total": 5, "items": [] })
        );
        assert_eq!(
            page(&query(0, &["a"], None), "items", vec![]),
            json!({ "position": 0, "items": [] })
        );
    }

    #[test]
    fn the_one_is_found_absent_or_unanswered() {
        let got = |list: &[&str], not_found: &[&str]| GetResult {
            list: ids(list),
            not_found: ids(not_found),
        };
        assert_eq!(
            the_one(got(&["a", "b"], &[]), "b", id_of, "Thing/get", "thing").unwrap(),
            "b"
        );
        let absent = the_one(got(&[], &["b"]), "b", id_of, "Thing/get", "thing").unwrap_err();
        assert_eq!(absent.kind, ErrorKind::InvalidArgs);
        assert_eq!(absent.detail, "no thing with id `b`");
        let neither = the_one(got(&["a"], &[]), "b", id_of, "Thing/get", "thing").unwrap_err();
        assert_eq!(neither.kind, ErrorKind::Internal);
        assert_eq!(
            neither.detail,
            "Fastmail's Thing/get answered neither thing `b` nor its absence"
        );
    }

    fn width(text: &str) -> usize {
        serde_json::to_string(text).unwrap().len() - 2
    }

    #[test]
    fn cut_width_bounds_the_serialized_width_on_a_character_boundary() {
        assert_eq!(cut_width("hello", 5), ("hello".to_owned(), false));
        assert_eq!(cut_width("hello", 4), ("hell…".to_owned(), true));
        assert_eq!(cut_width("", 0), (String::new(), false));
        // As the kit's `cut` would cut text with nothing to escape.
        for (text, max) in [("héllo wörld", 3), ("😀😀", 5), ("abc", 2)] {
            assert_eq!(cut_width(text, max), brenn_provider_kit::cut(text, max));
        }
        assert_eq!(cut_width("a\"b", 2), ("a…".to_owned(), true));
        assert_eq!(cut_width("a\"b", 3), ("a\"…".to_owned(), true));
        assert_eq!(cut_width("\n\n\n", 5), ("\n\n…".to_owned(), true));
        assert_eq!(cut_width("\u{1}\u{1}", 11), ("\u{1}…".to_owned(), true));
        assert_eq!(cut_width("é", 1), ("…".to_owned(), true));
        for text in [
            "a\"b\\c\nd\te\r\u{8}\u{c}\u{1}\u{1f}\u{7f}é😀".repeat(40),
            "\u{1}".repeat(100),
            "€".repeat(100),
        ] {
            for max in [0, 1, 5, 6, 7, 64, 255, 400] {
                let (cut, was_cut) = cut_width(&text, max);
                let kept = cut.strip_suffix('…').unwrap_or(&cut);
                assert!(width(kept) <= max, "{max}: {kept:?}");
                assert_eq!(was_cut, kept.len() < text.len());
                if was_cut {
                    let next = text[kept.len()..].chars().next().unwrap();
                    assert!(width(kept) + width(&next.to_string()) > max);
                }
            }
        }
    }

    #[test]
    fn capped_list_shows_the_first_cap_and_counts_the_rest() {
        let mut out = Args::new();
        capped_list(&mut out, "words", ["a", "b", "c"], 2, |w| cut_value(w, 1));
        assert_eq!(
            Value::Object(out),
            json!({ "words": ["a", "b"], "words_omitted": 1 })
        );
        let mut out = Args::new();
        capped_list(&mut out, "words", ["a", "bc"], 2, |w| cut_value(w, 1));
        assert_eq!(Value::Object(out), json!({ "words": ["a", "b…"] }));
        let mut out = Args::new();
        capped_list(&mut out, "words", Vec::<&str>::new(), 2, |w| json!(w));
        assert!(out.is_empty());
    }
}
