The README tells an operator that installing a component whose class provides
tools — adding one more Fastmail component to a deployment — takes a restart
of Brenn. That is true of Brenn releases up to 0.33.0 and of every release
that refuses a reload adding a `wasm_consumer` with a `provide` block. A later
Brenn applies that reload. Once a Brenn release carrying the change exists,
the README's paragraph on adding a component says "reload" and names the
first Brenn version that applies it, so an operator on an older Brenn still
knows to restart; what still takes a restart (changing or removing a
component's tools) is said there too. Best done together with
`restart-wording-after-key-reload`. Done = the README matching a released
Brenn. The `TODO(restart-wording-after-provider-reload)` comment sits beside
the example document's fit test in `BUILD.bazel`.
