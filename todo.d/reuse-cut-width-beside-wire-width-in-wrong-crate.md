The mail components bound every string they copy out of Fastmail's answer
with the provider kit's `cut`, which counts UTF-8 bytes before JSON
escaping, while the result ceiling counts the answer as serialized: a quote
or newline takes two bytes there and any other control character six. A
page of escape-heavy subjects or previews can therefore be refused by the
ceiling although every string is within its bound. `fastmail-contacts`
bounds by serialized width with `cut_width` in its `card.rs`. Moving the
mail components to serialized-width bounds changes where their answers cut
and what their bounds are sized at, which is a decision about two shipped
tools; once it is taken, `cut_width` moves into the provider crate beside
`wire_width` (which measures a different thing: the width of text escaped
twice, inside a nested call's JSON body) and a width-cutting variant of
`failure::optional` serves both components. Done is the mail components
bounding by serialized width through the shared helper, or a decision that
byte bounds stay. The
`TODO(reuse-cut-width-beside-wire-width-in-wrong-crate)` comment sits at the
byte bounds in `fastmail-mail-read/src/logic.rs`.
