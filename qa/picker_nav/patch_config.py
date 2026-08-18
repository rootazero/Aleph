#!/usr/bin/env python3
"""Turn a freshly generated Aleph config into an inert Panel-only QA daemon.

No mock provider and no agent turns: every item this fixture exists for is
Panel-side interaction (a keyboard walk, a conditional scroll fade, a phone
add flow), so nothing in the run needs a model. What it *does* need is a
catalogue with a realistic mix — two presets configured, fifty-odd not — since
the picker's whole job is telling those apart, and a fixture where everything
is unconfigured cannot show the "configured" marking at all.

The two configured rows are real catalogue ids (`groq`, `deepseek`), not
invented names: a provider the catalogue has never heard of lands in the
configured section without ever marking a catalogue row, which is exactly the
half the marking is for.
"""
import argparse
import re

p = argparse.ArgumentParser()
p.add_argument("path")
p.add_argument("--gateway-port", required=True)
args = p.parse_args()

src = open(args.path).read()


def drop_sections(text, pred):
    out, keep = [], True
    for line in text.splitlines():
        m = re.match(r"^\[+([^\]]+)\]+\s*$", line)
        if m:
            keep = not pred(m.group(1))
        if keep:
            out.append(line)
    return "\n".join(out) + "\n"


src = drop_sections(src, lambda s: s.startswith(("channels", "providers", "agents")))


def set_key(text, section, key, value):
    lines = text.splitlines()
    out, cur, inserted = [], None, False
    for line in lines:
        m = re.match(r"^\[+([^\]]+)\]+\s*$", line)
        if m:
            cur = m.group(1)
            out.append(line)
            if cur == section:
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


for section, key, value in [
    ("gateway", "host", '"127.0.0.1"'),
    ("gateway", "port", args.gateway_port),
    ("cron", "enabled", "false"),
    ("heartbeat", "enabled", "false"),
    ("mcp", "enabled", "false"),
    ("acp", "enabled", "false"),
    ("evolution", "enabled", "false"),
    ("memory", "enabled", "false"),
    ("skills", "enabled", "false"),
]:
    src = set_key(src, section, key, value)

# `api_key` is skip_serializing but still deserializes, so an inline key keeps
# the run away from the real secrets vault. Nothing dials these — the fixture
# never sends a turn.
#
# `verified = true` is pinned rather than earned: `providers.catalog{view:
# "configured"}` — the view the chat model picker reads — filters on `verified
# && enabled`, and `verified` is only ever set by a real `providers.test`
# round-trip to the vendor. Without this the picker is empty and the keyboard
# walk has nothing but its Default row to walk.
src += """
[providers.groq]
enabled = true
verified = true
protocol = "openai"
base_url = "https://api.groq.com/openai/v1"
api_key = "qa-dummy-not-a-real-key"
models = ["llama-3.3-70b-versatile", "llama-3.1-8b-instant"]
timeout_seconds = 300

[providers.deepseek]
enabled = true
verified = true
protocol = "openai"
base_url = "https://api.deepseek.com/v1"
api_key = "qa-dummy-not-a-real-key"
models = ["deepseek-chat", "deepseek-reasoner"]
timeout_seconds = 300

[[agents.list]]
id = "main"
name = "QA Main"
"""

open(args.path, "w").write(src)
print(f"patched {args.path}")
