# brenn-fastmail

Fastmail tooling for [Brenn](https://github.com/rnortman/brenn): provider
components, built outside brenn's repository with brenn's component rules,
that let a Brenn agent reach a Fastmail account over JMAP (RFC 8620,
RFC 8621) through Brenn's `api-request` tool. The account's API token lives in
the deployment's key file and never reaches a component.

The repository holds one component, `fastmail-mail-read`, which gives an
agent four read-only tools: `FastmailMailboxes` (the account's mailboxes),
`FastmailSearch` (messages by sender, recipient, subject, text, mailbox, date
or keyword), `FastmailRead` (one message's headers and text, attachments
listed) and `FastmailThread` (a conversation's newest messages). Nothing it
does writes to the account. It is built on `fastmail-jmap`, a transport-free
JMAP crate: the session object, the request envelope with result references,
method responses read by call id, and the read-only mail methods
(`Mailbox/get`, `Email/query`, `Email/get`, `Thread/get`).

## Status

One component, `fastmail-mail-read`, with four read-only tools. It sends no
mail and changes nothing in the account, has no Sieve support, and lists
attachments without fetching them. Nothing is released yet: there is no tag.
The tree is built and tested against a pinned brenn commit; no test reaches
a live Fastmail account, so the checks in Fastmail setup below are the
operator's to run.

## Getting started

1. Install bazelisk, with git and make the only host prerequisites
   ([Building](docs/developing.md#building)).
2. Create a Fastmail API token and confirm it reaches the account
   ([Fastmail setup](#fastmail-setup), steps 1–3).
3. Build the tree with `make bundle` and install it as a mount
   ([Adding the bundle to a deployment](#adding-the-bundle-to-a-deployment)).
4. Add the configuration and run `brenn config-check` on it before
   restarting (the same section, its last paragraph).

## Fastmail setup

1. In Fastmail's settings, under Privacy & Security → Integrations → API
   tokens, create a token with read-only access to mail.
2. Write `Bearer <token>` — the word, one space, the token, nothing else — to
   a file owned by the user brenn runs as, mode `0600`. Brenn sends the
   file's content verbatim as the `Authorization` header and refuses a key
   holding anything but visible ASCII and spaces.
3. Confirm the token reaches the account and the session lists mail. The
   commands below are bash, and hand curl the header through a file
   descriptor, so the token never appears on curl's command line, where
   other local users and command-line audit logs could read it:

   ```
   curl -s -H @<(printf 'Authorization: %s\n' "$(cat <file>)") \
     https://api.fastmail.com/jmap/session \
     | jq '.primaryAccounts, (.capabilities | keys)'
   ```

   `primaryAccounts` must name an account for `urn:ietf:params:jmap:mail`.
4. Confirm the API path, which the component sends without a trailing slash:

   ```
   curl -s -o /dev/null -w '%{http_code}\n' -X POST \
     -H @<(printf 'Authorization: %s\n' "$(cat <file>)") \
     -H 'Content-Type: application/json' \
     --data '{"using":["urn:ietf:params:jmap:core"],"methodCalls":[]}' \
     https://api.fastmail.com/jmap/api
   ```

   Expect `200`. A `401` is the token; a redirect means Fastmail moved the
   endpoint, and brenn does not follow redirects.

## Adding the bundle to a deployment

`make bundle` builds the release tree — `components/fastmail-mail-read/` and
`modules/fastmail-mail-read.brenn` — and `make bundle-dir` prints its absolute
path; a deploy pipeline copies that tree and writes a `VERSION` file beside
it, which a mount requires. This repository ships no installer.

Install the tree as one mount, in a versioned directory behind a symlink so a
swap is atomic and a rollback is a re-pin, and never inside one of brenn's own
install directories (brenn's next deploy would delete it). Declare it in the
mounts document the server reads with `--mounts`:

```
mount fastmail { path = "/var/lib/brenn/bundles/fastmail"; }
```

where `/var/lib/brenn/bundles/fastmail` is a symlink to, say,
`fastmail.v0.1.0/`. Then add to the configuration, as
`config/example.brenn` has it. The import goes at the top of the document,
with the other `use` lines:

```
use @fastmail-mail-read::*;
```

A read's answer is Fastmail's JSON as a string inside the tool result, and
escaping grows it, so raise the bus cap to carry `api-request`'s default
256 KiB `max_response_bytes` (or lower `max_response_bytes` on the provider
instead):

```
messaging {
  max_body_bytes = 393216;
}
```

The key provider. The key file is the one written above:

```
api_key_provider fastmail {
  key_file = "/etc/brenn/fastmail/api-token";
  header = "Authorization";
  api_hosts = ["https://api.fastmail.com"];
}
```

The inbox the instance's answers arrive on. The class's four tools together
allow 12 live requests, each with one nested call outstanding; a result can
be re-delivered once, so `retain_depth` is at least 24, rounded up to 32:

```
channel at "brenn:tool-results/fastmail_mail" {
  push_depth = 1;
  retain_depth = 32;
  standing_retain_depth = 32;
  noise = alarm;
}
```

The instance. It may call exactly two endpoints; `config` names the key
provider and the host it calls:

```
new fastmail_mail: FastmailMailRead {
  grants = [log, store, config, tools, provides];
  store_path = "/var/lib/brenn/stores/fastmail_mail";
  tool api-request {
    allow { provider = "fastmail"; method = "GET";  host = "api.fastmail.com"; path = "/jmap/session"; }
    allow { provider = "fastmail"; method = "POST"; host = "api.fastmail.com"; path = "/jmap/api"; }
    rate_limit { burst = 20; sustained_per_minute = 60; }
  }
  config = {
    provider = "fastmail",
    host = "api.fastmail.com"
  };
}
```

And on each agent that may read the mail, which must run in a `container`:

```
  tool fastmail-mailboxes {}
  tool fastmail-search {}
  tool fastmail-read {}
  tool fastmail-thread {}
```

Run `brenn config-check` on the document before stopping the service, then
restart: a key provider and the `messaging` section are read at boot, and a
reload that changes either is refused.

## Limits an operator should know

- **Answer size.** `api-request` cuts an answer at the provider's
  `max_response_bytes` (default 256 KiB, at most 512 KiB), and the bus
  refuses a message over `messaging.max_body_bytes` after escaping. Either
  reaches the agent as a refusal naming the setting and the argument that
  narrows the next call (`limit`, `max_body_bytes` or `max`). The default
  `max_body_bytes` of a read (12000) fits brenn's default bus cap; a read near
  its ceiling of 60000, or a full page of search results, needs the raised
  cap above.
- **Arguments.** A nested call's arguments are capped at 64 KiB; every
  request this component builds is far under it.
- **Time.** A tool call has 240 seconds end to end. Each step — the session
  read, the JMAP call, and for a thread a second JMAP call — is one nested
  call of at most 60 seconds.
- **One key per deployment.** Every agent granted the tools reads the same
  account with the same token; there is no per-user account. Its access is
  the operator's.
- **The caller needs a user.** brenn admits a userless caller's key call with
  `GET` only, and every tool posts to `/jmap/api`, so the tools answer agents
  acting for a user, not another component calling on its own account.
- **401 and 403 are answers.** Brenn does not refresh a key or read
  Fastmail's error bodies. A dead or narrowed token reaches the agent as a
  refusal telling the operator to replace the key file and restart.

## Documentation

- [`docs/developing.md`](docs/developing.md) — building, the consumer's facts
  bzlmod needs, the layout and the lock files.
- [`AGENTS.md`](AGENTS.md) — the gate, the changelog and the TODO
  conventions.
- [`spec/fastmail-mail-read.brenn`](spec/fastmail-mail-read.brenn) — the
  tools' input schemas and the instance's settings.
- [`config/example.brenn`](config/example.brenn) — the checked example
  configuration the excerpts above come from.

## License

Apache-2.0. See `LICENSE` and `NOTICE`.
