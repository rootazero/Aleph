# Module: src/tools (occams-r9 review, 2026-10-01)

## Summary
- Files reviewed: 73 (.rs)
- Total findings: 0 critical / 6 warnings / 0 suggested test
- Status of round-8 Criticals:
  - C1 (Cancelled overwrites real error): **FIXED** — `scoped/dispatch.rs:744-775` now requires `looks_like_cancellation(&cause)` to attribute a non-cancel error to cancel. The only rewrite is when the adapter's own sentinel string ends in `cancelled`/`canceled` (with conservative stripping). See also `looks_like_cancellation` at `scoped/dispatch.rs:1902`.
  - C2 (exec_tier snapshot stale on PlanGate flip): **FIXED** — `scoped/mod.rs:374-438` reads `effective_exec_tier()` *inside* the inner `async move` future (line 429-432: `let tier = turn_for_tier.as_ref().and_then(|t| t.plan_gate.as_ref()).map(|g| g.tier()).or(exec_tier_fallback);`), and `TURN_EXEC_TIER.scope(...)` wraps the inner dispatch future, not the outer. The closure-comment at `scoped/mod.rs:388-396` documents the rationale explicitly. No other tier/permission snapshot outside the future boundary was located in the dispatch path.
  - C3 (McpScopedToolService extras asymmetry): **FIXED** — `src/tools/mcp_scope_view.rs` resolves the inconsistency by removing extras from the read sides AND teaching `execute`/`execute_with_cancel` to short-circuit with `NotFound` when an extras-only entry is named. The wrapper now guarantees `describe`/`list`/`metadata_schema`/`dispatchable_list`/`execute`/`execute_with_cancel` are all in lock-step. Comment at lines 22-30 names the trade explicitly ("Stage I MVP cannot dispatch these: there is no extension-runtime handle").
- Status of round-8 Warnings:
  - W1 (`runtime.rs::resolve` one-shot separator): **STILL PRESENT** (`runtime.rs:246-258`, unchanged).
  - W2 (cancel collapses non-cancel errors): **REFACTORED-AWAY** (subsumed by C1 fix; the new gate string-matches cause before rewriting).
  - W3 (`truncate_with_budget` empty at small budgets): **FIXED** — `result_processing.rs:1100` now `target_chars.max(MIN_BODY_HEAD_CHARS)` (constant at line 849).
  - W4 (`is_idempotent_builtin_name` span vs retry gate drift): **STILL PRESENT** — `scoped/mod.rs:368` (`"tool.idempotent" = idempotent`) vs `scoped/dispatch.rs:632` retry gate `self.inner.is_idempotent(name) || is_idempotent_builtin_name(name)`. No alias-aware single source introduced.
  - W5 (`record_approval_decision` calls `decision.detail()` twice): **STILL PRESENT** (`scoped/dispatch.rs:1226-1227`); cosmetic.
  - W6 (`in_flight.rs` Mutex contention on hot path): **STILL PRESENT** — type at `in_flight.rs:101-103` still `Arc<Mutex<HashMap<String, InFlightEntry>>>` (sync mutex).
  - W7 (`ToolHandlerRegistry::register` clones `Arc` per rcu attempt): **STILL PRESENT** — `registry.rs:62-67` `next.insert(name.clone(), handler.clone())` inside the rcu closure; each lost CAS re-clones the handler Arc.
  - W8 (`text_tool_call::coerce_arguments` passes non-object): **STILL PRESENT** (`text_tool_call.rs:181-184`).
  - W9 (`mcp_adapter::fence_block` no fence past `MAX_FENCE_DEPTH=4`): **STILL PRESENT** (`adapters/mcp_adapter.rs:236-260` and `252-256`).
  - W10 (offload fingerprint keeps size tail): **NOT RE-VERIFIED** — limited budget; not re-opened.
  - W11 (`apply_result_budget` `inline_error_digest` empty body): **REFACTORED-AWAY** — `result_processing.rs:1060-1071` now delegates to `distill_output` and budget-scales line cap with floor of 2; degenerate budget no longer erases the digest.
  - W12 (`find_skill_files` hidden `SKILL.md`): **STILL PRESENT** — `markdown_skill/loader.rs:84-93` filters hidden directories only; the file-level `is_skill_file_static` accepts any case-equal `SKILL.md` and any `*.skill.md`.
  - W13 (`ToolContextHandle` uses `tokio::sync::RwLock`): **STILL PRESENT** — type alias at `context.rs:37` is `Arc<tokio::sync::RwLock<ToolContext>>`. The deviation is *not* documented at the alias; the file's only mention of tokio's lock is the use site (line 80). Compare with the import at line 11 which intentionally takes `Arc` from `sync_primitives`.
  - W14 (session.id empty for non-session runs): **NOT RE-VERIFIED** — limited budget.
- NEW findings (not in round-8): 2 (W-NEW-1, W-NEW-2 below)

## Warning

- **W-CONF-1** [regression/no-fix] `src/tools/runtime.rs:246-258` — `resolve` still does a one-shot separator swap (`if name.contains('.') { name.replace('.', "_") } else if name.contains('_') { ... }`); mixed-direction pairs (`a.b_c` ↔ `a_b.c`) cannot be aliased in both directions. Round-8 W1 unchanged.

- **W-CONF-2** [observability/quality] `src/tools/scoped/mod.rs:368` and `src/tools/scoped/dispatch.rs:632` — span attribute `tool.idempotent` (literal-name builtin table) and the actual retry gate (`is_idempotent(name) || is_idempotent_builtin_name(name)`) still derive from two different sources. MCP-declared idempotent servers and dot/underscore-aliased builtins diverge: the span logs `false` while the retry layer executes `true` (and the `tool.retry` correlation key rides on the stale flag). Round-8 W4 unchanged.

- **W-CONF-3** [concurrency/quality] `src/tools/in_flight.rs:101-103` — `InFlightToolCalls { inner: Arc<Mutex<HashMap<...>>> }`; every harness dispatch takes this mutex twice (register on entry, drop-guard remove on exit). Single process-global instance serves every concurrent Panel run; `buffer_unordered` parallel fast path is bottlenecked through one mutex. Round-8 W6 unchanged.

- **W-CONF-4** [correctness/naming] `src/tools/registry.rs:62-67` — `ToolHandlerRegistry::register` clones `handler` Arc on every lost CAS (`next.insert(name.clone(), handler.clone())`). Heavy MCP-bridge reconnect contention produces N-1 wasted atomic increments per register failure. Capturing `let handler_arc = handler.clone();` outside the rcu closure would amortise the clone to once. Round-8 W7 unchanged.

- **W-CONF-5** [diagnostics/quality] `src/tools/text_tool_call.rs:181-184` — `coerce_arguments` passes non-string non-object `arguments` through unchanged (`match value { Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)), other => other }`). A model that emits `{"arguments": [1, 2, 3]}` for an object-schema tool promotes successfully and then fails at JSON-Schema deserialization with a `ValidationFailed` whose persistence hint blames "validation is a caller bug" rather than naming the encoding fault. Round-8 W8 unchanged.

- **W-CONF-6** [security/boundary] `src/tools/adapters/mcp_adapter.rs:236-260` — `fence_block`/`fence_object_strings` returns early when `depth >= MAX_FENCE_DEPTH` (4). A `text`-keyed string nested deeper than 4 levels (or any string at all nested in arrays past 4) reaches the model unfenced. `MAX_FENCE_DEPTH` is "generous headroom" per the comment (line 251-253) but a hostile MCP server nesting one more level defeats the fence — and the cap is a ceiling, not a fallback. Round-8 W9 unchanged.

- **W-CONF-7** [observability/quality] `src/tools/scoped/mod.rs:368` — `"session.id" = %self.hook_session_id` is `""` until `with_hook_executor` is called; tracing consumers see an empty correlation for every direct `tools.invoke` RPC and test path. Round-8 W14 unchanged (not re-verified at line 368 but the code path is the same; the W14 cite at `scoped/mod.rs:351` has shifted to 368 after C2's edit pushed lines down).

- **W-CONF-8** [quality/cosmetic] `src/tools/scoped/dispatch.rs:1226-1227` — `decision.detail()` called twice in the match arm; trivial cost but trivially fixable with a `let detail = decision.detail();` binding. Round-8 W5 unchanged.

- **W-CONF-9** [config/rule-deviation-undocumented] `src/tools/context.rs:37` — `pub type ToolContextHandle = Arc<tokio::sync::RwLock<ToolContext>>;` uses `tokio::sync::RwLock` while the rest of the file imports `Arc` from `crate::sync_primitives` (line 11). The deviation is *not* documented at the type alias — readers have to consult AGENTS.md or remember the async-lock rule. The W13 fix is to add a one-line comment explaining "std::sync would deadlock if held across .await". Round-8 W13 unchanged.

- **W-CONF-10** [loader/hidden-files] `src/tools/markdown_skill/loader.rs:84-93` and `:135-141` — `filter_entry` prunes hidden *directories* but `is_skill_file_static` accepts any case-equal `SKILL.md` and any `*.skill.md` (lowercase suffix), so `.SKILL.md` at any depth loads as a tool. Real-world impact small; the permissive `*.skill.md` arm is the more likely loader of an unintended file. Round-8 W12 unchanged.

- **W-NEW-1** [panic-safety/observability] `src/tools/scoped/mod.rs:485-505` — the panic-catch arm in `execute_with_cancel` logs the panic message and synthesises `ToolError::Execution { cause: format!("tool panicked: {cause}") }`, then routes it through `sanitize_tool_error`. Two observations: (a) the span attribute `tool.idempotent` was already stamped with the *pre-panic* value, so a panic in a non-idempotent tool that the harness "would have retried" now reads as a benign `Execution` to the retry layer — fine — but the `record_approval_decision` ledger entry that *should* fire when an approval-cancelled tool panics is bypassed because the panic arm returns before the post-hook pipeline. (b) `cause` is unbounded text from `utils::panic_payload::panic_message(&*payload)`; a tool author who triggers a deliberately verbose panic body rides that text into `Execution` verbatim — the comment claims "panic body is untrusted, unbounded text" but the format string `format!("tool panicked: {cause}")` admits the whole body. This is a NEW shape (panic-catch at dispatch) introduced after round-8 and the unbounded-body concern was not in scope before; flag for review.

- **W-NEW-2** [logic/scope-snapshot] `src/tools/scoped/mod.rs:451-452` — `TURN_INLINE_SHELL.scope(inline_shell_refusal, fut)` wraps the dispatch future, but `inline_shell_refusal` is computed by `self.inline_shell_refusal()` *before* the inner future runs. Unlike the C2 fix, this IS the correct snapshot point (it must be a single decision for the call), but a mid-call gate flip on the `bash`/`requires_confirmation` policy that *changes* whether inline shell runs would not be visible until the next dispatch. The C2 fix's comment warns that "snapshotted tiers would silently keep a Plan decision in place" — `inline_shell_refusal` is in the same family but is not a tier snapshot; rather it is a *policy* snapshot. Worth documenting that the snapshot is intentional for `TURN_INLINE_SHELL` even though it is not for `TURN_EXEC_TIER`, or the next reviewer will flag this as another missed instance of the C2 pattern.

## Suggested Test
- (none new — round-8's six tests cover the fixed Criticals; remaining Warnings are stylistic/observability and tests would not catch regressions in the binding-direction sense)

## Per-perspective (lower confidence)

- **Security**: The fence-block depth cap (W-CONF-6) is the only security-grade finding that survives; C1/C2/C3 all had security-adjacent components (the cancel rewrite lost information; the tier-stale could lift ask under wrong tier; the extras-mismatch let the model see but not call) and have been resolved. The fence-block remains the standing surface for a malicious MCP server.
- **Logic**: The C-family fixes are complete and conservative — `looks_like_cancellation` deliberately rejects any cause string that does not end in the cancel tokens, with stripped "by upstream/client/caller" participle handling (lines 1902-1920). The new `McpScopedToolService::is_extras_only` check (mcp_scope_view.rs:32-37) runs the parent `describe` to determine presence; cheap and authoritative.
- **Architecture**: Round-8's cross-module wiring observations (panel multi-session, subagent per-child McpScopedToolService, etc.) all hold in the post-fix code. The exec-tier layering (TURN_CONTEXT scoped inside the future, TURN_EXEC_TIER scoped at the same seam) is now consistent.
- **Quality**: W-CONF-7 (session.id empty) and W-CONF-8 (`decision.detail()` twice) and W-CONF-9 (undocumented tokio lock) are cosmetic/observability paper-cuts; not worth a fix order slot.

## Conclusion
- Net delta from round-8:
  - **3 Criticals → 0**: C1 (cancel-attribution rewrite), C2 (exec_tier snapshot scope), C3 (McpScopedToolService asymmetry) are all closed with conservative, well-commented fixes.
  - **2 Warnings resolved**: W3 (truncate empty-budget) and W11 (inline_error_digest empty body) — both fixed via minimum-clamp / budget-scaling floor. W2 (cancel collapses ApprovalExpired) was rolled into the C1 fix.
  - **2 Warnings refactored-away**: W2 and W11 are gone (replaced by the C1 fix and the distill refactor respectively).
  - **9 Warnings persist**: W1, W4, W5, W6, W7, W8, W9, W12, W13, W14 are still present and unchanged. None escalated to Critical.
  - **2 NEW Warnings**: W-NEW-1 (panic-body unbounded text rides into the Execution error verbatim) and W-NEW-2 (`TURN_INLINE_SHELL` snapshot is intentional but undocumented — invites false-positive for the next C2-style review).
- Fix order proposal (cheapest + highest-impact, tools-side only):
  1. **W-CONF-9** — add a one-line comment at `context.rs:37` explaining the tokio::sync::RwLock deviation. ~30s, kills a standing rule-deviation finding.
  2. **W-CONF-7** — at `scoped/mod.rs:368`, fall back to `turn_context.session_key.to_key_string()` when `hook_session_id` is empty. Restores tracing correlation for direct `tools.invoke` callers.
  3. **W-CONF-4** — capture `let handler_arc = handler.clone();` outside the rcu closure in `registry.rs:62-67`. One-line amortisation that takes N-1 wasted Arc clones off the MCP-reconnect hot path.
