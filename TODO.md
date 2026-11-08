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
needs that host as a second `api_hosts` entry on the key provider, and a
binary body on the provider kit's call type, which the kit does not carry
today; both are later work. Until then `FastmailRead` lists each attachment's
metadata and fetches none. The `TODO(blob-transfer)` comment sits at the
attachment emitter in `fastmail-mail-read/src/logic.rs`.

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

## `sieve-capability-check`

Check whether the account's JMAP session advertises
`urn:ietf:params:jmap:sieve` (RFC 9661, JMAP for Sieve Scripts), in bash,
with `<file>` the key file the README's Fastmail setup writes (the header goes
through a file descriptor, so the token stays off curl's command line):

```
curl -s -H @<(printf 'Authorization: %s\n' "$(cat <file>)") \
  https://api.fastmail.com/jmap/session | jq .capabilities
```

If it is present, server-side filter authoring is a provider over
`SieveScript/get`, `SieveScript/set` and `SieveScript/validate`. If it is
absent, JMAP offers no path to server-side filters, and the alternatives are
keyword- or mailbox-driven filtering in a component, or no filter authoring.
The answer decides the shape of the filters provider. The
`TODO(sieve-capability-check)` comment sits beside the capability constants in
`jmap/src/session.rs`.
