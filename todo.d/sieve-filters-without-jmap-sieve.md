Fastmail offers no supported JMAP path for authoring server-side mail filters:
its developer documentation lists no Sieve scope or capability, and the
session of a mail-scoped token, read 2026-11-09 with the README's step-3
`curl`, advertises no `urn:ietf:params:jmap:sieve` (RFC 9661). Filter
authoring is out of this project's scope for now. Routes known, recorded for a
later decision: (1) Fastmail's web session reaches the account's filter script
over JMAP at the same endpoint with the private capabilities
`https://www.fastmail.com/dev/rules` and `https://www.fastmail.com/dev/user`,
methods `SieveBlocks/get` and `SieveBlocks/set` on a `singleton` object whose
`sieveAtStart`, `sieveAtMiddle` and `sieveAtEnd` blocks are editable beside
Fastmail's own `sieveForRules` and `sieveForBlocked`; third-party tools use
it, the API is unpublished, and it is authenticated by a web-session bearer
token and the `__Host-s_` cookie copied from a browser, which brenn's
`api_key_provider` cannot carry today (one key file, one header, cookie
refused) and which a deployer must re-capture when the session expires. (2)
Filtering by an agent on a `brenn-automation` cron with the existing
`FastmailSearch` and `FastmailOrganize` tools, which needs no code and costs
an LLM turn per run. (3) A deterministic filter component in wasm, which needs
brenn to let a component with no acting user write with a credential (brenn's
`consumer-own-credentials`). (4) Semi-automation through the web UI's filter
export and import, which needs brenn's outbound file delivery (brenn's
`agent-file-delivery-beyond-markdown`). Done is a `fastmail-sieve` component
on one of these routes, or a decision that none is wanted. The
`TODO(sieve-filters-without-jmap-sieve)` comment sits beside the capability
constants in `jmap/src/session.rs`.
