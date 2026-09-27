#!/usr/bin/env python3
"""Plant one MCP-kind plugin whose server is `mcp_mock_server.py`.

`aleph.runtime = "mcp"` is kept as the explicit spelling, not because it is
needed: since P4.15 a CC `plugin.json` that declares `mcpServers` or ships a
`.mcp.json` is `PluginKind::Mcp` without it (`manifest/cc_plugin_json.rs`).

The server lands in `.mcp.json` next to the manifest, the same file
`tests/plugin_lifecycle_roundtrip.rs::plant_plugin` writes. One reader,
`mcp_config::parse_declared_servers`, feeds both the row's
`mcp_servers_count` and the mount's spawn, so the server id is
`plugin:qa-scope/mock` on both. Its command is `sys.executable`, an absolute
interpreter outside the plugin root — accepted (only relative or
`${CLAUDE_PLUGIN_ROOT}`-rooted commands must stay inside the root). No
`commands` field: the CC adapter falls back to `commands/` when the field is
absent (`component_source::resolve_dirs`).
"""
import json
import sys
from pathlib import Path

installed = Path(sys.argv[1])
mock = Path(sys.argv[2]).resolve()
d = installed / "qa-scope"
(d / ".claude-plugin").mkdir(parents=True, exist_ok=True)
(d / "commands").mkdir(parents=True, exist_ok=True)
(d / ".claude-plugin" / "plugin.json").write_text(json.dumps({
    "name": "qa-scope",
    "version": "1.0.0",
    "description": "scope stage: MCP server + one command",
    "aleph": {"runtime": "mcp"},
}, indent=2))
(d / ".mcp.json").write_text(json.dumps({
    "mcpServers": {"mock": {"command": sys.executable, "args": [str(mock)]}}
}, indent=2))
(d / "commands" / "qa-scope-cmd.md").write_text("---\ndescription: scope stage command\n---\nPong\n")
print(f"planted {d}")
