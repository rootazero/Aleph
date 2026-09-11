# Aggregate Review: src/fetch, src/gateway, src/generation, src/goal, src/group_chat

**Date:** 2026-09-09
**Reviewer:** static (5 parallel subagent pass, 2-perspective protocol: rust-logic-audit + severed-wire-audit)
**Branches:** all fixes committed directly to `main` (no PR, per user directive)
**Worktrees:** 5 isolated worktrees branched from `main` for the parallel review pass, removed after fixes landed
**Final integration:** 7 review commits pushed directly to `main`

## Pipeline

1. **Worktree setup**: 5 worktrees branched from `main` at `c03b1cac0` (review/fetch, review/gateway, review/generation, review/goal, review/group_chat)
2. **Parallel static review** (no `cargo check` mid-flight per protocol): each subagent used rust-logic-audit + severed-wire-audit lenses, with graphify-out/2026-09-08/ as semantic context
3. **Findings surfaced**: 0 P0 / 12 P1 / 13 P2 / 14 P3 = **39 findings** total
4. **Fixes applied directly to main**: 11 P1 + 6 P2 + 1 P3 fixed; 1 P1 + 7 P2 + 13 P3 deferred with rationale
5. **Single cargo check at the end** with memory limits (`CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1`) — passes for `alephcore` lib with only 4 pre-existing warnings
6. **Worktrees removed**; review branches deleted (`review/{module}`)

## Module Totals

| Module | Path | Findings | P0 | P1 | P2 | P3 | Fixed |
|-------:|------|---------:|---:|---:|---:|---:|----:|
| 1 | `src/fetch`         |  4 | 0 | 0 | 2 | 2 |  2 |
| 2 | `src/gateway`       |  7 | 0 | 0 | 4 | 3 |  1 |
| 3 | `src/generation`    | 23 | 0 |11 | 6 | 6 | 13 |
| 4 | `src/goal`          |  4 | 0 | 0 | 1 | 3 |  1 |
| 5 | `src/group_chat`    |  1 | 0 | 1 | 0 | 0 |  1 |
| **TOTAL** |            | **39** | **0** | **12** | **13** | **14** | **18** |

## Findings fixed (selection)

| Module | ID | Sev | Title | Commit |
|-------:|----|----:|-------|--------|
| group_chat | P1-01 | P1 | empty coordinator plans bypass max_rounds | 98a37917a |
| generation | P1-01 | P1 | RateLimitError::retry_after dropped at From boundary | 43229df5a |
| generation | P1-02 | P1 | SerializationError mapped to AlephError::IoError | 43229df5a |
| generation | P1-03 | P1 | ProviderError status_code + provider dropped | 43229df5a |
| generation | P1-04 | P1 | GenerationProviderRegistry::contains misses canonical_index | 43229df5a |
| generation | P1-05 | P1 | DownloadError always retryable regardless of status | 43229df5a |
| generation | P1-06 | P1 | ReplicateProviderBuilder::build swallows reqwest errors | 43229df5a |
| generation | P1-08 | P1 | openai_whisper::load_local symlink-to-absolute traversal | 43229df5a |
| generation | P1-10 | P1 | factory error message omits volcengine_tts | 43229df5a |
| generation | P2-02 | P2 | api_key empty-string accepted by 6 builders | 43229df5a |
| generation | P2-05 | P2 | is_retryable excludes HTTP 408 | 43229df5a |
| generation | P2-06 | P2 | "wait 0 seconds" for sub-second retry_after | 43229df5a |
| generation | P2-07 | P2 | MockGenerationProvider pub in production | 43229df5a |
| generation | (regression) | — | 3 new tests pinning From boundary + DownloadError retry | 7e935400f |
| fetch | P2-01 | P2 | firecrawl size-cap error mislabels transport failures | d8a12aee7 |
| fetch | P2-02 | P2 | firecrawl test success never persists verified | d8a12aee7 |
| goal | P2-01 | P2 | GoalStore uses std::sync::Mutex directly (R8 violation) | 402452790 |
| gateway | P2-03 | P2 | `cb_` MessageId prefix is convention-based | 57e2ac445 |
| (post-edit) | — | — | cargo check fallout (Replicate build self-move; ProviderError String not Option; missing verified_shared field) | 7d7c27ba6 |

## Findings deferred (with rationale)

| Module | ID | Sev | Title | Reason |
|-------:|----|----:|-------|--------|
| generation | P1-07 | P1 | voice_http_client hardening only used by 2 of ~8 voice providers | High blast radius across 6+ provider modules; bundle with retry-adoption pass |
| generation | P1-09 | P1 | retry_transient has only 1 production caller | Mechanical refactor across 18 providers; high blast radius |
| generation | P1-11 | P1 | openai_compat edit.rs dead unwrap_or branch | No behaviour bug; clippy cleanup |
| generation | P2-03 | P2 | volcengine_tts resolve_endpoint accepts arbitrary host | Operator-controlled but defense-in-depth gap; bundle with model-path validation |
| generation | P2-04 | P2 | model_path interpolated into URL without charset validation | Defense-in-depth; bundle with SSRF guard above |
| generation | P2-08 | P2 | probe.rs hard-coded Chinese strings | i18n concern; needs i18n pass on probe surfaces |
| generation | P3-01/02 | P3 | response-body download no size cap on 8+ providers | OOM-only risk; bundle with retry-adoption |
| generation | P3-03 | P3 | polling upper bounds use attempt counters not wall-clock | Low-impact today (fal is the only high-volume path) |
| generation | P3-04 | P3 | dead trait surface (check_progress, cancel, edit_image, supports_image_editing) | Dead-surface cleanup; needs gateway wiring decision |
| generation | P3-05 | P3 | dead error-classifier API on GenerationError | Same as above |
| generation | P3-06 | P3 | connection-error mapping collapses TLS/decode/redirect | Observability nit |
| fetch | P3-01 | P3 | magic number 60 duplicated across fetch/builtin/policy | Three-call-site refactor; low-impact hygiene |
| fetch | P3-02 | P3 | firecrawl serde_json parse error can echo api_key | Low-probability leak vector |
| goal | P3-01 | P3 | pause_all_owned_by reads now_ms from SystemTime | Single-call-site cosmetic |
| goal | P3-02 | P3 | pause_all_active has no direct unit test | Test coverage gap; no current defect |
| goal | P3-03 | P3 | GoalWakeService::claim_and_spawn informational | Wiring verified intact |
| gateway | P2-01 | P2 | bot_loop_protection severed wire | DECIDE in source comments; needs architectural choice |
| gateway | P2-02 | P2 | channel start webhook mount race | Sub-microsecond window; lock-ordering decision |
| gateway | P2-04 | P2 | AgentLifecycleEvent::Registered/Deleted no subscriber | Cross-module (Panel + handlers); UI pass |
| gateway | P3-01 | P3 | /stop reply says "no active run" when cancel_session fails | UX; needs i18n-aware receipt |
| gateway | P3-02 | P3 | check_mention substring heuristic false-positives | Heuristic-vs-adapter-signal design choice |
| gateway | P3-03 | P3 | ChannelRegistry::unregister always None for started | API contract change; coordination needed |

## Cargo check

`CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo check -p alephcore --lib` — passes.
`CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo check -p alephcore --tests` — 16 errors remain, **all pre-existing** in unrelated modules (`src/vision/types.rs::OcrResult` missing `lines` field, `src/extension/stop_gate.rs::ExtensionStopHookVerifier` missing `veto_count` method, `src/acp/manager/persistence.rs` await in non-async closure). Confirmed by running the same check on `HEAD~6` before any of these review commits.

`CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo clippy -p alephcore --lib --no-deps` — 8 warnings, all pre-existing.

## Cross-module concerns

- **generation P1-07 / P1-09 (voice_http_client adoption + retry_transient adoption)**: both are mechanical refactors across 6+ / 18 providers respectively. Best done together in a dedicated voice-policy + retry-adoption pass with one test suite, not interleaved with a static review.
- **gateway P2-01 (bot_loop_protection severed wire)**: the pre-existing comment in `pair_loop_guard.rs:355-375` documents this as DECIDE. The fix needs an architectural choice (per-channel `HashMap<channel_id, PairLoopGuardConfig>` vs global block).
- **gateway P2-04 (AgentLifecycleEvent::Registered/Deleted no subscriber)**: a Panel-side wiring concern; the source-of-truth topics live in `gateway/agent_lifecycle.rs`.
- **generation error taxonomy**: with the From boundary now preserving status_code, provider, retry_after, and not misclassifying SerializationError, the dead error-classifier methods (`needs_user_action`, `should_fallback`, `retry_after`, `provider_name`, `user_friendly_message`) become more attractive to wire into gateway/voice/outbound.rs. Worth a dedicated classifier-wiring pass.

## What's NOT in this review (explicitly out of scope)

- The 2026-08-31 prior pass for the same 5 modules (committed as `c03b1cac0`, `26a10fb8d`, etc.) is referenced for context but not re-flagged. Prior findings are tracked separately.
- Test code in src/acp/, src/vision/, src/extension/ with 16 pre-existing compile errors — none are in the 5 modules under review.
- Architecture-level R1-R10 redlines — referenced when relevant (R8 for sync_primitives; R9 for prompt-contract surface) but not in scope for static review.
