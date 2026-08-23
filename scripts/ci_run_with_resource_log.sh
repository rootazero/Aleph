#!/usr/bin/env bash
#
# Run a command while logging free memory and free disk beside its output.
#
# Why a sampler and not a `df` before and after: a hosted runner that exhausts
# a resource does not fail the command. It stops responding, GitHub terminates
# the whole VM, and the only thing in the log is
#
#     ##[error]Process completed with exit code 143.
#     ##[error]The runner has received a shutdown signal.
#
# Memory exhaustion and disk exhaustion print that same pair, so no post-mortem
# reading is possible — the machine is already gone. The one reading that
# survives is one that was already written to the log, which is what this does.
#
# Deliberately not a mitigation. It changes nothing about the build; it only
# makes the next death name which resource ran out, so the fix can be chosen
# instead of guessed.
set -euo pipefail

INTERVAL="${CI_RESOURCE_LOG_INTERVAL:-15}"

sample() {
  local mem disk
  if [ -r /proc/meminfo ]; then
    # MemAvailable, not MemFree: the kernel's own estimate of what a new
    # allocation can actually get, which is the number that predicts an OOM.
    mem="$(awk '/^MemAvailable:/ {print int($2/1024)}' /proc/meminfo)MiB"
  elif command -v vm_stat >/dev/null 2>&1; then
    mem="$(vm_stat | awk '
      /page size of/ {ps = $8}
      /Pages free/ {gsub(/\./, "", $3); f = $3}
      /Pages inactive/ {gsub(/\./, "", $3); i = $3}
      END {if (ps) printf "%dMiB", (f + i) * ps / 1048576; else print "n/a"}')"
  else
    mem="n/a"
  fi
  disk="$(df -Pk . 2>/dev/null | awk 'NR==2 {printf "%dMiB", $4/1024}')"
  printf '[res] %s mem_avail=%s disk_avail=%s\n' \
    "$(date -u +%H:%M:%S)" "${mem:-n/a}" "${disk:-n/a}"
}

sample
# The `kill -0` is belt-and-braces for the trap below. A sampler that outlives
# this script holds the step's stdout open, and the runner waits for every
# writer to close it — so a missed kill would turn a passing job into one that
# hangs to its timeout. This way the loop exits on its own within one interval
# whatever happens to the trap.
parent=$$
( while sleep "$INTERVAL"; do kill -0 "$parent" 2>/dev/null || break; sample; done ) &
sampler=$!
trap 'kill "$sampler" 2>/dev/null || true' EXIT

"$@"
