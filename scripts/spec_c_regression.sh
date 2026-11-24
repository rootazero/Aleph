#!/usr/bin/env bash
# Spec C — cross-process safety regression gate.
#
# The four invariants Spec C enforces used to be text greps in this file. A
# grep cannot tell production code from test code — both live in the same
# files — so the gate reported the test suite's own `Connection::open` calls
# as violations, and a comment could satisfy the "every subcommand declares a
# policy" check. The checks now live in `tests/spec_c_invariants.rs`, where
# the parser sees `#[cfg(test)]` and skips it.
#
# The invariants:
#   1. All SQLite open() calls route through `utils::sqlite_open`
#      (which sets WAL + busy_timeout=5000).
#   2. The vault file and acp_sessions.json are only written through
#      `vault_io` and `atomic_io::write_atomic` (atomic temp+rename).
#   3. Every CLI subcommand module declares its lock policy in a
#      `//! lock-policy: …` line, and the declaration matches the code.
#   4. The singleton lock has one owner: the lock file name lives in
#      `utils::instance_lock`, and only the server boot and `cli::policy`
#      acquire it.
#
# This script stays as a thin wrapper so the documented path keeps working.
# CI runs the same test directly.

set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> Spec C invariants: SQLite opens · vault/ACP writes · CLI lock policy · singleton lock"
exec cargo test -p alephcore --test spec_c_invariants
