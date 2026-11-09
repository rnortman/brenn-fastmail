The README's configuration excerpts (both `api_key_provider` blocks, both
results channels, both component instances and the agent's grants) are copied
by hand from `config/example.brenn`. `//:example_fit` and `//:config_check`
hold the example to what brenn accepts; nothing holds the README to the
example, so a change to a spec setting, a path or an `allow` clause can leave
the deployer's runbook stale with the gate green. Either a Bazel test that
finds every configuration excerpt in `README.md` and asserts it appears in
`config/example.brenn` (ignoring whitespace), or a README that quotes less
and points at the example; which one is the choice to make. The
`TODO(quality-readme-config-excerpts-unguarded-and-doubled)` comment sits at
`//:example_fit` in `BUILD.bazel`.
