#!/usr/bin/env python3
"""Rewrite `[mcp_server]` in a generated Aleph config, and optionally open the
gateway to the LAN for the `auth` stage.

`toml::to_string` writes `expose = [...]` on one line, but nothing here should
depend on that: the whole `[mcp_server]` section is dropped and re-appended,
the same way `qa/busy_input/patch_config.py` drops `[channels]`/`[providers]`.
"""
import argparse
import re

p = argparse.ArgumentParser()
p.add_argument("path")
p.add_argument("--expose", required=True, help="comma-separated tool names")
p.add_argument("--lan", action="store_true", help='[gateway] host="0.0.0.0" + allow_insecure_remote=true')
args = p.parse_args()

src = open(args.path).read()


def drop_section(text, name):
    out, keep = [], True
    for line in text.splitlines():
        m = re.match(r"^\[+([^\]]+)\]+\s*$", line)
        if m:
            keep = m.group(1).strip() != name
        if keep:
            out.append(line)
    return "\n".join(out) + "\n"


def set_key(text, section, key, value):
    lines = text.splitlines()
    out, cur, inserted = [], None, False
    for line in lines:
        m = re.match(r"^\[+([^\]]+)\]+\s*$", line)
        if m:
            cur = m.group(1).strip()
            out.append(line)
            if cur == section and not inserted:
                out.append(f"{key} = {value}")
                inserted = True
            continue
        if cur == section and re.match(rf"^\s*{re.escape(key)}\s*=", line):
            continue
        out.append(line)
    text = "\n".join(out) + "\n"
    if not inserted:
        text += f"\n[{section}]\n{key} = {value}\n"
    return text


src = drop_section(src, "mcp_server")
names = [n.strip() for n in args.expose.split(",") if n.strip()]
src += "\n[mcp_server]\nenabled = true\nexpose = [" + ", ".join(f'"{n}"' for n in names) + "]\n"

if args.lan:
    src = set_key(src, "gateway", "host", '"0.0.0.0"')
    src = set_key(src, "gateway", "allow_insecure_remote", "true")

open(args.path, "w").write(src)
print(f"[mcp_server].expose = {names}" + (" (LAN open)" if args.lan else ""))
