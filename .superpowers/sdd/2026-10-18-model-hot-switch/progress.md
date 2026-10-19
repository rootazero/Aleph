# SDD ledger — plan: docs/superpowers/plans/2026-10-18-model-hot-switch.md

## Setup

- Workspace: `D:/Workspace/Aleph-model-hot-switch/.superpowers/sdd/2026-10-18-model-hot-switch`
- Branch: `feat/model-hot-switch`
- Base before implementation: `0076b2f`
- Spec authority: `docs/superpowers/specs/2026-10-18-model-hot-switch-design.md`
- Implementation constraint: only the dedicated worktree may be modified; no worker may dispatch another worker/reviewer.

## Pre-flight plan conflict scan

| Scope | Producer/consumer or internal check | Finding | Ruling |
|---|---|---|---|
| Task 1 ↔ Task 2 | Task 1 `SessionModelSnapshot`/generation API consumed by Task 2 `SessionModelProvider` | Interface dependency is explicit; Task 2 must wait for the names and semantics to land. | Ruling: run Task 1 first; Task 2 consumes the committed API. Cost if wrong: provider adapter rework only. |
| Task 1 review override | Reviewer found that `update_session_model` schedules unguarded async metadata patches and that the new generation key is not rejected by `sessions.patch`; this violates the spec’s durable ordering and single-writer constraints despite the original three-file Task 1 scope. | The spec is authoritative over the plan’s initial file list. | Ruling: extend Task 1 to `src/gateway/session_manager/ops/modify.rs` for generation-conditional metadata update and `src/gateway/handlers/session/db_handlers/modify.rs` for rejecting `MODEL_PIN_GENERATION_SESSION_KEY`. Cost if wrong: one extra atomic metadata path and a larger review; leaving it unfixed can resurrect an old model after restart. |
| Task 1 ↔ Task 3 | Task 1 session generation/persistence consumed by Task 3 `AssistantMessage` provenance and runner integration | Both touch `turn_model.rs`/session identity but Task 3 does not need to rewrite Task 1’s setter. | Ruling: Task 3 may inspect but must not alter Task 1’s selection writer; resolve conflicts by preserving one writer. Cost if wrong: duplicate state or resume mismatch. |
| Task 2 ↔ Task 3 | Task 2 `ResolvedModelTarget`/`last_request_target` consumed by Task 3 runner/Think path | Direct interface dependency; Task 3 cannot start until Task 2 is committed. | Ruling: sequential boundary after Task 2. Cost if wrong: provenance fields cannot prove actual target. |
| Task 2 ↔ Task 4 | Task 2 request resolver used by surface-triggered selection; Task 4 uses existing `/model`/`select_model` writer | No shared implementation file mandated beyond provider contracts; Task 4 must not create another writer. | Ruling: surface tests may be prepared in parallel, but surface code waits for Task 2 provider contract. Cost if wrong: a second route could silently bypass request resolution. |
| Task 3 ↔ Task 4 | Task 3 changes `select_model`-adjacent semantics and Task 4 changes `select_model` diagnostics | Same control seam; both must preserve explicit override precedence and `sessions.patch` rejection. | Ruling: Task 3 owns request precedence/provenance; Task 4 owns user-facing adapter/diagnostic adjustments and rebases/merges after Task 3. Cost if wrong: CLI/Panel may report a state different from the request path. |
| Task 3 ↔ Task 5 | Task 3 changes `SessionEvent::AssistantMessage` serialization/projection; Task 5 documents and audits it | Task 5 consumes final event field names and cannot document before Task 3 stabilizes them. | Ruling: defer all reference-document edits to Task 5. Cost if wrong: stale contract docs. |
| Task 4 ↔ Task 5 | Task 4 adds surface/compaction tests; Task 5 runs full checks and documentation census | Straight dependency, no contradictory behavior. | Ruling: Task 5 is final integration gate. |
| Task 1 interface hardening | Independent review found both `SessionPinSink::persist_with_generation` and `SessionStore::patch_session_if_model_pin_generation_newer` had defaults that could silently downgrade to unguarded legacy writes; review also found malformed durable generation treated as missing. | The approved spec requires durable selection to be ordered and fail-closed; compatibility defaults cannot weaken that guarantee. | Ruling: make generation-aware sink/store paths fail closed by default (or required where internal implementations are exhaustive), keep only the legacy `persist`/`patch_session` APIs for unrelated callers, and reject malformed generation metadata during durable write and rehydrate. Add focused tests. |
| Task 1 final review | Final independent review found no Critical or Important blocker after ordinary `patch_session` guards, `u64::MAX` refusal, and malformed metadata fail-closed handling. | Focused tests and rustfmt passed; full workspace verification remains Task 5. | Ruling: Task 1 is ready to commit; preserve its one-writer/generation seam when implementing Task 2+. |
| Task 3 self-consistency | Optional AssistantMessage fields are durable, ignored by prompt builders, populated after calls, and coexist with run snapshots/explicit override. | Internally consistent; event enum match expansion may be broader than the listed files. | Ruling: update every compiler-discovered exhaustive match and keep provenance out of prompts. Cost if wrong: serialization or prompt regression. |
| Task 4 self-consistency | `/model` and Panel picker route to `select_model`, refusal preserves old selection, status displays session selection, compaction uses existing seam. | Internally consistent. | Ruling: no new Panel-only writer or new fallback chain. Cost if wrong: split-brain session state. |
| Task 4 command-model bypass | Final integration review found `slash_command_body::command_model_pin` still accepted arbitrary declared model ids as `ModelOverride::Raw`, bypassing selectable roster and readiness gates. | A command/plugin declared model is a turn selection surface and must share the same fail-closed resolver; no raw unknown id may enter the run path. | Ruling: resolve declared models through `resolve_selectable_model`, return a qualified override, and add task-local tests for unknown, unavailable, ambiguous, and unpublished publication. Preserve request override precedence and declared aliases/retirement semantics. |
| Task 5 SDD evidence | Final review found no feature-specific SDD report despite the approved plan requiring independent acceptance evidence. | Code and focused tests are insufficient evidence without the boundary matrix and exact commands recorded. | Ruling: add `task-5-report.md` before final commit, with actual worktree, diff scope, commands, results, warnings, and residual risk. |
| Final integration review | Review found async persistence warning-only, raw workflow/per-turn override bypass, and pre-request provenance recording. | All three could violate durable selection, fail-closed selection, or actual-target provenance semantics. | Ruling: `select_model` uses `update_session_model_durable` and rollback on store rejection; runner resolves explicit per-turn targets through selectable roster and defers an unavailable flow fallback when session selection exists; `SessionModelProvider` records target only after successful delegation. |

## Global rulings carried into implementation

- Session model selection has one authoritative writer: `select_model` via the existing `/model` gateway path; `sessions.patch` remains rejected for model pins.
- Live selection changes affect the next provider request boundary, while an in-flight request keeps an immutable target and an explicit per-request override remains higher precedence.
- Request provenance will use optional fields on the existing durable `SessionEvent::AssistantMessage` if the existing event shape supports it, preserving backward-compatible serde defaults and prompt exclusion; no new prompt-visible event is introduced without a spec-driven reason.
- Invalid or unavailable selected providers fail closed rather than silently falling back.

## Progress

- Task 1: complete in commit `4061ee89c`; generation ordering, ordinary patch guards, malformed/MAX fail-closed verified
- Task 2: implemented and focused provider/decorator tests passed; awaiting final integration gate
- Task 3: implemented and focused runner/event/think tests passed; awaiting final integration gate
- Task 4: implemented; selector roster/rejection and Panel tests passed; command-model bypass fixed and target-worktree slash tests passed
- Task 5: complete in commit `7f053a30a`; final diff review, SDD evidence, core check/clippy, focused tests, Panel checks, docs and pre-commit hook passed
