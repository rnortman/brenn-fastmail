One HTML-to-text reader for mail. `fastmail-mail-read/src/html.rs` and
brenn's Gmail reader (`html_to_text` and `decode_entities` in
`brenn-google-workspace/src/gmail/decode.rs`) render a message's HTML part
as text for a model, in two implementations that already differ in their
line-break and entity rules and will drift further. brenn
exporting the reader from a neutral, dependency-free crate with a guest
target that out-of-tree components may depend on would let this repository
drop its copy; that crate and its place in brenn's public build contract are
brenn's to design. The `TODO(reuse-html-to-text-reimplements-gmail-decoder-and-drops-text)`
comment sits at the top of `fastmail-mail-read/src/html.rs`.
