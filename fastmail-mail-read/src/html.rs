//! An HTML message part rendered as plain text for a model to read: no
//! markup, no scripts or styles, block boundaries as line breaks, table cells
//! as tabs, character references decoded. A link keeps its text and loses its
//! address. It is a reader's rendering, not a parser: malformed markup yields
//! text, never an error.

// TODO(reuse-html-to-text-reimplements-gmail-decoder-and-drops-text): brenn's
// Gmail reader renders mail HTML for a model too; one shared reader would
// replace this one.

/// Elements dropped with everything inside them. `head` is not one: its end
/// tag is optional, and its only text-bearing children are these.
const DROPPED: &[&str] = &["script", "style", "title"];
/// Elements whose start and end are line breaks.
const BLOCKS: &[&str] = &[
    "br",
    "p",
    "div",
    "li",
    "tr",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "ul",
    "ol",
    "table",
    "hr",
    "pre",
];
/// Elements whose start is a tab.
const CELLS: &[&str] = &["td", "th"];

/// `html` as plain text.
///
/// A `<` opens a tag only when a letter, `/`, `!` or `?` follows it;
/// otherwise it is text, as in `3 < 5 and 7 > 2`. Inside a tag a quote opens
/// a quoted value only right after `=`, and a `>` inside one does not end
/// the tag. A tag never closed is text; a comment or dropped element never
/// closed runs to the end of the input.
pub fn to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut text_from = 0;
    let mut at = 0;
    while let Some(found) = html[at..].find('<') {
        let open = at + found;
        let next = html[open + 1..].chars().next();
        if !next.is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?')) {
            at = open + 1;
            continue;
        }
        let end = if html[open..].starts_with("<!--") {
            push_text(&mut out, &html[text_from..open]);
            comment_end(html, open + 4)
        } else {
            let Some(end) = tag_end(html, open) else {
                break;
            };
            push_text(&mut out, &html[text_from..open]);
            let tag = &html[open + 1..end - 1];
            let name = tag_name(tag);
            if DROPPED.contains(&name.as_str()) && !tag.starts_with('/') && !tag.ends_with('/') {
                dropped_end(html, &name, end)
            } else {
                if BLOCKS.contains(&name.as_str()) {
                    out.push('\n');
                } else if CELLS.contains(&name.as_str()) && !tag.starts_with('/') {
                    out.push('\t');
                }
                Some(end)
            }
        };
        let Some(end) = end else {
            text_from = html.len();
            break;
        };
        text_from = end;
        at = end;
    }
    push_text(&mut out, &html[text_from..]);
    tidy(&out)
}

/// The byte after the end of the comment whose `<!--` ends at `body`, or
/// `None` when it is never closed. `<!-->` and `<!--->` are whole empty
/// comments; otherwise the first `-->` or `--!>` ends it. One pass: the scan
/// stops at the close.
fn comment_end(html: &str, body: usize) -> Option<usize> {
    let rest = &html[body..];
    if rest.starts_with('>') {
        return Some(body + 1);
    }
    if rest.starts_with("->") {
        return Some(body + 2);
    }
    let mut at = 0;
    while let Some(found) = rest[at..].find("--") {
        let dashes = at + found;
        let after = &rest[dashes + 2..];
        if after.starts_with('>') {
            return Some(body + dashes + 3);
        }
        if after.starts_with("!>") {
            return Some(body + dashes + 4);
        }
        at = dashes + 1;
    }
    None
}

/// The byte after the `>` closing the tag that opens at `open`, quoted
/// attribute values skipped, or `None` when the tag is never closed. A quote
/// opens a value only when the last non-whitespace character before it is
/// `=`, so `alt=Bob's` holds no quoted value.
fn tag_end(html: &str, open: usize) -> Option<usize> {
    let mut quote = None;
    let mut after_equals = false;
    for (i, c) in html[open + 1..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') if after_equals => quote = Some(c),
            (None, '>') => return Some(open + 1 + i + 1),
            (None, _) => {}
        }
        if quote.is_none() && !c.is_ascii_whitespace() {
            after_equals = c == '=';
        }
    }
    None
}

/// A tag's element name, lower-cased, without its `/` or attributes.
fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('/')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// The byte after the close tag of the dropped element `name` whose open tag
/// ends at `from`, or `None` when it is never closed. A close is `</` and the
/// name in any case, then `>`, `/` or whitespace, so `</styles` is not one.
/// One pass: the scan stops at the close.
fn dropped_end(html: &str, name: &str, from: usize) -> Option<usize> {
    let bytes = html.as_bytes();
    let mut at = from;
    while let Some(found) = html[at..].find("</") {
        let open = at + found;
        let name_end = open + 2 + name.len();
        let closes = bytes
            .get(open + 2..name_end)
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name.as_bytes()))
            && bytes
                .get(name_end)
                .is_some_and(|&b| b == b'>' || b == b'/' || b.is_ascii_whitespace());
        if closes {
            return tag_end(html, open);
        }
        at = open + 2;
    }
    None
}

/// Text between tags, with whitespace runs collapsed to one space, as a
/// browser renders it, and character references decoded.
fn push_text(out: &mut String, text: &str) {
    let mut rest = text;
    while !rest.is_empty() {
        let c = rest.chars().next().expect("rest is not empty");
        if c.is_whitespace() {
            if !out.ends_with([' ', '\n', '\t']) && !out.is_empty() {
                out.push(' ');
            }
            rest = &rest[c.len_utf8()..];
        } else if c == '&' {
            let (decoded, used) = reference(rest);
            out.push_str(&decoded);
            rest = &rest[used..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
}

/// The character reference at the start of `text` (which starts with `&`),
/// decoded, and how many bytes it spans. An unknown or unterminated reference
/// is the `&` alone.
fn reference(text: &str) -> (String, usize) {
    let literal = ("&".to_string(), 1);
    // `;` is ASCII, so its byte offset is a char boundary.
    let Some(end) = text.bytes().take(12).position(|b| b == b';') else {
        return literal;
    };
    let body = &text[1..end];
    let decoded = match body.strip_prefix('#') {
        Some(hex) if hex.starts_with(['x', 'X']) => u32::from_str_radix(&hex[1..], 16)
            .ok()
            .and_then(char::from_u32)
            .map(String::from),
        Some(decimal) => decimal
            .parse()
            .ok()
            .and_then(char::from_u32)
            .map(String::from),
        None => NAMED
            .iter()
            .find(|(name, _)| *name == body)
            .map(|(_, text)| (*text).to_owned()),
    };
    match decoded {
        Some(decoded) => (decoded, end + 1),
        None => literal,
    }
}

/// The named character references decoded: the markup ones, and the
/// typography, symbols and accented letters mail commonly spells this way.
/// Names are case-sensitive. A space-like reference is a space; an invisible
/// one is nothing.
const NAMED: &[(&str, &str)] = &[
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("quot", "\""),
    ("apos", "'"),
    ("nbsp", " "),
    ("ensp", " "),
    ("emsp", " "),
    ("thinsp", " "),
    ("shy", ""),
    ("zwj", ""),
    ("zwnj", ""),
    ("ndash", "\u{2013}"),
    ("mdash", "\u{2014}"),
    ("lsquo", "\u{2018}"),
    ("rsquo", "\u{2019}"),
    ("sbquo", "\u{201a}"),
    ("ldquo", "\u{201c}"),
    ("rdquo", "\u{201d}"),
    ("bdquo", "\u{201e}"),
    ("laquo", "\u{ab}"),
    ("raquo", "\u{bb}"),
    ("lsaquo", "\u{2039}"),
    ("rsaquo", "\u{203a}"),
    ("hellip", "\u{2026}"),
    ("bull", "\u{2022}"),
    ("middot", "\u{b7}"),
    ("dagger", "\u{2020}"),
    ("Dagger", "\u{2021}"),
    ("permil", "\u{2030}"),
    ("prime", "\u{2032}"),
    ("sect", "\u{a7}"),
    ("para", "\u{b6}"),
    ("copy", "\u{a9}"),
    ("reg", "\u{ae}"),
    ("trade", "\u{2122}"),
    ("deg", "\u{b0}"),
    ("plusmn", "\u{b1}"),
    ("times", "\u{d7}"),
    ("divide", "\u{f7}"),
    ("frac14", "\u{bc}"),
    ("frac12", "\u{bd}"),
    ("frac34", "\u{be}"),
    ("micro", "\u{b5}"),
    ("iexcl", "\u{a1}"),
    ("iquest", "\u{bf}"),
    ("larr", "\u{2190}"),
    ("rarr", "\u{2192}"),
    ("uarr", "\u{2191}"),
    ("darr", "\u{2193}"),
    ("euro", "\u{20ac}"),
    ("pound", "\u{a3}"),
    ("yen", "\u{a5}"),
    ("cent", "\u{a2}"),
    ("curren", "\u{a4}"),
    ("aacute", "\u{e1}"),
    ("agrave", "\u{e0}"),
    ("auml", "\u{e4}"),
    ("ccedil", "\u{e7}"),
    ("eacute", "\u{e9}"),
    ("egrave", "\u{e8}"),
    ("ecirc", "\u{ea}"),
    ("iacute", "\u{ed}"),
    ("ntilde", "\u{f1}"),
    ("oacute", "\u{f3}"),
    ("ouml", "\u{f6}"),
    ("uacute", "\u{fa}"),
    ("uuml", "\u{fc}"),
    ("szlig", "\u{df}"),
    ("Eacute", "\u{c9}"),
    ("Auml", "\u{c4}"),
    ("Ouml", "\u{d6}"),
    ("Uuml", "\u{dc}"),
];

/// Each line trimmed of spaces and tabs at its ends, a run of blank lines
/// between two lines of text kept as one blank line, and none at either end.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = false;
    for line in text.split('\n') {
        let line = line.trim_matches([' ', '\t']);
        if line.is_empty() {
            blank = true;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank { "\n\n" } else { "\n" });
        }
        blank = false;
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_text_is_unchanged() {
        assert_eq!(to_text("Hello, Alice."), "Hello, Alice.");
    }

    #[test]
    fn tags_are_dropped_and_link_text_kept() {
        assert_eq!(
            to_text(r#"<b>Hi</b> <a href="https://example.com/x">see this</a>!"#),
            "Hi see this!"
        );
    }

    #[test]
    fn scripts_styles_heads_and_comments_go_with_their_content() {
        assert_eq!(
            to_text(
                "<html><head><title>T</title><style>p{}</style></head><body>\
                 <!-- hidden --><SCRIPT>alert(1)</SCRIPT>shown</body></html>"
            ),
            "shown"
        );
    }

    #[test]
    fn blocks_break_lines_and_cells_tab() {
        assert_eq!(
            to_text("<p>one</p><p>two<br>three</p><table><tr><td>a</td><td>b</td></tr></table>"),
            "one\n\ntwo\nthree\n\na\tb"
        );
        assert_eq!(
            to_text("<h1>Title</h1><ul><li>x</li><li>y</li></ul>"),
            "Title\n\nx\n\ny"
        );
    }

    #[test]
    fn entities_decode() {
        assert_eq!(
            to_text(
                "a &amp; b &lt;c&gt; &quot;d&quot; &apos;e&#39; f&nbsp;g &#233; &#x263A; &bogus; &"
            ),
            "a & b <c> \"d\" 'e' f g é ☺ &bogus; &"
        );
    }

    #[test]
    fn whitespace_collapses_and_blank_runs_merge() {
        assert_eq!(
            to_text("  one\n   two  <div></div><div></div><div></div> three "),
            "one two\n\nthree"
        );
    }

    #[test]
    fn malformed_markup_is_text_not_an_error() {
        assert_eq!(to_text("a < b and <unclosed"), "a < b and <unclosed");
        assert_eq!(to_text("x<script>never closed"), "x");
        assert_eq!(to_text("x<!-- never closed"), "x");
    }

    #[test]
    fn a_lt_that_opens_no_tag_is_text() {
        assert_eq!(to_text("3 < 5 and 7 > 2"), "3 < 5 and 7 > 2");
        assert_eq!(
            to_text("<p>the amount is < $500 and due > Friday</p>"),
            "the amount is < $500 and due > Friday"
        );
        assert_eq!(to_text("a <= b, c <3 d, e<-f>"), "a <= b, c <3 d, e<-f>");
    }

    #[test]
    fn a_gt_in_a_quoted_value_does_not_end_the_tag() {
        assert_eq!(to_text(r#"<a title="x>y">link</a> after"#), "link after");
        assert_eq!(to_text("<a title='x>y'>link</a>"), "link");
        assert_eq!(to_text("<img alt=Bob's>text"), "text");
        assert_eq!(
            to_text(r#"<style data-x="</style>">p{}</style>shown"#),
            "shown"
        );
    }

    #[test]
    fn comments_end_as_html_ends_them() {
        assert_eq!(to_text("a<!-- x --!>b"), "ab");
        assert_eq!(to_text("a<!-->b"), "ab");
        assert_eq!(to_text("a<!--->b"), "ab");
    }

    #[test]
    fn a_head_without_its_end_tag_keeps_the_body() {
        assert_eq!(
            to_text(
                "<html><head><meta charset=utf-8><body><p>Your invoice is attached.</p></body></html>"
            ),
            "Your invoice is attached."
        );
        assert_eq!(
            to_text("<head><style>a{}</style><body><header>Hi</header></head><p>after</p>"),
            "Hi\nafter"
        );
    }

    #[test]
    fn a_dropped_element_ends_only_at_its_own_close() {
        assert_eq!(to_text("<style>a{} </styles> b{}</STYLE >shown"), "shown");
        assert_eq!(to_text("<title>x</title/>shown"), "shown");
        assert_eq!(to_text("x<script>never closed </script"), "x");
    }

    #[test]
    fn crafted_runs_of_comments_and_dropped_elements_read_in_one_pass() {
        assert_eq!(to_text(&"<!---->".repeat(8571)), "");
        assert_eq!(
            to_text(&format!("{}x", "<style></style>".repeat(4000))),
            "x"
        );
        assert_eq!(to_text("a<!-- x - -- y ---!>b<!-- -- ---->c"), "abc");
    }

    #[test]
    fn typographic_and_accented_references_decode() {
        assert_eq!(
            to_text("&ldquo;Caf&eacute;&rdquo; &mdash; it&rsquo;s &euro;5&hellip;&shy;"),
            "\u{201c}Café\u{201d} \u{2014} it\u{2019}s €5…"
        );
        assert_eq!(to_text("&EACUTE; &Eacute;"), "&EACUTE; É");
    }
}
