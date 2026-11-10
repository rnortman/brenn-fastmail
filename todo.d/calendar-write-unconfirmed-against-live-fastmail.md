The calendar write component is written from RFC 4791, RFC 5545 and
Fastmail's help pages and has not met a live account. Unconfirmed: that
Fastmail honours `If-None-Match: *` and `If-Match` on a calendar resource
and answers a stale one `412`; that a `GET` of an event answers an `ETag`;
that an event written with a `TZID` and no `VTIMEZONE` is accepted and
shown in that zone — RFC 7809 allows it only to a server that advertises
`calendar-no-timezone` in answer to `OPTIONS`, a method Brenn's `api-request`
does not carry, so the component cannot check; if Fastmail refuses it,
the alternatives are a floating local time or writing the `VTIMEZONE`; that
the entity tag a `GET` answers is a strong one and reaches the component;
that a new event is stored at the path it was `PUT` to; that a `PUT` to a
calendar that does not exist answers 404; that an event without `ORGANIZER`
or `ATTENDEE` causes no mail; and whether an app password can be limited to
calendars. Done = the README's Fastmail setup step 8 run against an account
and its result recorded here, with whatever the component needed changed. The
`TODO(calendar-write-unconfirmed-against-live-fastmail)` comment sits where
the create tool builds its event, in `fastmail-calendar-write/src/logic.rs`.
