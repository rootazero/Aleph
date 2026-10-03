# Module: src/wizard (occams-r10 review, 2026-10-02)

## Summary

- Files reviewed: 6 (`mod.rs`, `types.rs`, `prompter.rs`, `session.rs`, `flows/mod.rs`, `flows/onboarding.rs`)
- LOC: 1,706 (r9 was 1,306; +400 from new tests, KNOWN GAP comment, `validate_answer`, and updated model catalogue)
- Total findings: **1 high (carry-over) / 4 warning (1 carry-over, 3 new) / 1 suggested (1 carry-over) + 3 new suggested tests**
- r9 carry-over status (from `wizard-2026-08-29.md`):
  - [r9 High #1] Unbounded wait for client answer leaks task/session — **FIXED** (`ANSWER_TIMEOUT = 15 min` in `prompter.rs:23`; race-safe reclaim in `prompt()` at `prompter.rs:117-152`)
  - [r9 High #2] `OnboardingData` silently discarded at wizard end — **STILL PRESENT** (DEFERRED; replaced with `KNOWN GAP` comment at `flows/onboarding.rs:317-326`; defect unchanged)
  - [r9 Medium #1] `answer()` accepts any JSON, no server-side validation — **FIXED** (`validate_answer` at `session.rs:42-101`, exhaustive per-variant match, called from `answer()` at `session.rs:354`)
  - [r9 Medium #2] `next()` drops buffered notes on terminal status — **FIXED** (`next()` at `session.rs:248-294` now acquires lock first, drains via `try_recv()` before consulting status)
  - [r9 Medium #3] Answering a note step stales with `StepNotFound` — **FIXED** (`answer()` at `session.rs:366-371` now returns `Ok(())` when current step is a `Note` and no sender is pending)
  - [r9 Low #1] Answer value interpolated into error string — **FIXED** (error at `session.rs:380-383` now names the step id only)
  - [r9 Low #2] Duplicate pending step id silently overwrote first sender — **FIXED** (`Entry` API + `WizardSessionError::Internal("Duplicate pending step id '<id>'")` at `prompter.rs:101-110`)
  - [r9 Low #3] Second answer reported `StepNotFound` — **FIXED** (distinguishes `None` + Note vs `None` + non-Note at `session.rs:374-377`)
  - [r9 Low #4] `cancel_tx` is `RwLock` only ever write-locked — **STILL PRESENT** (cosmetic, DEFERRED unchanged)
  - [r9 Low #5] Hardcoded `model_options()` catalogue — **PARTIALLY ADDRESSED** (model ids updated to `claude-opus-4-8` / `claude-sonnet-4-6` / `claude-haiku-4-5` / `gpt-5.5` / `gpt-5.4` / `gemini-2.5-pro` etc.; structural drift risk unchanged — DEFERRED)
- NEW findings (not in r9): 1 critical (carry-over) / 4 warning / 3 suggested test

Round-9 applied five of its seven inline fixes correctly. The headline risk
(OnboardingData dropped) was deliberately re-scoped as a cross-module API
change; the in-module layer now owns an accurate `KNOWN GAP` marker rather
than a misleading comment. The four "still open" r9 items are re-evaluated
below; only the OnboardingData path is a true carry-over; the other three
were cosmetic or structural and were not progressed.

## Critical

(none)

## High

### **H-CARRY-1** [category: logic / architecture] `src/wizard/flows/onboarding.rs:317-326` — `OnboardingData` is still discarded when the wizard completes (r9 High #2, DEFERRED)

- **Description**: The onboarding wizard's stated contract — "Aleph is ready!" — is a lie in the literal sense. `OnboardingFlow::run` builds `OnboardingData` on the stack (`flows/onboarding.rs:294-300`), threads it through every configure_* stage, calls `review_and_finalize(prompter, &data)` (which formats and shows it as a summary note), and then returns `Ok(())`. `WizardFlow::run` yields only `Result<(), _>`; `WizardSession` and the gateway's `WizardSessionManager` (out of module) expose no channel for a flow's result. The boot path (`src/bin/aleph-server/commands/start/mod.rs`) only registers a flow factory. Net: collected answers never persist; the user's primary provider, primary model, and **plaintext primary API key** are dropped the moment the outro step is delivered. The literal r9 finding's "the outro is a lie" symptom is unchanged; the `KNOWN GAP` comment (`flows/onboarding.rs:317-326`) at least makes the lie visible in code.
- **Evidence**:
  - `flows/onboarding.rs:294-302` — `WizardFlow::run` calls `Self::review_and_finalize(prompter, &data).await?` and returns `Ok(())`; `data` goes out of scope at function return.
  - `session.rs:124-128` — `pub trait WizardFlow { async fn run(&self, prompter: &RpcPrompter) -> Result<(), WizardSessionError>; }` — no `OnboardingData` (or generic result) on the signature.
  - `flows/onboarding.rs:317-326` — `KNOWN GAP (deferred)` comment naming the exact cross-module call sites that must change.
- **Suggested fix** (still cross-module; flag this for the next round that owns gateway/handlers/wizard.rs and the boot path):
  1. Add a result seam to `WizardFlow`: `async fn run(&self, prompter: &RpcPrompter) -> Result<FlowResult, WizardSessionError>` where `FlowResult` is a serde-tagged enum the wizard framework owns (`Empty`, `Onboarding(OnboardingData)`).
  2. Have `WizardSessionManager` carry the terminal `FlowResult` alongside the terminal `WizardStatus`; expose it on the `wizard.next` RPC when `done: true`.
  3. The boot path (`src/bin/aleph-server/commands/start/mod.rs`) consumes `OnboardingData` and applies it to `Config` + the vault.
- **Jev severity check**: Jev `score` schema returns aggregate only in XML transport; per-finding calibration fell back to inline. Inline score: **3 (high)** — defect worsens with each passing release (more answers collected → more plaintext API keys left in process memory at zero benefit).

## Warning

### **W-NEW-1** [category: quality] `src/wizard/flows/onboarding.rs:170` — Final outro tells the user to run `al chat`, but the command is `aleph chat`

- **Description**: The literal outro text is `"Aleph is ready! Run 'al chat' to start."` (`flows/onboarding.rs:170`). The rest of the codebase consistently uses `aleph chat` (verified by grep — `gateway/handlers/chat.rs:1035` and `gateway/handlers/agent.rs:399` / `:2542` both name the command as `aleph chat`). The wizard's final user-facing note gives a wrong command name on the very screen that exists to onboard the user to the CLI.
- **Evidence**:
  - `flows/onboarding.rs:170` — `prompter.outro("Aleph is ready! Run 'al chat' to start.").await?;`
  - r9 report noted this outro as `"Aleph is ready! Run 'aleph chat' to start."` — so the typo is **new in r10**, not carried over.
  - Grep `aleph chat` in `src/**/*.rs` returns multiple matches; grep for the bare quoted `"al chat"` returns no other matches.
- **Suggested fix**: change to `"Aleph is ready! Run 'aleph chat' to start."`. One-token change.
- **Jev severity check**: inline score **1 (low)** — but surfaced as warning because it lands on the final UX touchpoint of an onboarding flow.

### **W-NEW-2** [category: logic] `src/wizard/flows/onboarding.rs:316-318` — Saying "No" at the review screen kills the entire wizard

- **Description**: `review_and_finalize` shows the summary and then asks `"Apply this configuration?"` (`flows/onboarding.rs:160`). If the user answers `false`, the function returns `Err(WizardSessionError::Cancelled)` (line 316). The spawn task in `session.rs:170-194` turns `Cancelled` into terminal status `Cancelled`. The flow task exits, the prompter is dropped, the step channel is closed, and the client sees `WizardStatus::Cancelled`. The user is back at square one and must restart the whole onboarding to edit a single mistake. Setup wizards conventionally offer "go back and edit the answer" rather than "throw everything away". This is a UX/Logic defect, not necessarily a security one — but the failure mode is data loss from the user's perspective.
- **Evidence**:
  - `flows/onboarding.rs:160` — `let confirmed = prompter.confirm("Apply this configuration?", true).await?;`
  - `flows/onboarding.rs:316` — `if !confirmed { return Err(WizardSessionError::Cancelled); }`
  - No "edit" path exists in `OnboardingFlow::run` (`flows/onboarding.rs:294-302`).
- **Suggested fix** (small, in-module):
  - If the user declines, treat it as a re-prompt: wrap `review_and_finalize` in a `loop { ... match confirmed { true => break, false => continue } }`. Cheap and preserves the user's collected answers (the `data` is in scope).
  - Or, accept the current behaviour as deliberate and rename the prompt to "Discard all answers and cancel?" so the destructive intent is in the wording.
- **Jev severity check**: inline score **2 (medium)** — correctness defect visible to every user who second-guesses a default.

### **W-NEW-3** [category: security] `src/wizard/session.rs:75-83` + `src/wizard/flows/onboarding.rs:111-122` — `validate_answer` accepts the empty string for `sensitive` API-key Text steps

- **Description**: `validate_answer` for `StepType::Text` (`session.rs:75-83`) checks `value.is_string()` and returns `Ok(())` for any string, including `""`. `RpcPrompter::text` (`prompter.rs:194-214`) similarly returns whatever string came back. `configure_primary` (`flows/onboarding.rs:111-122`) and `configure_secondary` (`flows/onboarding.rs:159-200`) both call `prompter.text(..., true /* sensitive */)` for the API key, with no post-validation. A client (legitimate, malicious, or buggy) can submit an empty `""`, and `OnboardingData::primary_api_key` will carry `Some("")`. Once the deferred result-path lands (H-CARRY-1), that empty string flows into `Config` / vault and produces a non-functional config that looks superficially valid (provider set, key set — just empty).
- **Evidence**:
  - `session.rs:75-83` — `StepType::Text => if value.is_string() { Ok(()) } else { ... }` accepts `""`.
  - `prompter.rs:194-214` — `RpcPrompter::text` only checks `value.as_str()`; passes empty through.
  - `flows/onboarding.rs:111-122` and `:159-200` — no `if api_key.trim().is_empty()` guard.
- **Suggested fix**:
  - In `validate_answer` for `StepType::Text`, when `step.sensitive` is true, reject `""` and whitespace-only values: `if step.sensitive && value.as_str().is_none_or(str::is_empty) { return Err(...); }`.
  - In `OnboardingFlow::configure_primary`/`configure_secondary`, after `prompter.text(...)`, do `if api_key.is_empty() { prompter.note("API key cannot be empty. Try again."); /* re-prompt or skip */ }`.
  - Decide deliberately whether whitespace-only is also rejected.
- **Jev severity check**: inline score **2 (medium)** — security-class issue (untrusted client path accepts bogus credential), user-impact (broken config).

### **W-NEW-4** [category: architecture] `src/wizard/session.rs:248-294` — Concurrent `next()` calls are not serialised and silently disagree on the answer

- **Description**: `WizardSession::next()` first reads `is_done()`, then either `try_lock`s (settled path) or `lock().await`s (live path) on `step_rx`, then `try_recv()`s, then potentially `recv().await`s. There is no documentation of a "one in-flight `next()` per session" invariant, and the API shape (a public `async fn next(&self)` returning a future) does not enforce it. Two outcomes under concurrent calls:
  - **Live path, one already holds the lock**: the second `next()` blocks on `step_rx.lock().await`. When the first one returns, the lock is released and the second gets it. The second then `try_recv`s — but the step the first one consumed is gone. `try_recv()` returns `Empty`, the second sees the session is still `Running`, falls through to `rx.recv().await`, and the client effectively sees "no step available yet". Two clients polling in parallel will both wait for the *next* step; the order in which they receive buffered steps is undefined.
  - **Settled path**: if the session has settled, `next()` takes `try_lock`; if another worker holds the lock, it returns `terminal_result()` immediately. The first worker that gets the lock returns the buffered step (or `terminal_result` if drained). The two responses disagree.
  - The gateway's `wizard.next` JSON-RPC handler presumably serialises calls (one client = one in-flight), so the bug is latent in the framework, not observable from the production path today. It would surface the moment a future gateway author batches or pipelines `wizard.next` calls.
- **Evidence**:
  - `session.rs:248-294` — `next()` body.
  - `session.rs:256-259` — settled path uses `try_lock` and short-circuits to `terminal_result()` on contention.
  - `session.rs:262-273` — live path uses `lock().await`, no documentation.
  - `session.rs:283-293` — final `rx.recv().await` may return a step that another concurrent caller is also waiting for.
- **Suggested fix** (small, in-module):
  - Document the contract on `WizardSession::next`: "one in-flight call per session; concurrent calls are not supported and may interleave".
  - Or, take `&mut self` on `next` (forces `&mut self` callers — but `WizardSession` is wrapped in `Arc<>` at the gateway, so this would force the gateway to break the `Arc`).
  - Or, replace `step_rx: Arc<Mutex<Receiver>>` with a `mpsc::Receiver<WizardStep>` owned by the session plus a per-session semaphore / queue that ensures FIFO delivery to a single in-flight consumer.
- **Jev severity check**: inline score **2 (medium)** — silent ordering corruption under future gateway batching; today a documentation gap.

### **W-CARRY-2** [category: quality] `src/wizard/session.rs:174` — `cancel_tx: Arc<RwLock<Option<oneshot::Sender<()>>>>` is only ever write-locked (r9 Low #4)

- **Description**: The only access is `self.cancel_tx.write().unwrap_or_else(|e| e.into_inner()).take()` in `cancel()` (`session.rs:380-383`). An `RwLock` enables concurrent reads, but no code path reads. `Mutex` states intent and removes the read/write machinery.
- **Evidence**: grep `cancel_tx` in `session.rs` returns three locations (declaration `:174`, `cancel()` `:382`, comment `:175`) — all writes.
- **Suggested fix**: `Arc<Mutex<Option<oneshot::Sender<()>>>>` and change the `take()` site accordingly. Pure cosmetics; correct code today.
- **Jev severity check**: inline score **0 (cosmetic)**.

## Suggested Test

### **T1** `session::tests` — empty string is rejected for sensitive steps

- **Suggested test**: extend `validate_answer_enforces_step_shape` to cover a sensitive Text step that rejects `""`, `" "`, and accepts `"  x  "`. Or, since `validate_answer` does not yet see sensitivity, push the rejection into `validate_answer` first and then write the test against that.

### **T2** `flows::onboarding::tests` (new) — `confirm(false)` at review re-prompts instead of cancelling

- **Suggested test**: a `TestFlow`-shaped flow that calls `confirm` twice and expects the second call to fire. Currently `OnboardingFlow::review_and_finalize` returns `Err(Cancelled)` on `false`; if the suggestion in W-NEW-2 is taken (loop), add a test that loops, supplies `false` once then `true`, and confirms `Ok(())`.

### **T3** `session::tests` (new) — concurrent `next()` either blocks the second until the first returns, or is documented as unsupported

- **Suggested test**: spawn two `tokio::spawn`s that each call `session.next().await`. Verify the contract; or, after documenting the invariant, add a `debug_assert!` that panics on re-entrancy (`std::cell::Cell` flag inside the session is enough since `next()` is `&self`).

## Per-perspective findings

### Security

- **W-NEW-3** (above): empty API-key accepted by `validate_answer` for sensitive Text steps.
- **G1** [quality]: `OnboardingData` stores API keys as plain `Option<String>`. There is no `zeroize` / `secrecy` / equivalent wrapper (no such dependency in `Cargo.toml`). Once H-CARRY-1 lands and the data is persisted, the keys sit in process memory in plaintext until drop. Today the keys are dropped seconds after being collected (H-CARRY-1), so the exposure window is small, but the wrapper is the right place to put hygiene before a result seam is added. **Suggested test**: not applicable (no observable behaviour to test without a secret-marshaller dependency). Inline severity: **1 (low)**, deferred.
- **G2** [quality]: `RpcPrompter` returns the answer value to the flow as `serde_json::Value` (`prompter.rs:104-148`). For a sensitive Text step, the value goes through `serde_json::Value::String(String)` (heap-allocated) and is then either deserialised into `String` (`prompter.rs:206-211`) or echoed back into `answers.value`. The library has no `Zeroizing<String>` in the call chain. No fix without a marshaller; flagged for the H-CARRY-1 follow-up.
- **No prompt-injection surface observed.** `intro` / `note` / `outro` take `&str` from `OnboardingFlow`; only `WizardStep.message` carries it. The message is shown verbatim to the client via `WizardNextResult` and never executed. No template engine, no shell interpolation.

### Logic

- **H-CARRY-1** (above): OnboardingData dropped.
- **W-NEW-2** (above): `confirm(false)` kills the wizard.
- **W-NEW-4** (above): concurrent `next()` silently disagrees.
- **L1** [logic, r10 evidence]: `session.rs:208-211` — `prompt_no_wait` ordering with respect to the spawn-task `select!` is correct but worth noting. When `intro()`/`note()`/`outro()` `prompt_no_wait` succeeds, the message is queued in the channel. When the flow returns `Ok(())` afterward, the spawn-task settles `Done`. The buffered steps are then drained on the next `next()` (per the r9 fix). The race window is: any client `next()` between `flow.run()` returning and `spawn` task writing `Done` will see the buffered step then `Done` (the r9 drain fix made this correct). Verified by `buffered_notes_survive_flow_completion` test.
- **L2** [logic, r10 evidence]: timeout path (`prompter.rs:117-152`) — confirmed race-safe by the r9 comment (`Late wizard answer recovered at timeout boundary`). Verified by `prompt_times_out_and_reclaims_pending_sender` (uses `start_paused = true`, so no wall-clock cost).
- **L3** [logic, r10 evidence]: `validate_answer` is total over the 5-variant `StepType` (`session.rs:42-101`); a future variant fails to compile. Good.
- **No cancellation/timeout behaviour change**. Timeout still 15 min, still returns `AnswerTimeout`, still settles `Error`.

### Architecture

- **H-CARRY-1** (above): the architectural seam is missing.
- **W-NEW-4** (above): concurrency contract undocumented.
- **A1** [architecture, r10 evidence]: `RpcPrompter` deliberately owns `step_tx`; `WizardSession` does not (`session.rs:155-164`, comment `:155-160`). This is the **correct** design — channel closure signals flow completion. Documented in two comments. No issue.
- **A2** [architecture, r10 evidence]: `#[async_trait]` overhead is acceptable here — wizard calls are low-frequency and the trait is shared between `RpcPrompter` and any future test mocks. No reason to switch to `trait-variant` (Rust 1.75+) for this module.
- **A3** [architecture, r10 evidence]: `OnboardingFlow` is a unit struct (`flows/onboarding.rs:38-40`); all state is in `OnboardingData` per-run. This keeps `run()` signature stable for H-CARRY-1's future `FlowResult` extension — no instance refactor needed.
- **R1–R10 (AGENTS.md redlines)**: no violations.
  - R1: no platform APIs; pure async Rust over channels.
  - R3: dependencies are `serde`, `serde_json`, `tokio`, `thiserror`, `async-trait`, `tracing`, `uuid`, `chrono` (none of these are new since r9) — all already core.
  - R4: the module holds no business logic; `OnboardingFlow` is a question script and option catalogue.
  - R7/R10: no shell-specific behaviour, no middleware layer.

### Quality

- **W-NEW-1** (above): outro command typo.
- **W-CARRY-2** (above): `cancel_tx` `RwLock` → `Mutex`.
- **G3** [quality]: `flows/onboarding.rs:289-302` — `review_and_finalize` `format!` string carries nested `if/else` inside `format_args!` for `Secondary:` and `Messaging Apps:`. ~10 lines of branching embedded in a string literal. Readability suffers; the bug from W-NEW-2 (the wrong interpretation of "no" as cancel) is harder to spot because of this. Suggested refactor: extract two helpers `format_secondary(data) -> String` and `format_messaging(data) -> String`. Inline severity: **0 (style)** — flag for a future cleanup.
- **G4** [quality, r10 evidence]: `prompter.rs:25-29` — `RPCPrompter::send` (`step_tx.send`) failure path: `self.step_tx.send(step).await.is_err()` then removes the pending sender. Correct, but uses `.unwrap_or_else(|e| e.into_inner())` (line `:104`) — uniform with the rest of the code (good).
- **G5** [quality, r10 evidence]: `flows/onboarding.rs:127-130` — hardcoded model catalogue (`model_options`). Updated for r10 (`claude-opus-4-8`, `gpt-5.5`, etc.) but will drift; same finding as r9 Low #5. Belongs behind the provider registry. Inline severity: **1 (low)**, deferred.
- **G6** [quality, r10 evidence]: `flows/onboarding.rs:170` — same file as W-NEW-1.
- **Magic numbers**: `mpsc::channel(16)` (`sessions.rs:5` + `prompter.rs:32`, mirrored in tests), `15 * 60` (timeout, `prompter.rs:23`), `for _ in 0..200` + `5ms` (`session.rs:485-491` test helper, `wait_for_terminal`). All have context: channel capacity backs backpressure for buffered notes; timeout is documented in the const comment; `wait_for_terminal` is a test helper with a hard 1s budget. None are unjustified.
- **Naming consistency**: `RpcPrompter` (struct, RPC channel) vs `WizardPrompter` (trait, abstraction) is conventional; `PendingAnswer` is `pub(crate)` and used across `prompter.rs` + `session.rs`. No issues.

## Conclusion

### Net delta from r9

The r9 review identified seven fixable findings and two deferred ones (OnboardingData path and `cancel_tx` `Mutex`). r10 closes six of the seven fixable findings correctly and leaves the seventh (`cancel_tx`) deferred unchanged. The OnboardingData path remains deferred and now carries an accurate `KNOWN GAP` note in place of the r9 misleading comment. The four genuinely new findings in r10 are: an outro command typo (W-NEW-1, trivial), a UX bug where "No" at review kills the wizard (W-NEW-2), a security-class defect where empty API keys pass `validate_answer` (W-NEW-3), and a concurrency-contract gap on `next()` (W-NEW-4). The module is **shape-degraded** relative to r9 in the sense that three of four new findings are latent bugs that the r9 fixes did not anticipate, but the framework primitives (`validate_answer`, `ANSWER_TIMEOUT`, drain-before-status) are now correct and well-tested.

### Fix order proposal (this module only)

1. **W-NEW-1** — one-token fix to `flows/onboarding.rs:170` (5 min, no risk).
2. **W-NEW-3** — extend `validate_answer` to reject empty/whitespace-only strings for `step.sensitive = true` Text steps, then add T1 test (30 min, low risk).
3. **W-NEW-4** — add a `// SAFETY: ...` doc comment to `WizardSession::next` documenting the single-in-flight contract, OR enforce with a `Cell<bool>` re-entrancy guard. (15 min, low risk.)
4. **W-NEW-2** — change `OnboardingFlow::review_and_finalize` to loop on `false` rather than cancel. Add T2 test. (45 min, low risk.)
5. **W-CARRY-2** — `cancel_tx` `RwLock` → `Mutex`. Pure rename. (5 min, no risk.)
6. **H-CARRY-1** — out of module. Escalate to next round's gateway/handlers/wizard.rs owner. Flag in the round-10 synthesis that the r9+r10 same finding has now been deferred twice.

### Suggested test (in addition to T1, T2, T3 above)

A property-style test in `session::tests` that drains a flow emitting N (where N ∈ {0, 1, 16, 17}) notes via `tokio::join!` between the client `next()` calls and the spawn-task settle, to confirm the drain-before-status invariant holds at channel-boundary cases.

---

**Module verdict: shape-degraded** — three latent new defects (W-NEW-2 UX, W-NEW-3 security, W-NEW-4 concurrency contract) on top of the standing high-severity OnboardingData carry-over; framework primitives themselves are correct.