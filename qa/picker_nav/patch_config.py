#!/usr/bin/env python3
"""Turn a freshly generated Aleph config into an inert Panel-only QA daemon.

No agent turns: every item this fixture exists for is Panel-side interaction (a
keyboard walk, a conditional scroll fade, a phone add flow), so nothing here
sends a message to a model. What it *does* need is a catalogue with a realistic
mix — presets configured, fifty-odd not — since the picker's whole job is
telling those apart, and a fixture where everything is unconfigured cannot show
the "configured" marking at all.

The two preset rows are real catalogue ids (`groq`, `deepseek`), not invented
names, because item 10 is about clicking an already-configured **preset** row
and getting its existing editor instead of a setup form.

(An earlier version of this note justified that choice by claiming a provider
the catalogue has never heard of "lands in the configured section without ever
marking a catalogue row". The two rows below disproved it on 2026-08-18: the
server's catalogue carries configured non-preset providers as rows of their
own, and `qa-mock` / `qa-dead` show up at the end of the picker wearing the
same 「已配置」 badge. The choice is still right; the reason given for it was
not.)

Two further rows exist for item 12 ("test connection"), which is the one item
that is *not* purely Panel-side — the button's whole content is a round-trip
that leaves the process:

  * `qa-mock` points at the local stub (`mock_provider.py`) and ships
    **`verified = false`**. The presets above pin `verified = true` because the
    chat model picker filters on it; this row must not, because the assertion
    is that a phone-only owner can *earn* it — `providers.test` is the only
    writer of `verified` in the whole system, so false→true in config.toml is
    the effect, and pinning it true would assert nothing.
  * `qa-dead` points at a **closed port** on the same host. Its failure is a
    real refusal from the real client stack, not a stub's idea of one, and it
    gives item 12's second assertion (a verdict is keyed by provider, not a
    bare bool shared by every row) two rows with *opposite* verdicts to walk
    between — two successes could not tell a stale verdict from a fresh one.
"""
import argparse
import re

p = argparse.ArgumentParser()
p.add_argument("path")
p.add_argument("--gateway-port", required=True)
p.add_argument("--mock-port", required=True)
p.add_argument("--dead-port", required=True)
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

# The item-12 rows, deliberately AFTER the preset block above so the comment
# explaining why those pin `verified = true` still sits on the block it
# describes. These two pin it FALSE — see the module docstring.
src += f"""
[providers.qa-mock]
enabled = true
verified = false
protocol = "anthropic"
base_url = "http://127.0.0.1:{args.mock_port}"
api_key = "qa-dummy-not-a-real-key"
models = ["qa-mock-model"]
timeout_seconds = 30

[providers.qa-dead]
enabled = true
verified = false
protocol = "anthropic"
base_url = "http://127.0.0.1:{args.dead_port}"
api_key = "qa-dummy-not-a-real-key"
models = ["qa-dead-model"]
timeout_seconds = 30
"""

open(args.path, "w").write(src)
print(f"patched {args.path}")
