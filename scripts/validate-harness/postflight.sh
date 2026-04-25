#!/usr/bin/env bash
# Post-flight: stop aleph, extract notable events, dump SQLite, render REPORT.md, audit pollution, cleanup.
# Spec: docs/superpowers/specs/2026-04-25-harness-production-validation-design.md §7.2
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
EVIDENCE_DIR="${ALEPH_TEST_EVIDENCE_DIR:-$REPO_ROOT/target/test-evidence}"

# ----------------------------------------------------------------------------
# Step 0: Resolve test HOME and PID from run-meta.json
# ----------------------------------------------------------------------------
[[ -f "$EVIDENCE_DIR/run-meta.json" ]] || {
  echo "FATAL: $EVIDENCE_DIR/run-meta.json not found — did preflight run?" >&2
  exit 1
}

TEST_HOME=$(jq -r .test_home "$EVIDENCE_DIR/run-meta.json")
PID=$(jq -r .aleph_pid "$EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"

# ----------------------------------------------------------------------------
# Step 1: Graceful shutdown of aleph (TERM, sleep 3, KILL)
# ----------------------------------------------------------------------------
echo "==> [1/6] stopping aleph (pid=$PID)"
kill -TERM "$PID" 2>/dev/null || true
sleep 3
kill -KILL "$PID" 2>/dev/null || true

# ----------------------------------------------------------------------------
# Step 2: Notable events extract (errors, warnings, key trace lines)
# ----------------------------------------------------------------------------
echo "==> [2/6] extracting notable events"
if [[ -f "$LOG" ]]; then
  grep -E "ERROR|WARN|compaction|subagent|stop_hook|pii|sandbox\.denied|approval\.requested" "$LOG" \
    > "$EVIDENCE_DIR/notable-events.log" || true
else
  echo "WARN: $LOG not found; skipping notable-events extract" >&2
fi

# ----------------------------------------------------------------------------
# Step 3: SQLite dumps for later audit
# ----------------------------------------------------------------------------
echo "==> [3/6] dumping SQLite tables"
DB="$TEST_HOME/.aleph/data/state.db"
if [[ -f "$DB" ]]; then
  for table in session_events memory_events agent_events traces sessions; do
    sqlite3 "$DB" -header -csv "SELECT * FROM $table" \
      > "$EVIDENCE_DIR/db-$table.csv" 2>/dev/null || \
      echo "WARN: table $table not found or empty" >&2
  done
else
  echo "WARN: $DB not found; skipping SQLite dumps" >&2
fi

# ----------------------------------------------------------------------------
# Step 4: Render REPORT.md via render-report.py (Task 11)
# ----------------------------------------------------------------------------
echo "==> [4/6] rendering REPORT.md"
if [[ -x "$SCRIPT_DIR/render-report.py" ]]; then
  python3 "$SCRIPT_DIR/render-report.py" \
    --evidence-dir "$EVIDENCE_DIR" \
    --output "$EVIDENCE_DIR/REPORT.md" || \
    echo "WARN: render-report.py failed" >&2
else
  echo "WARN: render-report.py not yet implemented; skip" >&2
fi

# ----------------------------------------------------------------------------
# Step 5: Pollution audit on real ~/.aleph/data (must be unmodified since test start)
# ----------------------------------------------------------------------------
echo "==> [5/6] auditing real ~/.aleph/data for pollution"
NEW=$(find "$HOME/.aleph/data" -newer "$EVIDENCE_DIR/run-meta.json" 2>/dev/null | head -10)
if [[ -z "$NEW" ]]; then
  echo "==> ✅ real ~/.aleph/data unmodified"
else
  echo "==> ⚠️  real ~/.aleph/data has changes since test start:" >&2
  echo "$NEW" >&2
fi

# ----------------------------------------------------------------------------
# Step 6: Interactive cleanup of test HOME
# ----------------------------------------------------------------------------
echo
echo "==> [6/6] cleanup"
read -p "Delete test HOME $TEST_HOME ? (y/N) " yn
if [[ "$yn" == "y" || "$yn" == "Y" ]]; then
  rm -rf "$TEST_HOME"
  echo "==> deleted"
else
  echo "==> kept; remove manually with: rm -rf $TEST_HOME"
fi
echo
echo "==> Post-flight complete. See $EVIDENCE_DIR/REPORT.md"
