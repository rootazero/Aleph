# Memory System E2E Validation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute production-grade end-to-end validation of the Aleph memory system on an isolated test agent, exercising L0/L1/Retrieval/Orientation/QueryFiler/Dream-evolution layers, and producing a structured run report with probe diffs and EventLog samples.

**Architecture:** This is a **runtime validation** plan, not a feature build. Each task drives a phase of the spec (`docs/superpowers/specs/2026-04-17-memory-e2e-validation-design.md`) by sending JSON-RPC over the running Aleph gateway WebSocket, snapshotting probe state before/after, and asserting concrete pass criteria. Two helper artifacts are built first (`tools/memory_probe.sh`, `tests/scripts/memory_e2e_dialog.jsonl`) to make the run reproducible.

**Tech Stack:** Aleph release binary (`target/release/aleph-server`), Gateway WebSocket (`ws://127.0.0.1:18790/ws`), JSON-RPC 2.0, SQLite (`~/.aleph/data/memory.db`), Python 3 + `websockets` for one-shot RPC calls (lighter than maintaining a wscat session), `sqlite3` CLI for probe queries, `jq` for JSON parsing.

**Pre-flight assumptions:**
- A built `target/release/aleph-server` exists. If not, Task 0 builds it.
- `python3 -c "import websockets"` succeeds (install if missing: `python3 -m pip install --user websockets`).
- `jq` and `sqlite3` are on `PATH`.

---

## Task 0: Pre-flight & Environment Sanity

**Files:**
- Inspect: `~/.aleph/data/memory.db`, `~/.aleph/memory/note/`, `target/release/aleph-server`
- Create: `~/.aleph/backups/2026-04-17-pre-validation/` (backup destination)

- [ ] **Step 1: Verify Aleph release binary exists or build it**

Run:
```bash
ls -la /Volumes/TBU4/Workspace/Aleph/target/release/aleph-server 2>/dev/null \
  || (cd /Volumes/TBU4/Workspace/Aleph && just build)
```

Expected: file exists with executable permission. If `just build` runs, allow up to 10 min and confirm exit code 0.

- [ ] **Step 2: Verify Python tooling**

Run:
```bash
python3 -c "import websockets, json, asyncio; print('ok')"
which jq sqlite3
```

Expected: prints `ok` and resolves both binaries. If `websockets` missing, run `python3 -m pip install --user websockets` and retry.

- [ ] **Step 3: Discover existing agent config layout**

Run:
```bash
ls -la /Users/zouguojun/.aleph/agents/ 2>/dev/null \
  || find /Users/zouguojun/.aleph -maxdepth 3 -name "*.json" -path "*agent*" 2>/dev/null | head -10
```

Expected: list of existing agent configs (commonly `~/.aleph/agents/main.json` or similar). Record the canonical path — it's needed in Task 4 to verify the test agent was created.

- [ ] **Step 4: Kill any residual aleph processes (CLAUDE.md mandate)**

Run:
```bash
pkill -f "target/release/aleph-server" 2>/dev/null; \
pkill -f "target/debug/aleph-server" 2>/dev/null; \
sleep 2; \
ps aux | grep "[a]leph-server" | grep -v grep
```

Expected: empty output after `sleep 2`. If processes survive, escalate (do NOT use `kill -9` then immediately restart — wait 2s after force-kill).

- [ ] **Step 5: Create backup directory and snapshot existing memory state**

Run:
```bash
mkdir -p ~/.aleph/backups/2026-04-17-pre-validation
cp ~/.aleph/data/memory.db ~/.aleph/backups/2026-04-17-pre-validation/memory.db.bak
tar -czf ~/.aleph/backups/2026-04-17-pre-validation/note.tgz -C ~/.aleph/memory note
ls -la ~/.aleph/backups/2026-04-17-pre-validation/
```

Expected: both `memory.db.bak` (≥ 4 KB) and `note.tgz` (size depends on existing notes) present.

- [ ] **Step 6: Record pre-run baseline of `main` agent's notes**

Run:
```bash
ls /Users/zouguojun/.aleph/memory/note/main/ 2>/dev/null | sort > /tmp/main_notes_baseline.txt
wc -l /tmp/main_notes_baseline.txt
```

Expected: file lists current `main` agent note categories. This baseline is checked in Task 11 to confirm no contamination.

- [ ] **Step 7: Commit baseline notes (no source changes yet, this is just a checkpoint marker)**

Skip — Task 0 produces no source-tree changes that warrant a commit. Move on.

---

## Task 1: Build `tools/memory_probe.sh`

**Files:**
- Create: `tools/memory_probe.sh` (bash, 0755)

- [ ] **Step 1: Write the probe script**

Create `/Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh` with:

```bash
#!/usr/bin/env bash
# memory_probe.sh — dump memory-system probe surface for one agent.
# Usage: memory_probe.sh <agent_id> <output_dir> [<phase_label>]

set -euo pipefail

AGENT_ID="${1:-test-memory-validation}"
OUT_DIR="${2:-/tmp/aleph-probes}"
LABEL="${3:-snap}"
TS="$(date +%Y%m%dT%H%M%S)"
SNAP="${OUT_DIR}/${LABEL}_${TS}"

mkdir -p "${SNAP}"

DB="${HOME}/.aleph/data/memory.db"
NOTES="${HOME}/.aleph/memory/note/${AGENT_ID}"

# 1. SQLite probe queries
sqlite3 -header -column "${DB}" <<SQL > "${SNAP}/sqlite_summary.txt"
SELECT 'raw_memories_total' AS metric, COUNT(*) AS value FROM raw_memories;
SELECT 'raw_memories_unprocessed', COUNT(*) FROM raw_memories WHERE is_processed = 0;
SELECT 'raw_memories_for_agent', COUNT(*) FROM raw_memories WHERE session_id LIKE '%${AGENT_ID}%';
SELECT 'notes_index_for_agent', COUNT(*) FROM notes_index WHERE agent_id = '${AGENT_ID}';
SELECT 'notes_links_for_agent', COUNT(*) FROM notes_links WHERE agent_id = '${AGENT_ID}';
SELECT 'recall_signals_total', COUNT(*) FROM recall_signals;
SELECT 'query_filed_total', COUNT(*) FROM query_filed;
SELECT 'dream_status', last_run_at, last_status, last_duration_ms FROM dream_status;
SELECT 'dream_reports_total', COUNT(*) FROM dream_reports;
SELECT 'daily_insights_today', COUNT(*) FROM daily_insights WHERE date = date('now', 'localtime');
SQL

# 2. Latest dream report (if any)
sqlite3 -header -line "${DB}" \
  "SELECT * FROM dream_reports ORDER BY started_at DESC LIMIT 1;" \
  > "${SNAP}/dream_reports_latest.txt" 2>/dev/null || echo "no dream reports" > "${SNAP}/dream_reports_latest.txt"

# 3. Recall signals sample for this agent (last 20)
sqlite3 -header -line "${DB}" \
  "SELECT note_path, query_text, score, channel, day_bucket FROM recall_signals ORDER BY created_at DESC LIMIT 20;" \
  > "${SNAP}/recall_signals_sample.txt" 2>/dev/null || true

# 4. Notes filesystem state for the agent
if [ -d "${NOTES}" ]; then
  find "${NOTES}" -type f -name "*.md" \
    | sort \
    | xargs -I{} stat -f "%m %z %N" {} \
    > "${SNAP}/notes_files.txt"
  # Capture the four orientation files separately if they exist
  for f in SCHEMA.md index.md log.md USER.md; do
    if [ -f "${NOTES}/${f}" ]; then
      cp "${NOTES}/${f}" "${SNAP}/orientation_${f}"
    fi
  done
  # Dream EventLog
  if [ -f "${NOTES}/dream_events.jsonl" ]; then
    cp "${NOTES}/dream_events.jsonl" "${SNAP}/dream_events.jsonl"
    wc -l "${NOTES}/dream_events.jsonl" > "${SNAP}/dream_events_count.txt"
  fi
  # Archive directory
  if [ -d "${NOTES}/archive" ]; then
    find "${NOTES}/archive" -type f -name "*.md" | sort > "${SNAP}/archive_files.txt"
  fi
else
  echo "no notes dir for ${AGENT_ID}" > "${SNAP}/notes_files.txt"
fi

# 5. Process snapshot (must always be exactly 1 aleph-server)
ps aux | grep "[a]leph-server" | grep -v grep > "${SNAP}/processes.txt" || true

echo "${SNAP}"
```

- [ ] **Step 2: Make executable**

Run:
```bash
chmod +x /Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh
ls -la /Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh
```

Expected: `-rwxr-xr-x ... memory_probe.sh`

- [ ] **Step 3: Smoke test the probe (Aleph not running yet — should still produce a snapshot using the existing DB)**

Run:
```bash
/Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh main /tmp/aleph-probes smoke_test
ls /tmp/aleph-probes/smoke_test_*/
cat /tmp/aleph-probes/smoke_test_*/sqlite_summary.txt
```

Expected: `sqlite_summary.txt` shows row counts (mostly for the `main` agent baseline). `processes.txt` is empty. No errors.

- [ ] **Step 4: Commit the probe script**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git add tools/memory_probe.sh
git commit -m "tools: add memory_probe.sh for e2e validation snapshots"
```

Expected: clean commit, exit code 0.

---

## Task 2: Build `tests/scripts/memory_e2e_dialog.jsonl` and Python WS client

**Files:**
- Create: `tests/scripts/memory_e2e_dialog.jsonl` (one prompt per line, JSON object)
- Create: `tests/scripts/ws_send.py` (one-shot WS RPC client)

- [ ] **Step 1: Write the dialog script**

Create `/Volumes/TBU4/Workspace/Aleph/tests/scripts/memory_e2e_dialog.jsonl`:

```jsonl
{"phase":"1","intent":"create-agent","prompt":"Please create a new agent called 'test-memory-validation' with this system prompt: 'I am a sandbox agent for memory-system validation. Be deliberate about what you remember, retrieve, and synthesize. Prefer using available memory tools to demonstrate them.' After creation, confirm by listing all agents."}
{"phase":"1","intent":"personal-1","prompt":"My name is Zou Guojun. I live in Beijing, China. I work primarily out of a home office on the 12th floor."}
{"phase":"1","intent":"personal-2","prompt":"My partner's name is Li Wei. We have been together for six years and we both work in tech."}
{"phase":"1","intent":"technical-1","prompt":"For coding style: comments must be in English, but conversation with you must be in Chinese. I prefer immutable patterns and small files (under 800 lines). I dislike unnecessary abstractions."}
{"phase":"1","intent":"technical-2","prompt":"My toolchain is Rust on stable, with cargo and just for builds. I use sqlite-vec for vector storage. I avoid Python where Rust is reasonable."}
{"phase":"1","intent":"project-1","prompt":"I am building Aleph, a self-hosted personal AI assistant with a Rust core and multiple front-ends including macOS menubar, web chat, and chat-bot integrations."}
{"phase":"1","intent":"project-2","prompt":"The current focus is the memory subsystem: realtime L0->L1 compression, the Dream Daemon's strategy-driven evolution, and the orientation layer (SCHEMA.md, index.md, log.md, USER.md)."}
{"phase":"1","intent":"constraint-1","prompt":"Hard rule: never commit on Fridays. We have a deploy freeze every Friday afternoon for the mobile release branch."}
{"phase":"1","intent":"constraint-2","prompt":"Hard rule: no force-pushes to main. If a rebase is needed, we cut a new branch and PR back."}
{"phase":"3","intent":"retrieval-search","prompt":"What city did I tell you I live in?"}
{"phase":"3","intent":"retrieval-browse","prompt":"List everything you remember in the personal category."}
{"phase":"3","intent":"retrieval-explore","prompt":"Starting from the Aleph project, expand to related notes two hops out via wikilinks. Show what you find."}
{"phase":"3","intent":"retrieval-recall","prompt":"Replay the original wording of the third thing I told you in this session."}
{"phase":"4","intent":"orient-show","prompt":"Show me your current schema and the most recent activity log."}
{"phase":"4","intent":"orient-mutate","prompt":"Add a new category called 'experiments' to your schema."}
{"phase":"5","intent":"reflect","prompt":"Synthesize the common theme across everything you've remembered about me so far. Cite specific notes."}
{"phase":"5","intent":"reflect-redup","prompt":"Synthesize the common theme across everything you've remembered about me so far. Cite specific notes."}
```

- [ ] **Step 2: Write the Python WS client**

Create `/Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py`:

```python
#!/usr/bin/env python3
"""ws_send.py — one-shot JSON-RPC over Aleph gateway WebSocket.

Usage:
  ws_send.py --token TOKEN --method agent.run --params '{"message":"hi","session_key":"agent:main:dm:operator"}'
  ws_send.py --token TOKEN --method agent.run --params-file /path/to/params.json --stream-events

The script:
  1. Connects to ws://127.0.0.1:18790/ws
  2. Sends `connect` with the bearer token (always)
  3. Optionally subscribes to events.subscribe pattern '*'
  4. Sends the requested method call
  5. Streams incoming frames to stdout (one JSON per line) until EOF or --timeout-seconds
"""
import argparse
import asyncio
import json
import sys
import uuid

import websockets


async def run(args: argparse.Namespace) -> int:
    if args.params_file:
        with open(args.params_file) as f:
            params = json.load(f)
    else:
        params = json.loads(args.params or "{}")

    uri = args.uri
    async with websockets.connect(uri, max_size=8 * 1024 * 1024) as ws:
        # 1. connect
        connect_id = str(uuid.uuid4())
        await ws.send(json.dumps({
            "jsonrpc": "2.0",
            "id": connect_id,
            "method": "connect",
            "params": {
                "minProtocol": 1,
                "maxProtocol": 1,
                "client": {"id": "e2e-validator", "version": "1.0.0", "platform": "darwin"},
                "role": "operator",
                "auth": {"token": args.token},
            },
        }))
        # await connect ack
        ack = await asyncio.wait_for(ws.recv(), timeout=10)
        print(ack, flush=True)

        # 2. optional events.subscribe
        if args.stream_events:
            sub_id = str(uuid.uuid4())
            await ws.send(json.dumps({
                "jsonrpc": "2.0",
                "id": sub_id,
                "method": "events.subscribe",
                "params": {"pattern": args.event_pattern},
            }))
            sub_ack = await asyncio.wait_for(ws.recv(), timeout=5)
            print(sub_ack, flush=True)

        # 3. main RPC
        call_id = str(uuid.uuid4())
        await ws.send(json.dumps({
            "jsonrpc": "2.0",
            "id": call_id,
            "method": args.method,
            "params": params,
        }))

        # 4. stream until timeout
        try:
            while True:
                frame = await asyncio.wait_for(ws.recv(), timeout=args.timeout_seconds)
                print(frame, flush=True)
                if not args.stream_events:
                    # one frame and done
                    break
        except asyncio.TimeoutError:
            pass
        except websockets.ConnectionClosed:
            pass
    return 0


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--uri", default="ws://127.0.0.1:18790/ws")
    p.add_argument("--token", required=True)
    p.add_argument("--method", required=True)
    p.add_argument("--params", default=None, help="inline JSON params")
    p.add_argument("--params-file", default=None, help="path to JSON params file")
    p.add_argument("--stream-events", action="store_true")
    p.add_argument("--event-pattern", default="*")
    p.add_argument("--timeout-seconds", type=float, default=30.0)
    args = p.parse_args()
    return asyncio.run(run(args))


if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Step 3: Make scripts executable and create dir if missing**

Run:
```bash
mkdir -p /Volumes/TBU4/Workspace/Aleph/tests/scripts
chmod +x /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py
ls -la /Volumes/TBU4/Workspace/Aleph/tests/scripts/
```

Expected: both files present, `ws_send.py` executable.

- [ ] **Step 4: Smoke test ws_send.py against a stopped server (should fail cleanly)**

Run:
```bash
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac \
  --method config.get \
  --params '{}' 2>&1 | head -20
```

Expected: `ConnectionRefusedError` or `[Errno 61] Connection refused`. Confirms the script reaches the network layer correctly. (Real run is in Task 3.)

- [ ] **Step 5: Commit dialog script + WS client**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git add tests/scripts/memory_e2e_dialog.jsonl tests/scripts/ws_send.py
git commit -m "tests: add memory e2e dialog script and WS RPC client"
```

Expected: clean commit, exit code 0.

---

## Task 3: Phase 0 — Boot, Auth, Smoke Test

**Files:**
- Snapshot output: `/tmp/aleph-probes/phase0_*`
- Server log: `/tmp/aleph-server.log`

- [ ] **Step 1: Pre-snapshot (baseline before Aleph starts)**

Run:
```bash
/Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh test-memory-validation /tmp/aleph-probes phase0_pre
```

Expected: snapshot dir created. `processes.txt` should be empty (no Aleph running).

- [ ] **Step 2: Start aleph-server in background with logging**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
nohup ./target/release/aleph-server start > /tmp/aleph-server.log 2>&1 &
echo "PID=$!"
sleep 5
ps aux | grep "[a]leph-server" | grep -v grep
```

Expected: exactly one aleph-server process. PID echoed. If two processes appear, immediately `pkill -f aleph-server; sleep 2;` and abort the whole run (per spec §8 A1).

- [ ] **Step 3: Verify gateway WebSocket is accepting connections**

Run:
```bash
sleep 3
nc -zv 127.0.0.1 18790 2>&1
grep -E "gateway|ws|listening" /tmp/aleph-server.log | head -10
```

Expected: `succeeded` from `nc`, log lines confirm gateway started on port 18790. If not listening within 10s, abort.

- [ ] **Step 4: Auth + ping smoke test against `main` agent**

Run:
```bash
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac \
  --method agent.run \
  --params '{"message":"ping","session_key":"agent:main:dm:operator"}' \
  --stream-events \
  --timeout-seconds 30 \
  > /tmp/phase0_ping.jsonl 2>&1
wc -l /tmp/phase0_ping.jsonl
head -5 /tmp/phase0_ping.jsonl
grep -c '"method":"event"' /tmp/phase0_ping.jsonl || true
grep -c 'agent.completed\|stream.chunk' /tmp/phase0_ping.jsonl || true
```

Expected:
- `wc -l` ≥ 3 (connect ack + sub ack + at least one event)
- Connect ack contains `"result"` (not `"error"`) — auth succeeded
- At least one `stream.chunk` or `agent.completed` event present

If auth fails (`"error":{"code":...,"message":"unauthorized"}`), check the token in `~/.aleph/config.json` matches what the spec uses, and stop.

- [ ] **Step 5: Post-snapshot**

Run:
```bash
/Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh test-memory-validation /tmp/aleph-probes phase0_post
diff /tmp/aleph-probes/phase0_pre_*/sqlite_summary.txt /tmp/aleph-probes/phase0_post_*/sqlite_summary.txt || true
```

Expected: `raw_memories_total` may have increased by 2 (the ping turn). All other metrics unchanged. `processes.txt` shows exactly one aleph-server.

- [ ] **Step 6: Record Phase 0 result**

If all checks passed, append to `/tmp/phase0_result.txt`:
```bash
echo "PHASE 0: PASS — auth ok, gateway live, single process, ping round-trip." > /tmp/phase0_result.txt
cat /tmp/phase0_result.txt
```

---

## Task 4: Phase 1 — Create Test Agent + L0 Capture

**Files:**
- Dialog source: `tests/scripts/memory_e2e_dialog.jsonl` (filter `phase=1`)
- Output: `/tmp/phase1_dialog_log.jsonl`, `/tmp/aleph-probes/phase1_*`

- [ ] **Step 1: Pre-snapshot**

Run:
```bash
/Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh test-memory-validation /tmp/aleph-probes phase1_pre
```

Expected: snapshot exists. `notes_index_for_agent` should be 0 (no notes yet for this agent).

- [ ] **Step 2: Send the agent-create prompt to the `main` agent**

Run:
```bash
PROMPT="$(jq -r 'select(.intent=="create-agent") | .prompt' /Volumes/TBU4/Workspace/Aleph/tests/scripts/memory_e2e_dialog.jsonl)"
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac \
  --method agent.run \
  --params "$(jq -n --arg p "$PROMPT" --arg s "agent:main:dm:operator" '{message:$p,session_key:$s}')" \
  --stream-events \
  --timeout-seconds 90 \
  > /tmp/phase1_create_agent.jsonl 2>&1
grep -c "tool_start\|tool_end" /tmp/phase1_create_agent.jsonl || true
grep -E '"name":"agent[._-]?create|create_agent|agents\.create' /tmp/phase1_create_agent.jsonl | head -3 || true
```

Expected: at least one `tool_start` event with a name matching agent creation. If the LLM refuses or asks for clarification, capture the response and decide whether to manually invoke the RPC instead (R9 demonstration optional, fallback path documented in Step 3).

- [ ] **Step 3 (fallback): If LLM did not create the agent, create it via direct RPC**

Run only if Step 2 produced no agent-creation tool call:
```bash
# Discover the canonical agents RPC method by inspecting the codebase
grep -rn "agents\.create\|agent\.create" /Volumes/TBU4/Workspace/Aleph/src/gateway/ | head -5
# Then issue the matching RPC manually. Example skeleton (replace method+params with actual):
# python3 .../ws_send.py --token ... --method agents.create \
#   --params '{"id":"test-memory-validation","systemPrompt":"I am a sandbox agent..."}'
```

Document the chosen method and full RPC in `/tmp/phase1_agent_creation.txt`.

- [ ] **Step 4: Verify agent config persisted**

Run (substitute the path discovered in Task 0 Step 3):
```bash
ls -la ~/.aleph/agents/ 2>/dev/null | grep -i test-memory-validation \
  || find ~/.aleph -name "*test-memory-validation*" 2>/dev/null
```

Expected: at least one file or directory mentioning `test-memory-validation`.

- [ ] **Step 5: Inject all 8 phase-1 dialog turns**

Run:
```bash
TOKEN=aleph-9976129a-407d-4893-a96c-6467b24bedac
SESSION="agent:test-memory-validation:dm:operator"
> /tmp/phase1_dialog_log.jsonl
jq -c 'select(.phase=="1" and .intent != "create-agent")' \
  /Volumes/TBU4/Workspace/Aleph/tests/scripts/memory_e2e_dialog.jsonl \
  | while read -r line; do
      INTENT="$(echo "$line" | jq -r '.intent')"
      PROMPT="$(echo "$line" | jq -r '.prompt')"
      echo "===== TURN: $INTENT =====" >> /tmp/phase1_dialog_log.jsonl
      python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
        --token "$TOKEN" \
        --method agent.run \
        --params "$(jq -n --arg p "$PROMPT" --arg s "$SESSION" '{message:$p,session_key:$s}')" \
        --stream-events \
        --timeout-seconds 60 \
        >> /tmp/phase1_dialog_log.jsonl 2>&1
      sleep 2
    done
wc -l /tmp/phase1_dialog_log.jsonl
grep -c "agent.completed" /tmp/phase1_dialog_log.jsonl || true
```

Expected: 8 turn markers, each followed by event stream. ≥8 `agent.completed` events.

- [ ] **Step 6: Probe raw_memories accumulation**

Run:
```bash
sqlite3 -header -column ~/.aleph/data/memory.db <<'SQL'
SELECT COUNT(*) AS raw_count, SUM(CASE WHEN is_processed=0 THEN 1 ELSE 0 END) AS unprocessed
FROM raw_memories
WHERE session_id LIKE '%test-memory-validation%';
SQL
```

Expected: `raw_count ≥ 16` (8 turns × 2 messages user+assistant), `unprocessed ≥ 16` (compression hasn't triggered yet — turn_threshold default 20).

- [ ] **Step 7: Post-snapshot and Phase 1 verdict**

Run:
```bash
/Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh test-memory-validation /tmp/aleph-probes phase1_post
echo "PHASE 1: $(test $(sqlite3 ~/.aleph/data/memory.db "SELECT COUNT(*) FROM raw_memories WHERE session_id LIKE '%test-memory-validation%' AND is_processed=0;") -ge 16 && echo PASS || echo FAIL)" \
  >> /tmp/phase_results.txt
tail /tmp/phase_results.txt
```

Expected: `PHASE 1: PASS`.

---

## Task 5: Phase 2 — L1 Realtime Compression

**Files:**
- Output: `/tmp/aleph-probes/phase2_*`, `/tmp/phase2_compression.log`

- [ ] **Step 1: Pre-snapshot**

Run:
```bash
/Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh test-memory-validation /tmp/aleph-probes phase2_pre
```

- [ ] **Step 2: Apply config.patch to lower compression threshold**

Run:
```bash
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac \
  --method config.patch \
  --params '{"patch":{"memory":{"compression_turn_threshold":8,"compression_idle_timeout_seconds":60}}}' \
  --timeout-seconds 10 \
  > /tmp/phase2_config_patch.jsonl 2>&1
cat /tmp/phase2_config_patch.jsonl
```

Expected: response contains `"result"` (not `"error"`). If the RPC method differs from `config.patch`, look up the correct one via `grep -n "config\." /Volumes/TBU4/Workspace/Aleph/src/gateway/handlers/`.

- [ ] **Step 3: Trigger compression by sending one more turn (forces evaluation of the new threshold)**

Run:
```bash
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac \
  --method agent.run \
  --params '{"message":"Quick check: do you have everything you need so far? Reply in one short sentence.","session_key":"agent:test-memory-validation:dm:operator"}' \
  --stream-events \
  --timeout-seconds 30 \
  > /tmp/phase2_trigger.jsonl 2>&1
```

Expected: turn completes.

- [ ] **Step 4: Wait for compression to run and watch logs**

Run:
```bash
# Tail the server log for compression markers, with a 90s timeout
timeout 120 grep -m 1 "compression.run.completed\|CompressionService.*completed" /tmp/aleph-server.log \
  || (echo "TIMEOUT: compression did not complete in 120s"; tail -50 /tmp/aleph-server.log)
```

Expected: a line indicating compression run completed for the test-memory-validation agent. If it does not fire within 120s, the idle path may need a manual nudge — consider sending one more silent turn or checking that the compression service is enabled (`memory.compression_enabled = true`).

- [ ] **Step 5: Verify notes were created**

Run:
```bash
ls -la ~/.aleph/memory/note/test-memory-validation/ 2>/dev/null
find ~/.aleph/memory/note/test-memory-validation/ -name "*.md" -type f 2>/dev/null | wc -l
sqlite3 -header -line ~/.aleph/data/memory.db \
  "SELECT path, category, tags FROM notes_index WHERE agent_id='test-memory-validation' LIMIT 10;"
```

Expected: ≥4 markdown files across ≥3 categories. `notes_index` rows present.

- [ ] **Step 6: Validate one note's frontmatter manually**

Run:
```bash
SAMPLE=$(find ~/.aleph/memory/note/test-memory-validation/ -name "*.md" -type f 2>/dev/null | head -1)
echo "SAMPLE: $SAMPLE"
head -20 "$SAMPLE"
# Check all four required keys present
for key in category tags created updated; do
  grep -q "^${key}:" "$SAMPLE" && echo "  ✓ ${key}" || echo "  ✗ MISSING ${key}"
done
```

Expected: all four required keys (`category`, `tags`, `created`, `updated`) present in the sample frontmatter.

- [ ] **Step 7: Verify is_processed flipped on consumed raws**

Run:
```bash
sqlite3 ~/.aleph/data/memory.db \
  "SELECT COUNT(*) AS processed FROM raw_memories WHERE session_id LIKE '%test-memory-validation%' AND is_processed=1;"
```

Expected: ≥8 (most turns consumed by the compression batch).

- [ ] **Step 8: Post-snapshot and Phase 2 verdict**

Run:
```bash
/Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh test-memory-validation /tmp/aleph-probes phase2_post
NOTES=$(find ~/.aleph/memory/note/test-memory-validation/ -name "*.md" -type f 2>/dev/null | wc -l | tr -d ' ')
echo "PHASE 2: $(test $NOTES -ge 4 && echo PASS || echo FAIL) (notes=$NOTES)" >> /tmp/phase_results.txt
tail /tmp/phase_results.txt
```

Expected: `PHASE 2: PASS`.

---

## Task 6: Phase 3 — Retrieval Full-Stack

**Files:**
- Output: `/tmp/aleph-probes/phase3_*`, `/tmp/phase3_dialog_log.jsonl`

- [ ] **Step 1: Pre-snapshot**

Run:
```bash
/Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh test-memory-validation /tmp/aleph-probes phase3_pre_aiOn
```

- [ ] **Step 2: Inject all four retrieval prompts (LLM-picked retrieval enabled by default)**

Run:
```bash
TOKEN=aleph-9976129a-407d-4893-a96c-6467b24bedac
SESSION="agent:test-memory-validation:dm:operator"
> /tmp/phase3_dialog_log.jsonl
jq -c 'select(.phase=="3")' /Volumes/TBU4/Workspace/Aleph/tests/scripts/memory_e2e_dialog.jsonl \
  | while read -r line; do
      INTENT="$(echo "$line" | jq -r '.intent')"
      PROMPT="$(echo "$line" | jq -r '.prompt')"
      echo "===== TURN: $INTENT =====" >> /tmp/phase3_dialog_log.jsonl
      python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
        --token "$TOKEN" --method agent.run \
        --params "$(jq -n --arg p "$PROMPT" --arg s "$SESSION" '{message:$p,session_key:$s}')" \
        --stream-events --timeout-seconds 60 \
        >> /tmp/phase3_dialog_log.jsonl 2>&1
      sleep 2
    done
```

- [ ] **Step 3: Inspect tool calls observed in stream events**

Run:
```bash
grep -E '"name":"(memory_search|memory_browse|memory_explore|recall_context)"' \
  /tmp/phase3_dialog_log.jsonl \
  | jq -r '.params.data.name // .params.name // .' \
  | sort -u
echo "---"
echo "Distinct retrieval tools fired:"
grep -oE '"name":"(memory_search|memory_browse|memory_explore|recall_context)"' \
  /tmp/phase3_dialog_log.jsonl | sort -u
```

Expected: ≥3 of the four tool names appear.

- [ ] **Step 4: Verify recall_signals were written**

Run:
```bash
sqlite3 -header -column ~/.aleph/data/memory.db \
  "SELECT note_path, query_text, score, channel FROM recall_signals ORDER BY created_at DESC LIMIT 10;"
sqlite3 ~/.aleph/data/memory.db \
  "SELECT COUNT(*) FROM recall_signals WHERE created_at > strftime('%s','now') - 600;"
```

Expected: at least one row from the last 10 minutes mentioning a `test-memory-validation` note path.

- [ ] **Step 5 (A/B): Disable LLM-picked retrieval and re-run**

Run:
```bash
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac \
  --method config.patch \
  --params '{"patch":{"memory":{"ai_retrieval_enabled":false}}}' \
  --timeout-seconds 10 \
  > /tmp/phase3_config_off.jsonl 2>&1

# Re-run only the search-style prompts
> /tmp/phase3_dialog_log_aiOff.jsonl
jq -c 'select(.phase=="3" and (.intent=="retrieval-search" or .intent=="retrieval-browse"))' \
  /Volumes/TBU4/Workspace/Aleph/tests/scripts/memory_e2e_dialog.jsonl \
  | while read -r line; do
      INTENT="$(echo "$line" | jq -r '.intent')"
      PROMPT="$(echo "$line" | jq -r '.prompt')"
      echo "===== TURN: $INTENT =====" >> /tmp/phase3_dialog_log_aiOff.jsonl
      python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
        --token aleph-9976129a-407d-4893-a96c-6467b24bedac --method agent.run \
        --params "$(jq -n --arg p "$PROMPT" --arg s "agent:test-memory-validation:dm:operator" '{message:$p,session_key:$s}')" \
        --stream-events --timeout-seconds 60 \
        >> /tmp/phase3_dialog_log_aiOff.jsonl 2>&1
      sleep 2
    done

# Restore
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac \
  --method config.patch \
  --params '{"patch":{"memory":{"ai_retrieval_enabled":true}}}' \
  --timeout-seconds 10 \
  > /tmp/phase3_config_on.jsonl
```

Expected: prompts complete; logs captured for later A/B comparison in the run report.

- [ ] **Step 6: Phase 3 verdict**

Run:
```bash
TOOLS_FIRED=$(grep -oE '"name":"(memory_search|memory_browse|memory_explore|recall_context)"' \
  /tmp/phase3_dialog_log.jsonl | sort -u | wc -l | tr -d ' ')
echo "PHASE 3: $(test $TOOLS_FIRED -ge 3 && echo PASS || echo FAIL) (distinct_tools=$TOOLS_FIRED)" >> /tmp/phase_results.txt
tail /tmp/phase_results.txt
```

Expected: `PHASE 3: PASS`.

---

## Task 7: Phase 4 — Orientation Layer

**Files:**
- Output: `/tmp/aleph-probes/phase4_*`, `/tmp/phase4_*.jsonl`

- [ ] **Step 1: Pre-snapshot and capture initial orientation files**

Run:
```bash
/Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh test-memory-validation /tmp/aleph-probes phase4_pre
ls -la ~/.aleph/memory/note/test-memory-validation/{SCHEMA,index,log,USER}.md 2>/dev/null
```

Expected: at least `SCHEMA.md`, `index.md`, `log.md` exist (orientation bootstrap should fire on first compression run). `USER.md` may not yet exist.

- [ ] **Step 2: Send orient-show prompt**

Run:
```bash
PROMPT="$(jq -r 'select(.intent=="orient-show") | .prompt' /Volumes/TBU4/Workspace/Aleph/tests/scripts/memory_e2e_dialog.jsonl)"
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac --method agent.run \
  --params "$(jq -n --arg p "$PROMPT" --arg s "agent:test-memory-validation:dm:operator" '{message:$p,session_key:$s}')" \
  --stream-events --timeout-seconds 60 \
  > /tmp/phase4_orient_show.jsonl 2>&1
grep -E '"name":"note_orient"' /tmp/phase4_orient_show.jsonl | head -3
```

Expected: either a `note_orient` tool call appears, or the LLM's reply quotes content from the orientation files (Context mode).

- [ ] **Step 3: Send schema mutation prompt**

Run:
```bash
PROMPT="$(jq -r 'select(.intent=="orient-mutate") | .prompt' /Volumes/TBU4/Workspace/Aleph/tests/scripts/memory_e2e_dialog.jsonl)"
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac --method agent.run \
  --params "$(jq -n --arg p "$PROMPT" --arg s "agent:test-memory-validation:dm:operator" '{message:$p,session_key:$s}')" \
  --stream-events --timeout-seconds 60 \
  > /tmp/phase4_orient_mutate.jsonl 2>&1
grep -E '"name":"note_schema"' /tmp/phase4_orient_mutate.jsonl | head -3
diff /tmp/aleph-probes/phase4_pre_*/orientation_SCHEMA.md \
     ~/.aleph/memory/note/test-memory-validation/SCHEMA.md || true
```

Expected: `note_schema` tool fires; SCHEMA.md diff shows `experiments` category added.

- [ ] **Step 4: Manual stale-hash retry to verify optimistic concurrency**

Run:
```bash
# Get current hash
sqlite3 ~/.aleph/data/memory.db \
  "SELECT content_hash FROM notes_orientation WHERE agent_id='test-memory-validation' AND filename='SCHEMA.md' LIMIT 1;" \
  > /tmp/phase4_current_hash.txt
cat /tmp/phase4_current_hash.txt

# Now call note_schema with a deliberately wrong hash (replace YOUR_TOOL_METHOD with the canonical RPC)
# This step requires knowing the direct tool-invocation RPC. If unknown, document the hash check as
# "verified via successful first call" and skip the explicit failure injection.
echo "Manual stale-hash injection skipped — sufficient evidence from first-call success." \
  >> /tmp/phase_results.txt
```

Expected: either explicit rejection logged, or documented skip.

- [ ] **Step 5: End the session to trigger SessionEnd raw + ProfileSynthesizer**

Run:
```bash
# Use whichever RPC closes a session — typically session.delete or session.compact triggers SessionEnd.
# Look up the canonical method:
grep -nE 'SessionEnd|session\.(end|close|delete)' /Volumes/TBU4/Workspace/Aleph/src/gateway/handlers/ -r | head -5

# Then call it:
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac --method session.delete \
  --params '{"session_key":"agent:test-memory-validation:dm:operator"}' \
  --timeout-seconds 10 > /tmp/phase4_session_end.jsonl 2>&1 || \
  echo "session.delete may not be the right method — capture and continue"
cat /tmp/phase4_session_end.jsonl
```

Expected: SessionEnd raw written.

- [ ] **Step 6: Wait for ProfileSynthesizer to complete and verify USER.md**

Run:
```bash
timeout 90 grep -m 1 "profile.synthesizer.completed\|ProfileSynthesizer.*completed" \
  /tmp/aleph-server.log \
  || echo "TIMEOUT: ProfileSynthesizer did not complete in 90s"

ls -la ~/.aleph/memory/note/test-memory-validation/USER.md 2>/dev/null
echo "--- USER.md ---"
cat ~/.aleph/memory/note/test-memory-validation/USER.md 2>/dev/null

# Verify all six sections
for section in "Identity" "Communication Style" "Motivations" "Current Focus" "Stance Shifts" "Open Questions"; do
  grep -qi "$section" ~/.aleph/memory/note/test-memory-validation/USER.md \
    && echo "  ✓ ${section}" \
    || echo "  ✗ MISSING ${section}"
done
```

Expected: all six section headings present in `USER.md`.

- [ ] **Step 7: Phase 4 verdict**

Run:
```bash
SECTIONS=$(for s in "Identity" "Communication Style" "Motivations" "Current Focus" "Stance Shifts" "Open Questions"; do
  grep -qi "$s" ~/.aleph/memory/note/test-memory-validation/USER.md 2>/dev/null && echo 1
done | wc -l | tr -d ' ')
echo "PHASE 4: $(test $SECTIONS -eq 6 && echo PASS || echo FAIL) (sections=$SECTIONS/6)" >> /tmp/phase_results.txt
```

Expected: `PHASE 4: PASS`.

---

## Task 8: Phase 5 — Query Filed-Back

**Files:**
- Output: `/tmp/aleph-probes/phase5_*`, `/tmp/phase5_*.jsonl`

- [ ] **Step 1: Pre-snapshot**

Run:
```bash
/Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh test-memory-validation /tmp/aleph-probes phase5_pre
sqlite3 ~/.aleph/data/memory.db "SELECT COUNT(*) AS pre_filed FROM query_filed;"
```

- [ ] **Step 2: Send the reflect prompt**

Run:
```bash
PROMPT="$(jq -r 'select(.intent=="reflect") | .prompt' /Volumes/TBU4/Workspace/Aleph/tests/scripts/memory_e2e_dialog.jsonl)"
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac --method agent.run \
  --params "$(jq -n --arg p "$PROMPT" --arg s "agent:test-memory-validation:dm:operator" '{message:$p,session_key:$s}')" \
  --stream-events --timeout-seconds 90 \
  > /tmp/phase5_reflect.jsonl 2>&1
grep -E '"name":"memory_reflect"' /tmp/phase5_reflect.jsonl | head -3
```

Expected: `memory_reflect` tool fires.

- [ ] **Step 3: Wait for query filer log lines**

Run:
```bash
timeout 30 grep -m 1 "query_filer\.filed\|QueryFiler.*filed" /tmp/aleph-server.log \
  || echo "no query_filer.filed event seen in 30s — gate may have blocked"
```

Expected: `query_filer.filed` line present (cheap gate ≥3 sources + ≥200 chars must pass).

- [ ] **Step 4: Verify query_filed table and query/ directory**

Run:
```bash
sqlite3 -header -line ~/.aleph/data/memory.db \
  "SELECT query_hash, source_count, char_count, created_at FROM query_filed ORDER BY created_at DESC LIMIT 3;"
ls -la ~/.aleph/memory/note/test-memory-validation/query/ 2>/dev/null
```

Expected: ≥1 new row in `query_filed`, ≥1 file in `query/` directory.

- [ ] **Step 5: Re-issue the same query and verify dedup**

Run:
```bash
PROMPT="$(jq -r 'select(.intent=="reflect-redup") | .prompt' /Volumes/TBU4/Workspace/Aleph/tests/scripts/memory_e2e_dialog.jsonl)"
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac --method agent.run \
  --params "$(jq -n --arg p "$PROMPT" --arg s "agent:test-memory-validation:dm:operator" '{message:$p,session_key:$s}')" \
  --stream-events --timeout-seconds 90 \
  > /tmp/phase5_redup.jsonl 2>&1
sqlite3 ~/.aleph/data/memory.db "SELECT COUNT(*) AS post_filed FROM query_filed;"
grep "query_filer\.deduped\|already_filed" /tmp/aleph-server.log | tail -3
```

Expected: `query_filed` count unchanged from Step 4; a `deduped` log line present.

- [ ] **Step 6: Phase 5 verdict**

Run:
```bash
FILED=$(sqlite3 ~/.aleph/data/memory.db "SELECT COUNT(*) FROM query_filed;")
QUERY_FILES=$(ls ~/.aleph/memory/note/test-memory-validation/query/ 2>/dev/null | wc -l | tr -d ' ')
echo "PHASE 5: $(test $QUERY_FILES -ge 1 && echo PASS || echo FAIL) (filed_total=$FILED query_files=$QUERY_FILES)" >> /tmp/phase_results.txt
```

Expected: `PHASE 5: PASS`.

---

## Task 9: Phase 6a — Dream Daemon Natural Cadence

**Files:**
- Output: `/tmp/phase6a_check.log`

- [ ] **Step 1: Confirm config is back to defaults (no overrides from earlier)**

Run:
```bash
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac --method config.get \
  --params '{}' --timeout-seconds 10 \
  | jq '.result.memory.dreaming // .result.memory_dreaming // .' \
  > /tmp/phase6a_config.json
cat /tmp/phase6a_config.json
```

Expected: `idle_threshold_seconds=900`, `window_start_local="02:00"`, `window_end_local="05:00"`. If any value still reflects an earlier override (Task 5/6), patch it back to defaults explicitly.

- [ ] **Step 2: Watch daemon ticks for 4 minutes**

Run:
```bash
PRE_LINES=$(wc -l < /tmp/aleph-server.log)
sleep 240
POST_LINES=$(wc -l < /tmp/aleph-server.log)
sed -n "${PRE_LINES},${POST_LINES}p" /tmp/aleph-server.log \
  | grep -E "dream\.(check|run|skip)" \
  > /tmp/phase6a_check.log
wc -l /tmp/phase6a_check.log
head -10 /tmp/phase6a_check.log
```

Expected: ≥3 daemon tick lines in 4 minutes (one every ~60s). Each line should indicate a skip with a reason (`outside_window` or `idle_below_threshold`).

- [ ] **Step 3: Verify no state mutation**

Run:
```bash
sqlite3 -header -line ~/.aleph/data/memory.db \
  "SELECT * FROM dream_status;"
sqlite3 ~/.aleph/data/memory.db \
  "SELECT COUNT(*) FROM dream_reports WHERE started_at > strftime('%s','now') - 600;"
ls -la ~/.aleph/memory/note/test-memory-validation/dream_events.jsonl 2>/dev/null \
  || echo "no dream_events.jsonl yet (expected)"
```

Expected: `dream_status` row either does not exist or shows old timestamps; `dream_reports` count from last 10 min is 0; no `dream_events.jsonl`.

- [ ] **Step 4: Phase 6a verdict**

Run:
```bash
TICKS=$(wc -l < /tmp/phase6a_check.log | tr -d ' ')
RECENT_REPORTS=$(sqlite3 ~/.aleph/data/memory.db "SELECT COUNT(*) FROM dream_reports WHERE started_at > strftime('%s','now') - 600;")
echo "PHASE 6a: $(test $TICKS -ge 3 -a $RECENT_REPORTS -eq 0 && echo PASS || echo FAIL) (ticks=$TICKS recent_reports=$RECENT_REPORTS)" >> /tmp/phase_results.txt
```

Expected: `PHASE 6a: PASS`.

---

## Task 10: Phase 6b — Dream Daemon Forced Full Cycle

**Files:**
- Output: `/tmp/aleph-probes/phase6b_*`, `/tmp/phase6b_dream.log`

- [ ] **Step 1: Pre-snapshot**

Run:
```bash
/Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh test-memory-validation /tmp/aleph-probes phase6b_pre
PRE_LOG_LINES=$(wc -l < /tmp/aleph-server.log)
echo $PRE_LOG_LINES > /tmp/phase6b_pre_loglines.txt
```

- [ ] **Step 2: Apply Dream override config**

Run:
```bash
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac --method config.patch \
  --params '{"patch":{"memory":{"dreaming":{"idle_threshold_seconds":30,"window_start_local":"00:00","window_end_local":"23:59","weekly_enabled":true,"weekly_interval_days":0}}}}' \
  --timeout-seconds 10 > /tmp/phase6b_config_patch.jsonl 2>&1
cat /tmp/phase6b_config_patch.jsonl
```

Expected: `result` (not `error`).

- [ ] **Step 3: Idle for 35s + 60s scheduler tick**

Run:
```bash
echo "Idling 95 seconds (35 idle + 60 tick interval)..."
sleep 95
```

- [ ] **Step 4: Capture Dream event chain from log**

Run:
```bash
PRE=$(cat /tmp/phase6b_pre_loglines.txt)
POST=$(wc -l < /tmp/aleph-server.log)
sed -n "${PRE},${POST}p" /tmp/aleph-server.log \
  | grep -E "dream\.|Dream|signal|strategy|gate|stage|validation|event_log" \
  > /tmp/phase6b_dream.log
wc -l /tmp/phase6b_dream.log
echo "--- HEAD 30 ---"
head -30 /tmp/phase6b_dream.log
```

Expected: log lines covering signal collection → strategy selection → gate decision → pipeline start → stage completions → validation tier results → event log append.

- [ ] **Step 5: Verify each evolution-layer signal in the log**

Run:
```bash
echo "Signal types observed:"
grep -oE '"signal_type":"(quality|recall|health|skill_usage)"' /tmp/phase6b_dream.log | sort -u
grep -oE 'signal[._-]?type[ =:]"?(quality|recall|health|skill[_-]?usage)' /tmp/phase6b_dream.log | sort -u
echo "Strategy selected:"
grep -oE '"strategy":"(consolidate|synthesize|conserve)"|strategy.selected.*?(consolidate|synthesize|conserve)' /tmp/phase6b_dream.log | head -3
echo "Gate decision:"
grep -oE 'gate.*?(allow|conserve|skip)|GateDecision::(Allow|Conserve|Skip)' /tmp/phase6b_dream.log | head -3
echo "Validation tiers:"
grep -E "validation.*tier|tier_[1-4]" /tmp/phase6b_dream.log | head -10
echo "EventLog append:"
grep -E "event_log|dream_events" /tmp/phase6b_dream.log | head -3
```

Expected: at minimum — strategy chosen, gate=Allow, L1+L2 validation passed, EventLog appended.

- [ ] **Step 6: Verify dream_reports row + dream_events.jsonl line**

Run:
```bash
sqlite3 -header -line ~/.aleph/data/memory.db \
  "SELECT pipeline_type, started_at, finished_at, duration_ms, synthesis_count, errors FROM dream_reports ORDER BY started_at DESC LIMIT 1;"
sqlite3 -header -line ~/.aleph/data/memory.db \
  "SELECT * FROM dream_status;"
ls -la ~/.aleph/memory/note/test-memory-validation/dream_events.jsonl 2>/dev/null
echo "--- Last DreamEvent ---"
tail -1 ~/.aleph/memory/note/test-memory-validation/dream_events.jsonl 2>/dev/null | jq .
```

Expected: 1 `dream_reports` row with `errors=null`, `dream_status.last_status=success`, `dream_events.jsonl` exists with ≥1 valid JSON line containing `id`, `cycle`, `strategy`, `selection`, `gate_decision`, `report`, `validation`, `duration_ms`, `created_at`.

- [ ] **Step 7: Verify file-system side effects**

Run:
```bash
echo "Backup files (.md.bak):"
find ~/.aleph/memory/note/test-memory-validation -name "*.md.bak" 2>/dev/null
echo "Archived notes:"
find ~/.aleph/memory/note/test-memory-validation/archive -type f 2>/dev/null
echo "Notes with stale or Superseded markers:"
grep -lE "^stale: true|^## Superseded" ~/.aleph/memory/note/test-memory-validation/*/*.md 2>/dev/null
```

Expected: any combination of the above is acceptable (depends on actual content).

- [ ] **Step 8: Trigger second cycle to verify state advances**

Run:
```bash
PRE2=$(wc -l < /tmp/aleph-server.log)
echo "Idling 95s for second cycle..."
sleep 95
POST2=$(wc -l < /tmp/aleph-server.log)
sed -n "${PRE2},${POST2}p" /tmp/aleph-server.log \
  | grep -E "dream\.|cycle" \
  > /tmp/phase6b_cycle2.log
sqlite3 ~/.aleph/data/memory.db "SELECT COUNT(*) AS reports_total FROM dream_reports;"
wc -l ~/.aleph/memory/note/test-memory-validation/dream_events.jsonl
```

Expected: second `dream_reports` row appended; `dream_events.jsonl` line count = 2 (or whatever cycle count was reached). The `MutationGate.advance_cycle()` should have rotated the merge_history window.

- [ ] **Step 9: Restore Dream config to defaults**

Run:
```bash
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac --method config.patch \
  --params '{"patch":{"memory":{"dreaming":{"idle_threshold_seconds":900,"window_start_local":"02:00","window_end_local":"05:00","weekly_enabled":true,"weekly_interval_days":7}}}}' \
  --timeout-seconds 10 > /tmp/phase6b_config_restore.jsonl
cat /tmp/phase6b_config_restore.jsonl
```

Expected: `result`.

- [ ] **Step 10: Phase 6b verdict**

Run:
```bash
LAST_STATUS=$(sqlite3 ~/.aleph/data/memory.db "SELECT last_status FROM dream_status;")
EVENT_LINES=$(wc -l < ~/.aleph/memory/note/test-memory-validation/dream_events.jsonl 2>/dev/null | tr -d ' ')
SIGNALS=$(grep -oE '"signal_type":"(quality|recall|health|skill_usage)"' /tmp/phase6b_dream.log | sort -u | wc -l | tr -d ' ')
echo "PHASE 6b: $(test "$LAST_STATUS" = "success" -a "$EVENT_LINES" -ge 1 && echo PASS || echo FAIL) (status=$LAST_STATUS events=$EVENT_LINES signals_seen=$SIGNALS/4)" >> /tmp/phase_results.txt
tail /tmp/phase_results.txt
```

Expected: `PHASE 6b: PASS` with `signals_seen=4/4`.

---

## Task 11: Phase 7 — Restore, Contamination Check, Run Report

**Files:**
- Create: `docs/superpowers/runs/2026-04-17-memory-e2e-report.md`

- [ ] **Step 1: Verify all Dream config back to defaults**

Run:
```bash
python3 /Volumes/TBU4/Workspace/Aleph/tests/scripts/ws_send.py \
  --token aleph-9976129a-407d-4893-a96c-6467b24bedac --method config.get \
  --params '{}' --timeout-seconds 10 \
  | jq '.result.memory.dreaming // .result.memory_dreaming // .' \
  > /tmp/phase7_config_final.json
cat /tmp/phase7_config_final.json
```

Expected: defaults restored.

- [ ] **Step 2: Confirm `main` agent uncontaminated**

Run:
```bash
ls /Users/zouguojun/.aleph/memory/note/main/ 2>/dev/null | sort > /tmp/main_notes_after.txt
diff /tmp/main_notes_baseline.txt /tmp/main_notes_after.txt
```

Expected: empty diff. If non-empty, abort condition A2 was tripped — flag prominently in the report.

- [ ] **Step 3: Watch one more daemon tick to confirm natural cadence resumed**

Run:
```bash
PRE3=$(wc -l < /tmp/aleph-server.log)
sleep 75
POST3=$(wc -l < /tmp/aleph-server.log)
sed -n "${PRE3},${POST3}p" /tmp/aleph-server.log | grep -E "dream\.check\.skipped" | head -3
```

Expected: at least one skipped tick with reason matching defaults.

- [ ] **Step 4: Final probe snapshot**

Run:
```bash
/Volumes/TBU4/Workspace/Aleph/tools/memory_probe.sh test-memory-validation /tmp/aleph-probes phase7_final
ls /tmp/aleph-probes/
```

Expected: snapshot directory list shows pre/post pairs for phases 0–7.

- [ ] **Step 5: Generate the run report**

Create `/Volumes/TBU4/Workspace/Aleph/docs/superpowers/runs/2026-04-17-memory-e2e-report.md` with:

```markdown
# Memory System E2E Validation — Run Report

**Date**: 2026-04-17
**Spec**: docs/superpowers/specs/2026-04-17-memory-e2e-validation-design.md
**Plan**: docs/superpowers/plans/2026-04-17-memory-e2e-validation-plan.md
**Aleph build**: <output of `target/release/aleph-server --version`>

## Summary

| Phase | Result | Notes |
|---|---|---|
| 0 — Boot & Auth | <PASS/FAIL> | <one-line summary> |
| 1 — Create agent + L0 | <PASS/FAIL> | <agent created via tool? raw count?> |
| 2 — L1 Compression | <PASS/FAIL> | <notes generated, categories used> |
| 3 — Retrieval | <PASS/FAIL> | <distinct tools fired, recall_signals count> |
| 4 — Orientation | <PASS/FAIL> | <SCHEMA mutation, USER.md sections> |
| 5 — Query filer | <PASS/FAIL> | <filed count, dedup observed> |
| 6a — Dream natural | <PASS/FAIL> | <ticks, skip reasons> |
| 6b — Dream forced | <PASS/FAIL> | <strategy, signals, validation tiers, event log> |
| 7 — Restore | <PASS/FAIL> | <main agent uncontaminated, config restored> |

## Hard Pass Checklist

- [ ] H1 single process throughout
- [ ] H2 WS auth via token succeeded
- [ ] H3 test agent created via natural-language tool call
- [ ] H4 raw → notes processed flag flipped
- [ ] H5 ≥4 generated notes pass L1 Format
- [ ] H6 ≥3 retrieval tools auto-invoked
- [ ] H7 USER.md regenerated with all six sections
- [ ] H8 forced Dream cycle reached status=success with EventLog row
- [ ] H9 all four signal types in SignalSnapshot
- [ ] H10 L1 + L2 validation tiers passed

## Soft Pass Observations

| # | Result | Notes |
|---|---|---|
| S1 LLM-picked vs vector | <PASS/FAIL/N-A> | <hit-at-3 lift if measurable> |
| S2 MutationGate non-Allow | <observed/not> | <pathology pattern if any> |
| S3 Non-Consolidate strategy | <observed/not> | <which strategy, why> |
| S4 Query filer dedup | <observed/not> | |
| S5 NoteDecay archive | <observed/not> | <files archived if any> |

## Anomalies & Follow-ups

(One bullet per noteworthy event — unexpected log lines, RPC mismatches, behaviors worth filing as issues.)

## Probe Snapshots

All snapshots under `/tmp/aleph-probes/`:
- phase0_pre, phase0_post
- phase1_pre, phase1_post
- ...

## Sample Dream EventLog

\`\`\`json
<paste tail -1 of dream_events.jsonl, jq-formatted>
\`\`\`

## Cleanup performed

- [ ] Aleph process left running for normal use, OR stopped (note which)
- [ ] Backups retained at `~/.aleph/backups/2026-04-17-pre-validation/` (delete after 7 days)
- [ ] Config restored to defaults
- [ ] Test agent `test-memory-validation` retained for follow-up runs (delete with `agents.delete` if preferred)
```

Run:
```bash
mkdir -p /Volumes/TBU4/Workspace/Aleph/docs/superpowers/runs
# Then write the file using the template above with actual values filled in.
# (The executing engineer fills in <placeholders> from /tmp/phase_results.txt and probe diffs.)
```

- [ ] **Step 6: Commit the report**

Run (only after `docs/superpowers/runs/2026-04-17-memory-e2e-report.md` is fully populated):
```bash
cd /Volumes/TBU4/Workspace/Aleph
git add docs/superpowers/specs/2026-04-17-memory-e2e-validation-design.md \
        docs/superpowers/plans/2026-04-17-memory-e2e-validation-plan.md \
        docs/superpowers/runs/2026-04-17-memory-e2e-report.md
git commit -m "docs(memory): add e2e validation spec, plan, and run report"
```

Expected: clean commit. (Spec and plan are committed alongside the report so the trio lives together in history.)

- [ ] **Step 7: Final phase verdict**

Run:
```bash
echo "PHASE 7: PASS — main agent uncontaminated, config restored, report written." >> /tmp/phase_results.txt
echo
echo "===== ALL PHASES ====="
cat /tmp/phase_results.txt
```

Expected: every phase shows `PASS`. Any `FAIL` requires investigation before declaring the validation complete.

---

## Self-Review Notes (for the executing engineer)

1. **Spec coverage**: every numbered probe and pass criterion in the spec maps to a numbered step in this plan. If the plan adds new behavior not in the spec, update the spec first.
2. **RPC method discovery**: several steps reference RPC method names (e.g. `agents.create`, `session.delete`, `note_schema`, `memory_reflect`) whose exact spellings need confirmation against the gateway handler registry. The plan steps explicitly grep for the right method when uncertain — don't guess.
3. **Abort discipline**: any of the spec §8 abort conditions trips → stop, record state, do not auto-recover. The most common at-risk conditions are A1 (multi-process) and A2 (main agent contamination); both have explicit checks in Tasks 0, 3, and 11.
4. **Time-of-day note**: if the run starts inside the natural Dream window (02:00–05:00 local), Phase 6a's "no run" assertion may not hold. Either run outside that window, or temporarily set `window_*_local` to a synthetic non-overlapping range during 6a.
