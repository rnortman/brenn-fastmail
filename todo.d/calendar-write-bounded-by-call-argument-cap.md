`FastmailCalendarEventUpdate` writes an event back whole, and Brenn bounds a
nested call's arguments at 64 KiB, so an event whose iCalendar text is over
60,000 bytes as the call carries it (escaped as JSON: each line end counts
four bytes, each `"` and `\` two) — one with many changed occurrences or an
inline attachment — is refused and is changed in Fastmail. Lifting the bound
waits on brenn's tracker entry `api-request-body-caps-bound-file-transfer`.
Done = an update writes back any event the read limit lets it read, or a
recorded decision that the bound stays. The
`TODO(calendar-write-bounded-by-call-argument-cap)` comment sits at the bound
in `fastmail-calendar-write/src/logic.rs`.
