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
