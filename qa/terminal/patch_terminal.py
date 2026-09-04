#!/usr/bin/env python3
"""The two config keys this fixture cannot run without.

`[agents.defaults] workspace_root` — `pty.spawn`'s `cwd` is a REQUEST, not an
authorisation: `gateway::pty::jail::resolve_spawn_cwd` refuses any directory
outside the registered workspace roots, and `workspace_roots()` resolves
exactly this key. Without it every spawn in the `cwd` stage would land in the
scratch home's default workspace instead of the three directories the stage
needs to tell apart, and the refusal ("cwd … is outside every registered
workspace") reads like a fixture path bug rather than a policy answer.

`[policies.terminal] enabled` — default-on today, written anyway. A fixture
whose subject is the embedded terminal must not be able to go green because
some future default flipped and every stage silently spawned nothing; the
error `handle_spawn` returns in that case is explicit, and this makes sure we
would see it rather than inherit it.

Not an append, for the reason `qa/file_search/patch_sandbox.py` records: a
generated config already carries these tables, and a duplicate header is
`duplicate key`, which the server reports AFTER printing a banner with the
default port — so it reads like a port clash rather than a config error.

Usage:  patch_terminal.py CONFIG WORKSPACE_ROOT
"""
import sys

path, workspace_root = sys.argv[1], sys.argv[2]
lines = open(path).read().splitlines()

WANT = [
    ("agents.defaults", "workspace_root", f'"{workspace_root}"'),
    ("policies.terminal", "enabled", "true"),
]


def section_of(index: int) -> str | None:
    """The table `lines[index]` sits in, or `None` above the first header."""
    for line in reversed(lines[:index]):
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            return stripped.strip("[]")
    return None


for section, key, value in WANT:
    entry = f"{key} = {value}"
    # Replace in place when the key already exists IN THIS TABLE. A key of the
    # same name under a different header is a different key, and rewriting it
    # would edit someone else's setting while leaving ours absent.
    for i, line in enumerate(lines):
        if line.strip().startswith(f"{key} ") or line.strip().startswith(f"{key}="):
            if section_of(i) == section:
                lines[i] = entry
                break
    else:
        header = f"[{section}]"
        if header in lines:
            at = lines.index(header)
            lines[at + 1 : at + 1] = [entry]
        else:
            lines += ["", header, entry]
    print(f"terminal: [{section}] {entry}")

open(path, "w").write("\n".join(lines) + "\n")
