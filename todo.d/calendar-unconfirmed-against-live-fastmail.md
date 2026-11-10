The calendar component is written from RFC 4791, RFC 5397 and Fastmail's help
pages and has not met a live account. Unconfirmed: that an app password in a
`Basic` header is accepted at `caldav.fastmail.com`; that
`PROPFIND /dav/principals/` answers the credential's `current-user-principal`,
and that principal its `calendar-home-set`; that those paths have the shapes
the example's clauses admit (`/dav/principals/user/<address>/`,
`/dav/calendars/user/<address>/`); that Fastmail answers a time-range
`calendar-query` with each recurring event expanded into its occurrences,
their times in UTC, when `calendar-data` names both a property subset and
`expand`; and whether an app password can be limited to reading. Done = the
README's Fastmail setup step 7 run against an account and its result recorded
here, with whatever the component needed changed. The
`TODO(calendar-unconfirmed-against-live-fastmail)` comment sits at the window
query's request builder in `dav/src/caldav.rs`.
