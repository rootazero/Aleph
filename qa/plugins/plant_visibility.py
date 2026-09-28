#!/usr/bin/env python3
"""Plant the `visibility` stage's project plugin: one skill, one agent, one MCP server.

  python3 plant_visibility.py <project_root> <mcp_mock_server.py>

Lands in `<project_root>/.aleph/plugins/qa-vis/` — the directory
`collect_plugin_dirs` walks for every project `projects.add` registered, and
the only origin that yields a `ScopeKey::Project` row. A plugin planted under
`$ALEPH_HOME` would be `Global` and visible everywhere, which would prove
discovery, not visibility.

One plugin carries all three components on purpose: the claim is that ONE
visibility answer governs every face a plugin reaches the model through, so
the three arms must share an owner. The manifest is `plant_scope.py`'s shape
(the `scope` stage mounts it today); its `aleph.runtime = "mcp"` is explicit
but no longer required — the `.mcp.json` beside the manifest already makes it
`PluginKind::Mcp` (P4.15). The CC adapter resolves `skills/` and `agents/`
whatever the runtime.

The server id is `plugin:qa-vis/vis`; the name the model sees is that id
sanitised plus `__qa_echo` (`McpHandler::qualified_name`). The driver reads the
exact spelling off the mock's request log rather than re-deriving it here.
"""
import json
import sys
from pathlib import Path

project = Path(sys.argv[1])
mock = Path(sys.argv[2]).resolve()
d = project / ".aleph" / "plugins" / "qa-vis"
(d / ".claude-plugin").mkdir(parents=True, exist_ok=True)
(d / "skills" / "qa-vis-skill").mkdir(parents=True, exist_ok=True)
(d / "agents").mkdir(parents=True, exist_ok=True)
(d / ".claude-plugin" / "plugin.json").write_text(json.dumps({
    "name": "qa-vis",
    "version": "1.0.0",
    "description": "visibility stage: a skill, an agent and an MCP server that exist only in this project",
    "aleph": {"runtime": "mcp"},
}, indent=2))
(d / ".mcp.json").write_text(json.dumps({
    "mcpServers": {"vis": {"command": sys.executable, "args": [str(mock)]}}
}, indent=2))
(d / "skills" / "qa-vis-skill" / "SKILL.md").write_text(
    "---\nname: qa-vis-skill\ndescription: a skill that exists only in this project\n---\nDo the project thing.\n"
)
(d / "agents" / "qa-vis-agent.md").write_text(
    "---\nname: qa-vis-agent\ndescription: a helper that exists only in this project\n---\nYou help.\n"
)
print(f"planted {d}")
