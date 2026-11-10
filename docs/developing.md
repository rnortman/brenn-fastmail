# Developing brenn-fastmail

## Building

```
make setup    # once per clone: wire the git hooks, lay the scrub kit
make check    # the whole gate
make test     # every test, without the lint aspects
make bundle   # build the deployable tree
make help     # every target
```

The only host prerequisites are bazelisk (installed as `bazel`), git and make;
every toolchain and tool is pinned in `MODULE.bazel` and fetched by checksum.
The first `make check` on a cold machine fetches the Rust and LLVM toolchains
and builds brenn's config tooling from source, and takes a while.

The gate, the changelog fragments and the TODO system are documented in
[`AGENTS.md`](../AGENTS.md).

## What is the consumer's to state

bzlmod reads these from the root module only, so this repository repeats them
and brenn cannot supply them:

- **The brenn pin.** `git_override(module_name = "brenn", commit = ...)` in
  `MODULE.bazel`. The commit must be *served by* brenn's public remote,
  because the fetch is anonymous: one that exists only in a local checkout
  fails module resolution. Build-time contracts — the macros, `brenn-guest`'s
  API, the provider kit, the scaffold's generated shape — are hard-cut at this
  pin: a bump is when a rename or a changed shape is paid for, and never
  between bumps.
- **The Rust toolchain.** `rust.toolchain(...)` with brenn's `RUST_VERSION`
  and `extra_target_triples = ["wasm32-unknown-unknown"]`.
- **The `fltk` override**, at brenn's commit. fltk is in no registry, and a
  dependency's `git_override` is ignored.
- **The `aspect_rules_js` override.** Nothing here ships a surface kind, so
  nothing runs brenn's jco; it is stated because it is a consumer's fact, and
  a surface component added later needs it.
- **The fltk serde flag** in `.bazelrc`,
  `--@fltk//crates/fltk-serde-core:serde=@brenn//brenn-dsl:fltk_serde`.
  Without it brenn's `dsl_cli` fails to compile with mismatched-serde-instance
  errors.

And one naming rule: every crate hub is `fastmail_*`; rules_rust refuses two
hubs of one name across modules.

Three pins are copied from brenn by hand and nothing holds them equal
mechanically: `.bazelversion`, `RUST_VERSION` in `MODULE.bazel`, and the fltk
commit. Re-copy all three when the brenn pin moves. `//tools/pins:pins_test`
holds this repository's own copies of the Rust version and its version number
equal across its manifests.

### Working against a brenn the public remote does not serve

When the pin is ahead of brenn's public `main`, or brenn is being changed
locally, write one line into the gitignored `.bazelrc.user` at the repository
root:

```
common --override_module=brenn=/path/to/brenn
```

Every `bazel` invocation reads it: the Makefile's, the git hooks', the Claude
Code write hook's and your own. Without it, each of them fails at module
resolution while the pin is unpublished — including the pre-commit hook, so no
commit is possible. Never edit the pin instead, never put the line in CI, and
never call this repository green on an overridden run: an override makes the
pin irrelevant. `make check` prints a notice while the line is in force.

## Layout

- `MODULE.bazel`, `.bazelrc`, `.bazelversion` — the module graph, the brenn
  pin and the consumer's facts above.
- `jmap/` — `fastmail-jmap`, the JMAP crate. No I/O: it builds request bodies
  and reads answers, and is compiled for the host (its tests) and for wasm32
  (inside a component). `src/method.rs` holds what every data type's
  standard methods share, `src/set.rs` what every `/set` shares (its
  arguments, its result and the patch), `src/mail.rs` the mail methods,
  `src/contacts.rs` the contacts methods, the JSContact card reader, and the
  cards and patches the contacts write tools send.
- `dav/` — `fastmail-dav`, the WebDAV and CalDAV crate, `jmap/`'s
  counterpart with the same build shape. No I/O: `src/href.rs` turns a
  server's href into a path brenn will send, `src/multistatus.rs` reads a
  multi-status answer (through `roxmltree`, private to the crate),
  `src/caldav.rs` holds the request bodies and their typed readers, and
  `src/ical.rs` reads iCalendar by hand: content lines, parameters, TEXT
  values and the shape of a date. Its fixtures are under `dav/fixtures/`.
- `provider/` — `fastmail-provider`, the plumbing the provider components
  share: settings, the `account`, id, keyword and UTC date arguments, the
  session step and the API call, the DAV call and the mapping of a DAV
  answer's status (`src/dav.rs`), result bounds including `cut_width`, and
  the mapping of Fastmail's answers onto the request's failure. One target
  serves the components' wasm32 builds and their host tests; its `testutil`
  module carries those tests' fixtures.
- `spec/` — the components' authored specifications, the module root a
  deployment's `use @fastmail-mail-read::*;`,
  `use @fastmail-mail-write::*;`, `use @fastmail-contacts::*;`,
  `use @fastmail-contacts-write::*;` and `use @fastmail-calendar::*;`
  resolve against.
- `fastmail-mail-read/` — the read provider: `src/logic.rs` (arguments, JMAP
  requests, answers and compaction, all host-testable), `src/html.rs` (HTML
  parts as plain text) and `src/lib.rs` (the guest glue).
- `fastmail-mail-write/` — the write provider: `src/logic.rs` (organize and
  the mailbox tools, the step machine, all host-testable), `src/draft.rs`
  (the draft tool's arguments and three steps) and `src/lib.rs` (the guest
  glue).
- `fastmail-contacts/` — the contacts provider: `src/logic.rs` (arguments,
  the step machine, requests and failures, all host-testable), `src/card.rs`
  (a card as a search summary and as a full answer, with every list cap and
  string bound) and `src/lib.rs` (the guest glue).
- `fastmail-contacts-write/` — the contacts write provider: `src/logic.rs`
  (the create and update arguments, the step machine, the update's patch
  from its read, requests and failures, all host-testable) and `src/lib.rs`
  (the guest glue).
- `fastmail-calendar/` — the calendar provider, over CalDAV: `src/logic.rs`
  (arguments, the step machine, requests and failures, all host-testable),
  `src/event.rs` (an event as a window's occurrence and as a full answer,
  with every cap) and `src/lib.rs` (the guest glue).
- `config/example.brenn` — the deployer's document: the five key providers,
  the five instances with their clauses and an agent granted the tools.
  `//:example_fit` compiles it and `//:config_check` lowers it, so the
  example's shape and settings cannot drift from what brenn accepts; neither
  holds the agent's `tool` grants to the tools the instances provide, which
  are checked by hand against the specifications' `provide` names. The
  README's excerpts are copied from it by hand,
  and no target compares them with it.
- `deployed-components.txt` — the packages the bundle ships, held equal to
  the `component_package` targets by `//:deployed_components_test`.
- `tools/config_check.sh` — `brenn config-check` over the example document.
- `tools/scrub/` — `brenn-scrub` and the gitleaks release it pins, run by
  every gate. `tools/allthechanges/` — the changelog and TODO fragment tool.
- `tools/pins/` — a test holding the version pins that appear in more than
  one file equal, and a test that it fails when any one of them diverges.
- `docs/` — this file.
- `.githooks/`, `.claude/`, `.gitleaks.toml` — the scrub kit: the pre-commit
  and pre-push hooks, the Claude Code write hook, the public scan rules.
- `changelog.d/`, `todo.d/` — pending changelog and TODO fragments, gathered
  into `CHANGELOG.md` and `TODO.md` at release.

## Lock files

- `MODULE.bazel.lock` pins the Bazel module graph.
- `Cargo.lock` and `cargo-bazel-lock.json` pin the workspace's hub,
  `fastmail_crates`: `roxmltree` and `memchr`, which `dav/` links.
- `tools/scrub/Cargo.lock` and `tools/scrub/cargo-bazel-lock.json` pin the
  scanner's crates (`fastmail_scrub_crates`); `tools/allthechanges/Cargo.lock`
  and `tools/allthechanges/cargo-bazel-lock.json` the fragment tool's
  (`fastmail_allthechanges_crates`).

`make repin` re-renders every crate hub's locks after a manifest change or a
tool bump.
