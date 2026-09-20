#!/usr/bin/env bash
# scripts/verify-flaky-tests-serial.sh
#
# Three tests are marked `#[ignore]` because they pass in isolation but flake
# under the heavy parallel fan-out of the full `cargo test --lib` suite on
# a contended runner (one is a Windows PTY geometry race, the other two are
# file-lock starvation under std::thread contention — same root cause,
# separate stores). Run this script to re-verify them serially whenever you
# touch any of these tests or their global manager / store state:
#
#   bash scripts/verify-flaky-tests-serial.sh
#
# Exits non-zero if any test fails, so CI can call this as a separate
# job without paying for the parallelism tax.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

# --test-threads=1 forces serial execution inside the test binary; the
# `#[ignore]` is what makes the tests skipped by default.
exec cargo test -p alephcore --lib \
    -- --include-ignored --test-threads=1 \
       gateway::pty::tests::a_write_reaches_a_real_subscriber_over_the_pty_screen_topic \
       skill::usage::tests::concurrent_bumps_do_not_lose_counts \
       tools::usage::store::tests::concurrent_records_do_not_lose_counts