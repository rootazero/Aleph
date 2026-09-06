#!/bin/zsh
BIN="$1"
{
  echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"spike","version":"0"}}}'
  sleep 1
  echo '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}'
  sleep 0.3
  echo '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
  sleep 2
} | "$BIN" mcp 2>mcp.stderr
