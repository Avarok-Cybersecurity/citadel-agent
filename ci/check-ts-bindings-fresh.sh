#!/usr/bin/env bash
# The committed TypeScript wire types must be exactly what the Rust generates.
#
# typescript-client/src/types is tracked and consumed by the parent through the
# submodule pointer, so a Rust type change committed without regenerating ships
# a page whose types disagree with the agent it talks to. This CI step used to
# be `test -f` on three files: a changed field, a new variant, or a hand edit all
# passed.
#
# Every generated file is deleted first, then regenerated, so the comparison
# also catches a type deleted from the Rust (its stale .ts would otherwise
# survive) and a hand-added file. git status, not git diff, so untracked new
# bindings count too.
#
# It judges the regenerated files against the INDEX (the worktree column of
# `git status --porcelain`), not against HEAD: in CI the two are the same, and
# before a commit a correctly regenerated, staged binding passes instead of
# failing by construction.
set -euo pipefail
cd "$(dirname "$0")/.."
DIR=typescript-client/src/types

find "$DIR" -maxdepth 1 -name '*.ts' -delete
./generate_types.sh > /dev/null

STALE=$(git status --porcelain -- "$DIR" | grep -v '^. ' || true)
if [ -n "$STALE" ]; then
    echo "::error::typescript-client/src/types differs from what the Rust generates"
    echo "$STALE" | head -50
    git --no-pager diff --stat -- "$DIR" | tail -20
    echo
    echo "FAIL: run ./generate_types.sh and commit typescript-client/src/types."
    exit 1
fi
echo "OK: typescript-client/src/types is exactly what the Rust generates."
