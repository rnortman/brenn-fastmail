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
