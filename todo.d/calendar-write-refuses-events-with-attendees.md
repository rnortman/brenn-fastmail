The calendar write tools create no event with attendees and refuse to change
or delete an event that has an `ORGANIZER` or an `ATTENDEE`. A CalDAV server
with implicit scheduling (RFC 6638) mails invitations, updates, cancellations
and replies from the user's account when such an event is stored, changed or
deleted, and no tool here sends mail. So an agent cannot move or cancel a
meeting, only an event that is the user's alone. The refusal is a default
nobody who owns an account has confirmed, and it is a question for the owner:
let the server tell the other people, as it does when the user edits the
event in Fastmail, behind an argument that defaults to telling nobody; change
the event while marking it so the server tells nobody, which alters how the
event is stored; or keep the refusal. Done = the owner's choice recorded here,
and the write tools handling scheduled events under it — the refusal stays
only by that choice. The `TODO(calendar-write-refuses-events-with-attendees)`
comment sits at the check that refuses, in
`fastmail-calendar-write/src/resource.rs`.
