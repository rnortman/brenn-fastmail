# brenn-fastmail

Fastmail tooling for [Brenn](https://github.com/rnortman/brenn): provider
components, built outside brenn's repository with brenn's component rules,
that let a Brenn agent reach a Fastmail account over JMAP (RFC 8620,
RFC 8621, RFC 9610) through Brenn's `api-request` tool. Each user's API token lives in
a key file the operator places, bound to that user, and never reaches a
component: an agent reads the Fastmail account of the user it is serving.

The repository holds four components. `fastmail-mail-read` gives an agent four
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
group's members, each entry with the `key` an update names it by). Nothing it
does writes to the account. `fastmail-contacts-write` gives an agent two
tools that change contacts: `FastmailContactCreate` (a new contact in one
address book, with its name and bare lists of emails, phones and the rest)
and `FastmailContactUpdate` (the name, the address books, and one entry per
call added, changed or removed, keeping everything the call does not name).
Nothing it does deletes a contact or writes an address book. All four are built
on `fastmail-jmap`, a transport-free JMAP crate: the session object, the
request envelope with result references, method responses read by call id,
the mail methods the tools use (`Mailbox/get`, `Mailbox/query`,
`Mailbox/set`, `Email/query`, `Email/get`, `Email/set`, `Thread/get`,
`Identity/get`) and the contacts methods (`AddressBook/get`,
`ContactCard/query`, `ContactCard/get`, `ContactCard/set`), whose cards are
JSContact (RFC 9553).

## Status

Four components: `fastmail-mail-read`, with four read-only tools;
`fastmail-mail-write`, which organizes messages and mailboxes and saves
drafts; `fastmail-contacts`, with three read-only tools over contacts
(`FastmailAddressBooks`, `FastmailContactSearch`, `FastmailContactRead`); and
`fastmail-contacts-write`, which creates and updates contacts
(`FastmailContactCreate`, `FastmailContactUpdate`). A contact is never
deleted, and an address book never written. Nothing here sends mail, there is no Sieve support, and attachments
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
   apart is the deployer's choice. For the contacts write component, each
   user creates a fourth token per account with read-write access to
   contacts; it goes in a fourth key provider, `fastmail-contacts-write`,
   which only the contacts write instance names, so the contacts read
   instance's tokens cannot change a contact.
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
   code alone, not on the token. For a contacts token, read-only or
   read-write, `primaryAccounts` must name an account for
   `urn:ietf:params:jmap:contacts`, which the capabilities list; it need not
   list mail. A contacts read-write token can also delete contacts and write
   address books: that the contacts write component does neither rests on its
   code alone, not on the token. Note that account id for steps 5 and 6.
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
6. For each contacts read-write file, run in the same sitting as step 5 the
   requests `fastmail-contacts-write` sends, against a scratch contact the
   check creates and then deletes. Define a helper, with the file and the
   contacts account id from step 3:

   ```
   jmap() {
     curl -s -X POST \
       -H @<(printf 'Authorization: %s\n' "$(cat <file>)") \
       -H 'Content-Type: application/json' \
       --data "{\"using\":[\"urn:ietf:params:jmap:core\",\"urn:ietf:params:jmap:contacts\"],
         \"methodCalls\":[$1]}" \
       https://api.fastmail.com/jmap/api | jq '.methodResponses'
   }
   ```

   (a) Find the default address book's id, the one with `isDefault: true`:

   ```
   jmap '["AddressBook/get",{"accountId":"<account id>","ids":null},"c0"]'
   ```

   (b) Create the scratch contact as `FastmailContactCreate` does, with a
   `uid`, a name and one email at key `k1`:

   ```
   jmap '["ContactCard/set",{"accountId":"<account id>","create":{"contact":{
     "@type":"Card","version":"1.0",
     "uid":"urn:uuid:0f8fad5b-d9cb-469f-a165-70867728950e",
     "addressBookIds":{"<book id>":true},
     "name":{"full":"Scratch Example","components":[
       {"kind":"given","value":"Scratch"},{"kind":"surname","value":"Example"}]},
     "emails":{"k1":{"address":"scratch@example.com"}}}}},"c0"]'
   ```

   Note the `id` under `created.contact`.

   (c) Read it back:

   ```
   jmap '["ContactCard/get",{"accountId":"<account id>","ids":["<contact id>"]},"c0"]'
   ```

   Note the answer's `state`, the keys under `emails`, the `uid` and the
   whole `name` object.

   (d) Update it as `FastmailContactUpdate` does, under that `state`: an
   edit of the email, a name change, a first phone and a first address
   written as full text. `<email key>` is the key under `emails` that (c)
   showed:

   ```
   jmap '["ContactCard/set",{"accountId":"<account id>","ifInState":"<state>",
     "update":{"<contact id>":{
       "emails/<email key>/address":"scratch2@example.com",
       "name/full":"Scratch Sample",
       "name/components":[{"kind":"given","value":"Scratch"},{"kind":"surname","value":"Sample"}],
       "phones":{"k1":{"number":"+1 555 0100"}},
       "addresses":{"k1":{"full":"1 Example Street\nExample City"}}}}},"c0"]'
   ```

   (e) Read it back again, as in (c), and note its `state`.

   Each of (f), (g) and (h) sends one path, so a refusal names its path.
   Send each under the `state` before it: (f) under (e)'s, (g) and (h) under
   the `newState` of the answer before. A refused update leaves the contact
   as it was; carry on with its `newState`.

   (f) Add a second email, as an add to a list the card holds does.
   `<new key>` is `k<n>` for the smallest `n` that (e) showed no key under
   `emails` for (`k2` when it showed only `k1`):

   ```
   jmap '["ContactCard/set",{"accountId":"<account id>","ifInState":"<state>",
     "update":{"<contact id>":{
       "emails/<new key>":{"address":"scratch3@example.com"}}}},"c0"]'
   ```

   (g) Set an address-book flag. `<flag book id>` is another book's id from
   (a) when there is one, else `<book id>`, which the contact is already in:

   ```
   jmap '["ContactCard/set",{"accountId":"<account id>","ifInState":"<newState>",
     "update":{"<contact id>":{
       "addressBookIds/<flag book id>":true}}},"c0"]'
   ```

   (h) Remove the first email, as a remove does:

   ```
   jmap '["ContactCard/set",{"accountId":"<account id>","ifInState":"<newState>",
     "update":{"<contact id>":{
       "emails/<email key>":null}}},"c0"]'
   ```

   (i) Read it back again, as in (c). If (g) named another book, remove the
   contact from it under (i)'s `state`, as removing a book does, and read it
   back once more:

   ```
   jmap '["ContactCard/set",{"accountId":"<account id>","ifInState":"<state>",
     "update":{"<contact id>":{
       "addressBookIds/<flag book id>":null}}},"c0"]'
   ```

   (j) Open the contact in Fastmail's own contacts editor and look at its
   name, emails, phone and address. (k) Delete it there.

   Record, for a report against this repository: any `error`, or any
   `notCreated` or `notUpdated` type with its `description`, naming the step;
   whether the keys sent (`k1`, `<new key>`) are the keys Fastmail holds;
   whether the `uid` sent is the `uid` it holds; the `name` object it holds
   after (b) and after (d), whole (which of `full`, `components`, `isOrdered`
   and `sortAs` it keeps or adds); whether, after (i), `emails` holds
   `<new key>` and no longer `<email key>`, and `addressBookIds` holds
   `<flag book id>`; whether the last read shows that book gone; and whether
   the name and the full-text address show in Fastmail's editor. A refusal
   of (d) says one of its five paths is refused, not which, and leaves the
   other four unanswered: report its `description` whole. When (g) had no
   other book to name, record that the `null` form of an address-book path
   went untried. An answer naming `stateMismatch` means another client
   changed a contact since the `state` sent: read it as in (c) and send the
   refused step again under that read's `state`.

## Adding the bundle to a deployment

`make bundle` builds the release tree — `components/fastmail-mail-read/`,
`components/fastmail-mail-write/`, `components/fastmail-contacts/`,
`components/fastmail-contacts-write/`, `modules/fastmail-mail-read.brenn`,
`modules/fastmail-mail-write.brenn`, `modules/fastmail-contacts.brenn` and
`modules/fastmail-contacts-write.brenn` — and `make bundle-dir` prints its absolute
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
use @fastmail-contacts-write::*;
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

The contacts write component's provider holds the contacts read-write tokens
for the same accounts, under the same names and with the same `default`:

```
api_key_provider fastmail-contacts-write {
  header = "Authorization";
  api_hosts = ["https://api.fastmail.com"];
  account alice {
    user = "alice";
    key_file = "/etc/brenn/fastmail-contacts-write/alice";
    default = true;
  }
  account alice-work {
    user = "alice";
    key_file = "/etc/brenn/fastmail-contacts-write/alice-work";
  }
  account bob {
    user = "bob";
    key_file = "/etc/brenn/fastmail-contacts-write/bob";
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

The contacts write class's two tools allow 4 live requests, so 2 × 4 = 8 fits
the default of 16; declare its inbox anyway so a full one raises an alarm:

```
channel at "brenn:tool-results/fastmail_contacts_write" {
  push_depth = 1;
  retain_depth = 16;
  standing_retain_depth = 16;
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

new fastmail_contacts_write: FastmailContactsWrite {
  grants = [log, store, config, tools, provides];
  store_path = "/var/lib/brenn/stores/fastmail_contacts_write";
  tool api-request {
    allow { provider = "fastmail-contacts-write"; method = "GET";  host = "api.fastmail.com"; path = "/jmap/session"; }
    allow { provider = "fastmail-contacts-write"; method = "POST"; host = "api.fastmail.com"; path = "/jmap/api"; }
    rate_limit { burst = 20; sustained_per_minute = 60; }
  }
  config = {
    provider = "fastmail-contacts-write",
    host = "api.fastmail.com"
  };
}
```

And on each agent that may use the account, which must run in a
`container`, the read tools, the write tools where the agent may change the
account, the contacts tools where it may read contacts, and the contacts
write tools where it may change them:

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
  tool fastmail-contact-create {}
  tool fastmail-contact-update {}
```

A draft is from one of the account's sending identities: the only one, or the account's
main address; when the account has several and none is main, the agent is
refused with their addresses and names one as `from`, an address alone,
since the identity's own display name is the draft's From name. An identity
whose address contains `*` is never sent from.

Run `brenn config-check` on the document before stopping the service, then
restart: a key provider and the `messaging` section are read at boot, and a
reload that changes either is refused; installing a component whose class
provides tools, such as adding `fastmail-mail-write`, `fastmail-contacts` or
`fastmail-contacts-write` to a deployment that runs only the read component,
also takes a restart, as does the new key provider each brings.

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
  narrow it: a refusal names the setting to raise. A contact update first
  reads the one list it changes (and the name, when it changes the name), so
  it meets the same cap only for a contact whose one list is very large.
- **Arguments.** A nested call's arguments are capped at 64 KiB; every
  request the components build fits under it by construction.
  `FastmailOrganize` takes 1–500 message ids and up to 20 entries in each of
  its four lists, and refuses ids and changes whose `update` object would be
  over 30000 bytes, asking for fewer ids. `FastmailDraft` takes up to 10
  recipients in each of `to`, `cc` and `bcc`, a subject of up to 500
  characters, and a body of up to 20000 characters and 30000 bytes as sent,
  where a line break counts as three and a quote or backslash as four.
  `FastmailContactCreate` refuses a contact whose texts together are over
  30000 bytes as sent, asking for fewer or shorter entries;
  `FastmailContactUpdate` changes one entry per call, its longest text a
  4000-character note.
- **Time.** A tool call has 240 seconds end to end. Each step — the session
  read, the JMAP call, and for a thread a second JMAP call — is one nested
  call of at most 60 seconds. A draft is three steps: the session, a lookup
  of the sending identity, the Drafts mailbox and any original, then the
  write. Each contacts read tool, and `FastmailContactCreate`, is two steps:
  the session, then one JMAP call. `FastmailContactUpdate` is three: the
  session, a read of the contact, then the write, which Fastmail refuses if
  any contact changed in between; the agent is told to ask again.
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
  "cannot write mail", and on the contacts write instance with "cannot write
  contacts", the same instruction: a token with read-write access to mail, or
  to contacts.

## Documentation

- [`docs/developing.md`](docs/developing.md) — building, the consumer's facts
  bzlmod needs, the layout and the lock files.
- [`AGENTS.md`](AGENTS.md) — the gate, the changelog and the TODO
  conventions.
- [`spec/fastmail-mail-read.brenn`](spec/fastmail-mail-read.brenn),
  [`spec/fastmail-mail-write.brenn`](spec/fastmail-mail-write.brenn),
  [`spec/fastmail-contacts.brenn`](spec/fastmail-contacts.brenn) and
  [`spec/fastmail-contacts-write.brenn`](spec/fastmail-contacts-write.brenn) — the
  tools' input schemas and the instances' settings.
- [`config/example.brenn`](config/example.brenn) — the checked example
  configuration the excerpts above come from.

## License

Apache-2.0. See `LICENSE` and `NOTICE`.
