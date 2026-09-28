#!/usr/bin/env python3
"""Minimal stdio MCP server for the `scope` stage.

The contract is stated ONCE, as the four `match` arms of `run_mock_server` in
`tests/plugin_lifecycle_roundtrip.rs` — this file is a port of that function,
not a second statement of it. Python because every QA driver here is python
and this stage runs the shipped `aleph-server`, not a test-feature build.
"""
import json
import sys

for raw in sys.stdin:
    raw = raw.strip()
    if not raw:
        continue
    try:
        msg = json.loads(raw)
    except json.JSONDecodeError:
        continue
    method = msg.get("method", "")
    # Notifications (no id) get no answer.
    if "id" not in msg:
        continue
    rid = msg["id"]
    if method == "initialize":
        out = {"jsonrpc": "2.0", "id": rid, "result": {
            "protocolVersion": "2025-03-26",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "qa-mock", "version": "0"},
        }}
    elif method == "tools/list":
        out = {"jsonrpc": "2.0", "id": rid, "result": {"tools": [{
            "name": "qa_echo",
            "description": "echoes its input",
            "inputSchema": {"type": "object", "properties": {}},
        }]}}
    elif method == "ping":
        out = {"jsonrpc": "2.0", "id": rid, "result": {}}
    else:
        out = {"jsonrpc": "2.0", "id": rid,
               "error": {"code": -32601, "message": f"method not found: {method}"}}
    sys.stdout.write(json.dumps(out) + "\n")
    sys.stdout.flush()
