---
name: aleph-e2e-verify
description: >
  Production-level E2E verification for Aleph modules via live server WebSocket.
  Use when: user says "verify", "e2e test", "production verify", "validate module",
  "生产验证", "验证XX模块", or after completing a major feature that adds tools or
  RPC endpoints. Also use when user runs /aleph-e2e-verify [module-name].
  Flags: --debug (dump raw WS messages), --skip-build (reuse last binary).
---

# Aleph E2E Verify

Verify Aleph modules against the live server. Two-layer testing: tool-call verification (LLM picks the right tool) + state verification (database actually changes).

## Seven Steps — No Skipping

| # | Step | Command | Gate |
|---|------|---------|------|
| 1 | **Kill** | `pkill -f aleph-server; sleep 2; ps aux \| grep "[a]leph-server"` | Zero processes running |
| 2 | **Build** | `just build` (or `just build-debug`) | Exit code 0 |
| 3 | **Start** | `target/release/aleph-server start &` then `sleep 5` | — |
| 4 | **Verify** | Check process + TCP port + read shared token | All 3 pass |
| 5 | **Design** | Analyze module → define test phases | Each phase has both layers |
| 6 | **Run** | Generate + execute Python test script | Script completes |
| 7 | **Report** | Summary table with PASS/FAIL + evidence | — |

**Step 1 is CRITICAL** — running multiple aleph instances corrupts the vault (all API keys lost). Always kill first, even if "nothing is running."

### Step 4 detail: Verify Server Ready

```bash
ps aux | grep "[a]leph-server" | grep release          # process alive
lsof -i -P | grep aleph | grep TCP | grep LISTEN       # port listening
sqlite3 ~/.aleph/data/security.db \
  "SELECT plaintext_token FROM shared_token LIMIT 1;"   # get auth token
```

If any check fails, STOP.

### Step 5 detail: Design Test Scenarios

For each test phase, define:
1. **Chat prompt** — natural language that should trigger specific tool(s)
2. **Expected tools** — which `stream.tool_start` events to expect
3. **State check** — which RPC or DB query confirms the operation took effect

**Module analysis**: read `builtin_tools/` for tools, `gateway/handlers/` for RPC, `definitions.rs` for registration.

### Step 6 detail: Generate Test Script

Use `scripts/aleph_e2e_client.py` as the reusable client library. Import `AlephClient`, `print_test`, `print_section`, `get_shared_token`.

For WebSocket protocol details (event methods, field names, auth flow), read [references/websocket-protocol.md](references/websocket-protocol.md).

Save generated test to `tests/{module}_e2e_test.py`.

### Step 7 detail: Report Format

```
=== Aleph E2E Verification Report ===
Module: {name} | Build: {type} ({duration}) | Server: :{port}

| Phase | Test | Result | Evidence |
|-------|------|--------|----------|
| ...   | ...  | PASS   | ...      |

Summary: X/Y PASS
```

On FAIL: list raw tool_calls + state query, suggest causes (tool not registered? store not wired?).
On all PASS: commit test script with `git add -f tests/{module}_e2e_test.py`.

## Red Flags

| Thought | Reality |
|---------|---------|
| "Server's not running, skip kill" | You don't know. `ps aux` first. |
| "Build too slow, test old binary" | Stale code = useless test. |
| "Tool was called, skip state check" | Tool call ≠ state change. Both layers required. |
| "I'll write tests from memory" | Read actual tool implementations first. |
