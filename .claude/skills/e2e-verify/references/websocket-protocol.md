# Aleph WebSocket Protocol Reference

## Connection

- Endpoint: `ws://127.0.0.1:{port}/ws` (default port: 18790)
- Auth: first message must be `connect` RPC with `shared_token`
- Token source: `sqlite3 ~/.aleph/data/security.db "SELECT plaintext_token FROM shared_token LIMIT 1;"`

## JSON-RPC Format

Request: `{"jsonrpc": "2.0", "id": N, "method": "...", "params": {...}}`
Response: `{"jsonrpc": "2.0", "id": N, "result": {...}}` or `{"jsonrpc": "2.0", "id": N, "error": {...}}`
Notification: `{"method": "stream.*", "params": {...}}` (no `id`)

## Streaming Events (notifications during `chat.send`)

| Method | Key Fields (in `params`) | Description |
|--------|--------------------------|-------------|
| `stream.run_accepted` | `run_id`, `session_key` | Run started |
| `stream.response_chunk` | `delta`, `full_text`, `seq`, `is_final` | Text token |
| `stream.tool_start` | `tool_name`, `tool_id`, `params`, `seq` | Tool invocation began |
| `stream.tool_end` | `result`, `duration_ms`, `tool_id`, `seq` | Tool completed |
| `stream.run_complete` | `run_id`, `summary`, `total_duration_ms` | Run finished |
| `stream.run_failed` | `error`, `run_id` | Run errored |

All `params` are nested inside `data.params` — access as `data["params"]["tool_name"]`.

## Useful RPC Methods

| Method | Params | Returns | Use For |
|--------|--------|---------|---------|
| `connect` | `shared_token`, `device_name` | `token`, `device_id` | Authentication |
| `chat.send` | `message`, `agent_id` | `run_id`, `session_key` | Start chat |
| `teams.list` | — | `[TeamSummary]` | Verify team exists |
| `teams.get` | `team_id` | Team + members + tasks | Verify team state |

## Existing Test Scripts

- `tests/teams_e2e_test.py` — Teams module (9 tools, 8 phases)
- `tests/tc2_test_suite.py` — General test suite
