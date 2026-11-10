- A fifth component, `fastmail-calendar`, lets the agent read the account's
  calendars over CalDAV. It has three read-only tools. `FastmailCalendars`
  lists the calendars. `FastmailCalendarEvents` lists the events of one
  calendar that overlap a window given as dates or UTC instants, each
  recurring event expanded by Fastmail into its occurrences, their times in
  UTC. `FastmailCalendarEvent` answers one event in full: its time in its own
  zone, description, organizer, attendees and their responses, recurrence
  rules and changed occurrences. Nothing in the component writes to the
  account.
- The calendar component needs an app password per account, limited to
  calendars, written as a `Basic` header value. The README and
  `config/example.brenn` put it in a fifth key provider, `fastmail-calendar`,
  on `caldav.fastmail.com`, and give the instance clauses that admit
  `PROPFIND`, `REPORT` and `GET` on Fastmail's CalDAV paths and no writing
  method. The README's setup gains a check, with `curl`, of every request the
  component sends.
- The calendar component needs a brenn whose `api-request` carries
  `PROPFIND`, `REPORT` and paths ending in `/`, which no brenn release does
  yet. It has not met a live Fastmail account. Installing it takes a restart.
