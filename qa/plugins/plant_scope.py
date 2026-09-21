#!/usr/bin/env python3
"""Plant one MCP-kind plugin whose server is `mcp_mock_server.py`.

`aleph.runtime = "mcp"` is required: a CC `plugin.json` without it parses as
`PluginKind::Static` (`manifest/cc_plugin_json.rs`, the `else` arm of the
`aleph` section) and the `mcp_server` mount step is keyed on that kind
(`extension/lifecycle.rs::mount_parsed`), so its servers are never mounted —
which is also why the `manifest` stage's `qa-inline` fixture can point at
`echo` and pass.

The server lands in `.mcp.json` next to the manifest, the same file
`tests/plugin_lifecycle_roundtrip.rs::plant_plugin` writes; the daemon reads
it through `mcp_config::read_mcp_json` at mount, so the server id is
`plugin:qa-scope/mock`. No `commands` field: the CC adapter falls back to
`commands/` when the field is absent (`component_source::resolve_dirs`).
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
