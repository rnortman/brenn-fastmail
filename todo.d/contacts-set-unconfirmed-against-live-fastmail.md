No test reaches a live Fastmail account, so what `fastmail-contacts-write`
depends on is unconfirmed: that Fastmail accepts `ContactCard/set` create and
update from a token with read-write access to contacts; that it accepts a
created card's `uid` and entry keys and keeps them; that it accepts
`ifInState` and the patch paths an update sends (`<property>/<key>`,
`<property>/<key>/<sub-property>`, a whole property, `name/full`,
`name/components`, `addressBookIds/<id>`, and `null` at an entry or an
address-book path); what `name` it stores for a card
created with `full` and `components`; which `SetError` types it answers
with; and that a name and an address written as full text show in
Fastmail's own editor. Run the README's Fastmail setup step 6 against a
scratch contact and record the answer. Done is the recorded answer and
either no change or a component that sends what Fastmail accepts. The
`TODO(contacts-set-unconfirmed-against-live-fastmail)` comment sits where
the update builds its patch, in `fastmail-contacts-write/src/logic.rs`.
