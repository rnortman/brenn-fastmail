Check whether the account's JMAP session advertises
`urn:ietf:params:jmap:sieve` (RFC 9661, JMAP for Sieve Scripts), in bash,
with `<file>` the key file the README's Fastmail setup writes (the header goes
through a file descriptor, so the token stays off curl's command line):

```
curl -s -H @<(printf 'Authorization: %s\n' "$(cat <file>)") \
  https://api.fastmail.com/jmap/session | jq .capabilities
```

If it is present, server-side filter authoring is a provider over
`SieveScript/get`, `SieveScript/set` and `SieveScript/validate`. If it is
absent, JMAP offers no path to server-side filters, and the alternatives are
keyword- or mailbox-driven filtering in a component, or no filter authoring.
The answer decides the shape of the filters provider. The
`TODO(sieve-capability-check)` comment sits beside the capability constants in
`jmap/src/session.rs`.
