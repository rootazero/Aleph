# Task 1 — shared ACP launch admission

Status: DONE_WITH_CONCERNS.

Changed paths:
- `src/acp/manager/harness_admin.rs`: shared `admitted_adapter` reads adapter and trust policy under the existing adapter → config lock order; disabled trust returns `HarnessDenied`. Restore skips denied entries without emitting removal events.
- `src/acp/manager/lifecycle.rs`: prompt mode resolution, native acquisition, and one-shot execution use shared admission. Direct control RPC contracts remain intact.
- `src/acp/manager/tests.rs`: actual process-start marker regression for configured/enabled disabled-trust native, one-shot, ensure-session, and old-record restore paths; restore emits no persistence event.
- `src/builtin_tools/acp_tools.rs`: delegated invocation regression; existing tool permission and cwd behavior remain intact.
- `src/teams/dispatcher/runner.rs`: dispatches a real ACP member task through a manager configured with disabled trust; asserts failed outcome, `harness_denied`, and no process marker.

Validation:
- Pre-fix `CARGO_TARGET_DIR="$PWD/target" cargo test -p alephcore --lib disabled_`: **93 passed, 2 failed**. Native manager path created its marker and failed with ACP stdin Broken pipe; Teams path created its marker and failed `disabled team harness launched`. Delegated tool already denied launch.
- Final `CARGO_TARGET_DIR="$PWD/target" cargo test -p alephcore --lib disabled_`: **95 passed, 0 failed**; all three added regressions pass without subprocess markers.
- Related groups run directly from that generated test binary: manager **31 passed**, ACP tools **8 passed**, dispatcher runner **14 passed**; zero failures.
- `rustfmt --check --edition 2021` on all five changed Rust files: passed.
- Source whitespace check: passed.

Build isolation: all Cargo artifacts use this worktree's `target`; no competing builds or main checkout artifacts. An obsolete compile with a corrected test-fixture import was stopped, then the regression run completed. The initial green compile was restarted after final Unix gating/documentation edits so final verification matches the committed source.

Concerns:
- Compile emits an existing `dead_code` warning for `PairedShellRun` fields at `src/gateway/busy_queue/spawn.rs:36`; no changes were made outside Task 1 to suppress it. A full library suite and strict Clippy were not run.
- Binding admission to registration identity at publication remains Task 2. Task 1 reads a consistent registration snapshot but does not solve replacement during subprocess startup.
- Durable saved-record lifecycle redesign remains Task 3. Task 1 leaves denied records untouched and emits no removal event.
- Process-marker regressions use `/bin/sh` and are gated to Unix.
