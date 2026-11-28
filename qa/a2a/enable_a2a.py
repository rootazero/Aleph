#!/usr/bin/env python3
"""Enable only the opt-in loopback A2A route in a scratch QA config."""
import re
import sys
from pathlib import Path

config = Path(sys.argv[1])
gateway_port, mock_port = sys.argv[2:4]
text = config.read_text()


def set_value(source: str, section: str, key: str, value: str) -> str:
    lines = source.splitlines()
    out = []
    current = None
    found_section = False
    inserted = False
    for line in lines:
        match = re.match(r"^\s*\[([^\]]+)\]\s*$", line)
        if match:
            if current == section and not inserted:
                out.append(f"{key} = {value}")
                inserted = True
            current = match.group(1)
            found_section |= current == section
            out.append(line)
            continue
        if current == section and re.match(rf"^\s*{re.escape(key)}\s*=", line):
            continue
        out.append(line)
    if found_section:
        if not inserted:
            out.append(f"{key} = {value}")
    else:
        if out and out[-1].strip():
            out.append("")
        out.extend([f"[{section}]", f"{key} = {value}"])
    return "\n".join(out) + "\n"


for section, key, value in [
    ("gateway", "host", '"127.0.0.1"'),
    ("gateway", "port", gateway_port),
    ("a2a", "enabled", "true"),
    ("a2a.server.security", "local_bypass", "true"),
    ("cron", "enabled", "false"),
    ("heartbeat", "enabled", "false"),
    ("mcp", "enabled", "false"),
    ("acp", "enabled", "false"),
    ("evolution", "enabled", "false"),
    ("memory", "enabled", "false"),
    ("skills", "enabled", "false"),
]:
    text = set_value(text, section, key, value)

text += f'''\n[providers.qa-mock]\nenabled = true\nprotocol = "anthropic"\nbase_url = "http://127.0.0.1:{mock_port}"\napi_key = "qa-dummy-not-a-real-key"\nmodels = ["qa-mock-model"]\ntimeout_seconds = 600\nstream_idle_timeout_secs = 0\n\n[[agents.list]]\nid = "main"\nname = "QA Main"\ndefault = true\nmodel = "qa-mock-model"\nprovider = "qa-mock"\nsystem_prompt = "A2A SDK interoperability fixture."\n'''
config.write_text(text)
print(f"patched isolated config: Gateway 127.0.0.1:{gateway_port}, local A2A bypass, mock provider 127.0.0.1:{mock_port}")
