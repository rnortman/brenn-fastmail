No test reaches a live Fastmail account, so two things `fastmail-contacts`
depends on are unconfirmed: that Fastmail names its contact methods as
RFC 9610 does, and that it sorts `ContactCard/query` by `name/surname` then
`name/given`, which RFC 9610 section 3.3.2 only recommends; an
`unsupportedSort` answer makes `FastmailContactSearch` refuse every search.
Run the README's Fastmail setup step 5 with a contacts token and record the
answer. Done is the recorded answer and either no change or a search that
sorts by a property Fastmail supports. The
`TODO(contacts-query-unconfirmed-against-live-fastmail)` comment sits at the
search's sort in `fastmail-contacts/src/logic.rs`.
