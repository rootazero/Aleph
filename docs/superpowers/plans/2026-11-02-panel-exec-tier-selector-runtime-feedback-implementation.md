# Panel ExecTierPicker Runtime Feedback and Selection Reliability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the existing `ExecTierPicker` visibly reliable when permission data or session persistence fails, while preserving the current `sessions.patch`, first-message `chat.send`, session snapshot, and dangerous-command floor contracts.

**Architecture:** Keep `ExecTierPicker` as the product owner of the composer tier pill. Move only deterministic request/state decisions into a small adjacent state module so they can be tested without mounting Leptos. The component will use session identity plus a monotonically increasing request generation to discard late results, commit existing-session selections only after `sessions.patch` succeeds, and retain local first-message selections.

**Tech Stack:** Rust 2021, Leptos 0.8 CSR, `leptos_i18n`, existing `DashboardState` RPC wrappers, Cargo unit tests for `aleph-panel`, existing Panel localization and semantic theme tokens.

**Spec:** `docs/superpowers/specs/2026-11-02-panel-exec-tier-selector-runtime-feedback-design.md`

## Global Constraints

- Preserve `ToolPermissionsApi::get_global` as the only catalogue read and `ToolPermissionsResponse::session_tier_presets()` as the compatibility fallback.
- Preserve `set_exec_tier` → `sessions.patch` for existing sessions and `session_dials_for_send(false, ...)` → `ChatApi::send` for first messages.
- `None` continues to mean Follow global; no new RPC method, wire field, permission tier, or authorization rule.
- Full-tier retry must require the existing second confirmation again.
- A late async result applies only when the captured session identity and request generation are still current; session switches reset pending, error, and confirmation state.
- Empty, refusal, and generic failure must remain visibly distinct; raw RPC/backend errors must not become selector copy.
- Do not add dependencies, selector UI paths, pairing UI, direct Tauri calls, browser credential storage, fake actors, or test endpoints.
- Before modifying tests, read the applicable `.opencode/skills` testing guide and follow its package command requirements.
- Before every Cargo command, run `python qa/browser_live/check_memory.py` from the repository root and use `CARGO_BUILD_JOBS=1`.
- Native WebView2 validation remains a separate gate and is not required to implement this plan.

## Review Focus

- A save for session A completing after switching to session B must not mutate B; test request identity and generation rejection in Task 1.
- A failed `sessions.patch` must not leave the pill displaying an uncommitted override; test rollback and retry in Task 2.
- An empty or refused catalogue must not hide the control or silently look like Follow global; test visible state and Retry in Task 2.
- A new conversation must still carry its selected tier on the first `chat.send`; preserve and test `session_dials_for_send` in Task 3.
- Full confirmation must not be bypassed by Retry, popover close, or session switching; test all three paths in Task 2.

---

### Task 1: Add deterministic picker state and request-generation rules

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/chat/exec_tier_picker_state.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/chat/mod.rs`
- Test: `interfaces/webchat/src/platform/wide/views/chat/exec_tier_picker_state.rs` (`#[cfg(test)]`)

**Interfaces:**

- Produces `CatalogState` with `Loading`, `Ready`, `Empty`, `Refused`, and `Failed` variants.
- Produces `SaveState` with `Idle`, `Saving`, and `Failed` variants.
- Produces `PickerRequest { session_key: Option<String>, generation: u64 }`.
- Produces `next_generation(current: u64) -> u64`.
- Produces `accepts_result(current: &PickerRequest, request: &PickerRequest) -> bool`.
- Produces `selection_commit_allowed(save_state: SaveState, session_key: Option<&str>) -> bool` or an equivalent pure predicate with the same behavior; the exact name may follow local style, but its input/output contract must remain explicit.

- [ ] **Step 1: Read the applicable test skill**

Read the relevant `.opencode/skills` guide for Rust/Panel unit tests before editing the test module. Record its package-specific test command in the implementation notes.

- [ ] **Step 2: Write failing unit tests**

Add tests named:

- `request_result_is_rejected_after_session_switch`
- `request_result_is_rejected_after_newer_generation`
- `request_result_is_accepted_for_current_session_and_generation`
- `catalog_state_distinguishes_empty_refused_and_failed`
- `existing_session_commit_requires_successful_save`
- `new_session_selection_does_not_require_save`

Assert exact session identity/generation behavior and that `None` remains a valid Follow-global selection.

- [ ] **Step 3: Run the focused tests to confirm RED**

Run from the Panel package directory as required by the test skill, using the package’s `cargo test --lib` filter for the new state tests. Expected: compile/test failure because the state module and predicates do not yet exist.

- [ ] **Step 4: Implement the pure state module**

Implement the enums, request identity, generation increment, and predicates in `interfaces/webchat/src/platform/wide/views/chat/exec_tier_picker_state.rs`. Keep this module free of Leptos, RPC, browser APIs, and raw error strings. Use `Option<String>` for the captured session key and `u64` for generation.

- [ ] **Step 5: Register the module and run GREEN**

Add `pub mod exec_tier_picker_state;` to `interfaces/webchat/src/platform/wide/views/chat/mod.rs`, then rerun the focused state tests. Expected: all new state tests PASS.

- [ ] **Step 6: Commit the isolated state contract**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/mod.rs interfaces/webchat/src/platform/wide/views/chat/exec_tier_picker_state.rs
git commit -m "fix(panel): model exec tier picker request state"
```

---

### Task 2: Make catalogue loading and session selection transactional

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/exec_tier_picker.rs`
- Modify: `interfaces/webchat/locales/en.json`
- Modify: `interfaces/webchat/locales/zh.json`
- Test: `interfaces/webchat/src/platform/wide/views/chat/exec_tier_picker_state.rs`

**Interfaces:**

- Consumes Task 1 `CatalogState`, `SaveState`, `PickerRequest`, `next_generation`, and `accepts_result`.
- Existing component remains `pub fn ExecTierPicker() -> impl IntoView`.
- Existing RPC signatures remain `ToolPermissionsApi::get_global(&DashboardState) -> Result<ToolPermissionsResponse, String>` and `set_exec_tier(&DashboardState, &str, Option<&str>) -> Result<(), String>`.

- [ ] **Step 1: Extend failing state tests for transitions**

Add tests named:

- `load_failure_retains_failed_state_and_can_retry`
- `empty_catalogue_is_not_ready`
- `save_failure_keeps_previous_override`
- `save_success_commits_new_override`
- `retrying_full_requires_fresh_confirmation`
- `session_switch_resets_pending_save_and_confirmation`

Pin that a failed refresh does not silently convert a previous visible result into Follow global, that Empty is distinct from Refused, and that a Full retry starts unconfirmed.

- [ ] **Step 2: Run focused tests to confirm RED**

Run the focused Panel unit-test command. Expected: the new transition tests fail because the component still uses optimistic `chat.session_exec_tier.set` and has no explicit load/save state.

- [ ] **Step 3: Add explicit local signals and request identity**

In `ExecTierPicker`, add local signals for catalogue state, save state, current request generation, captured session identity, and retryable selection. Keep catalogue state UI-local; never add it to `SessionSnapshot` or `ChatState`.

On each load, increment the generation and capture the current session key. Apply success/failure only when `accepts_result` passes and the component is still live. Use latest-request-wins or single-flight behavior so an older refresh cannot overwrite a newer one. A failed refresh must render `Failed` or `Refused`, not erase the last committed session override.

- [ ] **Step 4: Render distinct catalogue states and retry**

Keep the pill available while loading, empty, refused, or failed. Disable tier rows until `Ready`; keep Follow global usable in Empty when the current session allows it. Render localized loading, empty, refusal, and generic failure copy. Add a Retry action that starts a new generation. Route refusal copy through the existing `admin_refusal` convention and do not display raw `Err(String)` text.

Add only semantic existing theme tokens: form/control tokens for the picker and feedback tokens for refusal/failure status. Do not introduce raw colors or reuse unrelated status tokens.

- [ ] **Step 5: Make existing-session selection transactional**

Change the existing-session path so it captures `{session_key, generation, selected tier}` and sets `SaveState::Saving` before calling `set_exec_tier`. Do not update `chat.session_exec_tier` or close the popover until the call succeeds and the request is still current. On success, commit the selected `Option<String>`, clear confirmation, and close. On failure, preserve the previous override, set `SaveState::Failed`, retain the explicit selection for retry, and show localized failure feedback.

For a new conversation with no session key, continue updating `chat.session_exec_tier` immediately and close after the local selection; do not call `sessions.patch`.

- [ ] **Step 6: Reset state on session changes and disposal**

Observe the active session identity. When it changes, increment generation, reset saving/error/retry/confirmation state, and prevent late responses from applying. Guard every post-`await` signal write with the request identity and component-liveness check supported by the existing Leptos version.

- [ ] **Step 7: Preserve Full confirmation across normal selection and retry**

Keep the existing two-click Full behavior. A failed Full save’s Retry must return to the unconfirmed state and require two clicks. Closing the popover or switching sessions must clear the arming state. Follow global remains a one-click `None` selection.

- [ ] **Step 8: Add localized strings**

Add English and Chinese entries for loading, no selectable tiers, retry, saving, save failure, and generic catalogue failure. Reuse the existing admin-refusal translation for refusal. Keep copy concise and state what the user can do next.

- [ ] **Step 9: Run focused tests to verify GREEN**

Run the state tests and the existing shared dial tests. Expected: all new transition tests and all existing dial tests PASS; no change to `session_dials_for_send` semantics.

- [ ] **Step 10: Commit the transactional picker**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/exec_tier_picker.rs interfaces/webchat/src/platform/wide/views/chat/exec_tier_picker_state.rs interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "fix(panel): make exec tier picker failures visible"
```

---

### Task 3: Lock the send, restore, and safety contracts

**Files:**
- Modify: `shared/ui_logic/src/state/composer_dials.rs` only if a regression exposes a real gap; otherwise no production change.
- Modify: `interfaces/webchat/src/platform/wide/views/chat/state/mod.rs` only if session-switch reset needs an existing hook; preserve the current snapshot shape.
- Test: `shared/ui_logic/src/state/composer_dials.rs`
- Test: `interfaces/webchat/src/platform/wide/views/chat/exec_tier_picker_state.rs`

**Interfaces:**

- Consumes `SessionKnobs`, `SendDials`, and `session_dials_for_send(session_exists: bool, pills: &SessionKnobs) -> SendDials`.
- Preserves `ChatApi::send(..., dials: &shared_ui_logic::state::SendDials, ...)` and wire field `exec_tier`.

- [ ] **Step 1: Add or strengthen contract tests**

Pin these existing behaviors:

- `first_send_carries_every_dial` keeps the selected tier on a new conversation;
- `a_live_session_still_carries_the_tier` keeps tier on every live-session send;
- `follow_global_carries_nothing` produces `None` for the override;
- session snapshot restore keeps the selected tier and restores Follow global as `None`;
- Full confirmation cannot be bypassed by Retry or session change.

- [ ] **Step 2: Run RED/GREEN focused contract checks**

Run the shared UI logic test filter and Panel state test filter. If the current code already passes, record that no production change is needed for this task; the task’s output is the regression coverage.

- [ ] **Step 3: Commit only if this task adds a meaningful regression test**

```bash
git add shared/ui_logic/src/state/composer_dials.rs interfaces/webchat/src/platform/wide/views/chat/state/mod.rs interfaces/webchat/src/platform/wide/views/chat/exec_tier_picker_state.rs
git commit -m "test(panel): preserve exec tier send and restore contracts"
```

Do not create a commit if no file changed.

---

### Task 4: Validate the Panel build and document the product boundary

**Files:**
- Modify: `docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md` only if the latest selector finding or native run status needs a dated note.
- Modify: `docs/reference/FEATURE_LOCATOR.md` §6.11.2 only if the same boundary note is required there.

**Interfaces:**

- No native QA protocol or cleanup contract changes.
- The selector fix is documented as a Panel runtime-feedback change; native acceptance remains separately reported by its actual run boundary.

- [ ] **Step 1: Run focused Panel tests**

From the Panel package directory, run the applicable unit-test command from the test skill. Expected: all picker state, shared dial, and existing Panel library tests PASS.

- [ ] **Step 2: Run the required static/type checks**

From repository root, run `python qa/browser_live/check_memory.py`, then with `CARGO_BUILD_JOBS=1` run `cargo check -p aleph-panel --lib --no-default-features --features panel-modular`. Expected: PASS with only baseline warnings, if unchanged.

- [ ] **Step 3: Run repository checks**

Run from repository root:

```bash
bun run check
python qa/browser_live/check_docs.py
git diff --check
```

Expected: all commands pass; no raw selector error text or forbidden direct transport path appears in the diff.

- [ ] **Step 4: Rebuild the Panel artifact when source checks pass**

From `D:/Workspace/Aleph/interfaces/webchat`, run:

```bash
bun run panel:tabs:build -- --clean --output=.panel-live/shell
```

Expected: a new `build.json` records the current `cdc4c87f20ac8f1a4a79e7bc4144b9ce721e50da` revision. Regenerate and independently validate provenance only if the acceptance workflow needs fresh artifacts.

- [ ] **Step 5: Update evidence documentation**

If the product fix was validated without a new native launch, record that distinction in the bilingual run ledger and `FEATURE_LOCATOR.md` only when needed. Do not call the selector validation native acceptance, and do not claim it resolves `monitor_auth_failed` without a native run proving that boundary.

- [ ] **Step 6: Commit documentation and final implementation**

```bash
git add interfaces/webchat shared/ui_logic docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md docs/reference/FEATURE_LOCATOR.md
git commit -m "fix(panel): make exec tier selection observable"
```

---

## Self-review

- **Spec coverage:** Goals and anti-goals map to Tasks 1–3; state distinctions and retry are Task 2; session identity, Full confirmation, first-send carriage, and restore are Tasks 1–3; verification and native separation are Task 4.
- **Step scan:** Every task step has one action and an expected result. Exact interfaces are named before use. Component implementation choices remain open only where the approved design intentionally leaves signal/CSS names to the builder.
- **Type consistency:** `PickerRequest`, `CatalogState`, `SaveState`, `next_generation`, and `accepts_result` are introduced in Task 1 and consumed consistently in Task 2. Existing `SessionKnobs`, `SendDials`, and `ChatApi::send` signatures remain unchanged in Task 3.
- **Review focus:** All five focus items have explicit tests in Tasks 1–3.
- **Proportion:** The plan specifies files, interfaces, tests, commands, and boundaries without reproducing implementation bodies.

Plan status: complete and ready for execution-method selection. No product code, build, or WebView2 launch has been performed after the design approval.
