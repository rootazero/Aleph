#!/usr/bin/env bash
# memory_probe.sh — dump memory-system probe surface for one agent.
# Usage: memory_probe.sh <agent_id> <output_dir> [<phase_label>]

set -euo pipefail

command -v sqlite3 >/dev/null || { echo "memory_probe: sqlite3 not found in PATH" >&2; exit 3; }

AGENT_ID="${1:-test-memory-validation}"
OUT_DIR="${2:-/tmp/aleph-probes}"
LABEL="${3:-snap}"

if ! [[ "${AGENT_ID}" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "memory_probe: agent_id must match [A-Za-z0-9_.-]+ (got: ${AGENT_ID})" >&2
  exit 2
fi

TS="$(date +%Y%m%dT%H%M%S)"
SNAP="${OUT_DIR}/${LABEL}_${TS}"

mkdir -p "${SNAP}"

DB="${HOME}/.aleph/data/memory.db"
NOTES="${HOME}/.aleph/memory/note/${AGENT_ID}"

# 1-3. SQLite probe queries (tolerate missing tables — script must always finish)
if [ ! -f "${DB}" ]; then
  echo "memory_probe: ${DB} does not exist yet" > "${SNAP}/sqlite_summary.txt"
else
  sqlite3 -header -column "${DB}" > "${SNAP}/sqlite_summary.txt" 2> "${SNAP}/sqlite_summary.err" <<SQL || true
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
fi

# 4. Notes filesystem state for the agent
if [ -d "${NOTES}" ]; then
  find "${NOTES}" -type f -name "*.md" -exec stat -f "%m %z %N" {} \; \
    | sort -k3 \
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
ps aux | grep "[a]leph-server" > "${SNAP}/processes.txt" || echo "no aleph-server process" > "${SNAP}/processes.txt"

echo "${SNAP}"
