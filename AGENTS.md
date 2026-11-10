# brenn-fastmail — agent instructions

## What this is

Fastmail tooling for Brenn: provider components that reach Fastmail over JMAP
(RFC 8620, RFC 8621) and CalDAV (RFC 4791) through Brenn's `api-request` tool, built outside brenn's
repository with brenn's component rules and packaged as a bundle a deployment
installs beside brenn's release. `README.md` is the project's page and the
deployer's runbook; read it first. `docs/developing.md` is the developer's
guide: building, the gate, the layout.

`jmap/` is the transport-free JMAP crate: request building and response
reading, no HTTP, no credential; `dav/` is its WebDAV and CalDAV
counterpart. `fastmail-mail-read/`,
`fastmail-mail-write/`, `fastmail-contacts/` and `fastmail-contacts-write/` are
the JMAP providers and `fastmail-calendar/` the CalDAV one, `provider/` the
plumbing they share,
their specifications in `spec/`.
`config/example.brenn` is the deployer's document the README quotes;
`//:example_fit` compiles it and `//:config_check` lowers it, holding its
shape and settings to what brenn accepts; neither holds the agent's `tool`
grants to the tools the instances provide, so a change to a specification's
tools or settings is made there in the same commit, and a grant is checked by
hand against the specifications' `provide` names.

## Build: Bazel, and only Bazel

Bazel is the build. The Makefile is a frontend whose every target is a
spelled-out `bazel` invocation (`release` also edits the two version manifests
with `sed`); there is no target that runs `cargo`, and the gate never will.

- Every toolchain and every tool the gate runs is pinned in `MODULE.bazel` and
  fetched by checksum. The only host prerequisites are bazelisk (as `bazel`),
  git and make.
- Nothing in a build or test action may reach for a host-installed program.
  A test that needs a service, a fixture generator or a linter gets it as a
  Bazel dependency. If a tool is genuinely impractical to bring in through
  Bazel, pick a different tool, and if that is not possible either, surface it
  as a decision rather than adding a host dependency.
- `.bazelrc` runs build and test actions sandboxed, without network. The one
  exception is `//:buildifier_test`, which runs unsandboxed because it walks
  the whole workspace. A target that needs either is a decision to surface,
  not a tag to add quietly. The gate does not check the posture: a tag that
  arrives through a brenn macro at a pin bump, or a host whose Bazel falls
  back to a sandbox without network isolation, passes `make check` unnoticed.
- `Cargo.toml` + `Cargo.lock` exist for `crate_universe` only. The workspace
  hub, `fastmail_crates`, holds `roxmltree` and `memchr` for `dav/`; every
  crate takes `serde` and `serde_json` from brenn's guest instance, and a
  crate that enables a `serde` feature must not be added. Two more hubs pin build tools, each with its
  own `Cargo.lock` and `cargo-bazel-lock.json`: `tools/scrub/` (the secret
  scanner) and `tools/allthechanges/` (the changelog and TODO fragment tool).
  Every hub is named `fastmail_*` so it can collide with nothing brenn owns.
- `make repin` re-renders all three hubs at once. After any manifest change or
  tool bump, run it and commit every lock file that changed.
- `RUST_VERSION` in `MODULE.bazel`, the channel in `rust-toolchain.toml`,
  `rust-version` in `Cargo.toml` and `msrv` in `clippy.toml` are held equal by
  `//tools/pins:pins_test`; so are the version in `Cargo.toml`
  `[workspace.package]` and `module(version = …)` in `MODULE.bazel`.
  `//tools/pins:pins_mismatch_test` proves the test fails when any one pin
  diverges. `rust-toolchain.toml` only serves rust-analyzer.

## The brenn pin

`MODULE.bazel` pins the brenn commit every rule, macro and host crate comes
from. Build-time contracts are hard-cut there: a macro rename or a changed
generated shape is paid for when the pin moves, never between bumps. The
provider kit's contract at the pin is what the components build against.

The pin must be **served by brenn's public remote**, because the fetch is
anonymous. A commit living only in a local checkout fails module resolution
for every `bazel` invocation — the Makefile's, the git hooks', the write
hook's.

Work against a brenn the public remote does not serve with one line in the
gitignored `.bazelrc.user`:

```
common --override_module=brenn=/path/to/brenn
```

That is the only mechanism, and it is what lets the git hooks and the write
hook run against an unpublished brenn: every `bazel` invocation reads the rc
chain. Never edit the pin to work locally, never put the line in CI, and
never call this repository green on an overridden run — an override makes the
pin irrelevant. `make check` prints a notice while the line is in force.

`MODULE.bazel` also restates facts bzlmod reads from the root module only, so
brenn cannot supply them: the Rust toolchain, the `fltk` override, the
`aspect_rules_js` override and the hub names. `.bazelversion`, `RUST_VERSION`
and the fltk commit are copied from brenn by hand with nothing holding them
equal; re-copy all three when the pin moves.

## Commands

| Command | What it does |
|---|---|
| `make check` | The gate: scrub kit check, fragment lint, build, every test, clippy, rustfmt, buildifier, the wasm32 clippy build, tree scan |
| `make build` | Build every target |
| `make test` | Every test, without the lint aspects (the inner loop) |
| `make bundle` | Build the deployable tree (`components/`, `modules/`) |
| `make bundle-dir` | Print that tree's absolute path |
| `make release VERSION=X.Y.Z` | Cut a release: gather the changelog and TODO fragments, bump the version, repin |
| `make todo-gather` | Fold `todo.d/` fragments into `TODO.md`, outside a release |
| `make changes ARGS=...` | Run allthechanges: `ARGS="query"`, `ARGS="todo query"`, `ARGS="gather --version X.Y.Z --dry-run"` |
| `make fix` | rustfmt and buildifier the tree in place |
| `make repin` | Re-render every crate hub's lock files |
| `make ide` | Write `rust-project.json` for rust-analyzer |
| `make setup` | Wire git at `.githooks` (once per clone) |
| `make scrub-tree` | Whole-tree secret sweep |
| `make clean` | Drop Bazel's outputs for this workspace |

## Gates

- A commit scans the staged change for secrets and then runs `make check`,
  which also lints every `changelog.d/` and `todo.d/` fragment. It takes
  minutes; give a commit a long timeout.
- A push scans the range being pushed.
- CI runs `make check` on every push and pull request, with the module
  lockfile in error mode, so a resolution nobody committed is a red build.
- The Claude Code write hook scrubs every write before it lands and blocks on
  any failure, including a scanner that fails to start.

Every scan runs `bazel run //tools/scrub:brenn-scrub`. Treat the local hooks
as the gate and CI as a backstop, not the other way round: a CI run says
nothing about the commit sitting in your working tree.

If a gate blocks a write or a commit, **surface it**. Never route around it:
no `--no-verify`, no unwiring `core.hooksPath`. The gate being wrong is a
thing that happens and is worth reporting; a bypassed gate is not recoverable
once it has been pushed.

## Principles

- Fail fast. Invalid input is rejected, not coerced. No fallbacks, no
  swallowed errors.
- No over-engineering, no speculative types: a JMAP method gets its argument
  builder and reader when a tool uses it. Scope discipline: do what was asked.

## Code standards

- Rust edition 2024, the pinned stable toolchain.
- No unused code. No backwards-compatibility shims.
- `make check` must pass: clippy with warnings denied, rustfmt, buildifier,
  every test.
- Comments follow brenn's `docs/comment-standard.md`: no references to
  documents a public reader cannot open, no narration; generic names
  (`alice`, `example.com`) in comments, examples and fixtures. Cite RFCs as
  `RFC 8621 section 4.4.1`.
- Default branch: `main`.

## Changelog

Every user-visible change adds a fragment, `changelog.d/<slug>.<section>.md`,
where `<section>` is one of `added`, `changed`, `deprecated`, `removed`,
`fixed`, `security`. A fragment is a Markdown list: its first line starts with
`- `, continuation lines are indented two spaces. Never edit `CHANGELOG.md`;
`make release` gathers the fragments into it. `make changes ARGS="query"` lints
and lists what is pending.

## TODO system

Two pieces that stay in sync:

- The master list: `TODO.md` at the repo root plus the fragments under
  `todo.d/` not yet gathered into it. Each entry has a slug, a description,
  and the deferral context.
- `TODO(slug)` comments in code, marking the spot where the work happens.

A new entry is a fragment, `todo.d/<slug>.md`: its first line is text, it has
no ATX heading outside a code fence, and every fence is closed. Never add an
entry by editing `TODO.md`. `make todo-gather` folds the fragments into
`TODO.md` at any time, `make release` at release time; between gathers
`make changes ARGS="todo query"` lists them. Removing a done TODO deletes its
fragment if it still is one, else its `` ## `<slug>` `` entry in `TODO.md`, and
its comment.

Slugs are the join key; adding a TODO requires both halves. Don't use TODOs for
vague aspirations. Every TODO describes a concrete thing, in a place where
"done" is obvious.

## Releasing

On a branch, run `make release VERSION=X.Y.Z` on a clean tree and review the
diff. Commit the gathered `CHANGELOG.md` and `TODO.md`, the deleted
fragments, both version manifests (`Cargo.toml`, `MODULE.bazel`) and every
lock file that changed. Land it, then tag the landed `main` commit `vX.Y.Z`
and push the tag.

## Public tree

Everything tracked here is published. `TODO.md`, `CHANGELOG.md`, their
fragments under `todo.d/` and `changelog.d/`, commit messages and every code
comment are public writing: no internal host names, no absolute paths into
other trees, no references to documents a public reader cannot open. Design
documents live outside this repository and are never cited from code.
`brenn-scrub` is the gate.
