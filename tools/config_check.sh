#!/usr/bin/env bash
# `brenn config-check` over this repository's deployer documents.
#
# The fit gate compiles, resolves and derives; it never lowers, so a refusal
# that lives in lowering is invisible to it. This runs the binary and the verb
# a bundle installer runs before it stops the service, so a document that
# lowering refuses is a red gate here rather than a failed restart.
#
# Arguments: the `brenn` binary, then every module file, then `--`, then every
# root document. A `--modules` per distinct directory among the module files,
# which is the rule a host is started with and the one an installer's roots
# satisfy.
set -uo pipefail

brenn="$1"
shift

modules=()
roots=()
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do
    dir=$(dirname "$1")
    seen=0
    for root in ${roots[@]+"${roots[@]}"}; do
        [ "$root" = "$dir" ] && seen=1
    done
    if [ "$seen" -eq 0 ]; then
        roots+=("$dir")
        modules+=(--modules "$dir")
    fi
    shift
done
if [ "$#" -eq 0 ]; then
    echo "FAIL: the module files are not terminated by \`--\`, so no root document was named."
    exit 1
fi
shift
if [ "${#modules[@]}" -eq 0 ]; then
    echo "FAIL: no module file was named, so there is no module root to check against."
    exit 1
fi
if [ "$#" -eq 0 ]; then
    echo "FAIL: no root document was named."
    exit 1
fi

status=0
for config in "$@"; do
    if out=$("$brenn" "${modules[@]}" config-check "$config" 2>&1); then
        echo "config-check: $out"
    else
        echo "FAIL: $config is refused by the check an installer runs:"
        echo "$out"
        status=1
    fi
done
exit "$status"
