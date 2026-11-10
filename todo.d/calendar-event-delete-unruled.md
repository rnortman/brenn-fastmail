`FastmailCalendarEventDelete` removes an event from a calendar, and a
recurring series whole with `series: true`. Over CalDAV the event is then
gone; this repository has no way to bring it back. The tool was built, and the
example configuration grants it, without the account owner having said an
agent should hold it, on the precedent of brenn's `CalendarDelete`, which does
the same on Google under the same series rule. It is a tool of its own, so a
deployment that does not grant it and drops the `DELETE` clause has create and
update without it. The question: keep it as built and granted; keep it out of
the example's grants so that a deployer opts in; or remove it.
`contact-update-entry-removal-unruled` asks the same of contacts and the two
are best answered together. Done = the owner's choice recorded here and the
tool, the example and the README matching it. The
`TODO(calendar-event-delete-unruled)` comment sits where the delete tool
builds its `DELETE`, in `fastmail-calendar-write/src/logic.rs`.
