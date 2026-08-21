#!/usr/bin/env python3
"""Turn a freshly generated Aleph config into an inert daemon for this fixture.

Two claims are being tested and both are about the *contents* of one store, so
the job here is to make sure nothing else writes it while the browser drives.

## Why there IS a provider, and why it dials a closed port

The first draft of this fixture configured none, on the reasoning that neither
`remember` nor `note_manage` calls a model. The seed then failed on every call
with `tools.invoke requires ToolRegistry (boot phase 2)`: `register_agent_handlers`
selects the real `ExecutionEngine` only when an API key is available, and the
builtin tool registry — the thing `tools.invoke` dispatches through — exists
only on that branch. "No provider" does not mean "tools that need no provider
still work"; it means there is no tool face at all.

So one provider is configured, pointed at a port nothing listens on. It is
enough to take the real-execution branch, and it cannot complete a turn even if
something tried: a dial gets ECONNREFUSED, not a plausible answer.

## No `[[agents.list]]` here

The generated config already ships one (`main`), and appending a second array
entry with the same id is a hard TOML parse failure at boot — the daemon dies
with a "duplicate key list in table agents" TOML error before the gateway is up.

## What stays off

Dreaming, cron and heartbeat are disabled *explicitly*, not merely starved.
"It cannot run because it has no reachable model" is a property of this
machine; "it is disabled" is a property of the fixture, and only the second
survives someone running this with a real key exported. Each is a background
writer of curated memory, which is precisely the concurrent second writer that
would make an assertion about the file's contents unfalsifiable.
"""
import argparse
import re

p = argparse.ArgumentParser()
p.add_argument("path")
p.add_argument("--gateway-port", required=True)
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


src = drop_sections(src, lambda s: s.startswith(("channels", "providers")))


def set_key(text, section, key, value):
    """Set `key` inside `[section]`, creating the section if it is absent."""
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
    ("skills", "enabled", "false"),
    # The one subsystem this fixture needs ON.
    ("memory", "enabled", "true"),
    ("memory.dreaming", "enabled", "false"),
]:
    src = set_key(src, section, key, value)

src += f"""
[providers.qa-dead]
enabled = true
verified = false
protocol = "anthropic"
base_url = "http://127.0.0.1:{args.dead_port}"
api_key = "qa-dummy-not-a-real-key"
models = ["qa-dead-model"]
timeout_seconds = 10
"""

open(args.path, "w").write(src)
print(f"patched {args.path}: gateway :{args.gateway_port}, one unreachable provider, daemons off")
