`FastmailCalendarEventUpdate` changes a recurring event's summary,
description and location on the series and refuses to change its time or
rule; no tool changes or deletes one occurrence. A change to the series' text
does not reach an occurrence that was already changed, which keeps its own.
`FastmailCalendarEventCreate` takes `RRULE` lines only, no `RDATE` or
`EXDATE`. One occurrence is written as an override component or an `EXDATE`
that names it in the series' own `DTSTART` form. `FastmailCalendarEvents`
shows an occurrence as a UTC instant and never in the event's time zone — a
limit of the read tool that was itself a default, not a requirement of CalDAV
— and this repository converts no time between zones, so the write tools
cannot name the occurrence. Lifting the limit therefore starts on the read
side: the window tool gives each occurrence in its event's zone, or the
component learns to convert. Done = one occurrence can be changed and
deleted, and a series' time and rule changed, with overrides kept valid; the
limits stay only by the account owner's choice, recorded here. The
`TODO(calendar-write-no-single-occurrence-edit)` comment sits at the refusal
in `fastmail-calendar-write/src/logic.rs`.
