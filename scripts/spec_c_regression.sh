#!/usr/bin/env bash
# Spec C — cross-process safety regression checks.
# Run before every commit that touches anything Spec C governs.
#
# This script exists to lock in the four invariants Spec C enforces:
#   1. All SQLite open() calls route through `utils::sqlite_open`
#      (which sets WAL + busy_timeout=5000).
#   2. The vault file and acp_sessions.json are only touched through
#      `vault_io` and `atomic_io::write_atomic` (atomic temp+rename).
#   3. Every CLI subcommand file declares its policy by calling
#      `with_policy`, `run_no_lock`, or `with_policy_owned`.
#   4. The legacy `acquire_instance_lock` helper is only referenced
#      from the thin compat shim in `daemon.rs`.
#
# Each check failing produces a clear ❌ line + the offending matches
# so Claude/an engineer can fix it without re-deriving the invariant.

set -euo pipefail
cd "$(dirname "$0")/.."

fail=0

echo "==> [1/4] SQLite opens must route through open_sqlite_safe / open_sqlite_readonly"
if git grep -nE "Connection::open\(|rusqlite::Connection::open\(" -- 'src/*.rs' 'src/**/*.rs' \
   | grep -v "utils/sqlite_open\.rs" \
   | grep -v "utils/spec_c_audit\.rs" \
   | grep -v "tests/" \
   | grep -v "^\s*//"; then
    echo "  ❌ direct rusqlite open detected — must use open_sqlite_safe"
    fail=1
else
    echo "  ✅"
fi

echo "==> [2/4] secrets.vault and acp_sessions.json must route through wrappers"
# Catch direct file-system operations on these paths, NOT mere path
# construction (e.g. `data_dir.join("secrets.vault")`) which is fine
# when the resulting PathBuf is then handed to VaultIo / write_atomic.
if git grep -nE "(std::fs::(read|read_to_string|write|create|remove_file|remove|rename|copy|metadata)|File::(open|create))" -- 'src/*.rs' 'src/**/*.rs' \
   | grep -E "(secrets\.vault|acp_sessions)" \
   | grep -v "vault_io\.rs" \
   | grep -v "atomic_io\.rs" \
   | grep -v "acp/manager\.rs" \
   | grep -v "spec_c_audit\.rs" \
   | grep -v "tests/" \
   | grep -v "^\s*//"; then
    echo "  ❌ raw fs op on vault/acp file detected"
    fail=1
else
    echo "  ✅"
fi

echo "==> [3/4] every CLI subcommand file dispatches via with_policy or run_no_lock"
missing=$(git grep -L "with_policy\|run_no_lock\|with_policy_owned" src/bin/aleph-server/commands/ \
          | grep '\.rs$' | grep -v 'mod\.rs' || true)
if [ -n "$missing" ]; then
    echo "  ❌ unannotated subcommand files:"
    echo "$missing" | sed 's/^/      /'
    fail=1
else
    echo "  ✅"
fi

echo "==> [4/4] no leftover acquire_instance_lock callers outside daemon thin wrapper"
if git grep -n "acquire_instance_lock" -- 'src/*.rs' 'src/**/*.rs' \
   | grep -v "src/bin/aleph-server/daemon\.rs" \
   | grep -v "spec_c_audit\.rs" \
   | grep -v "tests/" \
   | grep -v "^\s*//"; then
    echo "  ❌ stale acquire_instance_lock callers"
    fail=1
else
    echo "  ✅"
fi

if [ $fail -ne 0 ]; then
    echo
    echo "Spec C regression checks FAILED. Fix and re-run."
    exit 1
fi
echo
echo "Spec C regression checks PASS."
