# MCP Server Configuration Guide

## File Path
- Main: `~/.aleph/mcp_config.json`

## Operation Rules
1. Before modification: `cp ~/.aleph/mcp_config.json ~/.aleph/mcp_config.json.bak`
2. After editing: restart Aleph for changes to take effect (`aleph-server stop` then `start`)
3. File is NOT auto-reloaded and there is no MCP management tool — a restart is the only way to apply changes

## Structure

```json
{
  "version": 1,
  "servers": {
    "server-id": {
      "id": "server-id",
      "name": "Display Name",
      "transport": "stdio",
      "command": "/path/to/server",
      "args": ["--flag", "value"],
      "url": null,
      "env": {
        "API_KEY": "${MY_API_KEY}",
        "HOME": "${HOME}"
      },
      "requires_runtime": "node",
      "auto_start": true,
      "timeout_seconds": 60
    }
  }
}
```

## Field Reference

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `id` | Yes | — | Unique server identifier |
| `name` | No | same as id | Display name |
| `transport` | No | "stdio" | "stdio", "http", or "sse" |
| `command` | Yes (stdio) | — | Executable path |
| `args` | No | [] | Command arguments |
| `url` | Yes (http/sse) | — | Server URL |
| `env` | No | {} | Environment variables |
| `requires_runtime` | No | null | "node", "python", etc. |
| `auto_start` | No | true | Start on server boot |
| `timeout_seconds` | No | 60 | Connection timeout |

## Common Operations

### Add a new MCP server
Add entry to `servers` object, then restart Aleph to start it.

### Remove an MCP server
Delete entry from `servers` object, then restart Aleph to stop it.

### Disable without removing
Set `"auto_start": false` — server won't start on boot.

## Caveats
- Environment variables use `${VAR}` syntax — unknown vars left as-is
- For Node.js servers, use `"requires_runtime": "node"`
- The `version` field must be `1`
- JSON must be valid — use `file_read` (or `bash` cat) after writing to verify
