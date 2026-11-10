The README and the provider's refusal text tell the operator that adding,
removing or rotating an account, or replacing a dead token's or app
password's key file, takes a restart of Brenn, and that a reload changing a
key provider is refused. That is true of Brenn releases up to 0.33.0. A later
Brenn converges `api_key_provider` changes and key files rotated in place on
reload. Once a Brenn release carrying that change exists, the README's
account and token-rotation paragraphs, the 401/403 refusal string and the
calendar component's app-password refusal say "reload" and name the first
Brenn version that converges key providers, so an operator on an older Brenn
still knows to restart. The `TODO(restart-wording-after-key-reload)` comments
sit at the 401/403 refusal and the read-only-token sentence in
`provider/src/failure.rs` and at the app-password refusal in
`provider/src/dav.rs`.
