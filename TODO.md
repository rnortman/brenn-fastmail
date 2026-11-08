# TODO

The master list of deferred work. Each entry has a slug, a description, and the
reason it was deferred; the matching `TODO(slug)` comment in code marks the spot
where the work happens. Slugs are the join key: adding a TODO requires both
halves, and removing one removes both.

A new entry is never written here: it is a fragment, `todo.d/<slug>.md`, which
`make todo-gather` or `make release` folds in below as a `` ## `<slug>` ``
entry. Between gathers the open list is this file plus `todo.d/`;
`make changes ARGS="todo query"` lists the fragments.

Every entry is public writing.
