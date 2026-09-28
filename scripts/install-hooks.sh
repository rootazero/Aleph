#!/usr/bin/env bash
# Install git hooks from scripts/ into .git/hooks/.
#
# Run once after cloning (or after `git switch` to a branch that bumped
# hook scripts). Re-run safely: idempotent, just overwrites the links.
#
#     bash scripts/install-hooks.sh
#
# Why this exists instead of .pre-commit-config.yaml: the pre-commit
# framework adds a Python dependency (`pip install pre-commit`) for a
# single bash hook that doesn't need its plugin machinery. A plain
# scripts/* + .git/hooks/* setup is the lightest equivalent.

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
HOOKS_SRC="$REPO_ROOT/scripts"
HOOKS_DST="$REPO_ROOT/.git/hooks"

if [[ ! -d "$HOOKS_SRC" ]]; then
    echo "error: $HOOKS_SRC not found — are you running this from the repo root?" >&2
    exit 1
fi

installed=0
for hook in pre-commit; do
    src="$HOOKS_SRC/$hook"
    dst="$HOOKS_DST/$hook"

    if [[ ! -f "$src" ]]; then
        echo "skip: $src not found" >&2
        continue
    fi

    if [[ -f "$dst" && ! -L "$dst" ]]; then
        echo "note: $dst already exists (not a symlink); overwriting" >&2
    fi

    cp "$src" "$dst"
    chmod +x "$dst"
    echo "installed: $dst  (from $src)"
    installed=$((installed + 1))
done

if [[ $installed -eq 0 ]]; then
    echo "error: no hooks installed" >&2
    exit 1
fi

echo ""
echo "done. $installed hook(s) installed."
echo "bypass: git commit --no-verify"