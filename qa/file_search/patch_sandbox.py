#!/usr/bin/env python3
"""Add `[sandbox] deny_read_globs` entries to a generated config.

Not an append, for the reason `qa/plan_handoff/add_overrides.py` records about
its own table: a freshly generated config already carries a `[sandbox]`
section, and a second header of the same name is `duplicate key 'sandbox'` —
the server then refuses to boot *after* printing a banner with the default
port, so the fixture reads like a port clash rather than a config error.

Usage:  patch_sandbox.py CONFIG glob [glob ...]
"""
import sys

path, globs = sys.argv[1], sys.argv[2:]
lines = open(path).read().splitlines()
entry = "deny_read_globs = [" + ", ".join(f'"{g}"' for g in globs) + "]"

# An existing key wins over inserting a second one: TOML would take the first
# and quietly ignore ours, which is the one failure mode this whole file is
# about.
for i, line in enumerate(lines):
    if line.strip().startswith("deny_read_globs"):
        lines[i] = entry
        break
else:
    try:
        at = lines.index("[sandbox]")
        lines[at + 1 : at + 1] = [entry]
    except ValueError:
        lines += ["", "[sandbox]", entry]

open(path, "w").write("\n".join(lines) + "\n")
print(f"sandbox: {entry}")
