The RFC sections cited by comments, tool descriptions and tracker fragments of
the calendar write component were each compared with the fetched RFC text. The
citations that came with the calendar read component and the DAV crate before
it — in `dav/src/caldav.rs`, `dav/src/multistatus.rs`, `dav/src/href.rs`, the
reader half of `dav/src/ical.rs`, `fastmail-calendar/src/`,
`provider/src/dav.rs` where unchanged, and `spec/fastmail-calendar.brenn` —
were not part of that check, and nothing records that anyone read them
against the text. Done = each RFC section those files cite compared with the
RFC as published, any wrong citation or behaviour built on one fixed with its
test, or a recorded decision that the check is not wanted. The
`TODO(calendar-read-rfc-citations-unchecked)` comment sits at the head of
`dav/src/caldav.rs`.
