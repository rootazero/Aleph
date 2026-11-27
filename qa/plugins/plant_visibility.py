#!/usr/bin/env python3
"""Plant global and project copies of one plugin for the visibility stage."""
import json
import sys
from pathlib import Path

project = Path(sys.argv[1])
mock = Path(sys.argv[2]).resolve()
global_root = Path(sys.argv[3])

for root, suffix in [(project / ".aleph" / "plugins", "project"), (global_root, "global")]:
    plugin = root / "qa-vis"
    skill_name = f"qa-vis-{suffix}-skill"
    agent_name = f"qa-vis-{suffix}-agent"
    (plugin / ".claude-plugin").mkdir(parents=True, exist_ok=True)
    (plugin / "skills" / skill_name).mkdir(parents=True, exist_ok=True)
    (plugin / "agents").mkdir(parents=True, exist_ok=True)
    (plugin / "commands").mkdir(parents=True, exist_ok=True)
    (plugin / "commands" / "echo.md").write_text(
        f"---\ndescription: The {suffix} fixture command\n---\nQA_COMMAND_{suffix}: $1\n"
    )
    (plugin / ".claude-plugin" / "plugin.json").write_text(json.dumps({
        "name": "qa-vis",
        "version": "1.0.0",
        "description": f"QA visibility {suffix} plugin",
        "aleph": {"runtime": "mcp"},
    }, indent=2))
    (plugin / ".mcp.json").write_text(json.dumps({
        "mcpServers": {"vis": {"command": sys.executable, "args": [str(mock)]}},
    }, indent=2))
    (plugin / "skills" / skill_name / "SKILL.md").write_text(
        f"---\nname: {skill_name}\ndescription: The {suffix} fixture skill\n---\nQA skill.\n"
    )
    (plugin / "agents" / f"{agent_name}.md").write_text(
        f"---\nname: {agent_name}\ndescription: The {suffix} fixture agent\n---\nQA agent.\n"
    )
    print(f"planted {plugin}")
