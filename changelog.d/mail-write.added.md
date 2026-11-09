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
