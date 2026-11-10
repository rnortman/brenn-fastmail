# Changelog

Notable changes to brenn-fastmail. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-11-10

### Added

- A fifth component, `fastmail-calendar`, lets the agent read the account's
  calendars over CalDAV. It has three read-only tools. `FastmailCalendars`
  lists the calendars. `FastmailCalendarEvents` lists the events of one
  calendar that overlap a window given as dates or UTC instants, each
  recurring event expanded by Fastmail into its occurrences, their times in
  UTC. `FastmailCalendarEvent` answers one event in full: its time in its own
  zone, description, organizer, attendees and their responses, recurrence
  rules and changed occurrences. Nothing in the component writes to the
  account.
- The calendar component needs an app password per account, limited to
  calendars, written as a `Basic` header value. The README and
  `config/example.brenn` put it in a fifth key provider, `fastmail-calendar`,
  on `caldav.fastmail.com`, and give the instance clauses that admit
  `PROPFIND`, `REPORT` and `GET` on Fastmail's CalDAV paths and no writing
  method. The README's setup gains a check, with `curl`, of every request the
  component sends.
- The calendar component needs a brenn whose `api-request` carries
  `PROPFIND`, `REPORT` and paths ending in `/`, which no brenn release does
  yet. It has not met a live Fastmail account. Installing it takes a restart.
- A sixth component, `fastmail-calendar-write`, lets the agent change the
  account's calendars over CalDAV. It has three tools.
  `FastmailCalendarEventCreate` puts a new event on a calendar: all-day, at
  UTC instants, or at local times in a named zone, optionally repeating by up
  to three `RRULE` lines. `FastmailCalendarEventUpdate` changes an event's
  summary, description, location or time, writing every other line of the
  stored event back as it was read. `FastmailCalendarEventDelete` deletes an
  event, a recurring one whole only with `series: true`. A create never
  replaces a stored event, and an update or a delete applies only if the
  event is still the one that was read.
- The calendar write component needs its own app password per account,
  allowed to write calendars. The README and `config/example.brenn` put it in
  a sixth key provider, `fastmail-calendar-write`, and give the instance
  three clauses, `GET`, `PUT` and `DELETE` of an event path. Deletion is a
  tool and a clause of its own: a deployment that drops both creates and
  changes events without deleting them. The README's setup gains a check,
  with `curl`, of every kind of request the component sends.
- No calendar write sends mail. An event with an organizer or attendees is
  refused, and so are a recurring event's time and rule and any one
  occurrence of a series; those are changed in Fastmail. An event too large
  for one call (over 60,000 bytes) is refused before anything is written.
- The calendar write component has not met a live Fastmail account.
  Installing it takes a restart.
- A third component, `fastmail-contacts`, lets the agent read the account's
  contacts. It has three read-only tools. `FastmailAddressBooks` lists the
  address books. `FastmailContactSearch` finds contacts by text, name, email,
  phone, organization, address book, uid or kind, a page at a time, sorted by
  surname then given name, each with its name, preferred email and phone and
  an organization. `FastmailContactRead` answers one contact in full: names,
  nicknames, organizations and titles, emails, phones, online services,
  addresses, anniversaries, notes and a group's members. Nothing in the
  component writes to the account.
- The contacts component needs its own API token per account with read-only
  access to contacts. The README and `config/example.brenn` put it in a third
  key provider, `fastmail-contacts`. The token's session must offer contacts
  and need not offer mail; the README's setup gains a check that Fastmail
  answers the search's query. Installing the component takes a restart.
- A fourth component, `fastmail-contacts-write`, lets the agent create and
  update the account's contacts. `FastmailContactCreate` makes a contact in
  one address book with a name, its given name and surname, and bare lists
  of emails, phones, online services, addresses, organizations, titles,
  nicknames and notes, and a birthday. `FastmailContactUpdate` changes a
  contact's name, adds it to or removes it from address books, and adds,
  changes or removes one entry per call (an email with its label, contexts
  and preference, a phone with its features, and so on). An update writes
  only what it names: every other entry, field and property of the contact,
  including what these tools never show, is kept. It is refused, changing
  nothing, if any contact changed between its read and its write. Nothing
  in the component deletes a contact or writes an address book; removing
  an entry cannot be undone from Fastmail.
- The contacts write component needs its own API token per account with
  read-write access to contacts. The README and `config/example.brenn` put
  it in a fourth key provider, `fastmail-contacts-write`; the README's setup
  gains a check of the requests the component sends, run against a scratch
  contact. Installing the component takes a restart.
- `FastmailContactRead` now starts each entry of a contact's lists with its
  `key`, the name `FastmailContactUpdate` changes or removes it by, and
  answers `nicknames` as objects with `key` and `name`.
- A second component, `fastmail-mail-write`, lets the agent change the
  account, not just read it. It has four tools. `FastmailOrganize` files
  messages: it adds them to or removes them from mailboxes (which is how a
  message is moved or trashed) and sets or clears keywords such as read and
  flagged, reporting the outcome for each message. `FastmailMailboxCreate` and
  `FastmailMailboxUpdate` create, rename and move mailboxes. `FastmailDraft`
  saves a draft, or a reply threaded to an existing message, in Drafts, sent
  from one of the account's sending identities, for the user to review and
  send from Fastmail. Nothing in the component sends mail, deletes a mailbox
  or edits or deletes an existing draft.
- The write component needs its own API token with read-write mail access,
  and a Fastmail session that offers mail submission. The README and
  `config/example.brenn` put it in a second key provider, `fastmail-write`,
  next to the read component's read-only one. Installing the component takes
  a restart.

### Changed

- The bundle builds against brenn `227bedb`, whose provider kit carries a
  binary request body and reads an answer's bytes. The four JMAP components
  send and read what they did before.

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
