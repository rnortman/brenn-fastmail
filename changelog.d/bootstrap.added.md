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
  `make check` gate shared by the git hooks and CI.
