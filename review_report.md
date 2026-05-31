# Logic Review Report
**Module**: approval (`src/approval/`)
**Scope**: Full static review of 7 source files (mod.rs, types.rs, policy.rs, config.rs, adapters.rs, callback_sink.rs, session_route.rs)
**Date**: 2026-05-31
**Mode**: strict

## Findings

### [Critical] `MAX_SUBAGENT_DEPTH` boundary off-by-one allows 17 levels instead of 16
- **Location**: `src/approval/session_route.rs:23`
- **Trigger condition**: Construct a `SessionKey::Subagent` chain with exactly `MAX_SUBAGENT_DEPTH` (16) nested `Subagent` variants
- **Expected behavior**: Recursion should stop at depth 16, returning `None`
- **Actual behavior**: Condition `depth > MAX_SUBAGENT_DEPTH` evaluates to `false` when `depth == 16`, allowing one additional recursive call with `depth == 17`. The check only triggers at depth 17, meaning 17 levels of recursion are permitted (0 through 16 inclusive)
- **Suggested fix**: Change `if depth > MAX_SUBAGENT_DEPTH` to `if depth >= MAX_SUBAGENT_DEPTH` to enforce the documented limit exactly
- **Impact**: Stack exhaustion risk if subagent nesting approaches the limit; violates the explicit contract in the doc comment

### [Warning] `matches_glob` recompiles regex on every call
- **Location**: `src/approval/config.rs:97-101`
- **Risk**: Performance degradation if called in hot loops or with many patterns; each invocation allocates and compiles a fresh `regex::Regex`
- **Current impact**: Medium — the function is `pub` and exposed to external callers who may not realize the cost
- **Suggestion**: Add `#[doc(alias = "slow")]` or expand the doc comment to warn that the hot path should use `ConfigApprovalPolicy` (which pre-compiles rules). Alternatively, expose a `GlobMatcher` struct that caches compiled regexes

### [Warning] `glob_to_regex_str` pre-allocation may under-allocate
- **Location**: `src/approval/config.rs:58`
- **Risk**: `String::with_capacity(pattern.len() * 2)` can be insufficient when the pattern contains many metacharacters (e.g., `*.*.*` becomes `[^/]*\.[^/]*\.[^/]*`, which is ~3.5× the original length). While `String` will realloc transparently, repeated reallocation hurts performance
- **Current impact**: Low — only affects startup/policy reload time
- **Suggestion**: Increase multiplier to `* 4` or remove manual pre-allocation and let `String` grow dynamically

### [Warning] `format!` allocations on Deny/Ask fast paths
- **Location**: `src/approval/config.rs:291-293`, `318-319`, `321-324`, `330-334`
- **Risk**: Every `Deny` or `Ask` decision allocates a formatted `String` for the reason/prompt. Under high-throughput scenarios (e.g., automated security scanning), this creates unnecessary GC pressure
- **Current impact**: Low — approval checks are not typically high-frequency
- **Suggestion**: Use `Cow<'static, str>` for `ApprovalDecision::Deny` and `ApprovalDecision::Ask` so that static/default messages don't allocate

### [Warning] `with_timeout_ms` consumes `self` instead of `&mut self`
- **Location**: `src/approval/adapters.rs:64`
- **Risk**: The builder-style method takes ownership (`mut self`), preventing callers from modifying an adapter after it has been shared (e.g., inside an `Arc`). If a caller needs to adjust the timeout later, they must clone or recreate the adapter
- **Current impact**: Low — current callers only set the timeout at construction time
- **Suggestion**: Provide an additional `set_timeout_ms(&mut self, timeout_ms: u64)` method for mutable access, or change the existing method to `&mut self -> &mut Self`

### [Warning] Hard-coded Chinese string in callback sink
- **Location**: `src/approval/callback_sink.rs:38`
- **Risk**: Mixed-language codebase makes i18n harder; the rest of the crate uses English tracing messages
- **Current impact**: Low — internal error message only
- **Suggestion**: Extract to a constant or use the project's localization infrastructure if one exists

## Summary
| Level | Count |
|-------|-------|
| Critical | 1 |
| Warning | 5 |
| Suggested Test | 0 |

## Automated Verification (Phase 5)
- **cargo check -p alephcore**: ✅ Passed
- **cargo test -p alephcore --lib approval::**: ✅ All `src/approval` tests passed (174 passed, 3 filtered out)
- **Note**: 3 pre-existing failures in `exec::approval/` submodules (unrelated to `src/approval` scope):
  - `exec::approval::escalation::tests::test_resolve_path_with_symlinks_nonexistent`
  - `exec::approval::tests::security_sensitive_dir::test_system_level_keychain`
  - `exec::approval::tests::security_sensitive_dir::test_keychain_detection`

## Cross-Module Observations
- `src/approval/adapters.rs` and `src/approval/callback_sink.rs` correctly import `Arc` from `crate::sync_primitives` (compliant with R8)
- No lock hierarchy violations in `src/approval` (the module is lock-free)
- `pub fn matches_glob` has callers in tests and is a legitimate public utility; not orphaned
- `ChannelApprovalBridgeAdapter` and `ManagerCallbackSink` are both instantiated in `src/bin/aleph-server/commands/start/mod.rs` (properly wired)
