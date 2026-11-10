`provider::dav::dav_call` takes the method and `Depth` as strings and the
depth, body and `Accept` as three `Option`s whose valid combinations depend
on the method. Nothing but its doc comment stops a `PROPFIND` or `REPORT`
sent without `Depth`, which a server reads as `infinity` (RFC 4918 section
9.1); the calendar component holds the rule in its own `propfind` wrapper.
The shape that replaces it, such as a required depth type or one builder
per method, depends on the methods the second DAV caller issues, which no
component here has yet. Done is a signature under which a `PROPFIND` or
`REPORT` without a depth does not compile, used by every DAV caller, with
the component-local wrappers gone. The
`TODO(quality-dav-call-stringly-depth-and-option-sprawl)` comment sits at
`dav_call` in `provider/src/dav.rs`.
