# Task P7.1 Report — MCP server face real-machine fixture

Plan: docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P6P7-mcp-face.md §"Task P7.1"
Worktree HEAD: 89e30865d. Scope: qa/ only; zero `src/*.rs` changes.

## Files
- `qa/mcp_face/run.sh`, `qa/mcp_face/drive.py`, `qa/mcp_face/patch_mcp.py` (all chmod +x), contents verbatim from plan except the auth pre-flight deviation below.
- `qa/README.md`: 5 command rows (after `./qa/plugins/run.sh trust`) + one `mcp_face` prose bullet (「每个装置在证明什么」).

## Test results (SKIP_BUILD=1, fresh scratch dir per stage)
- handshake: all PASS, exit 0
- tools: all PASS, exit 0
- deny: all PASS, exit 0
- list_changed: all PASS, exit 0
- auth: one SKIP line, exit 0 (see deviation)
- unit: `cargo test -p alephcore --lib mcp_face` → 62 passed; 0 failed

## Mutations (fixture went red, then reverted)
1. `expose = ["grep"]` only, re-run tools → `[FAIL] file_read is listed…`, `[FAIL] agent_list is listed…` (plus agent_list-call FAILs), exit 4. Reverted (scratch dir discarded).
2. `MCP_APPROVAL_HINT` with "Aleph Panel" removed, rebuild, run deny → `[FAIL] …and names the Aleph Panel…`, exit 1. Source reverted, rebuilt.

## Deviation (drive.py stage_auth only)
Added an HTTP `/health` pre-flight before the first remote call: `except (URLError, TimeoutError, OSError) → print SKIP; return`. Without it, `auth` crashed with an unhandled TimeoutError. Root cause is environment, not Aleph: macOS Application Firewall is enabled (State=1) and blocks inbound non-loopback connections to the unsigned debug `aleph-server` (python3 is allowlisted; TCP handshake succeeds but HTTP hangs — dual-homed en0/en1 on the same 10.10.10/24). On hosts with working self-connectivity the pre-flight returns 200 and the remote assertions run as written.

## Boundaries / negatives
- `git diff 89e30865d -- src/harness/` empty; no `src/*.rs` touched in final state.
- `.cargo/config.toml` is untracked (gitignored) and NOT staged.
- Pre-existing red (untouched): `capability::census::tests::every_installed_global_is_a_capability_slot` (`src/capability/census.rs:823`).
- One transient `deny` FAIL (boot did not install) was caused by orphan `aleph-server` processes from my manual debugging; cleared via pkill, not a fixture defect.
- auth stage remains UNRUN (SKIP) on this host; the 401/wrong-bearer/token paths were not wire-exercised here.
