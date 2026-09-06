#!/usr/bin/env bash
# Run every T0 probe and park the raw JSON outside the repo. Not a test; never enters qa/.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# P1 (fix round 2): a committed script must never default to a session-scoped absolute path --
# t0-lib.mjs:27-30 says exactly that about OBSCURA/CHROME, and this file used to contradict its
# own sibling by hardcoding one session's scratchpad here. `${TMPDIR:-/tmp}` is set by the OS on
# every machine this script could run on; `T0_OUT` still overrides for anyone who wants a specific
# location (e.g. this session's own scratchpad).
OUT="${T0_OUT:-${TMPDIR:-/tmp}/aleph-t0-raw}"
mkdir -p "$OUT"
if [ -n "${T0_OUT:-}" ]; then
  echo "T0_OUT set; writing raw probe output under: $OUT" >&2
else
  echo "T0_OUT not set; writing raw probe output under: $OUT" >&2
fi
run() { # run <name> <script> [args...]
  local name="$1"; shift
  echo "== $name"
  node "$HERE/$1" "${@:2}" > "$OUT/$name.json" 2> "$OUT/$name.log"
  echo "   exit=$? out=$OUT/$name.json log=$OUT/$name.log"
}
run u1              t0-u1-port.mjs
run u2-chrome       t0-u2-boxmodel.mjs chrome
run u2-obscura      t0-u2-boxmodel.mjs obscura
run u3-chrome       t0-u3-pierce.mjs chrome
run u3-obscura      t0-u3-pierce.mjs obscura
run u4              t0-u4-oopif.mjs
run u5-chrome       t0-u5-coords.mjs chrome
run u5-obscura      t0-u5-coords.mjs obscura
run u6-chrome       t0-u6-cookies.mjs chrome
run u6-obscura      t0-u6-cookies.mjs obscura
run u9-chrome       t0-u9-inline.mjs chrome
run u9-obscura      t0-u9-inline.mjs obscura
run capture-chrome  t0-capture.mjs chrome
run capture-obscura t0-capture.mjs obscura
run hn-chromium     t0-hn.mjs
echo "== report"
node "$HERE/t0-report.mjs" "$OUT"
