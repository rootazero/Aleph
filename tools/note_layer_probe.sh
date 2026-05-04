#!/usr/bin/env bash
# note_layer_probe.sh — note-layer-specific probe surface (extends memory_probe.sh
# coverage with A/B/C2/R2 verification queries).
#
# Usage: note_layer_probe.sh <agent_id> <output_dir> [<phase_label>]

set -euo pipefail

command -v sqlite3 >/dev/null || { echo "note_layer_probe: sqlite3 not found in PATH" >&2; exit 3; }

AGENT_ID="${1:-test-note-layer-2026-05-04}"
OUT_DIR="${2:-/tmp/aleph-note-probes}"
LABEL="${3:-snap}"

if ! [[ "${AGENT_ID}" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "note_layer_probe: agent_id must match [A-Za-z0-9_.-]+ (got: ${AGENT_ID})" >&2
  exit 2
fi

TS="$(date +%Y%m%dT%H%M%S)"
SNAP="${OUT_DIR}/${LABEL}_${TS}"

mkdir -p "${SNAP}"

DB="${HOME}/.aleph/data/memory.db"
STATE_DB="${HOME}/.aleph/data/state.db"
NOTES="${HOME}/.aleph/memory/note/${AGENT_ID}"

# =============== R2 — event-type distribution & rename verification (state.db) ===============
sqlite3 -header -column "${STATE_DB}" > "${SNAP}/r2_event_types.txt" 2>&1 <<SQL || true
SELECT event_type, COUNT(*) AS n
FROM memory_events
GROUP BY event_type
ORDER BY n DESC;
SQL

sqlite3 -header -column "${STATE_DB}" > "${SNAP}/r2_recent_events.txt" 2>&1 <<SQL || true
SELECT seq, event_type, fact_id, datetime(timestamp,'unixepoch','localtime') AS ts
FROM memory_events
ORDER BY seq DESC
LIMIT 30;
SQL

# =============== A1/A2 — wikilink shape verification ===============
sqlite3 -header -column "${DB}" > "${SNAP}/a_wikilinks.txt" 2>&1 <<SQL || true
SELECT 'links_for_agent' AS metric, COUNT(*) AS value
FROM notes_links WHERE agent_id='${AGENT_ID}';

SELECT 'links_with_pipe_alias_LEAK' AS metric, COUNT(*) AS value
FROM notes_links WHERE agent_id='${AGENT_ID}' AND to_note LIKE '%|%';

SELECT 'links_resolved_to_full_path' AS metric, COUNT(*) AS value
FROM notes_links WHERE agent_id='${AGENT_ID}' AND to_note LIKE '%/%';

SELECT 'links_unresolved_bare' AS metric, COUNT(*) AS value
FROM notes_links WHERE agent_id='${AGENT_ID}' AND to_note NOT LIKE '%/%';
SQL

sqlite3 -header -column "${DB}" > "${SNAP}/a_links_sample.txt" 2>&1 <<SQL || true
SELECT from_note, to_note FROM notes_links
WHERE agent_id='${AGENT_ID}'
ORDER BY from_note, to_note;
SQL

# =============== Notes index for the agent ===============
sqlite3 -header -column "${DB}" > "${SNAP}/notes_index.txt" 2>&1 <<SQL || true
SELECT path, category, tags_json, created_at, updated_at, content_hash
FROM notes_index
WHERE agent_id='${AGENT_ID}'
ORDER BY updated_at DESC;
SQL

# =============== C2 — governance / provenance / review_queue / supersession ===============
# Test for table existence first; if absent, write an explicit marker.
GOVERN_TABLES=$(sqlite3 "${DB}" "SELECT GROUP_CONCAT(name) FROM sqlite_master WHERE type='table' AND name IN ('notes_provenance','notes_review_queue','notes_review_archive');" 2>/dev/null || echo '')
echo "governance_tables_present: ${GOVERN_TABLES}" > "${SNAP}/c2_tables.txt"

# Schema dump for governance tables to make queries safe
sqlite3 "${DB}" ".schema notes_provenance" > "${SNAP}/c2_schema_provenance.txt" 2>&1 || true
sqlite3 "${DB}" ".schema notes_review_queue" > "${SNAP}/c2_schema_review_queue.txt" 2>&1 || true
sqlite3 "${DB}" ".schema notes_review_archive" > "${SNAP}/c2_schema_review_archive.txt" 2>&1 || true

if echo "${GOVERN_TABLES}" | grep -q 'notes_provenance'; then
  sqlite3 -header -column "${DB}" > "${SNAP}/c2_provenance.txt" 2>&1 <<SQL || true
SELECT 'provenance_total' AS metric, COUNT(*) AS value FROM notes_provenance;
SELECT * FROM notes_provenance ORDER BY rowid DESC LIMIT 30;
SQL
fi

if echo "${GOVERN_TABLES}" | grep -q 'notes_review_queue'; then
  # `notes_review_queue` has no `note_path` column — paths live inside the
  # `candidate_json` blob (CandidateNote { note: { title, category, ... },
  # source_path, ... }). Extract the most useful fields with json_extract so
  # the dump is grep-friendly instead of one giant JSON blob per row.
  sqlite3 -header -column "${DB}" > "${SNAP}/c2_review_queue.txt" 2>&1 <<SQL || true
SELECT 'review_queue_total' AS metric, COUNT(*) AS value FROM notes_review_queue;
SELECT
    id,
    agent_id,
    severity,
    confidence,
    status,
    retry_count,
    json_extract(candidate_json, '$.note.title')    AS note_title,
    json_extract(candidate_json, '$.note.category') AS note_category,
    json_extract(candidate_json, '$.source_path')   AS source_path,
    json_extract(candidate_json, '$.action')        AS action,
    reason,
    created_at,
    decided_at,
    decision_actor
FROM notes_review_queue
ORDER BY rowid DESC
LIMIT 20;
SQL
fi

if echo "${GOVERN_TABLES}" | grep -q 'notes_review_archive'; then
  # Same shape applies to the archive table — candidate_json is the only
  # carrier of path/title info.
  sqlite3 -header -column "${DB}" > "${SNAP}/c2_review_archive.txt" 2>&1 <<SQL || true
SELECT 'review_archive_total' AS metric, COUNT(*) AS value FROM notes_review_archive;
SELECT
    id,
    agent_id,
    final_status,
    json_extract(candidate_json, '$.note.title')    AS note_title,
    json_extract(candidate_json, '$.note.category') AS note_category,
    json_extract(candidate_json, '$.source_path')   AS source_path,
    reason,
    created_at,
    archived_at
FROM notes_review_archive
ORDER BY rowid DESC
LIMIT 10;
SQL
fi

# =============== Frontmatter inspection (supersession + dates + tags) ===============
if [ -d "${NOTES}" ]; then
  : > "${SNAP}/frontmatter_summary.txt"
  while IFS= read -r f; do
    {
      echo "=== ${f} ==="
      awk '/^---$/{c++; if(c==2) exit} c>=1' "${f}"
      echo
    } >> "${SNAP}/frontmatter_summary.txt"
  done < <(find "${NOTES}" -type f -name '*.md' 2>/dev/null | sort)

  # Wikilink shape audit (raw bracket forms found in body)
  : > "${SNAP}/wikilink_audit.txt"
  while IFS= read -r f; do
    grep -oE '\[\[[^]]+\]\]' "${f}" 2>/dev/null | while read -r link; do
      echo "${f}:${link}"
    done
  done < <(find "${NOTES}" -type f -name '*.md' 2>/dev/null | sort) >> "${SNAP}/wikilink_audit.txt" || true

  # Filesystem layout
  find "${NOTES}" -type f -name '*.md' -exec stat -f "%m %z %N" {} \; 2>/dev/null | sort -k3 > "${SNAP}/notes_files.txt" || true
else
  echo "no notes dir for ${AGENT_ID}" > "${SNAP}/notes_files.txt"
fi

# =============== Recall signals scoped to agent ===============
sqlite3 -header -column "${DB}" > "${SNAP}/recall_signals.txt" 2>&1 <<SQL || true
SELECT note_path, query_text, score, channel, day_bucket
FROM recall_signals
WHERE note_path LIKE '${AGENT_ID}%' OR note_path LIKE '%/${AGENT_ID}/%'
ORDER BY created_at DESC
LIMIT 30;

SELECT 'recall_signals_total' AS metric, COUNT(*) AS value FROM recall_signals;
SQL

# =============== Dream daemon status ===============
sqlite3 -header -column "${DB}" > "${SNAP}/dream_status.txt" 2>&1 <<SQL || true
SELECT * FROM dream_status;
SELECT 'dream_reports_total' AS metric, COUNT(*) AS value FROM dream_reports;
SQL

sqlite3 -header -line "${DB}" > "${SNAP}/dream_reports_recent.txt" 2>&1 <<SQL || true
SELECT * FROM dream_reports ORDER BY started_at DESC LIMIT 3;
SQL

# Dream events JSONL for the agent
if [ -f "${NOTES}/dream_events.jsonl" ]; then
  cp "${NOTES}/dream_events.jsonl" "${SNAP}/dream_events.jsonl"
  wc -l "${NOTES}/dream_events.jsonl" > "${SNAP}/dream_events_count.txt"
fi

# =============== Process & lock ===============
ps aux | grep '[a]leph-server' > "${SNAP}/processes.txt" 2>/dev/null || echo "no aleph-server process" > "${SNAP}/processes.txt"
ls -la "${HOME}/.aleph/data/aleph.lock" 2>/dev/null > "${SNAP}/lockfile.txt" || true

echo "${SNAP}"
