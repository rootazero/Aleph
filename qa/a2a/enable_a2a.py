#!/usr/bin/env python3
"""Configure the scratch Gateway with A2A and the controlled provider."""
import re
import subprocess
import sys
from pathlib import Path

config = Path(sys.argv[1])
gateway_port, mock_port = sys.argv[2:4]
patch_provider = Path(__file__).resolve().parents[1] / "busy_input" / "patch_config.py"
subprocess.run(
    [sys.executable, str(patch_provider), str(config),
     "--gateway-port", gateway_port, "--mock-port", mock_port],
    check=True,
)
text = config.read_text()


def set_value(source: str, section: str, key: str, value: str) -> str:
    lines = source.splitlines()
    out = []
    current = None
    found_section = False
    inserted = False
    for line in lines:
        match = re.match(r"^\s*\[+([^\]]+)\]+\s*$", line)
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
    ("a2a", "enabled", "true"),
    ("a2a.server.security", "local_bypass", "true"),
    ("a2a.server.security", "tokens", '["qa-a2a-owner", "qa-a2a-other"]'),
]:
    text = set_value(text, section, key, value)

config.write_text(text)
print(f"patched isolated config: Gateway 127.0.0.1:{gateway_port}, local A2A bypass, mock provider 127.0.0.1:{mock_port}")
