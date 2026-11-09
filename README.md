# brenn-fastmail

Fastmail tooling for [Brenn](https://github.com/rnortman/brenn): provider
components, built outside brenn's repository with brenn's component rules,
that let a Brenn agent reach a Fastmail account over JMAP (RFC 8620,
RFC 8621, RFC 9610) through Brenn's `api-request` tool. Each user's API token lives in
a key file the operator places, bound to that user, and never reaches a
component: an agent reads the Fastmail account of the user it is serving.

The repository holds three components. `fastmail-mail-read` gives an agent four
read-only tools: `FastmailMailboxes` (the account's mailboxes),
`FastmailSearch` (messages by sender, recipient, subject, text, mailbox, date
or keyword), `FastmailRead` (one message's headers and text, attachments
listed) and `FastmailThread` (a conversation's newest messages). Nothing it
does writes to the account. `fastmail-mail-write` gives an agent four tools
that change it: `FastmailOrganize` (add messages to and remove them from
mailboxes, set and clear keywords such as read, flagged or a label),
`FastmailMailboxCreate` and `FastmailMailboxUpdate` (create, rename and move
mailboxes) and `FastmailDraft` (save a draft, or a reply to a message, in
Drafts for the user to review and send from Fastmail). Nothing it does sends
mail or permanently deletes anything. `fastmail-contacts` gives an agent
three read-only tools over the account's contacts: `FastmailAddressBooks`
(the account's address books), `FastmailContactSearch` (contacts by text,
name, email, phone, organization, address book, uid or kind) and
`FastmailContactRead` (one contact in full: names, organizations and titles,
emails, phones, online services, addresses, anniversaries, notes and a
group's members). Nothing it does writes to the account. All three are built
on `fastmail-jmap`, a transport-free JMAP crate: the session object, the
request envelope with result references, method responses read by call id,
the mail methods the tools use (`Mailbox/get`, `Mailbox/query`,
`Mailbox/set`, `Email/query`, `Email/get`, `Email/set`, `Thread/get`,
`Identity/get`) and the contacts methods (`AddressBook/get`,
`ContactCard/query`, `ContactCard/get`), whose cards are JSContact
(RFC 9553).

## Status

Three components: `fastmail-mail-read`, with four read-only tools;
`fastmail-mail-write`, which organizes messages and mailboxes and saves
drafts; and `fastmail-contacts`, with three read-only tools over contacts
(`FastmailAddressBooks`, `FastmailContactSearch`, `FastmailContactRead`).
Contacts are read, never written. Nothing here sends mail, there is no Sieve support, and attachments
are listed, not fetched. A draft is created, never edited in place or
destroyed. Releases are tagged `v<version>`; `CHANGELOG.md` lists them. The
tree is built and tested against a pinned brenn commit; no test reaches
a live Fastmail account, so the checks in Fastmail setup below are the
operator's to run.

## Getting started

1. Install bazelisk, with git and make the only host prerequisites
   ([Building](docs/developing.md#building)).
2. For each user, create a Fastmail API token and confirm it reaches their
   account ([Fastmail setup](#fastmail-setup), steps 1–3).
3. Build the tree with `make bundle` and install it as a mount
   ([Adding the bundle to a deployment](#adding-the-bundle-to-a-deployment)).
4. Add the configuration and run `brenn config-check` on it before
   restarting (the same section, its last paragraph).

## Fastmail setup

1. Each user creates, in Fastmail's settings under Privacy & Security →
   Integrations → API tokens, a token with read-only access to mail, one per
   Fastmail account they want their agents to read. For the write component,
   each user also creates a second token per account with read-write access
   to mail; it goes in a second key provider, `fastmail-write`, which only the
   write instance names, so the read instance's tokens cannot change the
   account. One read-write token per account in one provider that both
   instances name also works, and brenn accepts it, but the read component's
   guarantee that nothing it does writes then rests on its code alone, not on
   the token. For the contacts component, each user creates a third token per
   account with read-only access to contacts; it goes in a third key
   provider, `fastmail-contacts`, which only the contacts instance names. A
   single read-only token per account scoped for both mail and contacts, in
   one provider that both read instances name, also works; keeping them
   apart is the deployer's choice.
2. Write one file per account: `Bearer <token>` — the word, one space, the
   token, nothing else — in a file owned by the user brenn runs as, mode
   `0600`. Brenn sends the
   file's content verbatim as the `Authorization` header and refuses a key
   holding anything but visible ASCII and spaces.
3. For each file, confirm the token reaches the account and the session
   lists the capability its component needs. The
   commands below are bash, and hand curl the header through a file
   descriptor, so the token never appears on curl's command line, where
   other local users and command-line audit logs could read it:

   ```
   curl -s -H @<(printf 'Authorization: %s\n' "$(cat <file>)") \
     https://api.fastmail.com/jmap/session \
     | jq '.primaryAccounts, (.capabilities | keys)'
   ```

   `primaryAccounts` must name an account for `urn:ietf:params:jmap:mail`.
   For a read-write token, it must also name one for
   `urn:ietf:params:jmap:submission`, which the capabilities list: the draft's
   From is one of the account's sending identities, read through it, and
   without it `FastmailDraft` refuses every draft. That capability also lets
   the token send mail as the user, and any read-write token can destroy
   messages: that the write component sends and destroys nothing rests on its
   code alone, not on the token. For a contacts token, `primaryAccounts` must
   name an account for `urn:ietf:params:jmap:contacts`, which the
   capabilities list; it need not list mail. Note that account id for
   step 5.
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
5. For each contacts file, run the query `FastmailContactSearch` sends,
   sorted by surname then given name, against the contacts account id from
   step 3:

   ```
   curl -s -X POST \
     -H @<(printf 'Authorization: %s\n' "$(cat <file>)") \
     -H 'Content-Type: application/json' \
     --data '{"using":["urn:ietf:params:jmap:core","urn:ietf:params:jmap:contacts"],
       "methodCalls":[["ContactCard/query",{"accountId":"<account id>",
       "filter":{},"sort":[{"property":"name/surname","isAscending":true},
       {"property":"name/given","isAscending":true}],"position":0,"limit":1,
       "calculateTotal":true},"0"]]}' \
     https://api.fastmail.com/jmap/api | jq '.methodResponses'
   ```

   Expect one `ContactCard/query` response with `ids` and `total`. An
   `error` of type `unsupportedSort` means Fastmail does not sort contacts by
   name, and `FastmailContactSearch` will refuse every search on this
   server; `unknownMethod` means Fastmail does not serve the method under
   RFC 9610's name, and no contacts tool will work. Either is worth
   reporting against this repository.

## Adding the bundle to a deployment

`make bundle` builds the release tree — `components/fastmail-mail-read/`,
`components/fastmail-mail-write/`, `components/fastmail-contacts/`,
`modules/fastmail-mail-read.brenn`, `modules/fastmail-mail-write.brenn` and
`modules/fastmail-contacts.brenn` — and `make bundle-dir` prints its absolute
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
`config/example.brenn` has it. The imports go at the top of the document,
with the other `use` lines:

```
use @fastmail-mail-read::*;
use @fastmail-mail-write::*;
use @fastmail-contacts::*;
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

The key providers: one `account` block per file written above, each naming
the brenn user whose account it is. The user must be registered before the
restart that adds the block, or boot refuses it. An account's name is what an
agent passes as `account` to pick among a user's several accounts; mark one
`default = true` to read it when the agent names none. A user with one
account needs no `default`. Names are unique across the provider: two users'
work accounts are `alice-work` and `bob-work`.

```
api_key_provider fastmail {
  header = "Authorization";
  api_hosts = ["https://api.fastmail.com"];
  account alice {
    user = "alice";
    key_file = "/etc/brenn/fastmail/alice";
    default = true;
  }
  account alice-work {
    user = "alice";
    key_file = "/etc/brenn/fastmail/alice-work";
  }
  account bob {
    user = "bob";
    key_file = "/etc/brenn/fastmail/bob";
  }
}
```

The write component's provider holds the read-write tokens for the same
accounts, under the same names and with the same `default`, so an agent's
`account`, or its omission, means the same mailbox to both components:

```
api_key_provider fastmail-write {
  header = "Authorization";
  api_hosts = ["https://api.fastmail.com"];
  account alice {
    user = "alice";
    key_file = "/etc/brenn/fastmail-write/alice";
    default = true;
  }
  account alice-work {
    user = "alice";
    key_file = "/etc/brenn/fastmail-write/alice-work";
  }
  account bob {
    user = "bob";
    key_file = "/etc/brenn/fastmail-write/bob";
  }
}
```

The contacts component's provider holds the contacts tokens for the same
accounts, again under the same names and with the same `default`:

```
api_key_provider fastmail-contacts {
  header = "Authorization";
  api_hosts = ["https://api.fastmail.com"];
  account alice {
    user = "alice";
    key_file = "/etc/brenn/fastmail-contacts/alice";
    default = true;
  }
  account alice-work {
    user = "alice";
    key_file = "/etc/brenn/fastmail-contacts/alice-work";
  }
  account bob {
    user = "bob";
    key_file = "/etc/brenn/fastmail-contacts/bob";
  }
}
```

The inbox the read instance's answers arrive on. The class's four tools together
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

The write class's four tools allow 6 live requests, so 2 × 6 = 12 fits the
default of 16; declare its inbox anyway so a full one raises an alarm:

```
channel at "brenn:tool-results/fastmail_mail_write" {
  push_depth = 1;
  retain_depth = 16;
  standing_retain_depth = 16;
  noise = alarm;
}
```

The contacts class's three tools allow 10 live requests, so 2 × 10 = 20
rounds up to 32; the default of 16 does not hold it:

```
channel at "brenn:tool-results/fastmail_contacts" {
  push_depth = 1;
  retain_depth = 32;
  standing_retain_depth = 32;
  noise = alarm;
}
```

The instances. Each may call exactly two endpoints, through its own key
provider; `config` names the provider and the host it calls:

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

new fastmail_mail_write: FastmailMailWrite {
  grants = [log, store, config, tools, provides];
  store_path = "/var/lib/brenn/stores/fastmail_mail_write";
  tool api-request {
    allow { provider = "fastmail-write"; method = "GET";  host = "api.fastmail.com"; path = "/jmap/session"; }
    allow { provider = "fastmail-write"; method = "POST"; host = "api.fastmail.com"; path = "/jmap/api"; }
    rate_limit { burst = 20; sustained_per_minute = 60; }
  }
  config = {
    provider = "fastmail-write",
    host = "api.fastmail.com"
  };
}

new fastmail_contacts: FastmailContacts {
  grants = [log, store, config, tools, provides];
  store_path = "/var/lib/brenn/stores/fastmail_contacts";
  tool api-request {
    allow { provider = "fastmail-contacts"; method = "GET";  host = "api.fastmail.com"; path = "/jmap/session"; }
    allow { provider = "fastmail-contacts"; method = "POST"; host = "api.fastmail.com"; path = "/jmap/api"; }
    rate_limit { burst = 20; sustained_per_minute = 60; }
  }
  config = {
    provider = "fastmail-contacts",
    host = "api.fastmail.com"
  };
}
```

And on each agent that may use the account, which must run in a
`container`, the read tools, the write tools where the agent may change the
account, and the contacts tools where it may read contacts:

```
  tool fastmail-mailboxes {}
  tool fastmail-search {}
  tool fastmail-read {}
  tool fastmail-thread {}
  tool fastmail-organize {}
  tool fastmail-mailbox-create {}
  tool fastmail-mailbox-update {}
  tool fastmail-draft {}
  tool fastmail-address-books {}
  tool fastmail-contact-search {}
  tool fastmail-contact-read {}
```

A draft is from one of the account's sending identities: the only one, or the account's
main address; when the account has several and none is main, the agent is
refused with their addresses and names one as `from`, an address alone,
since the identity's own display name is the draft's From name. An identity
whose address contains `*` is never sent from.

Run `brenn config-check` on the document before stopping the service, then
restart: a key provider and the `messaging` section are read at boot, and a
reload that changes either is refused; installing a component whose class
provides tools, such as adding `fastmail-mail-write` or `fastmail-contacts`
to a deployment that runs only the read component, also takes a restart, as
does the new key provider each brings.

## Limits an operator should know

- **Answer size.** `api-request` cuts an answer at the provider's
  `max_response_bytes` (default 256 KiB, at most 512 KiB), and the bus
  refuses a message over `messaging.max_body_bytes` after escaping. Either
  reaches the agent as a refusal naming the setting and the argument that
  narrows the next call (`limit`, `max_body_bytes` or `max`; for contacts,
  the search's `limit`). The default `max_body_bytes` of a read (12000) fits
  brenn's default bus cap; a read near its ceiling of 60000, or a full page
  of search results, needs the raised cap above. A contact read or a default
  page of 10 contacts fits the default cap; a full page of 25, or a contact
  with long notes, needs the raised one. A contact read has no argument to
  narrow it: a refusal names the setting to raise.
- **Arguments.** A nested call's arguments are capped at 64 KiB; every
  request the components build fits under it by construction.
  `FastmailOrganize` takes 1–500 message ids and up to 20 entries in each of
  its four lists, and refuses ids and changes whose `update` object would be
  over 30000 bytes, asking for fewer ids. `FastmailDraft` takes up to 10
  recipients in each of `to`, `cc` and `bcc`, a subject of up to 500
  characters, and a body of up to 20000 characters and 30000 bytes as sent,
  where a line break counts as three and a quote or backslash as four.
- **Time.** A tool call has 240 seconds end to end. Each step — the session
  read, the JMAP call, and for a thread a second JMAP call — is one nested
  call of at most 60 seconds. A draft is three steps: the session, a lookup
  of the sending identity, the Drafts mailbox and any original, then the
  write. Each contacts tool is two steps: the session, then one JMAP call.
- **Writes are answered per message.** Fastmail applies `FastmailOrganize`'s
  change to each message on its own: the answer counts the messages changed
  and lists each refused one under `not_updated` with Fastmail's reason, and
  is a refusal only when none changed. A message must stay in at least one
  mailbox. A write whose answer is lost says what may already have happened
  and whether asking again is safe.
- **A key per user.** Each user's token is a file the operator places and
  an `account` block names; a user with no account is refused, naming the
  operator. Adding, removing or rotating an account takes a restart. A user
  with several accounts passes `account`, or the operator marks one
  `default`. Every agent serving a user reaches all of that user's accounts.
  A provider block that still holds one `key_file` is not refused: it serves
  that one mailbox to every user's agent, though the tools describe it as the
  user's own, and any call naming `account` fails. Rewrite it into `account`
  blocks.
- **The caller needs a user.** A userless caller, such as another component
  calling on its own account, has no key to use: the instance's clauses name
  no user to lend, so brenn refuses its first call. The tools answer agents
  acting for a user.
- **401 and 403 are answers.** Brenn does not refresh a key or read
  Fastmail's error bodies. A dead or narrowed token reaches the agent as a
  refusal telling the operator to replace that account's key file and
  restart. A read-only token on the write instance answers every write with
  "cannot write mail", the same instruction: a token with read-write access
  to mail.

## Documentation

- [`docs/developing.md`](docs/developing.md) — building, the consumer's facts
  bzlmod needs, the layout and the lock files.
- [`AGENTS.md`](AGENTS.md) — the gate, the changelog and the TODO
  conventions.
- [`spec/fastmail-mail-read.brenn`](spec/fastmail-mail-read.brenn),
  [`spec/fastmail-mail-write.brenn`](spec/fastmail-mail-write.brenn) and
  [`spec/fastmail-contacts.brenn`](spec/fastmail-contacts.brenn) — the
  tools' input schemas and the instances' settings.
- [`config/example.brenn`](config/example.brenn) — the checked example
  configuration the excerpts above come from.

## License

Apache-2.0. See `LICENSE` and `NOTICE`.
