`fastmail-draft`'s "give `from` as one of …" refusals list every distinct
candidate address, including an address that two or more sending identities
share when none of them is the account's main one. Take identities
`alice@example.com`, `ALICE@example.com` and `bob@example.org`, none main:
with `from` omitted the draft is refused with "give `from` as one of
alice@example.com, bob@example.org", and a retry with
`from = "alice@example.com"` meets the shared-address refusal ("2 sending
identities share `alice@example.com` and none is the account's main one … the
user removes the duplicate in Fastmail's settings"), which the instruction
could not have avoided. The same listing is shown when `from` matches no
identity. The omitted-`from` path already answers the shared-address refusal
when every candidate shares one address; this is the mixed case. Done is a
listing that names only addresses a `from` resolves, with the shared-address
sentence for each address left out, and a test holding the listing and the
retry it leads to as one path; whether that per-address rule is worth
carrying for a configuration not yet seen on Fastmail is the decision. The
`TODO(correctness-mixed-identity-listing-offers-from-certain-to-fail)` comment
sits at `listed` in `fastmail-mail-write/src/draft.rs`.
