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
