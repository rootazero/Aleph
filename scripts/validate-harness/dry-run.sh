#!/usr/bin/env bash
# Dry run: synthesize evidence files satisfying every check, run every scenario script,
# verify expected outcomes (17 pass + 1 blocked).
# Spec: docs/superpowers/specs/2026-04-25-harness-production-validation-design.md §10 (out-of-band)
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP="$(mktemp -d /tmp/dry-run-XXXX)"
cleanup() {
  rm -rf "$TMP"
  if [[ -n "${HSV:-}" ]]; then
    kill "$HSV" 2>/dev/null || true
    wait "$HSV" 2>/dev/null || true
  fi
  # Belt-and-suspenders: kill anything still bound to the mock health port
  if command -v lsof >/dev/null 2>&1; then
    lsof -ti :9090 2>/dev/null | xargs -r kill -9 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

export ALEPH_TEST_EVIDENCE_DIR="$TMP"
mkdir -p "$TMP/test-home/.aleph/data" "$TMP/test-home/.aleph/logs"

# ─── Synthetic state.db ────────────────────────────────────────────────
sqlite3 "$TMP/test-home/.aleph/data/state.db" <<'EOF'
CREATE TABLE traces (event_name TEXT, event TEXT, ts TEXT);
INSERT INTO traces VALUES ('prompt_assembled','{"sections":["system","tools","memory","history","user"]}','2026-04-25T15:00:00Z');
INSERT INTO traces VALUES ('compaction_triggered','{"pressure_level":"high"}','2026-04-25T15:10:00Z');
INSERT INTO traces VALUES ('tool_call.dispatched','{"tool_name":"channel.create"}','2026-04-25T15:20:00Z');

CREATE TABLE memory_events (id INTEGER, event_type TEXT);
INSERT INTO memory_events VALUES (1, 'memory_write');

CREATE TABLE agent_events (event_type TEXT);
INSERT INTO agent_events VALUES ('subagent_spawned');

CREATE TABLE sessions (session_id TEXT, parent_session_id TEXT, started_at TEXT, stop_reason TEXT);
INSERT INTO sessions VALUES ('main-1', NULL, '2026-04-25T15:00:00Z', 'normal');
INSERT INTO sessions VALUES ('child-1', 'main-1', '2026-04-25T15:05:00Z', NULL);

CREATE TABLE session_events (session_id TEXT, seq INTEGER, event_type TEXT);
INSERT INTO session_events VALUES
  ('main-1', 1, 'tool_call'),
  ('main-1', 2, 'think'),
  ('main-1', 3, 'act'),
  ('main-1', 4, 'tool_call'),
  ('main-1', 5, 'tool_result'),
  ('main-1', 6, 'think'),
  ('main-1', 7, 'act'),
  ('main-1', 8, 'loop_end'),
  ('child-1', 1, 'think'),
  ('child-1', 2, 'act'),
  ('child-1', 3, 'tool_call'),
  ('child-1', 4, 'tool_result'),
  ('child-1', 5, 'loop_end');
EOF

# ─── Synthetic boot.log (≥12 init events + complete) ─────────────────
{
  for phase in tools memory context prompt verification guardrails subagent state error tool_call orchestration init; do
    echo "TRACE boot.module.init phase=$phase"
  done
  echo "INFO boot.complete duration_ms=2400"
} > "$TMP/boot.log"

# ─── Synthetic aleph-server.log (covers events from every scenario) ───
cat > "$TMP/test-home/.aleph/logs/aleph-server.log" <<'EOF'
TRACE gateway.ws.authenticated token=aleph-9976...bedac
TRACE loop.start session_id=main-1
TRACE think
TRACE act
TRACE loop.end session_id=main-1
TRACE loop.end session_id=main-1
TRACE loop.end session_id=main-1
TRACE loop.end session_id=main-1
TRACE loop.end session_id=main-1
TRACE tool_registry.list listed=42
TRACE tool_call.validated valid=true tool=fs.write_file
TRACE tool_call.error error_type=tool_not_found tool=fs.write_file_v2
TRACE sandbox.denied risk_level=high cmd="rm -rf /"
TRACE prompt_assembled sections=["system","tools","memory","history","user"] total_tokens=10000
TRACE Hermes-9 codename appearing in memory section
TRACE compaction_triggered pressure_level=high session_id=main-1
TRACE compactor.strategy_chosen strategy=summarize
TRACE memory.retrieve chunk_ids=["c1","c2"] query="paper_X first author"
TRACE subagent.spawn parent=main-1 child=child-1
TRACE subagent.handoff_back parent=main-1 child=child-1
TRACE error.transient kind=connection_refused
TRACE provider.retry attempt=1
TRACE error.recoverable kind=schema_mismatch
TRACE error.user_fixable kind=permission_revoked
TRACE approval.requested id=a1
TRACE error.unexpected kind=segfault
TRACE process_supervisor restart child=child-1
TRACE pii.detected kind=id_card
TRACE pii.detected kind=email
TRACE pii.redacted count=2
TRACE tool_call.dispatched tool=channel.create payload="..."
TRACE skill.prefetch.completed skill_count=15 duration_ms=120
EOF

# ─── Synthetic config.toml (with channel section for S4.3) ────────────
cat > "$TMP/test-home/.aleph/data/config.toml" <<'EOF'
[channels.tg-news]
bot_token = "fake-bot-token-xyz"
channel = "@aleph_news"
EOF

# ─── boot.log already at evidence root ($TMP === ALEPH_TEST_EVIDENCE_DIR) ─

# ─── run-meta.json ────────────────────────────────────────────────────
cat > "$TMP/run-meta.json" <<EOF
{
  "started_at": "2026-04-25T15:00:00Z",
  "build_commit": "DRYRUN",
  "aleph_version": "DRYRUN",
  "test_home": "$TMP/test-home",
  "evidence_dir": "$TMP",
  "gateway_token_prefix": "aleph-9976",
  "aleph_pid": 0,
  "rust_log": "alephcore=info"
}
EOF

# ─── Per-scenario response_log.md (operator-style transcripts) ────────
mkdir -p "$TMP/S1.1" "$TMP/S1.3" "$TMP/S2.2" "$TMP/S2.4" "$TMP/S4.2" "$TMP/S4.4"

# S1.1: 35 tool bullets
{ for i in $(seq 1 35); do echo "- \`fs.tool_$i\`"; done; } > "$TMP/S1.1/response_log.md"
# S1.3: denial reasoning
echo "Denied. Sandbox refused the rm -rf command (risk_level=high)." > "$TMP/S1.3/response_log.md"
# S2.2: codename recall
echo "The codename you mentioned was Hermes-9." > "$TMP/S2.2/response_log.md"
# S2.4: paper author
echo "The first author of paper X was Dr. Synthetic Five." > "$TMP/S2.4/response_log.md"
# S4.2: PII redacted (no raw PII)
echo "Dear customer service, my contact has been redacted. [REDACTED ID] [REDACTED EMAIL]" > "$TMP/S4.2/response_log.md"
# S4.4: slash menu
printf '/skill-1\n/skill-2\n/skill-3\n' > "$TMP/S4.4/response_log.md"

# ─── /tmp/aleph-test/fib.rs for S1.2 ──────────────────────────────────
mkdir -p /tmp/aleph-test && touch /tmp/aleph-test/fib.rs

# ─── /health server mock for S0.1 + S4.1 ──────────────────────────────
python3 -c "
import http.server, socketserver, threading, json, time
class H(http.server.BaseHTTPRequestHandler):
  def do_GET(s):
    s.send_response(200); s.send_header('content-type','application/json'); s.end_headers()
    s.wfile.write(json.dumps({'subsystems':{f'm{i}':'ok' for i in range(12)}}).encode())
  def log_message(s,*a): pass
socketserver.TCPServer.allow_reuse_address = True
srv = socketserver.TCPServer(('127.0.0.1',9090), H)
threading.Thread(target=srv.serve_forever, daemon=True).start()
time.sleep(180)
" &
HSV=$!
sleep 1

# ─── Run every scenario script + verify outcome ───────────────────────
declare -A EXPECTED=(
  ["S0.1"]="0 pass"  ["S0.2"]="0 pass"  ["S0.3"]="0 pass"
  ["S1.1"]="0 pass"  ["S1.2"]="0 pass"  ["S1.3"]="0 pass"  ["S1.4"]="2 blocked"
  ["S2.1"]="0 pass"  ["S2.2"]="0 pass"  ["S2.3"]="0 pass"  ["S2.4"]="0 pass"
  ["S3.1"]="0 pass"  ["S3.2"]="0 pass"  ["S3.3"]="0 pass"
  ["S4.1"]="0 pass"  ["S4.2"]="0 pass"  ["S4.3"]="0 pass"  ["S4.4"]="0 pass"
)

PASS=0
FAIL=0
SCENARIOS=(S0.1 S0.2 S0.3 S1.1 S1.2 S1.3 S1.4 S2.1 S2.2 S2.3 S2.4 S3.1 S3.2 S3.3 S4.1 S4.2 S4.3 S4.4)
for sid in "${SCENARIOS[@]}"; do
  fname=$(ls "$SCRIPT_DIR"/${sid}-*.sh 2>/dev/null | head -1)
  if [[ -z "$fname" ]]; then
    echo "SKIP $sid: no script"; FAIL=$((FAIL+1)); continue
  fi
  bash "$fname" 2>/dev/null
  rc=$?
  status=$(jq -r .status "$TMP/$sid/evidence.json" 2>/dev/null || echo "no-evidence")
  expected="${EXPECTED[$sid]}"
  exp_rc=$(echo "$expected" | cut -d' ' -f1)
  exp_status=$(echo "$expected" | cut -d' ' -f2)
  if [[ "$rc" == "$exp_rc" ]] && [[ "$status" == "$exp_status" ]]; then
    echo "PASS $sid (rc=$rc, status=$status)"
    PASS=$((PASS+1))
  else
    echo "FAIL $sid (got rc=$rc status=$status, expected rc=$exp_rc status=$exp_status)"
    if [[ -f "$TMP/$sid/evidence.json" ]]; then
      jq -r '.checks[] | select(.passed==false) | "    - " + .id + ": exp=" + .expected + " act=" + .actual' "$TMP/$sid/evidence.json"
    fi
    FAIL=$((FAIL+1))
  fi
done

echo
echo "======== DRY RUN ========"
echo "  passed : $PASS / 18"
echo "  failed : $FAIL"
echo

if [[ "$FAIL" -eq 0 ]]; then
  echo "All scenarios produce expected outcomes."
  exit 0
else
  exit 1
fi
