- A sixth component, `fastmail-calendar-write`, lets the agent change the
  account's calendars over CalDAV. It has three tools.
  `FastmailCalendarEventCreate` puts a new event on a calendar: all-day, at
  UTC instants, or at local times in a named zone, optionally repeating by up
  to three `RRULE` lines. `FastmailCalendarEventUpdate` changes an event's
  summary, description, location or time, writing every other line of the
  stored event back as it was read. `FastmailCalendarEventDelete` deletes an
  event, a recurring one whole only with `series: true`. A create never
  replaces a stored event, and an update or a delete applies only if the
  event is still the one that was read.
- The calendar write component needs its own app password per account,
  allowed to write calendars. The README and `config/example.brenn` put it in
  a sixth key provider, `fastmail-calendar-write`, and give the instance
  three clauses, `GET`, `PUT` and `DELETE` of an event path. Deletion is a
  tool and a clause of its own: a deployment that drops both creates and
  changes events without deleting them. The README's setup gains a check,
  with `curl`, of every kind of request the component sends.
- No calendar write sends mail. An event with an organizer or attendees is
  refused, and so are a recurring event's time and rule and any one
  occurrence of a series; those are changed in Fastmail. An event too large
  for one call (over 60,000 bytes) is refused before anything is written.
- The calendar write component has not met a live Fastmail account.
  Installing it takes a restart.
