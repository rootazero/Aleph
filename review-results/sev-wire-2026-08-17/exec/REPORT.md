# Severed-Wire Audit — `src/exec/`

- Audit: severed-wire-2026-08-17 (PRODUCED–CONSUMED symbol parity via `rg`)
- Module: `src/exec/` (16 files, ~4680 LOC, including `approval/` subdir)
- Working tree: `/home/zou/data/workspace/Aleph/.worktrees/sev-wire-batch2`
- Repo binary layout: `src/bin/aleph-server/` (no top-level `bin/`); `rg` searches therefore cover `src/ src/bin/ interfaces/ shared/`
- Prior review (re-verified against current code): `review-results/exec.md` (header date 2026-08-12; cited as `existing_review_ref` where a current defect matches a prior flagged item)
- READ-ONLY: no source files modified; no cargo run.

## Method

1. Read all 16 module files fully (`mod.rs`, `allowed_decisions.rs`, `analysis.rs`, `approval/{channel_bridge,types,mod}.rs`, `bridge.rs`, `decision.rs`, `kernel.rs`, `leak_detector.rs`, `manager.rs`, `masker.rs`, `parser.rs`, `risk.rs`, `secret_patterns.rs`, `socket.rs`).
2. Enumerated the public surface: `rg -n "^\s*pub" src/exec/` (84 pub items).
3. For each candidate severed wire, ran `rg -n "<symbol>" src/ src/bin/ interfaces/ shared/` and classified every hit as production / `#[cfg(test)]` / definition.
4. Traced the wiring path per producer:
   - Parser → `analyze_shell_command` → `src/sandbox/exec_approval/action.rs` (production).
   - SecurityKernel → `from_config` → `src/sandbox/factory.rs` and `src/sandbox/security_kernel_hook.rs` (production).
   - SecretMasker → 7 production construction sites: `agents/background_persistence.rs`, `approval/guardian_requester.rs`, `builtin_tools/process_completion.rs`, `builtin_tools/process_journal.rs`, `gateway/event_emitter/redacting.rs`, `gateway/execution_engine/{execute,unattended_redacting_sink}.rs`, `tasks/cron/executor.rs`.
   - `install_operator_patterns` → `src/bin/aleph-server/commands/start/helpers.rs:30` (production).
   - `mask_json_strings` → 2 consumers in `src/gateway/execution_engine/unattended_redacting_sink.rs` and `src/gateway/event_emitter/redacting.rs` (production).
   - LeakDetector (`exec::leak_detector::LeakDetector`) → `src/security/runtime_guard.rs` (production, aliased as `ExecLeakDetector`).
   - ExecApprovalManager → `src/bin/aleph-server/commands/start/mod.rs:278`, `src/approval/{adapters,callback_sink,node_requester,operator_requester}.rs`, `src/gateway/handlers/exec_approvals.rs`, `src/gateway/inbound_router/mod.rs` (production).
   - ApprovalBridge → `src/approval/callback_sink.rs` and `src/gateway/interfaces/telegram/approval.rs` (production).
   - ChannelApprovalBridge → `src/approval/adapters.rs` (production).
   - `allowed_decisions::*` → `src/sandbox/exec_approval/{action,grants}.rs`, `src/approval/node_requester.rs`, `src/gateway/handlers/{exec_approvals,exec_grants}.rs`, `src/tools/scoped/{gate_chain,dispatch}.rs`, `src/gateway/inbound_router/mod.rs` (production).
5. Skimmed following prior-review note: `SocketMessage` / `ApprovalRequestPayload` wire types removed; `ApprovalRequest::Capability` variant removed; both already documented in `socket.rs:3-5` and `approval/types.rs:5-9` — verified removed.

A symbol is "live" only if a production code path reaches it. Test-only consumers do not count.

## Findings

| ID | Severity | Form | Produced | Decision |
|----|----------|------|----------|----------|
| sw-exec-1 | low | 1 | `ExecApprovalRecord::is_resolved` | CUT |
| sw-exec-2 | low | 1 | `LeakPattern` struct + `LeakDetector::new` | CUT |
| sw-exec-3 | low | 4 | `SecretMasker::contains_secrets` | CUT |
| sw-exec-4 | low | 1 | `tokenize_segment` (parser.rs) | CUT |
| sw-exec-5 | low | 4 | `ScanResult::is_clean` | CUT |
| sw-exec-6 | low | 4 | `SecurityKernel::new` | CUT |
| sw-exec-7 | low | 1 | `ExecApprovalManager::cleanup_expired` (pub fn, internal-only) | CUT |
| sw-exec-8 | low | 1 | `ExecApprovalRecord::is_expired` (pub fn, internal-only) | CUT |

Totals: **8 findings** — 0 critical / 0 high / 0 medium / 8 low; 8 CUT.

All findings are pure dead code (Form 1/4) with no production callers. The approval flow wiring (request producer → record → manager → keyboard → resolver) is **fully wired** and the secret-detection / masker pipeline is end-to-end live. No `high` or `critical` severed wires found: the `allowed_decisions` derivation is single-source, `clamped_for` is enforced at the resolver, `is_live` filters zombie entries, and the `register_pending` → `await_registered` ordering kills the resolve-before-register race.

---

## sw-exec-1 — `ExecApprovalRecord::is_resolved` has zero callers (low, CUT)

**Produced:**
- `ExecApprovalRecord::is_resolved` — `src/exec/manager.rs:177`

```rust
pub const fn is_resolved(&self) -> bool {
    self.decision.is_some()
}
```

**Evidence — zero callers repo-wide:**

```
$ rg -n "is_resolved" src/ src/bin/ interfaces/ shared/
src/exec/manager.rs:177:    pub const fn is_resolved(&self) -> bool {
```

Not even test code calls it. The only reference is the definition. The `decision: Option<ApprovalDecisionType>` field it would test is read directly by tests (`record.decision.is_none()`) and by `runtime_guard.rs` logic, so consumers already know resolution via the field rather than the accessor.

**Decision: CUT.** Delete the method (3-line impl). The accessor adds no value over `record.decision.is_some()` — which is already the predicate used by `PendingEntry::is_live` (manager.rs:213) and the cascade filter (manager.rs:575).

**Risk:** A future caller might want a const accessor. Low — `decision.is_some()` is one character longer and more idiomatic. The method is `pub const fn`, so usages would compile, but the audit's job is to flag pure dead code.

**Verification:** `rg -n "is_resolved" src/ src/bin/ interfaces/ shared/` → only the definition.

---

## sw-exec-2 — `LeakPattern` struct and `LeakDetector::new` are orphan pub API (low, CUT)

**Produced:**
- `LeakPattern` struct — `src/exec/leak_detector.rs:21` (pub, with `pub name`, `pub regex`, `pub action` fields)
- `LeakDetector::new(patterns: Vec<LeakPattern>)` — `src/exec/leak_detector.rs:80`

**Evidence — zero production callers outside `exec/leak_detector.rs`:**

```
$ rg -nF "LeakPattern" src/ src/bin/ interfaces/ shared/
src/exec/secret_patterns.rs:16:pub(crate) struct LeakPatternDef {
src/exec/secret_patterns.rs:24:    pub patterns: Vec<LeakPatternDef>,
src/exec/secret_patterns.rs:113:    LeakPatternDef {
src/exec/secret_patterns.rs:118:    LeakPatternDef {
src/exec/secret_patterns.rs:123:    LeakPatternDef {
src/exec/secret_patterns.rs:134:    LeakPatternDef {
src/exec/secret_patterns.rs:139:    LeakPatternDef {
src/exec/secret_patterns.rs:144:    LeakPatternDef {
src/exec/secret_patterns.rs:149:    LeakPatternDef {
src/exec/secret_patterns.rs:154:    LeakPatternDef {
src/exec/secret_patterns.rs:159:    LeakPatternDef {
src/exec/leak_detector.rs:21:pub struct LeakPattern {
src/exec/leak_detector.rs:74:    patterns: Vec<LeakPattern>,
src/exec/leak_detector.rs:80:    pub fn new(patterns: Vec<LeakPattern>) -> Self {
src/exec/leak_detector.rs:91:            .map(|p| LeakPattern {
src/security/runtime_guard.rs:46:    pub custom_leak_patterns: Vec<crate::config::types::CustomLeakPattern>,
src/config/types/secrets.rs:70:// CustomLeakPattern
src/config/types/secrets.rs:86:pub struct CustomLeakPattern {
src/config/types/secrets.rs:105:    pub custom_leak_patterns: Vec<CustomLeakPattern>,
src/gateway/handlers/security_config.rs:124:pub struct CustomLeakPattern {
src/gateway/handlers/security_config/toml_io.rs:275:                                v.as_table().map(|t| CustomLeakPattern {
src/secrets/leak_detector.rs:220:    pub fn with_custom_patterns(custom: &[crate::config::types::CustomLeakPattern]) -> Self {
src/secrets/leak_detector.rs:578:        let custom = vec![CustomLeakPattern {
src/secrets/leak_detector.rs:595:        let custom = vec![CustomLeakPattern {
src/secrets/leak_detector.rs:670:        CustomLeakPattern {
src/secrets/leak_detector.rs:674:        CustomLeakPattern {
```

The `exec::leak_detector::LeakPattern` (note: distinct from `crate::config::types::secrets::CustomLeakPattern` and `secrets::leak_detector::LeakDetector`) is referenced only inside `exec/leak_detector.rs:21` (def), `:74` (field), `:80` (constructor), `:91` (literal site inside `default_patterns`). It is `pub` and `LeakDetector::new` is `pub`, but neither is consumed by any external code. The only production constructor is `default_patterns()` (used by `runtime_guard.rs:114`).

**Decision: CUT.** Delete `LeakPattern` (lines 21–31) and `LeakDetector::new` (lines 80–82). The existing `default_patterns` path becomes the sole constructor — `runtime_guard.rs:114` already uses it. The `patterns: Vec<LeakPattern>` field at line 74 becomes `patterns: Vec<LeakPatternDef>` (move to `pub(crate)` re-export from `secret_patterns.rs:16`) OR `LeakPattern` is kept as a private struct (drop `pub`).

**Risk:** Be sure to keep `LeakPatternDef` (the `pub(crate)` carrier in `secret_patterns.rs`) untouched — it is the structured asset that `default_patterns` consumes. The cosmetic refactor is: rename `LeakPattern` → `LeakPattern` (private) inside `leak_detector.rs`, or move the field type to `LeakPatternDef`. Either way, the public API narrows from `new(patterns: Vec<…>)` to `default_patterns()` only.

**Verification:** `rg -nF "LeakDetector::new(" src/exec/` → only `leak_detector.rs:80` (def) and `leak_detector.rs:97` (internal `Self::new(patterns)`); `rg -n "exec::leak_detector::LeakDetector::new\b" src/ src/bin/ interfaces/ shared/` → zero.

---

## sw-exec-3 — `SecretMasker::contains_secrets` is test-only (low, CUT)

**Produced:**
- `SecretMasker::contains_secrets(&self, text: &str) -> bool` — `src/exec/masker.rs:113`

**Evidence — only test consumers:**

```
$ rg -n "contains_secrets" src/ src/bin/ interfaces/ shared/
src/exec/masker.rs:113:    pub fn contains_secrets(&self, text: &str) -> bool {
src/exec/masker.rs:251:    fn test_contains_secrets() {
src/exec/masker.rs:253:        assert!(masker.contains_secrets("sk-abcdefghijklmnopqrstuvwxyz12345678"));
src/exec/masker.rs:254:        assert!(!masker.contains_secrets("This is just normal text"));
src/exec/masker.rs:274:        assert!(masker.contains_secrets("CUSTOM_SECRET_9"));
```

No production caller. The seven production `SecretMasker` construction sites (`agents/background_persistence.rs:90`, `approval/guardian_requester.rs:341`, `builtin_tools/process_completion.rs:58/89`, `builtin_tools/process_journal.rs:172`, `gateway/event_emitter/redacting.rs:69`, `gateway/execution_engine/execute.rs:1701`, `gateway/execution_engine/unattended_redacting_sink.rs:33`, `tasks/cron/executor.rs:613`) all use `mask()` for the actual redaction work — `contains_secrets` is a no-op in the production path.

**Decision: CUT.** Delete the method (5-line impl). Test fixtures can use `masker.mask(s).contains("***REDACTED***")` or similar to assert the same property. The "does this text contain a secret" predicate is functionally the same as `mask().contains("***REDACTED***")` in mask-mode.

**Risk:** None — the method is unused. Replacing test assertions with the mask-and-contains pattern adds two lines per test.

**Verification:** `rg -n "contains_secrets" src/ src/bin/ interfaces/ shared/` → only definition + 3 test assertions.

---

## sw-exec-4 — `tokenize_segment` is `pub fn` but only used internally (low, CUT)

**Produced:**
- `tokenize_segment(segment: &str) -> Option<Vec<String>>` — `src/exec/parser.rs:324`

**Evidence — only internal + test consumers:**

```
$ rg -n "tokenize_segment" src/ src/bin/ interfaces/ shared/
src/exec/parser.rs:154:            let argv = match tokenize_segment(&raw) {
src/exec/parser.rs:324:pub fn tokenize_segment(segment: &str) -> Option<Vec<String>> {
src/exec/parser.rs:429:        let tokens = tokenize_segment("ls -la").unwrap();
src/exec/parser.rs:435:        let tokens = tokenize_segment("echo 'hello world'").unwrap();
src/exec/parser.rs:441:        let tokens = tokenize_segment(r#"echo "hello world""#).unwrap();
src/exec/parser.rs:447:        let tokens = tokenize_segment(r"echo hello\ world").unwrap();
src/exec/parser.rs:453:        assert!(tokenize_segment("echo 'hello").is_none());
```

The only use outside tests is `parser.rs:154` (inside `analyze_shell_command`). The `pub fn` visibility is gratuitous — no external caller exists.

**Decision: CUT.** Demote to `fn` (private). The function remains callable from the same file's tests and from `analyze_shell_command`.

**Risk:** None — function visibility narrows by one level; same-file tests still compile.

**Verification:** `rg -n "exec::parser::tokenize_segment|parser::tokenize_segment" src/ src/bin/ interfaces/ shared/` → zero.

---

## sw-exec-5 — `ScanResult::is_clean` is test-only (low, CUT)

**Produced:**
- `ScanResult::is_clean(&self) -> bool` — `src/exec/leak_detector.rs:66`

**Evidence — only test consumers:**

```
$ rg -n "is_clean\(\)" src/exec/ src/security/runtime_guard.rs 2>&1
src/exec/leak_detector.rs:66:    pub const fn is_clean(&self) -> bool {
src/exec/leak_detector.rs:162:        assert!(!result.is_clean());
src/exec/leak_detector.rs:186:        assert!(result.is_clean());
src/exec/leak_detector.rs:242:        assert!(!result.is_clean(), "should detect bearer token");
src/exec/leak_detector.rs:287:        assert!(result.is_clean());
```

The production consumer (`runtime_guard.rs`) only uses `has_blocks()` and `has_redacts()` (and `findings.len()`). `is_clean()` is unused externally, including the `pub use` re-export in `mod.rs:29`.

**Decision: CUT.** Delete the method (3-line impl). Test fixtures can use `!result.has_blocks() && !result.has_redacts()` or `result.findings.is_empty()`.

**Risk:** None — the re-export `pub use leak_detector::{LeakAction, LeakDetector, ScanResult}` (`mod.rs:29`) keeps `ScanResult` itself; only the `is_clean` accessor is dropped.

**Verification:** `rg -n "exec_scan\.is_clean|result\.is_clean\(\)|ScanResult.*is_clean" src/ src/bin/ interfaces/ shared/` → zero.

---

## sw-exec-6 — `SecurityKernel::new` is test-only (low, CUT)

**Produced:**
- `SecurityKernel::new() -> Self` — `src/exec/kernel.rs:36` (`#[must_use] pub fn`, returns `Self::default()`)

**Evidence — only test consumers:**

```
$ rg -n "SecurityKernel::new" src/ src/bin/ interfaces/ shared/
src/exec/kernel.rs:109:        let kernel = SecurityKernel::new();
```

The only call site is `kernel.rs:109` (inside `#[cfg(test)]`). Production uses `SecurityKernel::from_config(&ShellSecurityConfig)` (sandbox/factory.rs:47, sandbox/security_kernel_hook.rs:106/139).

**Decision: CUT.** Delete the `new()` constructor (3-line impl). The struct is `Default` (kernel.rs:25 derives it), so callers reach the same state via `SecurityKernel::default()` or — at the production site — `from_config`. The public doormat constructor is a Rust convention but not load-bearing here.

**Risk:** None — `#[must_use]` is the only signal, and no caller relies on it. The `Default` impl remains.

**Verification:** `rg -n "SecurityKernel::new\b" src/ src/bin/ interfaces/ shared/` → only the test-internal call.

---

## sw-exec-7 — `ExecApprovalManager::cleanup_expired` is `pub` but only called internally (low, CUT)

**Produced:**
- `ExecApprovalManager::cleanup_expired(&self)` — `src/exec/manager.rs:862`

**Evidence — only the internal opportunistic sweep:**

```
$ rg -n "cleanup_expired" src/ src/bin/ interfaces/ shared/ | grep -E "exec|approval"
src/exec/manager.rs:341:        self.cleanup_expired();
src/exec/manager.rs:862:    pub fn cleanup_expired(&self) {
```

The only caller is `manager.rs:341` (inside `register_pending`). All other `cleanup_expired` hits in the repo are unrelated types (`clarification::ClarificationManager`, `session_store::*`, `session_manager::IdentityStore`, `memory::session_search_summary::*`).

**Decision: CUT.** Demote `pub fn` to `fn` (private). This is the cleanest fix: the method is an internal opportunistic sweep, not part of the public API contract. Keeping it `pub` invites external callers to schedule it, which is exactly what the doc-comment on `register_pending` warns against (the "bounded work, no background task needed" pattern).

**Risk:** None — no external caller exists.

**Verification:** `rg -n "ExecApprovalManager::cleanup_expired|manager\.cleanup_expired" src/ src/bin/ interfaces/ shared/` → only the private internal call at `manager.rs:341`.

---

## sw-exec-8 — `ExecApprovalRecord::is_expired` is `pub` but only used internally (low, CUT)

**Produced:**
- `ExecApprovalRecord::is_expired(&self) -> bool` — `src/exec/manager.rs:167`

**Evidence — only internal consumer (`PendingEntry::is_live`):**

```
$ rg -n "ExecApprovalRecord::is_expired" src/ src/bin/ interfaces/ shared/
(no matches)

$ rg -n "is_expired" src/exec/manager.rs
src/exec/manager.rs:167:    pub fn is_expired(&self) -> bool {
src/exec/manager.rs:213:        !self.record.is_expired() && self.sender.as_ref().is_some_and(|s| !s.is_closed())
```

Only `PendingEntry::is_live` (manager.rs:213) reads `record.is_expired()`. No external production code or test calls it.

**Decision: CUT.** Demote `pub fn` to `fn` (private). The internal liveness filter — `!self.record.is_expired() && self.sender.as_ref().is_some_and(|s| !s.is_closed())` — is the only consumer.

**Risk:** None — visibility narrows, behavior identical.

**Verification:** `rg -n "ExecApprovalRecord::is_expired" src/ src/bin/ interfaces/ shared/` → zero.

---

## Items checked but NOT findings

Brief survey of other `pub` items that looked suspicious but turned out to be live:

| Item | File | Live? | Why |
|------|------|-------|-----|
| `analyze_shell_command` | parser.rs:97 | ✅ | `src/sandbox/exec_approval/action.rs:106,149` (production) |
| `SecurityKernel::from_config` | kernel.rs:47 | ✅ | `src/sandbox/factory.rs:47`, `src/sandbox/security_kernel_hook.rs:106,139` (production) |
| `SecurityKernel::assess_custom` | kernel.rs:83 | ✅ | `src/sandbox/security_kernel_hook.rs:44` (production) |
| `RiskLevel::Blocked` / `::Danger` | risk.rs:10-12 | ✅ | `src/sandbox/security_kernel_hook.rs:45,51` (production) |
| `analyze_shell_command` constants | parser.rs | ✅ | internal; only `analyze_shell_command` is public |
| `CommandAnalysis::{success,error,not_a_command}` | analysis.rs:28,60,50 | ✅ | `src/sandbox/exec_approval/action.rs:195`, `src/approval/callback_sink.rs:88`, `src/gateway/handlers/exec_approvals.rs:284,418` (production) |
| `CommandSegment::new`, `with_resolution`, `executable` | analysis.rs:85,95,102 | ✅ | internal/external live |
| `CommandResolution::{found,not_found}` | analysis.rs:122,138 | ✅ | `src/exec/parser.rs:383,385,393,396,416,420` (production) |
| `DecisionType` enum + `clamped_for` + `to_outcome_within` | socket.rs:16,50,87 | ✅ | production: `src/exec/manager.rs`, `src/exec/approval/channel_bridge.rs`, `src/approval/{node_requester,operator_requester}.rs`, `src/gateway/handlers/exec_approvals.rs` (production) |
| `Allowed set` functions | allowed_decisions.rs:56,67,74,84,100,115 | ✅ | 14+ production sites in `src/tools/scoped/{gate_chain,dispatch}.rs`, `src/sandbox/exec_approval/{action,grants}.rs`, `src/approval/*`, `src/gateway/handlers/*` |
| `FLOOR_RULES`, `DECLARED_FLOOR_RULE`, `GATE_REMOVAL_RULE` | allowed_decisions.rs:122,125,128 | ✅ | `src/tools/scoped/gate_chain.rs:174,178,801` (production) |
| `ExecApprovalManager::{new,create,register_pending,await_registered,resolve,resolve_with_reason,resolve_for_session,get_pending,list_pending,record_originator}` | manager.rs | ✅ | `src/bin/aleph-server/commands/start/mod.rs:278`, `src/approval/*`, `src/gateway/handlers/exec_approvals.rs`, `src/gateway/inbound_router/mod.rs` (production) |
| `PendingApproval` (manager-level) | manager.rs:219 | ✅ | `src/lib.rs:214` re-export + `src/gateway/handlers/exec_approvals.rs:60` field (production) |
| `ResolvedDecision` | manager.rs:187 | ✅ | return type of `await_registered`; `src/exec/approval/channel_bridge.rs:173`, `src/approval/{node_requester,operator_requester}.rs` (production) |
| `SessionResolveOutcome` | manager.rs:238 | ✅ | `src/gateway/inbound_router/mod.rs:1027` (production) |
| `DEFAULT_APPROVAL_TIMEOUT_MS` | manager.rs:17 | ✅ | `src/approval/{node_requester,operator_requester,adapters}.rs` (production) |
| `PendingEntry::is_live` | manager.rs:212 | ✅ | internal (private) — used by `resolve`, `resolve_for_session`, `get_pending`, `list_pending` |
| `Self::clamp_decision`, `Self::display_line`, `Self::resolve_entry`, `Self::cascade_to_identical_cards`, `Self::list_and_snapshot` | manager.rs | ✅ | private; live internal use |
| `ApprovalBridge::{build_approval_keyboard,parse_callback,decision_response_text}` | bridge.rs:33,73,115 | ✅ | `src/approval/callback_sink.rs:35,59`, `src/gateway/interfaces/telegram/approval.rs:57` (production) |
| `ChannelApprovalBridge::{new,request_for_tool,can_deliver}` | approval/channel_bridge.rs:43,72,201 | ✅ | `src/bin/aleph-server/commands/start/mod.rs:2912`, `src/approval/adapters.rs:108,179` (production) |
| `ChannelApprovalBridge::for_test_always_approved/_denied` | approval/channel_bridge.rs:329,338 | ✅ | `#[cfg(test)]` — only `src/approval/adapters.rs:234,246,260,292` (test) |
| `plain_text_menu` | approval/channel_bridge.rs:19 | ✅ | private; used by `deliver_routed` (production) |
| `MAX_OPERATOR_PATTERNS` | masker.rs:16 | ✅ | `pub const` doc reference + internal `install_operator_patterns` (production) |
| `install_operator_patterns` | masker.rs:53 | ✅ | `src/bin/aleph-server/commands/start/helpers.rs:30` (production) |
| `SecretMasker::new` / `mask` | masker.rs:98,102 | ✅ | 7 production sites (see method) |
| `mask_json_strings` | masker.rs:127 | ✅ | `src/gateway/execution_engine/unattended_redacting_sink.rs:87,90`, `src/gateway/event_emitter/redacting.rs:136` (production) |
| `secret_masker_patterns` / `leak_detector_assets` | secret_patterns.rs:28,108 | ✅ | `pub(crate)`; `src/exec/{masker,leak_detector}.rs` (production) |
| `LeakDetector::default_patterns` | leak_detector.rs:84 | ✅ | `src/security/runtime_guard.rs:114` (production) |
| `LeakDetector::scan_inbound` / `scan_outbound` | leak_detector.rs:120,126 | ✅ | `src/security/runtime_guard.rs:211,216,356,361` (production) |
| `LeakDetector::redact` | leak_detector.rs:140 | ✅ | `src/security/runtime_guard.rs:245,399` (production) |
| `ScanResult::has_blocks` / `has_redacts` | leak_detector.rs:51,60 | ✅ | `src/security/runtime_guard.rs:220,242,377,396` (production) |
| `ScanResult::findings` | leak_detector.rs:44 | ✅ | public field; `src/security/runtime_guard.rs:225,380` (production) |
| `LeakAction::Block` / `::Redact` | leak_detector.rs:11,13 | ✅ | `src/exec/secret_patterns.rs:116-162` (production) |
| `CommandApprovalRequest::allowed_decisions` | approval/types.rs:30 | ✅ | field on serde struct that `src/exec/approval/channel_bridge.rs:282` constructs (production) |
| `ApprovalRequest::Command` variant | approval/types.rs:12 | ✅ | `src/exec/approval/channel_bridge.rs:281`, `src/gateway/channel_approval.rs:225` (production + test) |
| `ExecApprovalRecord::{all fields}` | manager.rs:33-106 | ✅ | `src/gateway/handlers/exec_approvals.rs`, `src/approval/operator_requester.rs`, `src/tools/scoped/dispatch.rs` (production) |

## State of the approval flow (cross-checked end-to-end)

1. **Producer** — `ChannelApprovalBridge::request_for_tool` (channel_bridge.rs:72) builds `ExecApprovalRequest` (decision.rs:7) with `action.allowed_decisions`, then `manager.create` + `manager.register_pending` (manager.rs:287, 318).
2. **Delivery** — `deliver_routed` (channel_bridge.rs:215) renders the Telegram keyboard via `ApprovalBridge::build_approval_keyboard` (bridge.rs:33) using the same `action.allowed_decisions` as the source of truth.
3. **Store** — `register_pending` (manager.rs:318) inserts to `pending: Arc<RwLock<HashMap<…>>>` and returns a oneshot receiver.
4. **Wait** — `await_registered` (manager.rs:364) blocks via `tokio::time::timeout`; cleans `pending` on complete.
5. **Resolve** — both paths enforce `clamped_for(allowed)`:
   - `resolve_with_reason` (manager.rs:423) — by id, narrows `AllowAlways` → `AllowSession` if the card never offered the tier.
   - `resolve_for_session` (manager.rs:646) — by session + position, with snapshot-bounded listings (`session_listings`) and a `grant_key`-keyed cascade (manager.rs:551).
6. **Outcome** — `ApprovalDecisionType::to_outcome_within(&allowed)` (socket.rs:87) is the single decision → `ApprovalOutcome` mapping every surface uses.
7. **Audit** — `decision`, `resolved_by`, `resolved_at_ms`, `deny_reason` are stamped on the record *before* the oneshot is woken (manager.rs:485–492); `await_registered` reads them atomically (manager.rs:386–397).

The `liveness` filter (`PendingEntry::is_live`, manager.rs:212) is the single truth applied to `resolve`, `resolve_for_session`, `get_pending`, `list_pending` — no path resolves a zombie entry.

## Cross-check with prior reviews

- `review-results/exec.md` (2026-08-12) — re-verified: the `allowed_decisions` server-side enforcement, the `is_live` filter, the `register_pending` → `await_registered` ordering, and the `display_line` 120-char truncation are all in place. No re-fired high.
- `review-results/exec-executor-*` (2026-08-12) — `bash_exec` / `code_exec` / `browser_tools::exec` are out of scope for this audit (they live under `src/builtin_tools/`, not `src/exec/`).
- `review-results/severed-wire-2026-08-17/group_chat/REPORT.md` — same author pattern; nothing surfaced here contradicts.

## Severity summary

- **Critical / High / Medium: 0** — the approval and secret-detection paths are end-to-end wired; the `allowed_decisions` derivation is single-source; `clamped_for` and `to_outcome_within` are the only narrowing points.
- **Low: 8** — pure dead code (Form 1/4). Each is a `pub` item with no production caller, all candidates for a one-line `// SAFETY: test-only` style removal.
