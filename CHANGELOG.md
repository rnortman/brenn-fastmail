# Changelog

Notable changes to brenn-fastmail. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-11-08

### Changed

- **Each user now reads their own Fastmail mailbox.** Previously one API
  token served the whole deployment, so every user's agent read the same
  mailbox. The `api_key_provider fastmail` block in a deployment's
  configuration now holds one `account` block per Fastmail account, each
  naming the brenn user it belongs to and its key file, in place of the
  single `key_file`. A user may have several accounts, one marked
  `default = true`. Every tool takes an optional `account` argument to pick
  one; without it, the user's default or only account is read. **Config
  change required** for per-user reading: copy the README's key-provider
  excerpt. The old single-`key_file` block still loads and still gives every
  user's agent the same one mailbox, and a call that names `account`
  against it fails. Requires a brenn release that supports `account` blocks
  in an `api_key_provider`.

## [0.1.0] - 2026-11-08

### Added

- First release: read-only Fastmail mail access for Brenn agents.
  `fastmail-mail-read` is a Brenn provider component (a sandboxed WASM
  plugin that offers tools to agents) with four tools: `FastmailMailboxes`
  lists folders, `FastmailSearch` finds messages, `FastmailRead` reads one
  message as text (HTML converted to plain text; attachments listed, not
  downloaded) and `FastmailThread` shows the newest messages of a
  conversation. None of them can change or send mail.
- Deployment: `make bundle` builds a deployable bundle, and
  `config/example.brenn` is a checked example configuration. The README walks
  through creating a Fastmail API token and deploying the component.
- `fastmail-jmap`, a reusable Rust library for JMAP (Fastmail's mail
  protocol) with no network code of its own: it builds requests and reads
  responses for mailboxes, search, messages and threads, and reports
  Fastmail's errors in Fastmail's own words.
- The repository itself: a Bazel build pinned to a specific brenn commit, a
  secret scanner (`brenn-scrub`) on every write, commit and push, and one
  `make check` gate shared by the git hooks and CI. Build and test actions
  are configured to run sandboxed and without network access, except the
  formatting check for Bazel files, which reads the whole tree and runs
  unsandboxed. `docs/developing.md` is the developer's guide.
