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
