Confirm, with the write provider's read-write key file and the README's step-3
`curl`, that the session lists `urn:ietf:params:jmap:submission` and a primary
account for it. The only session recorded so far is a mail-scoped read-only
token's, which lists `core` and `mail` alone — a session advertises what its
token's scopes grant — so it does not answer the question, and without the
capability `FastmailDraft` refuses every draft by design. If a read-write
token scoped for submission also lacks it, the draft's From rule needs a
redesign before more draft work. Done is the recorded answer and either no
change or a redesigned `FastmailDraft` From rule. The
`TODO(submission-capability-read-write-token)` comment sits at the submission
refusal in `fastmail-mail-write/src/draft.rs`.
