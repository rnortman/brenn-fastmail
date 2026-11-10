Fastmail keeps notes as messages in the `Notes` mail folder, in a shape its
help pages do not give, so no notes tool is built: reading that folder already
works through the mail read tools, and a note written in a guessed shape may
not show as a note. What is missing is one sample only an account holder can
take: a note written in Fastmail's own notes screen, fetched over JMAP
(`Email/get` with every header, `bodyStructure`, `keywords` and `mailboxIds`),
private content replaced. Done is the sample's shape recorded here and either
notes tools that write that shape or a recorded reason they cannot be built.
The `TODO(notes-message-shape-unsampled)` comment sits at `set_request` in
`fastmail-mail-write/src/draft.rs`, where the draft's message is built for
`Email/set`.
