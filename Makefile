# Repo-root Makefile: a frontend to Bazel, the check gate, and hook wiring.
#
# Bazel is the build. Every recipe here is a spelled-out Bazel invocation (or
# git plumbing, or `release`'s edit of the two version manifests), so `check`
# is one definition that the pre-commit hook and CI both run, reproducible on
# a fresh clone with nothing installed but bazelisk.
#
# No flag here points the build at a local brenn: that override is a line in
# the gitignored `.bazelrc.user`, which every bazel invocation reads — these
# recipes, the git hooks and the write hook alike. See docs/developing.md.

# Recipes run under bash with -e and pipefail, so a failing command anywhere in a
# chain or a pipeline fails the target. Without this a broken step degrades into a
# green result, which is the worst thing a gate can do.
SHELL := /bin/bash
.SHELLFLAGS := -eu -o pipefail -c

.DEFAULT_GOAL := help

SCRUB := bazel run //tools/scrub:brenn-scrub --

CHANGES := bazel run //tools/allthechanges --

.PHONY: help
help:
	@echo "Repo-root targets (all of them run Bazel):"
	@echo "  make check         the gate: scrub kit, fragment lint, build, test, clippy, rustfmt, buildifier,"
	@echo "                     the wasm32 clippy build, tree scan"
	@echo "  make build         build every target"
	@echo "  make test          run every test, without the lint aspects"
	@echo "  make bundle        build the deployable tree (components/ and modules/)"
	@echo "  make bundle-dir    print that tree's absolute path"
	@echo "  make release VERSION=X.Y.Z"
	@echo "                     cut a release: gather changelog.d and todo.d, bump the version, repin"
	@echo "  make todo-gather   fold todo.d fragments into TODO.md, outside a release"
	@echo "  make changes       run allthechanges (ARGS=\"query\", ARGS=\"todo query\", ...)"
	@echo "  make fix           rustfmt and buildifier the tree in place"
	@echo "  make repin         re-render every crate hub's lock files after a Cargo.toml change or tool bump"
	@echo "  make ide           write rust-project.json for rust-analyzer"
	@echo "  make setup         wire git at .githooks (once per clone)"
	@echo "  make scrub-tree    whole-tree secret sweep"
	@echo "  make clean         drop Bazel's outputs for this workspace"

# The gate. The scrub kit check is cheap and its failure the most important,
# so it runs first. The fragment lint follows: a malformed changelog.d or
# todo.d fragment fails here rather than at release time. One `bazel test`
# then carries the build of every target, every test, and (via the config) the
# clippy and rustfmt aspects over every Rust target. An aspect only applies to
# top-level targets, and a crate a component compiles for wasm32 is reached
# through the component rule's platform transition, so the second build asks
# for those crates under the wasm32 platform to lint them as they ship. The
# tree scan runs last: it is the backstop that sees the whole tracked tree.
#
# The notice is not a gate: a run against a local brenn says nothing about the
# committed pin, and nothing on the command line shows that it is one.
#
# TODO(correctness-sandbox-network-posture-unenforced): no step checks that
# actions ran sandboxed and without network; .bazelrc only asks for it.
.PHONY: check
check:
	@if [[ -f .bazelrc.user ]] && grep -q 'override_module=brenn' .bazelrc.user; then \
	    echo "NOTE: .bazelrc.user overrides brenn with a local checkout; this run says nothing about the committed pin."; \
	fi
	$(SCRUB) init --check
	$(CHANGES) query
	$(CHANGES) todo query
	bazel test --config=lint //...
	bazel build --config=lint --platforms=@brenn//bazel/platforms:wasm32 //fastmail-mail-read:fastmail-mail-read //fastmail-mail-write:fastmail-mail-write //jmap:fastmail_jmap //provider:fastmail_provider
	$(SCRUB) tree

.PHONY: build
build:
	bazel build //...

.PHONY: test
test:
	bazel test //...

# The release tree a deploy pipeline installs: `components/`, `modules/` and
# the manifest, contract-checked by `//:bundle_contract`.
.PHONY: bundle
bundle:
	bazel build //:bundle

# Print the absolute path of that tree. A packaging step needs the path and
# cannot spell it: the configuration segment of an output path is Bazel's to
# choose. Fail-closed on every leg, so a caller never tars an empty string or a
# tree `bundle` did not build.
.PHONY: bundle-dir
bundle-dir:
	@root=$$(bazel info execution_root); \
	rel=$$(bazel cquery --output=files //:bundle); \
	test -n "$$root" || { echo "bazel info printed no execution_root" >&2; exit 1; }; \
	test -n "$$rel" || { echo "bazel cquery printed no output path for //:bundle" >&2; exit 1; }; \
	test -d "$$root/$$rel" || { echo "$$root/$$rel is not a directory; run make bundle first" >&2; exit 1; }; \
	printf '%s/%s\n' "$$root" "$$rel"

# Passthrough to allthechanges for queries and dry runs:
# `make changes ARGS="todo query"`,
# `make changes ARGS="gather --version 0.1.0 --dry-run"`.
ARGS ?=
.PHONY: changes
changes:
	$(CHANGES) $(ARGS)

# Folds every todo.d fragment into TODO.md. Independent of a release; the
# tool refuses (exit 1) when there is nothing to gather.
.PHONY: todo-gather
todo-gather:
	$(CHANGES) todo gather

# Cuts release VERSION (bare X.Y.Z, given on the command line): gathers
# changelog.d into a new CHANGELOG.md section and todo.d into TODO.md, writes
# the version into Cargo.toml and MODULE.bazel, checks it back with the
# patterns //tools/pins reads them by, and re-renders the lock files. It
# starts from a clean tree, untracked files included, so `git checkout -- .`
# undoes a failure at any step; it runs no git write, so the result is a diff
# to review and commit.
.PHONY: release
release:
	@if [[ "$(origin VERSION)" != "command line" \
	      || ! "$(VERSION)" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$$ ]]; then \
	    echo "usage: make release VERSION=X.Y.Z (bare version, no v prefix)" >&2; \
	    exit 1; fi
	@if [[ -n "$$(git status --porcelain)" ]]; then \
	    echo "make release: the working tree is not clean; commit or stash first" \
	        "(untracked files count: gather would fold and delete them)" >&2; \
	    exit 1; fi
	$(CHANGES) gather --version $(VERSION)
	@if compgen -G 'todo.d/*.md' >/dev/null; then \
	    $(CHANGES) todo gather || { \
	        echo "make release: todo gather refused after CHANGELOG.md was rewritten;" \
	            "\`git checkout -- .\` restores the tree" >&2; \
	        exit 1; }; fi
	sed -i '/^\[workspace\.package\]$$/,/^\[/s/^version = "[^"]*"$$/version = "$(VERSION)"/' Cargo.toml
	sed -i '/^module($$/,/^)$$/s/^    version = "[^"]*",$$/    version = "$(VERSION)",/' MODULE.bazel
	@[[ "$$(sed -n '/^\[workspace\.package\]$$/,/^\[/s/^version = "\([^"]*\)"$$/\1/p' Cargo.toml)" \
	      == "$(VERSION)" ]] || { \
	    echo "make release: [workspace.package] version in Cargo.toml is not $(VERSION);" \
	        "\`git checkout -- .\` restores the tree" >&2; \
	    exit 1; }
	@[[ "$$(sed -n '/^module($$/,/^)$$/s/^    version = "\([^"]*\)",$$/\1/p' MODULE.bazel)" \
	      == "$(VERSION)" ]] || { \
	    echo "make release: module() version in MODULE.bazel is not $(VERSION);" \
	        "\`git checkout -- .\` restores the tree" >&2; \
	    exit 1; }
	@$(MAKE) repin || { \
	    echo "make release: repin failed after CHANGELOG.md and the manifests were" \
	        "rewritten; rerun \`make repin\` if the failure was transient, else" \
	        "\`git checkout -- .\` restores the tree" >&2; \
	    exit 1; }
	@echo
	@echo "Release $(VERSION) is in the working tree. Next:"
	@echo "  1. review the diff (git status; git diff)"
	@echo "  2. commit it on a branch; the pre-commit make check runs //tools/pins:pins_test"
	@echo "  3. land it, tag the landed main commit v$(VERSION), and push the tag"

# Scoped to what `check` enforces that has a fixer, so `make fix && make check`
# is a clean cycle. Clippy findings are fixed by hand.
.PHONY: fix
fix:
	bazel run @rules_rust//:rustfmt
	bazel run //:buildifier

# crate_universe renders each Cargo.lock into a cargo-bazel-lock.json once and
# then trusts it. CARGO_BAZEL_REPIN re-resolves every hub the crate extension
# declares — the workspace's, tools/scrub's and tools/allthechanges's — with
# the pinned cargo; a hub whose inputs did not change renders byte-identically.
# Commit every lock file that changed.
.PHONY: repin
repin:
	CARGO_BAZEL_REPIN=1 bazel fetch --repo=@fastmail_crates --repo=@fastmail_scrub_crates --repo=@fastmail_allthechanges_crates

# rust-analyzer's view of the build. Not part of `check`: it writes into the
# source tree, and the file it writes is ignored.
.PHONY: ide
ide:
	bazel run @rules_rust//tools/rust_analyzer:gen_rust_project

# Wire git at the tracked hooks directory through the scrub kit's own init,
# which sets core.hooksPath, removes a stale .git/hooks/pre-commit and lays
# nothing that is already present. Idempotent; run once per clone.
.PHONY: setup
setup:
	@command -v bazel >/dev/null 2>&1 || { \
	    echo "bazel not found on PATH: install bazelisk as 'bazel'; every gate runs through it." >&2; \
	    exit 1; }
	$(SCRUB) init

.PHONY: scrub-tree
scrub-tree:
	$(SCRUB) tree

.PHONY: clean
clean:
	bazel clean
