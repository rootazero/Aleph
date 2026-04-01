#!/bin/bash
# Simulates Claude Code CLI oneshot: JSON output with "result" field
prompt="$*"
echo "{\"type\":\"result\",\"result\":\"echo: ${prompt}\"}"
