# TODO

The master list of deferred work. Each entry has a slug, a description, and the
reason it was deferred; the matching `TODO(slug)` comment in code marks the spot
where the work happens. Slugs are the join key: adding a TODO requires both
halves, and removing one removes both.

A new entry is never written here: it is a fragment, `todo.d/<slug>.md`, which
`make todo-gather` or `make release` folds in below as a `` ## `<slug>` ``
entry. Between gathers the open list is this file plus `todo.d/`;
`make changes ARGS="todo query"` lists the fragments.

Every entry is public writing.

## `blob-transfer`

Attachment download and blob upload. Fastmail serves blobs from a download
host other than the JMAP API host, so a provider that fetches an attachment
needs that host as a second `api_hosts` entry on the key provider; that is
later work. The provider kit carries a binary body on a call and reads an
answer's bytes, which no tool here uses yet. Until then `FastmailRead` lists each attachment's
metadata and fetches none. Brenn also bounds a call's body at 64 KiB of
arguments and an answer at 512 KiB, so attachment download and blob upload are
limited to small blobs until brenn's tracker entry
`api-request-body-caps-bound-file-transfer` is resolved. The
`TODO(blob-transfer)` comment sits at the attachment emitter in
`fastmail-mail-read/src/logic.rs`.

## `brenn-pin-on-branch-commit`

`MODULE.bazel` pins a brenn commit that brenn's public remote serves from a
branch, not from `main`. If that branch is deleted or rewritten before `main`
carries the commit, the anonymous fetch may fail and every `bazel` invocation
with it. Done = the pin names a commit on brenn's `main`. The comment sits at
the pin in `MODULE.bazel`.

## `bundle-carries-no-licence-texts`

The bundle a deployment installs holds components and specifications and no
licence text: not this project's `LICENSE` and `NOTICE`, and none for the
crates its components link — `serde`, `serde_json` and the rest of brenn's
guest crates, and from this repository's own hub `roxmltree` and `memchr`.
Brenn's bundle rule stages `components/`, `surface/` and `modules/` and
nothing else, so carrying a licence file takes a change to that rule in brenn,
or a decision to ship the texts beside the bundle. Done = a deployer who
installs the bundle has the licence texts of everything in it, or a recorded
decision that the source repository's files suffice. The
`TODO(bundle-carries-no-licence-texts)` comment sits at the bundle target in
`BUILD.bazel`.

## `calendar-event-delete-unruled`

`FastmailCalendarEventDelete` removes an event from a calendar, and a
recurring series whole with `series: true`. Over CalDAV the event is then
gone; this repository has no way to bring it back. The tool was built, and the
example configuration grants it, without the account owner having said an
agent should hold it, on the precedent of brenn's `CalendarDelete`, which does
the same on Google under the same series rule. It is a tool of its own, so a
deployment that does not grant it and drops the `DELETE` clause has create and
update without it. The question: keep it as built and granted; keep it out of
the example's grants so that a deployer opts in; or remove it.
`contact-update-entry-removal-unruled` asks the same of contacts and the two
are best answered together. Done = the owner's choice recorded here and the
tool, the example and the README matching it. The
`TODO(calendar-event-delete-unruled)` comment sits where the delete tool
builds its `DELETE`, in `fastmail-calendar-write/src/logic.rs`.

## `calendar-read-rfc-citations-unchecked`

The RFC sections cited by comments, tool descriptions and tracker fragments of
the calendar write component were each compared with the fetched RFC text. The
citations that came with the calendar read component and the DAV crate before
it — in `dav/src/caldav.rs`, `dav/src/multistatus.rs`, `dav/src/href.rs`, the
reader half of `dav/src/ical.rs`, `fastmail-calendar/src/`,
`provider/src/dav.rs` where unchanged, and `spec/fastmail-calendar.brenn` —
were not part of that check, and nothing records that anyone read them
against the text. Done = each RFC section those files cite compared with the
RFC as published, any wrong citation or behaviour built on one fixed with its
test, or a recorded decision that the check is not wanted. The
`TODO(calendar-read-rfc-citations-unchecked)` comment sits at the head of
`dav/src/caldav.rs`.

## `calendar-unconfirmed-against-live-fastmail`

The calendar component is written from RFC 4791, RFC 5397 and Fastmail's help
pages and has not met a live account. Unconfirmed: that an app password in a
`Basic` header is accepted at `caldav.fastmail.com`; that
`PROPFIND /dav/principals/` answers the credential's `current-user-principal`,
and that principal its `calendar-home-set`; that those paths have the shapes
the example's clauses admit (`/dav/principals/user/<address>/`,
`/dav/calendars/user/<address>/`); that Fastmail answers a time-range
`calendar-query` with each recurring event expanded into its occurrences,
their times in UTC, when `calendar-data` names both a property subset and
`expand`; and whether an app password can be limited to reading. Done = the
README's Fastmail setup step 7 run against an account and its result recorded
here, with whatever the component needed changed. The
`TODO(calendar-unconfirmed-against-live-fastmail)` comment sits at the window
query's request builder in `dav/src/caldav.rs`.

## `calendar-window-one-calendar-per-call`

`FastmailCalendarEvents` reads one calendar per call: an agent that wants a day across a user's calendars lists them with `FastmailCalendars` and asks for each. A window over several calendars in one call is one `REPORT` per calendar in sequence, each under its own read limit, and needs rules this tool does not have: what the answer says when one calendar could not be read or its answer was cut, how `limit` and the order apply across calendars, and how many calendars one call may read. Done = a window tool that reads several calendars under stated rules for a partial read, or a recorded decision that one calendar per call stays. The `TODO(calendar-window-one-calendar-per-call)` comment sits at the window tool's argument parser in `fastmail-calendar/src/logic.rs`.

## `calendar-write-bounded-by-call-argument-cap`

`FastmailCalendarEventUpdate` writes an event back whole, and Brenn bounds a
nested call's arguments at 64 KiB, so an event whose iCalendar text is over
60,000 bytes as the call carries it (escaped as JSON: each line end counts
four bytes, each `"` and `\` two) — one with many changed occurrences or an
inline attachment — is refused and is changed in Fastmail. Lifting the bound
waits on brenn's tracker entry `api-request-body-caps-bound-file-transfer`.
Done = an update writes back any event the read limit lets it read, or a
recorded decision that the bound stays. The
`TODO(calendar-write-bounded-by-call-argument-cap)` comment sits at the bound
in `fastmail-calendar-write/src/logic.rs`.

## `calendar-write-no-single-occurrence-edit`

`FastmailCalendarEventUpdate` changes a recurring event's summary,
description and location on the series and refuses to change its time or
rule; no tool changes or deletes one occurrence. A change to the series' text
does not reach an occurrence that was already changed, which keeps its own.
`FastmailCalendarEventCreate` takes `RRULE` lines only, no `RDATE` or
`EXDATE`. One occurrence is written as an override component or an `EXDATE`
that names it in the series' own `DTSTART` form. `FastmailCalendarEvents`
shows an occurrence as a UTC instant and never in the event's time zone — a
limit of the read tool that was itself a default, not a requirement of CalDAV
— and this repository converts no time between zones, so the write tools
cannot name the occurrence. Lifting the limit therefore starts on the read
side: the window tool gives each occurrence in its event's zone, or the
component learns to convert. Done = one occurrence can be changed and
deleted, and a series' time and rule changed, with overrides kept valid; the
limits stay only by the account owner's choice, recorded here. The
`TODO(calendar-write-no-single-occurrence-edit)` comment sits at the refusal
in `fastmail-calendar-write/src/logic.rs`.

## `calendar-write-refuses-events-with-attendees`

The calendar write tools create no event with attendees and refuse to change
or delete an event that has an `ORGANIZER` or an `ATTENDEE`. A CalDAV server
with implicit scheduling (RFC 6638) mails invitations, updates, cancellations
and replies from the user's account when such an event is stored, changed or
deleted, and no tool here sends mail. So an agent cannot move or cancel a
meeting, only an event that is the user's alone. The refusal is a default
nobody who owns an account has confirmed, and it is a question for the owner:
let the server tell the other people, as it does when the user edits the
event in Fastmail, behind an argument that defaults to telling nobody; change
the event while marking it so the server tells nobody, which alters how the
event is stored; or keep the refusal. Done = the owner's choice recorded here,
and the write tools handling scheduled events under it — the refusal stays
only by that choice. The `TODO(calendar-write-refuses-events-with-attendees)`
comment sits at the check that refuses, in
`fastmail-calendar-write/src/resource.rs`.

## `calendar-write-unconfirmed-against-live-fastmail`

The calendar write component is written from RFC 4791, RFC 5545 and
Fastmail's help pages and has not met a live account. Unconfirmed: that
Fastmail honours `If-None-Match: *` and `If-Match` on a calendar resource
and answers a stale one `412`; that a `GET` of an event answers an `ETag`;
that an event written with a `TZID` and no `VTIMEZONE` is accepted and
shown in that zone — RFC 7809 allows it only to a server that advertises
`calendar-no-timezone` in answer to `OPTIONS`, a method Brenn's `api-request`
does not carry, so the component cannot check; if Fastmail refuses it,
the alternatives are a floating local time or writing the `VTIMEZONE`; that
the entity tag a `GET` answers is a strong one and reaches the component;
that a new event is stored at the path it was `PUT` to; that a `PUT` to a
calendar that does not exist answers 404; that an event without `ORGANIZER`
or `ATTENDEE` causes no mail; and whether an app password can be limited to
calendars. Done = the README's Fastmail setup step 8 run against an account
and its result recorded here, with whatever the component needed changed. The
`TODO(calendar-write-unconfirmed-against-live-fastmail)` comment sits where
the create tool builds its event, in `fastmail-calendar-write/src/logic.rs`.

## `contact-update-entry-removal-unruled`

`FastmailContactUpdate` removes one entry of a contact (a `key` with
`remove: true`), which cannot be undone from Fastmail. It is the only tool in
this repository that deletes account data for good: the mail write tools
destroy nothing. The removal was built on a default that the account's owner
has not confirmed. The question is whether an agent should hold it. Keep it:
replacing a stale number or dropping a wrong address needs it, an in-place
edit already overwrites a value for good, and a deployer gates the tool by its
grant and by issuing the read-write contacts token. Take it out: it is one
argument of one tool; entries can then be added and edited, and removed only
in Fastmail's own editor. Or move it into a tool of its own, so a deployment
can grant editing without removal. Done = the owner's choice recorded here and
the tool matching it. The `TODO(contact-update-entry-removal-unruled)` comment
sits where the update's arguments read `remove`, in
`fastmail-contacts-write/src/logic.rs`.

## `contacts-query-unconfirmed-against-live-fastmail`

No test reaches a live Fastmail account, so two things `fastmail-contacts`
depends on are unconfirmed: that Fastmail names its contact methods as
RFC 9610 does, and that it sorts `ContactCard/query` by `name/surname` then
`name/given`, which RFC 9610 section 3.3.2 only recommends; an
`unsupportedSort` answer makes `FastmailContactSearch` refuse every search.
Run the README's Fastmail setup step 5 with a contacts token and record the
answer. Done is the recorded answer and either no change or a search that
sorts by a property Fastmail supports. The
`TODO(contacts-query-unconfirmed-against-live-fastmail)` comment sits at the
search's sort in `fastmail-contacts/src/logic.rs`.

## `contacts-set-unconfirmed-against-live-fastmail`

No test reaches a live Fastmail account, so what `fastmail-contacts-write`
depends on is unconfirmed: that Fastmail accepts `ContactCard/set` create and
update from a token with read-write access to contacts; that it accepts a
created card's `uid` and entry keys and keeps them; that it accepts
`ifInState` and the patch paths an update sends (`<property>/<key>`,
`<property>/<key>/<sub-property>`, a whole property, `name/full`,
`name/components`, `addressBookIds/<id>`, and `null` at an entry or an
address-book path); what `name` it stores for a card
created with `full` and `components`; which `SetError` types it answers
with; and that a name and an address written as full text show in
Fastmail's own editor. Run the README's Fastmail setup step 6 against a
scratch contact and record the answer. Done is the recorded answer and
either no change or a component that sends what Fastmail accepts. The
`TODO(contacts-set-unconfirmed-against-live-fastmail)` comment sits where
the update builds its patch, in `fastmail-contacts-write/src/logic.rs`.

## `contacts-write-flat-arguments-limit-what-is-set`

The contacts write tools take flat arguments, because the provider kit's
argument schema bounds scalars and arrays of scalars and has no object type.
So `FastmailContactCreate` takes bare values and one address book (labels,
contexts, features, preference, a title's kind, an online service's `service`
and `user`, and any anniversary but a birthday need a following update), and
`FastmailContactUpdate` changes one entry per call. Neither sets a name's
prefix, middle name or suffix as parts, an address's parts, a title's
organization or an organization's units, and a field of an entry cannot be
cleared in place. Every limit is stated in the tool descriptions. Done = the
provider kit can bound an array of objects and the tools take whole entries,
several per call, or a recorded decision that the flat form stays. The
`TODO(contacts-write-flat-arguments-limit-what-is-set)` comment sits at the
update's argument parser in `fastmail-contacts-write/src/logic.rs`.

## `correctness-implicit-method-responses-unreachable`

A reader for the response to an implicit method call. `Email/copy` with
`onSuccessDestroyOriginal` (RFC 8620 section 5.4) and `EmailSubmission/set`
with `onSuccessUpdateEmail` or `onSuccessDestroyEmail` (RFC 8621 section 7.5)
make the server call `Email/set` itself; its response follows the primary one
under the same call id. `Response::result` reads only the first response for an
id, so a `notUpdated` or `notDestroyed` in the implicit `Email/set`, or an
`error` in its place, is unread today. The first cycle that sends such a call
adds the reader (matching the id and the implicit method's name, an `error`
after the primary response being the implicit call's failure) and decides what
a failed implicit call means to the tool's caller. The
`TODO(correctness-implicit-method-responses-unreachable)` comment sits above
`Response::result` in `jmap/src/response.rs`.

## `correctness-mixed-identity-listing-offers-from-certain-to-fail`

`fastmail-draft`'s "give `from` as one of …" refusals list every distinct
candidate address, including an address that two or more sending identities
share when none of them is the account's main one. Take identities
`alice@example.com`, `ALICE@example.com` and `bob@example.org`, none main:
with `from` omitted the draft is refused with "give `from` as one of
alice@example.com, bob@example.org", and a retry with
`from = "alice@example.com"` meets the shared-address refusal ("2 sending
identities share `alice@example.com` and none is the account's main one … the
user removes the duplicate in Fastmail's settings"), which the instruction
could not have avoided. The same listing is shown when `from` matches no
identity. The omitted-`from` path already answers the shared-address refusal
when every candidate shares one address; this is the mixed case. Done is a
listing that names only addresses a `from` resolves, with the shared-address
sentence for each address left out, and a test holding the listing and the
retry it leads to as one path; whether that per-address rule is worth
carrying for a configuration not yet seen on Fastmail is the decision. The
`TODO(correctness-mixed-identity-listing-offers-from-certain-to-fail)` comment
sits at `listed` in `fastmail-mail-write/src/draft.rs`.

## `correctness-sandbox-network-posture-unenforced`

Make the gate enforce the sandboxed, network-less build posture `.bazelrc`
asks for. Today nothing checks it, and three ordinary paths break it while
`make check` stays green. First, a `requires-network` tag or execution
requirement on any action overrides `--sandbox_default_allow_network=false`.
Second, a test tagged `no-sandbox` or `local` falls back silently to the
`local` strategy under `--strategy=TestRunner=sandboxed,local`, with host
network. A brenn macro instantiated in `BUILD.bazel` could start carrying
such a tag at a pin bump. Third, on a host without Bazel's Linux sandbox
(no unprivileged user namespaces), `sandboxed` resolves to the
processwrapper sandbox, which isolates neither the network nor the source
tree. Candidate mechanisms: a `bazel query` step in `check` that requires
the set of targets tagged `no-sandbox`, `local` or `requires-network` to be
exactly `//:buildifier_test`; a canary `sh_test` that passes only if a TCP
connect through bash's `/dev/tcp` fails, which covers the third path on
whatever host runs the gate, CI included. Other repositories built on the
same scaffold share the posture and the gap, so the choice is best made once
for all of them. Done means: `make check` fails when an action other than
`//:buildifier_test` runs unsandboxed or with network. The
`TODO(correctness-sandbox-network-posture-unenforced)` comment sits above
`check` in the `Makefile`.

## `edit-hook-formats-files-outside-the-workspace`

The Claude Code edit hook `.claude/hooks/format.sh` runs the pinned `rustfmt`
on every Rust file a session writes, a file outside this workspace included.
`rustfmt` takes its edition and style from the `rustfmt.toml` above the file
it formats, so this repository's `rustfmt.toml` reaches only files under it,
and the hook passes no edition of its own. A file in another checkout —
brenn's, when a session started here edits both repositories — is therefore
formatted with rustfmt's defaults, the 2015 style, whatever that checkout's
own gate expects. It has reordered `use` lists in brenn's sources
(`{Value, json}` to `{json, Value}`), which brenn's gate rejects, and the edit
had to be undone by hand before the commit. The buildifier arm reaches a file
outside the workspace the same way; no symptom from it has been seen. Done =
the hook leaves a file outside `$CLAUDE_PROJECT_DIR` alone, or formats it by
its own repository's configuration. The
`TODO(edit-hook-formats-files-outside-the-workspace)` comment sits at the
`case` on the written file's name in `.claude/hooks/format.sh`.

## `notes-message-shape-unsampled`

Fastmail keeps notes as messages in the `Notes` mail folder, in a shape its
help pages do not give, so no notes tool is built: reading that folder already
works through the mail read tools, and a note written in a guessed shape may
not show as a note. What is missing is one sample only an account holder can
take: a note written in Fastmail's own notes screen, fetched over JMAP
(`Email/get` with every header, `bodyStructure`, `keywords` and `mailboxIds`),
private content replaced. Done is the sample's shape recorded here and either
notes tools that write that shape or a recorded reason they cannot be built.
The `TODO(notes-message-shape-unsampled)` comment sits at `set_request` in
`fastmail-mail-write/src/draft.rs`, where the draft's message is built for
`Email/set`.

## `quality-readme-config-excerpts-unguarded-and-doubled`

The README's configuration excerpts (both `api_key_provider` blocks, both
results channels, both component instances and the agent's grants) are copied
by hand from `config/example.brenn`. `//:example_fit` and `//:config_check`
hold the example to what brenn accepts; nothing holds the README to the
example, so a change to a spec setting, a path or an `allow` clause can leave
the deployer's runbook stale with the gate green. Either a Bazel test that
finds every configuration excerpt in `README.md` and asserts it appears in
`config/example.brenn` (ignoring whitespace), or a README that quotes less
and points at the example; which one is the choice to make. The
`TODO(quality-readme-config-excerpts-unguarded-and-doubled)` comment sits at
`//:example_fit` in `BUILD.bazel`.

## `restart-wording-after-key-reload`

The README and the provider's refusal text tell the operator that adding,
removing or rotating an account, or replacing a dead token's or app
password's key file, takes a restart of Brenn, and that a reload changing a
key provider is refused. That is true of Brenn releases up to 0.33.0. A later
Brenn converges `api_key_provider` changes and key files rotated in place on
reload. Once a Brenn release carrying that change exists, the README's
account and token-rotation paragraphs, the 401/403 refusal string and the
calendar component's app-password refusal say "reload" and name the first
Brenn version that converges key providers, so an operator on an older Brenn
still knows to restart. The `TODO(restart-wording-after-key-reload)` comments
sit at the 401/403 refusal and the read-only-token sentence in
`provider/src/failure.rs` and at the app-password refusal in
`provider/src/dav.rs`.

## `restart-wording-after-provider-reload`

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

## `reuse-cut-width-beside-wire-width-in-wrong-crate`

The mail components bound every string they copy out of Fastmail's answer
with the provider kit's `cut`, which counts UTF-8 bytes before JSON
escaping, while the result ceiling counts the answer as serialized: a quote
or newline takes two bytes there and any other control character six. A
page of escape-heavy subjects or previews can therefore be refused by the
ceiling although every string is within its bound. The contacts and calendar
components bound by serialized width with `cut_width` in the provider
crate's `results.rs`. What remains is a decision about two shipped tools:
moving the mail components to serialized-width bounds changes where their
answers cut and what their bounds are sized at; once it is taken, a
width-cutting variant of `failure::optional` serves every component. Done is
the mail components bounding by serialized width through the shared helper,
or a decision that byte bounds stay. The
`TODO(reuse-cut-width-beside-wire-width-in-wrong-crate)` comment sits at the
byte bounds in `fastmail-mail-read/src/logic.rs`.

## `reuse-html-to-text-reimplements-gmail-decoder-and-drops-text`

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

## `sieve-filters-without-jmap-sieve`

Fastmail offers no supported JMAP path for authoring server-side mail filters:
its developer documentation lists no Sieve scope or capability, and the
session of a mail-scoped token, read 2026-11-09 with the README's step-3
`curl`, advertises no `urn:ietf:params:jmap:sieve` (RFC 9661). Filter
authoring is out of this project's scope for now. Routes known, recorded for a
later decision: (1) Fastmail's web session reaches the account's filter script
over JMAP at the same endpoint with the private capabilities
`https://www.fastmail.com/dev/rules` and `https://www.fastmail.com/dev/user`,
methods `SieveBlocks/get` and `SieveBlocks/set` on a `singleton` object whose
`sieveAtStart`, `sieveAtMiddle` and `sieveAtEnd` blocks are editable beside
Fastmail's own `sieveForRules` and `sieveForBlocked`; third-party tools use
it, the API is unpublished, and it is authenticated by a web-session bearer
token and the `__Host-s_` cookie copied from a browser, which brenn's
`api_key_provider` cannot carry today (one key file, one header, cookie
refused) and which a deployer must re-capture when the session expires. (2)
Filtering by an agent on a `brenn-automation` cron with the existing
`FastmailSearch` and `FastmailOrganize` tools, which needs no code and costs
an LLM turn per run. (3) A deterministic filter component in wasm, which needs
brenn to let a component with no acting user write with a credential (brenn's
`consumer-own-credentials`). (4) Semi-automation through the web UI's filter
export and import, which needs brenn's outbound file delivery (brenn's
`agent-file-delivery-beyond-markdown`). Done is a `fastmail-sieve` component
on one of these routes, or a decision that none is wanted. The
`TODO(sieve-filters-without-jmap-sieve)` comment sits beside the capability
constants in `jmap/src/session.rs`.

## `submission-capability-read-write-token`

Confirm, with the write provider's read-write key file and the README's step-3
`curl`, that the session lists `urn:ietf:params:jmap:submission` and a primary
account for it. The only session recorded so far is a mail-scoped read-only
token's, which lists `core` and `mail` alone — a session advertises what its
token's scopes grant — so it does not answer the question, and without the
capability `FastmailDraft` refuses every draft by design. If a read-write
token scoped for submission also lacks it, the draft's From rule needs a
redesign before more draft work. Done is the recorded answer and either no
change or a redesigned `FastmailDraft` From rule. The
`TODO(submission-capability-read-write-token)` comment sits at the submission
refusal in `fastmail-mail-write/src/draft.rs`.
