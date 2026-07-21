# Module: src/exec

- Path: `src/exec/`
- Files scanned: 17
- Total LOC: 3946
- Confidence threshold: 80 (all reported findings considered actionable)

## Summary
| Severity | Count |
|----------|------:|
| critical | 2 |
| high     | 7 |
| medium   | 8 |
| low      | 12 |
| **Total**| **29** |

## High-Confidence Issues

### Perspective 1 — Security & Robustness
```
ISSUE|src/exec/approval/channel_bridge.rs:190-225|critical|plain-text `/approve` fallback (no `approval_capability`) sends a chat prompt with no originator gate, so any paired chat member can `/approve` an action raised by another member — group-chat approval bypass the button path explicitly fixes via `record_originator`.
ISSUE|src/exec/leak_detector.rs:124-158|critical|LeakDetector.scan() gates regex on Aho-Corasick prefix hit; a secret whose literal text carries no known prefix (custom vault tokens, JWTs without `bearer `, random HMAC blobs) escapes every regex check despite being high-value credentials.
ISSUE|src/exec/manager.rs:285-300|medium|register_pending's opportunistic cleanup_expired is O(N) over the pending map on every new registration; with many concurrent requests this is an O(N×R) DoS amplifier that also stalls the write lock for unrelated callers.
ISSUE|src/exec/parser.rs:96-159|medium|analyze_shell_command performs three independent linear passes (contains_unquoted_subshell, contains_unquoted_redirect, split_command_chain) over the input with no length cap, so a multi-GB command string exhausts memory/CPU before any of the security checks can return.
ISSUE|src/exec/leak_detector.rs:134-158|high|ScanFinding.matched_text carries up to 20 chars of the secret plus an ellipsis and is held inside the public ScanResult that flows through audit logs to disk/observability backends, exposing partial secrets sufficient for offline key-prefix attacks.
ISSUE|src/exec/secret_patterns.rs:96|medium|Discord token regex `[MN][A-Za-z\d]{23,}\.[\w-]{6}\.[\w-]{27}` has no anchor and matches many random dot-separated ids, causing the masker/leak detector to over-redact legitimate identifiers and create plausible cover for an actual Discord token to ride alongside other redacted strings unnoticed.
ISSUE|src/exec/parser.rs:353-401|medium|resolve_executable calls path.exists() to mark a binary as found; between analysis and execution a symlink can be swapped at the same path (TOCTOU), bypassing later resolution and pointed-at-file checks elsewhere — the sandbox layer mitigates but the analysis result is still labelled `found`.
ISSUE|src/exec/manager.rs:80-82|low|ExecApprovalRecord.created_at_ms and expires_at_ms use `SystemTime::now().as_millis() as u64` with `unwrap_or_default()`; if the wall clock ever reads pre-Unix-epoch (NTP misconfig, VM resume), both fields collapse to 0 and every approval auto-expires or auto-lives indefinitely depending on direction.
```

### Perspective 2 — Logic & Correctness
```
ISSUE|src/exec/approval/types.rs:28-33|high|ApprovalRequest::Capability(Box<CapabilityApprovalRequest>) variant has no production producer — only test code constructs it — so the capability-approval flow it implies (and its CapabilityApprovalRequest struct) is unreachable from real code paths.
ISSUE|src/exec/approval/types.rs:7-15|high|TrustStage::Verified carries the security-critical docstring "Executed multiple times, entered silent mode" but no production code ever advances trust stages or reads Verified to suppress prompts, so a future caller that trusts Verified to be silent-mode would still surface a prompt.
ISSUE|src/exec/approval/parameter_binding.rs:1-100|high|the entire module (ValidationRule, MappingType, CapabilityOverrides, FileSystemOverride, ProcessOverride, EnvironmentOverride, ParameterBinding) is consumed only by the dead CapabilityApprovalRequest seam — pure dead code that documents security-relevant validation rules the rest of the codebase never enforces.
ISSUE|src/exec/risk.rs:22-39|medium|RiskLevel::requires_approval / is_blocked / is_auto_safe is unused outside its own tests; assess_custom only ever returns Blocked/Danger so Safe/Caution and their predicates are aspirational half of a contract the caller cannot honour.
ISSUE|src/exec/kernel.rs:47-74|medium|SecurityKernel::from_config silently swallows every invalid user-supplied regex (tracing::warn then skip); the factory comment promises an operator-visible skip count that is never produced, leaving the kernel reporting success while custom_blocked/custom_danger rules quietly vanish.
ISSUE|src/exec/manager.rs:319-321|medium|await_registered's deny_reason harvest reads record.deny_reason via pending.remove(&id); if a deny-with-reason races a prior resolution that overwrote deny_reason with None, the awaiter receives decision=Some(Deny) but deny_reason=None — silently dropping the human's reason despite the resolver stamping it.
ISSUE|src/exec/manager.rs:652-674|low|cleanup_expired is only called opportunistically from register_pending; an expired live-but-not-resolved entry will linger in memory forever if no new approvals arrive, leaking pending entries (and their oneshot senders) until the process restarts.
ISSUE|src/exec/manager.rs:474-479|low|resolve_for_session sorts live entries by PendingEntry.created_at: Instant while list_pending sorts by record.created_at_ms: wall clock — two FIFO contracts can disagree under clock skew.
```

### Perspective 3 — Architecture Compliance
```
ISSUE|src/exec/kernel.rs:47-92|high|SecurityKernel.assess_custom is regex-based deterministic safety classification for shell commands; combined with SecurityKernelHook (security_kernel_hook.rs:46-66) it maps Blocked to SandboxHookResult::Deny, replacing LLM/prompt discretion with a hardcoded user-regex blacklist — violates R7 (deterministic risk scoring replaces core/intent reasoning) and R9 (configurability should be natural-language tools, not opaque regex).
ISSUE|src/exec/kernel.rs:83-92|high|SecurityKernel is used to detect user *intent* (what shell command is being run) via regex; R8 restricts regex to machine-format detection (JSON/URLs/identifiers), so applying regex to shell-command intent is an R8 violation.
ISSUE|src/exec/kernel.rs:47-74|high|every shell command pays regex-match + bounded-builder overhead on the hot path of assess_custom even when no custom patterns are configured (though Default is empty, callers feeding a non-empty config pay on every call); this middleware tax on the exec path contradicts R10's "intelligence lives in the prompt, thin harness".
ISSUE|src/exec/approval/channel_bridge.rs:294-301|medium|impl From<ApprovalAction> for ApprovalDecisionType is only exercised by the module's own tests; production flow goes through Capability.code path (request_for_tool), not this From — a half-wired approval-action → decision bridge that future callers may rely on incorrectly.
```

### Perspective 4 — Code Quality
```
ISSUE|src/exec/manager.rs:1177|low|file is 1177 lines — significantly over the 500-line guideline; the approval-manager concerns (registration, awaiting, session FIFO, listing, cleanup_expired, snapshots, deny-reason harvesting) could be split into a few focused modules, though splitting a security-critical core adds friction.
ISSUE|src/exec/parser.rs:627|low|file is 627 lines — over the 500-line guideline; consider extracting the three quote-aware scanners (contains_unquoted_subshell / contains_unquoted_redirect / split_command_chain / split_pipeline / tokenize_segment) into their own helpers sharing a single quote-state struct instead of five hand-rolled duplicate state machines.
ISSUE|src/exec/leak_detector.rs:99-105|low|LeakDetector::new uses .unwrap_or_else(|_| unreachable!(...)) for the Aho-Corasick build; if the static prefix list ever becomes invalid (e.g. a contributor adds an empty prefix) the detector panics on construction — prefer propagating the error to the caller.
ISSUE|src/exec/approval/channel_bridge.rs:60-63|low|#[cfg(test)] test_outcome_override is a hardcoded-bypass seam for tests on the production struct; a future refactor that misremoves the cfg gate exposes a one-line "always approve/deny" knob on a security-critical bridge — pin the bypass behind a feature guard or extract a trait.
ISSUE|src/exec/approval/parameter_binding.rs:62-67|low|FileSystemOverride.fs_type is a String for what is documented as one of two values ("read_only" / "read_write") in a module named "validation"; a stringly-typed cap on a security-relevant type invites drift to other string spellings downstream.
ISSUE|src/exec/secret_patterns.rs:39-42|low|AWS access-key regex `r"AKIA[A-Z0-9]{16}"` and the corresponding secret-access-key rule bundle a credential-shape detector and a credential-binding detector in different functions with no shared enum/discriminator; the masker and leak-detector copy the patterns in adjacent arrays and rely on comment cross-references to stay in sync.
ISSUE|src/exec/manager.rs:563-572|low|display_line walks `record.command.chars().count()` a second time just to decide on the trailing `…`; replace with `record.command.chars().nth(MAX).is_some()` to avoid the duplicate O(N) scan.
ISSUE|src/exec/analysis.rs:49-56|low|CommandAnalysis::executables returns Vec<&str> borrowed from the analysis; only one production call (action.rs:269) consumes it and only to assert presence — the borrowed shape forces a clone in callers that just want membership checks.
ISSUE|src/exec/manager.rs:285-300|low|register_pending acquires the write lock twice (cleanup_expired then insert); on std RwLock this is fine sequentially but invites a future caller to add a third acquire inside cleanup_expired and deadlock — note the lock-acquisition contract at the top of the method.
```
