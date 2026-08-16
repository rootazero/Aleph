#!/usr/bin/env python3
"""Point `[agents.defaults]` at roots that are NOT the default layout.

The bug this exists to expose is invisible whenever the two sides agree for
the uninteresting reason. Leaving these keys unset makes provisioning and the
resolver both fall back to `$ALEPH_HOME/agents` — byte-identical, and the
divergence only shows on an install that configures them. So the QA
configures them, and asserts the provisioned directories land there and
nowhere near the default layout.

Written in place rather than appended: TOML rejects a table defined twice, and
the generator may already have emitted `[agents.defaults]`. An append would
make the server refuse to start — after printing its startup banner, so the
failure reads like a port problem rather than a config one.
"""
import argparse
import re

p = argparse.ArgumentParser()
p.add_argument("path")
p.add_argument("--agents-root", required=True)
p.add_argument("--workspace-root", required=True)
args = p.parse_args()

lines = open(args.path).read().splitlines()
keys = {"agents_root": args.agents_root, "workspace_root": args.workspace_root}

out, in_section, seen_section = [], False, False
for line in lines:
    header = re.match(r"^\s*\[+([^\]]+)\]+\s*$", line)
    if header:
        in_section = header.group(1).strip() == "agents.defaults"
        out.append(line)
        if in_section:
            seen_section = True
            # Emit both keys directly under the header, then drop any later
            # copy inside this section, so the value written here is the one
            # the parser sees regardless of what the generator had put there.
            out += [f'{k} = "{v}"' for k, v in keys.items()]
        continue
    if in_section and re.match(r"^\s*(agents_root|workspace_root)\s*=", line):
        continue
    out.append(line)

if not seen_section:
    out += ["", "[agents.defaults]"] + [f'{k} = "{v}"' for k, v in keys.items()]

open(args.path, "w").write("\n".join(out) + "\n")
print(f"[agents.defaults] agents_root={args.agents_root} workspace_root={args.workspace_root}")
