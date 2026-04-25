#!/usr/bin/env bash
# S4.4 — skill prefetch: skills directories discovered + extension manager loading
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S4.4"
MODULE="skill_prefetch"
SEVERITY="medium"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"

DIRS=$(grep -cE "Found project-level \.claude/skills|Found global ~/\.aleph/skills|get_all_skills_dirs: Discovery complete" "$LOG" 2>/dev/null || true); DIRS=${DIRS:-0}
[[ "$DIRS" -ge 1 ]] && p=true || p=false
check "skills_dirs_discovered" "log_grep" ">= 1" "$DIRS" "$p"

EXT=$(grep -cE "Extension loading complete|Extension manager initialized|ExtensionManager: tool registry wired" "$LOG" 2>/dev/null || true); EXT=${EXT:-0}
[[ "$EXT" -ge 1 ]] && p=true || p=false
check "extension_loading_complete" "log_grep" ">= 1" "$EXT" "$p"

SKILLREG=$(grep -cE "skill\.list|skill\.read|read_config_guide|skill management tools" "$LOG" 2>/dev/null || true); SKILLREG=${SKILLREG:-0}
[[ "$SKILLREG" -ge 1 ]] && p=true || p=false
check "skill_tools_registered" "log_grep" ">= 1" "$SKILLREG" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?
