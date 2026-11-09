The Claude Code edit hook `.claude/hooks/format.sh` runs the pinned `rustfmt`
on every Rust file a session writes, a file outside this workspace included.
`rustfmt` takes its edition and style from the `rustfmt.toml` above the file
it formats, so this repository's `rustfmt.toml` reaches only files under it,
and the hook passes no edition of its own. A file in another checkout —
brenn's, when a session started here edits both repositories — is therefore
formatted with rustfmt's defaults, the 2015 style, whatever that checkout's
own gate expects. It has reordered `use` lists in brenn's sources
(`{Value, json}` to `{json, Value}`), which brenn's gate rejects, and the edit
had to be undone by hand before the commit. The buildifier arm reaches a file
outside the workspace the same way; no symptom from it has been seen. Done =
the hook leaves a file outside `$CLAUDE_PROJECT_DIR` alone, or formats it by
its own repository's configuration. The
`TODO(edit-hook-formats-files-outside-the-workspace)` comment sits at the
`case` on the written file's name in `.claude/hooks/format.sh`.
