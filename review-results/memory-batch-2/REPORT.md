# Memory Batch 2 — `src/memory/dreaming/*` Code Review

**Date**: 2026-08-12
**Path**: `src/memory/dreaming/*` (36 files, ~16 959 lines)
**Reviewer**: static (security / logic / architecture / quality)
**Threshold**: all findings actionable; no scoring pass.

## Module Totals

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    3 |     7 |    4 |   14 |

---

## Findings

### [HIGH] `dreaming/mod.rs:952-988` — nightly fan-out `loop {}` over corpora has no per-corpus timeout; one stuck corpus blocks the whole night
- **Category**: DoS / architecture
- **Description**: `loop { match maintenance_corpora(...).next() { ... } }` walks every non-base corpus and runs the full pipeline. A single corpus with a wedged `index_file` (a 100 MB note, a 10 k-deep wikilink chain) blocks the loop forever. `MAX_CORPUS_CYCLES_PER_NIGHT = 8` caps the *count*, not the *time*. A user with 20+ corpora is silently limited to 8/night, so the tail accumulates.
- **Suggested fix**: Wrap each `corpus_cycle` call in `tokio::time::timeout(Duration::from_secs(MAX_CORPUS_CYCLE_SECS), ...)`. On timeout, mark the corpus as `DreamStatus::Error` and continue. Default 30 minutes is generous; corpus budgets that large are a different problem.

### [HIGH] `dreaming/stages/mention_weave.rs:241, 308` — `unsafe { ... }` blocks dereference raw pointers for buffer re-use
- **Category**: security / safety
- **Description**: Two `unsafe` blocks use pointer arithmetic on a `Vec<u8>` reinterpreted as `*mut f32`. The intent is to avoid an `f32::from_le_bytes` loop on a 1024-element vector (≈ 4 KB), but the unsafety is unnecessary: `bytemuck::cast_slice` is already a transitive dep of the project. The handwritten `unsafe` block adds 30 lines of UB surface (alignment, size, aliasing) for one hot path.
- **Suggested fix**: Replace with `bytemuck::cast_slice::<u8, f32>(bytes)` (or `try_cast_slice` for the validation). Zero behavior change, zero `unsafe`. Add a test that the cast's length matches the `embedding.dim` from the DB row.

### [HIGH] `dreaming/stages/note_decay.rs:120-180` — `note.created_at < 7 * 86400` protection uses elapsed seconds, not calendar days
- **Category**: logic
- **Description**: `if now - note.created_at < 7 * 86400 { protect }`. `7 * 86400` is 604 800 seconds = exactly 7.0 solar days. A note created at 23:59:59 yesterday is exactly 1 day old; one created at 00:00:01 today is also 1 day. A clock skew of 1 hour across NTP correction can flip a borderline note in/out of protection. The `<` is also a `<=` problem: a note created *at* the boundary slips through.
- **Suggested fix**: Use `chrono::Duration::days(7).num_seconds()` and an explicit `>=` check. Pure hardening; not a behavior change for any honest clock.

### [MEDIUM] `dreaming/mod.rs:439` — `Lazy<AtomicI64>` for `LAST_ACTIVITY_TS` is shared globally; per-agent activity is lost
- **Category**: architecture
- **Description**: The `LAST_ACTIVITY_TS` atomic is global, so `last_user_activity()` returns the last activity across all agents. A user working in two projects in parallel will see the active one of project A's maintenance as "activity" and skip the dream for project B. The DreamDaemon's `ActivityGate` then under-fires.
- **Suggested fix**: Make the activity tracker per-(agent_id, corpus) — keyed by the composed partition id, not the base id. The activity API already takes an `agent_id`; the storage just needs to be a `DashMap<String, AtomicI64>` or similar.

### [MEDIUM] `dreaming/event_log.rs:135` — `loop {}` reading the event log has no end-of-stream signal
- **Category**: logic
- **Description**: The event-log consumer's `loop {}` depends on `recv()` returning `None` (channel closed) or an error. If the producer dies holding the sender, the consumer's `recv()` will block indefinitely; the surrounding `tokio::spawn` leaks. The fix is `try_recv` + `tokio::task::yield_now().await` in a bounded loop, with a per-tick cap on consumed events.
- **Suggested fix**: Add `let mut processed = 0; loop { match rx.try_recv() { Ok(ev) => { ...; processed += 1; if processed >= BATCH { processed = 0; tokio::task::yield_now().await; } } Err(TryRecvError::Empty) => { tokio::time::sleep(...).await; } Err(TryRecvError::Disconnected) => break, } }`.

### [MEDIUM] `dreaming/strategy.rs:1-100` — `DreamStrategy::Consolidate/Synthesize/Conserve` is matched without a fallthrough; future variants panic
- **Category**: quality
- **Description**: The selector switches on strategy; new variants added without updating the match produce a `match _ => unreachable!()` style site. A grep for `DreamStrategy::` shows 3 variants; a future "Distill" variant will fail to compile at the call site. The issue is *future* — today the variants are exhaustive.
- **Suggested fix**: Add an `Other` catch-all in the selector that logs a `tracing::warn!` and returns `Conserve` (the safest default). The compiler still catches new variants, but at runtime the daemon does not panic.

### [MEDIUM] `dreaming/evolution/budget.rs` — `EditBudget::try_spend` accepts `bytes: u64` but only checks `bytes_remaining`; not `edits_remaining`
- **Category**: logic
- **Description**: The function `if budget.try_spend(bytes)` returns `true` when EITHER budget is sufficient. A 1-edit / 1-byte budget consumes 1 byte and the function reports `true` (action may proceed), even though 1 byte is far too small for a meaningful supersede. The action then writes a tiny note that is immediately re-distilled.
- **Suggested fix**: Either (a) require `bytes >= MIN_SUPERSEDE_BYTES` (a constant, e.g. 64) and `try_spend` only when both budgets are non-zero, or (b) split into `try_spend_edits()` and `try_spend_bytes()` and require the caller to check both. The single-return signature today is a footgun.

### [MEDIUM] `dreaming/validation.rs:1-100` — dream-validation issues are aggregated into a `Vec<ValidationIssue>`; empty Vec is indistinguishable from a Vec of all-pass
- **Category**: logic
- **Description**: An empty `Vec<ValidationIssue>` can mean "no issues found" (good) or "the validator short-circuited at the first failure" (bad). The DreamReport consumes it as a single boolean.
- **Suggested fix**: Add a `ValidationIssue::short_circuited: bool` and have the consumer fail closed on `true`. Or split into `run_validation` (returns `Result<(), ValidationIssue>`) so the absence of issues is explicit.

### [MEDIUM] `dreaming/skill_gate.rs:300` — test references `existing_note_path: "../../etc/passwd".into()` for `sanitize_title`; the production gate accepts this because `sanitize_title` strips `..`
- **Category**: security
- **Description**: The test asserts that `../../etc/passwd` sanitises to `etcpasswd`, but the **gate** in `skill_gate.rs` does not call `sanitize_title` before passing the path to `apply_distill_action`. A hand-crafted action with `existing_note_path: "../../etc/passwd"` (bypassing the ingest gate) writes to `note/.../skill/etcpasswd.md` in the agent's directory — not a path traversal, but a misnamed file that confuses the resolver. The gate is on the LLM output, not on the action's *path* field.
- **Suggested fix**: Add `sanitize_title` (or a stricter `validate_existing_note_path`) inside `apply_distill_action`'s preconditions. The "valid" set is `^[a-z0-9-]+(/[a-z0-9-]+)?$` — anything else is rejected as malformed.

### [LOW] `dreaming/skill_gate.rs:30-100` — `SkillGateDecision::Reject(reason)` accumulates reasons in a single `String`; multi-reason cases lose the partial history
- **Category**: quality
- **Description**: When both format and semantic checks fail, only the format reason is retained. For audit, the full set matters.
- **Suggested fix**: `SkillGateDecision::Reject(Vec<RejectionReason>)` and render the joined reason only at the log site.

### [LOW] `dreaming/stages/note_review.rs:1-100` — `note_review` retries are unbounded per session; the queue's `retry_count` is incremented without a max
- **Category**: logic
- **Description**: A failed LLM review retries indefinitely; the `increment_review_retry` SQL function returns the new count, but no caller checks for a ceiling. The `archive_review` function exists but is never called.
- **Suggested fix**: Add a `MAX_REVIEW_RETRIES = 3` check at the call site; archive after the third failure with a clear `status = 'failed'`.

### [LOW] `dreaming/project_cycle.rs:1-100` — `corpus_needs_maintenance` reads the most recent dream report from disk on every call
- **Category**: performance
- **Description**: A nightly cycle touches N corpora; each call to `corpus_needs_maintenance` re-reads the agent's dream_reports table. The result is in-memory elsewhere (`DreamContext::report`); the function is recomputing it.
- **Suggested fix**: Pass the most-recent `DreamReport` (or its `created_at` and status) into the function instead of re-fetching.

### [LOW] `dreaming/stages/graph_recompute.rs:50-150` — `community` / `relevance` are recomputed from scratch every cycle
- **Category**: performance
- **Description**: The stage reads the full graph, computes communities, and stores them. The result changes slowly; re-computing nightly is the right cadence for `reference` category but is overkill for `transcript` (which churns hourly) and slow for `entity` (which is append-mostly).
- **Suggested fix**: Stage-level guard: skip the recompute if the underlying graph hasn't changed since the last pass (`graph_modified_at > last_recompute_at`).

## Cross-References

- `dreaming/mod.rs:952` and `dreaming/evolution/budget.rs` — the loop's per-corpus timeout complements the budget cap. Together they bound: (a) total wall-clock per night, (b) destructive edits per cycle. Neither alone is sufficient.
- `dreaming/stages/mention_weave.rs:241, 308` and `dreaming/stages/feedback_distill.rs` — both touch the embedding blob; replacing the `unsafe` once is cheaper than the next refactor.
- `dreaming/mod.rs:439` and `dreaming/strategy.rs` — per-corpus activity is the natural input to strategy selection; today the strategy selector runs on global signals.

## Strengths

- `dreaming/stages/mod.rs::is_provider_exhausted` is the right escape valve for "the LLM is dead, do not hammer it". The 13 000 calls/night number in the docstring is a real datapoint, not a guess.
- `dreaming/evolution/evidence.rs::gate_supersede_evidence` is the recall-evidence gate. Without it, a 0.5-confidence supersede of a 1000-recall note would destroy the user's most-accessed knowledge.
- `dreaming/evolution/budget.rs` introduces a per-cycle `EditBudget` that the stages (NoteDecay, NoteConsolidate, SkillDistill) all share. The right shape.
- `dreaming/validation.rs` runs a `DreamValidationReport` *after* the cycle, not as a precondition. Let the dream run, then audit; do not block the cycle on a brittle pre-check.
- `dreaming/stages/skill_distill.rs::parse_distill_response` extracts the outermost `{...}` and tolerates markdown fences. The `end <= start` guard is the kind of defensive check that lets the dream daemon survive a hostile LLM response.
