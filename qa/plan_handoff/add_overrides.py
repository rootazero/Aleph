#!/usr/bin/env python3
"""Add `[policies.tool_permissions.overrides]` entries to a generated config.

Not an append. A freshly generated Aleph config already CONTAINS an empty
`[policies.tool_permissions.overrides]` table, so appending a second header of
the same name produces `duplicate key 'overrides'` and the server refuses to
boot. (It refuses loudly, which is the good outcome — but the banner it prints
first shows the DEFAULT port, so the scenario looks like it started on the
wrong port rather than like a config error.)

Usage:  add_overrides.py CONFIG tool=action [tool=action ...]
"""
import re
import sys

path, entries = sys.argv[1], sys.argv[2:]
lines = open(path).read().splitlines()
added = [f"{k} = \"{v}\"" for k, v in (e.split("=", 1) for e in entries)]

HEADER = "[policies.tool_permissions.overrides]"
try:
    at = lines.index(HEADER)
    lines[at + 1 : at + 1] = added
except ValueError:
    # No table yet: create one at the end. `[policies.tool_permissions]` itself
    # may or may not exist; a bare sub-table header is valid TOML either way.
    lines += ["", HEADER] + added

open(path, "w").write("\n".join(lines) + "\n")
print(f"overrides: {', '.join(added)}")
