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
