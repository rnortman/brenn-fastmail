- **Each user now reads their own Fastmail mailbox.** Previously one API
  token served the whole deployment, so every user's agent read the same
  mailbox. The `api_key_provider fastmail` block in a deployment's
  configuration now holds one `account` block per Fastmail account, each
  naming the brenn user it belongs to and its key file, in place of the
  single `key_file`. A user may have several accounts, one marked
  `default = true`. Every tool takes an optional `account` argument to pick
  one; without it, the user's default or only account is read. **Config
  change required** for per-user reading: copy the README's key-provider
  excerpt. The old single-`key_file` block still loads and still gives every
  user's agent the same one mailbox, and a call that names `account`
  against it fails. Requires a brenn release that supports `account` blocks
  in an `api_key_provider`.
