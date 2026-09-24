# Browser Native Tools Round-1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the five real capability gaps vs pi-agent-browser-native (structured error contract + nextActions, effect-arrival probes, tab identity persistence, ref pre-dispatch checks, high-value-controls hint) and extend `browser_exec` with controlled `repeat`/`if` steps — without weakening any of the five security chokepoints.

**Architecture:** Route ① (chokepoint-centralized): one `browser_tools/recovery.rs` owns the failure-category enum + next-action registry; all 26 tools inherit it through the existing egress chokepoints. Core-layer work (tab identity, effect probes, exec DSL) lands in `src/browser/` + `exec.rs` first (line 2), tool-layer wiring second (line 1). `exec.rs` is exclusively line-2-owned.

**Tech Stack:** Rust (tokio), `crates/aleph-cdp` (sole CDP client — extend `methods::` wrappers only), existing `page_state` RefTable, FakeBackend testkit.

**Spec:** `docs/superpowers/specs/2026-09-24-browser-native-tools-r1-design.md`

## Global Constraints

- Worktree: `/home/zou/data/workspace/Aleph-browser-r1`, branch `browser-native-r1`. **Never touch main directly.**
- **No new dependencies.** CDP extensions only via `crates/aleph-cdp` `methods::` wrappers. No second CDP client.
- Memory guard: before **every** cargo invocation, `awk '/MemAvailable/{exit ($2<4194304)?1:0}' /proc/meminfo` — if it fails, wait and retry. Compile with `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1`.
- Commits: English, `<scope>: <description>` (e.g. `browser: ...`).
- R9: no new resident description bytes; touching any tool DESCRIPTION must respect `CATALOG_DESCRIPTION_CEILING_BYTES` (0 B headroom — trim before adding).
- nextActions are **data, not routing** (R7): never auto-execute, never reorder by preference, never hide the raw error.
- Every new guard must be falsified (delete the guarded line → test goes red) before the task is considered done.
- `BrowserError::StaleRef { ref_id, reason: StaleReason }` **already exists** (`src/browser/error.rs:229`, `StaleReason::{Navigated, NodeGone, Unknown}` at `error.rs:18-25`). Reuse it — do not create a parallel stale-ref error.
- Wire compatibility: tool error outputs keep `{success, message}`; the new `recovery` key is **additive only**.
- Verification: never rely on `cargo check` alone — it does not compile `#[cfg(test)]`. Minimum per task: `cargo test -p alephcore --lib <filter>`. Full seven-command set runs at T8.

## Review Focus

Inputs the spec is silent about, most likely to bite, and the task whose tests pin them:

1. **`repeat` whose `until` is already true on iteration zero** — must run zero iterations, not one (T4 test).
2. **`until` condition evaluation fails mid-loop** (transport error, not "condition false") — must abort the whole exec, never read as "condition met" or "keep going" (判据 §8) (T4 test).
3. **Ref minted by a different profile's snapshot** — `is_minted_shape` true, absent from this table → `StaleReason::Unknown` → `stale_ref` category, not a panic, not a click on nothing (T6 test).
4. **Effect probe on an engine without usable `Runtime.evaluate` semantics** — result must carry `effect_verification: skipped(engine)`, never a fabricated "verified" (判据 §8) (T3 test).
5. **Tab closed by the page itself (`window.close`) between `list_tabs` and the action** — `TabGone`, not generic `ActionFailed` (T1 test).

---

### Task 1: Tab identity registry (B3) — LINE 2

**Files:**
- Modify: `src/browser/tab_registry.rs` (add identity layer; keep existing touch/forget/select_victims API)
- Modify: `src/browser/error.rs:54` (new `TabGone` variant beside `TabNotFound`)
- Modify: `src/browser/manager.rs` (record identity at tab discovery points)
- Test: `src/browser/tab_registry.rs` `#[cfg(test)]` module

**Interfaces:**
- Consumes: existing `TabRegistry`, `tab_registry::active_tab_id(&[TabLine]) -> Option<String>`
- Produces (T5 consumes the error shape):
  ```rust
  // error.rs
  TabGone { tab_id: String, last_url: Option<String> },
  ```
  ```rust
  // tab_registry.rs
  pub struct TabIdentity { pub target_id: Option<String>, pub last_url: Option<String> }
  pub fn record_identity(&self, profile: &str, tab_id: &str, target_id: Option<String>, url: Option<String>);
  pub fn resolve_identity(&self, profile: &str, tab_id: &str, live_target_ids: &[String]) -> Result<TabIdentity, BrowserError>;
  pub fn last_url(&self, profile: &str, tab_id: &str) -> Option<String>;  // T6 reuses this for URL-drift
  ```

- [ ] **Step 1: Falsify-first — write the failing tests**

```rust
#[test]
fn tab_gone_is_structured_not_a_last_row_guess() {
    let reg = TabRegistry::new();
    reg.record_identity("p", "t1", Some("TARGET-1".into()), Some("https://a/".into()));
    // After re-attach the target is gone; the registry must NOT fall back to
    // "last listed row".
    let err = reg.resolve_identity("p", "t1", &[]).unwrap_err();
    assert!(matches!(err, BrowserError::TabGone { ref tab_id, .. } if tab_id == "t1"));
}

#[test]
fn page_closed_tab_between_list_and_action_is_tab_gone() {
    let reg = TabRegistry::new();
    reg.record_identity("p", "t1", Some("TARGET-1".into()), None);
    let err = reg
        .resolve_identity("p", "t1", &["TARGET-2".to_string()])
        .unwrap_err();
    assert!(matches!(err, BrowserError::TabGone { .. }));
}

#[test]
fn unknown_tab_id_says_so_instead_of_tab_gone() {
    let reg = TabRegistry::new();
    // Never recorded: this is "I don't know" (判据 §8), not TabGone.
    let err = reg.resolve_identity("p", "t9", &["TARGET-2".to_string()]).unwrap_err();
    assert!(matches!(err, BrowserError::TabNotFound(_)));
}
```

- [ ] **Step 2: Verify they fail**

Run: `awk '/MemAvailable/{exit ($2<4194304)?1:0}' /proc/meminfo && CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib tab_registry 2>&1 | tail -5`
Expected: FAIL — `record_identity` / `resolve_identity` do not exist (compile error is the red here; that counts as falsification for a new API).

- [ ] **Step 3: Count the writers first (判据 §6)**

Run: `rg -n "touch\(|record_activity|TabTable|ensure_tab|attach_tab" src/browser --type rust | rg -v "^\s*//" | head -30`
Write the list into the commit message body: every place tab identity is currently derivable (EngineHandle::TabTable, ProfileManager::tab_registry, playwright CLI session state). This census decides where `record_identity` is called from — every discovery point, or the registry is a second-class copy.

- [ ] **Step 4: Implement**

`tab_registry.rs`: add `identities: RwLock<HashMap<String, HashMap<String, TabIdentity>>>` keyed by profile then tab_id. `record_identity` upserts; `resolve_identity` returns `TabNotFound` for never-recorded, `TabGone{tab_id, last_url}` when recorded but `target_id` is `Some` and absent from `live_target_ids`, `Ok(identity)` otherwise. Locking: `unwrap_or_else(|e| e.into_inner())` (P7).

`error.rs`: add the `TabGone` variant with a Display that names the tab and last known URL, never claims the page state.

- [ ] **Step 5: Wire recording into the cdp backend path** — every place the cdp backend learns "tab X has targetId Y" calls `record_identity`. The playwright/MCP drivers keep the marker heuristic for *discovery*, but once discovered the registry holds the mapping; `active_tab_id`'s remaining "last row" fallbacks are deleted where a registry answer exists.

- [ ] **Step 6: Verify + commit**

Run: `CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib tab_registry 2>&1 | tail -5 && CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib --no-run 2>&1 | tail -3`
Expected: PASS; full lib compiles (catches call-site breakage from Step 5).
```bash
git add -A && git commit -m "browser: tab identity registry — targetId-first resolution, structured TabGone, no last-row guessing"
```

---

### Task 2: Debt — Task-14 session_key/profile unification (LINE 2)

**Files:**
- Modify: `src/browser/cdp_backend/mod.rs:87-110` (the `CdpBackend::new` constructor with the "Task 14 owns this" comment)
- Modify: callers of `CdpBackend::new` (find via `rg -n "CdpBackend::new" src/ -g '!*test*'`)
- Test: constructor-level test in `cdp_backend/mod.rs`

**Interfaces:**
- Consumes: `LaunchRequest { profile, session_key, .. }`
- Produces: `CdpBackend::new(registry, engine, req, ssrf_guard, command_timeout)` — **the separate `profile` argument is deleted**; `req.profile` becomes the single spelling, and `req.session_key` is derived at the one call site that builds `LaunchRequest`.

- [ ] **Step 1: Read the full comment block** (`sed -n '80,115p' src/browser/cdp_backend/mod.rs`) and list every production caller. Decide per caller: does its `session_key` legitimately differ from `profile`? (Design says they are *deliberately separable* — one profile, many sessions — so the fix is **one construction site** that names both from one source, not forcing equality.)

- [ ] **Step 2: Write the failing test**

```rust
#[test]
fn backend_profile_and_registry_key_are_one_spelling() {
    // Constructing with a LaunchRequest whose profile is "p" must yield a
    // backend that resolves profile "p" — there is no second argument to
    // disagree with it.
    let src = include_str!("mod.rs");
    let new_fn = src.split("pub fn new(").nth(1).expect("constructor");
    let sig_end = new_fn.find(") -> Self").expect("signature end");
    let sig = &new_fn[..sig_end];
    assert!(
        !sig.contains("profile: impl Into<String>"),
        "a second profile argument is a second truth (判据 §1) — build \
         LaunchRequest and profile from one source instead"
    );
}
```

- [ ] **Step 3: Verify fail** → red on current signature.

- [ ] **Step 4: Implement** — delete the `profile` parameter; `req.profile` is the authority. At the (single) production construction site, build `LaunchRequest` so `session_key` derives from the same principal-scoped profile key (`profile.rs:742` `principal_profile_key`). Update the comment: replace "Task 14 owns this" with a statement of what the code now does.

- [ ] **Step 5: Evaluate, don't expand** — `manager.rs:1171` (session-level sweep) and `cdp_backend/cookies.rs:666` (Task 16 prober): read each; if the fix is >30 min, record it verbatim in the commit message and FL §3.12 as still-deferred. Do not expand scope.

- [ ] **Step 6: Verify + commit**

Run: `CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib cdp_backend 2>&1 | tail -5 && CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib --no-run 2>&1 | tail -3`
```bash
git add -A && git commit -m "browser: one spelling of profile in CdpBackend::new (Task-14 debt)"
```

**Merge checkpoint A:** T1+T2 done → `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo test -p alephcore --lib 2>&1 | tail -3` (full lib run once) → merge `browser-native-r1` progress stays on branch; line 1 does not start rebasing yet.

---

### Task 3: Effect-arrival probes (B2) — LINE 2

**Files:**
- Modify: `src/browser/cdp_backend/actions.rs` (click/type/fill arms)
- Modify: `src/browser/engine/capability.rs` (new `effect_probe` field + `CAP_FIELDS` row, doc comment **naming the verbs** browser_click/browser_type/browser_fill_form per the R39 guard)
- Modify: `src/browser/backend.rs` (no trait change — probes live inside the cdp backend impl; the trait default stays as-is)
- Test: `src/browser/cdp_backend/actions.rs` tests + capability census update

**Interfaces:**
- Consumes: `crates/aleph-cdp` `Runtime.evaluate` (exists — used by wait_probe), `EngineCapabilities`
- Produces:
  ```rust
  // In the tool-facing result path (exec StepResult / click output JSON):
  "effect_verification": "verified" | "skipped(<engine>)" | "failed"
  ```
  ```rust
  // capability.rs
  ("effect_probe", |c| c.effect_probe),  // obscura: measured value, chromium: Supported
  ```

- [ ] **Step 1: Read how click is currently dispatched** (`rg -n "Input.dispatchMouseEvent|Runtime.evaluate" src/browser/cdp_backend/actions.rs | head`). If dispatch is via `Input.dispatch*`, page events are *trusted*; if via JS `.click()`, they are not — the probe's trust claim must match the dispatch path actually used. Write which one it is in the commit message.

- [ ] **Step 2: Failing tests**

```rust
#[test]
fn probe_js_marks_only_the_target_element() {
    let js = effect_probe_install_js("e7");
    assert!(js.contains("data-aleph-probe"), "probe must tag the resolved node, not document");
    assert!(js.contains("once: true") || js.contains("once:true"), "one-shot listener");
}

#[test]
fn skipped_is_an_honest_label_not_a_success() {
    // An engine whose capability row is not Supported must surface
    // skipped(<engine>) — never "verified" (判据 §8 / Review Focus #4).
    let v = EffectVerification::for_engine(Engine::Obscura);
    assert!(matches!(v, EffectVerification::Skipped(_)) || obscura_supports_probe());
}
```

- [ ] **Step 3: Implement** — add a dedicated `BrowserError::EffectNotDelivered { verb: &'static str, detail: String }` variant in `error.rs`（**deviation from spec §3, deliberate**: T5's `classify` is an exhaustive match over variants, so smuggling this through `ActionFailed(String)` + a message prefix would force string-matching inside classify — exactly the 判据 violation the enum exists to prevent; the category enum gains a 15th variant `EffectNotDelivered`, see T5）. Probe flow: before dispatch, `Runtime.evaluate` installing a one-shot capture listener on the resolved node that sets `window.__alephProbeHit = <event type>`; after dispatch, read it back; on absence → `EffectNotDelivered`. Read-back failure = `skipped`, not `failed`. Probe install/readback errors never turn a successful action into a failure (the probe *absence* is reported, the action stands). Define the tool-facing enum alongside: `pub enum EffectVerification { Verified, Skipped(Engine), Failed }`.

- [ ] **Step 4: Capability row** — add `effect_probe: Cap` to `EngineCapabilities` + `CAP_FIELDS`; measure obscura against the real binary before claiming Supported; the row's doc comment names the three verbs. Run the existing capability census tests — they enforce the row↔verb link.

- [ ] **Step 5: Verify + commit**

Run: `CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib cdp_backend 2>&1 | tail -5 && CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib capability 2>&1 | tail -3`
```bash
git add -A && git commit -m "browser: effect-arrival probes for click/type/fill on the cdp backend"
```

---

### Task 4: exec DSL — `repeat` / `if` + in-batch ref latch (T4) — LINE 2

**Files:**
- Modify: `src/builtin_tools/browser_tools/exec.rs` (**line-2 exclusive**)
- Test: same file's `#[cfg(test)]`

**Interfaces:**
- Consumes: `plan_actions` (exec.rs:420), `wait_probe` condition evaluation, `MAX_EXEC_ACTIONS=50`, existing per-step approval mapping
- Produces:
  ```rust
  #[serde(tag = "action", rename_all = "snake_case")]
  pub enum ExecAction {
      // … existing 16 variants …
      Repeat { actions: Vec<ExecAction>, until: ExecCondition, max_iterations: u32 },
      If { condition: ExecCondition, then: Vec<ExecAction>, otherwise: Option<Vec<ExecAction>> },
  }

  pub struct ExecCondition {
      #[serde(default)] pub text: Option<String>,
      #[serde(default)] pub text_gone: Option<String>,
      #[serde(default)] pub selector: Option<String>,
      #[serde(default)] pub url_contains: Option<String>,
  }
  ```
  (`ExecCondition` also derives `Default` — the tests use `..Default::default()`.)
  Exactly-one-field validation, same error wording style as `browser_wait_for`'s five-way mutex.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn plan_actions_expands_repeat_and_enforces_cap() {
    let inner = vec![ExecAction::PressKey { key: "Enter".into() }; 3];
    let actions = vec![ExecAction::Repeat {
        actions: inner, until: ExecCondition { selector: Some(".next".into()), ..Default::default() },
        max_iterations: 20,
    }];
    let err = plan_actions(&actions).unwrap_err();
    assert!(err.contains("limited to"), "3×20=60 expanded steps must exceed the cap of 50");
}

#[test]
fn repeat_with_until_already_true_runs_zero_iterations() { /* Review Focus #1 */ }

#[test]
fn until_condition_error_aborts_not_continues() { /* Review Focus #2: transport Err ≠ condition met */ }

#[test]
fn ref_step_after_navigate_is_refused_before_dispatch() {
    // plan: navigate, then click{ref_id}. The latch must reject the click
    // BEFORE any page side effect, with a resnapshot next-action hint.
}

#[test]
fn condition_requires_exactly_one_field() { /* zero → graceful error; two → graceful error */ }
```

- [ ] **Step 2: Verify fail** (compile error on missing variants = red).

- [ ] **Step 3: Implement planning** — `plan_actions` recurses into `Repeat.actions` / `If.then` / `If.otherwise`; expanded step count = `max_iterations × len(actions)` for Repeat (worst case is the honest count — the budget must cover the worst legal execution), `max(len(then), len(else))` for If; total > 50 → same rejection shape as today. Nested `Repeat` inside `Repeat` is rejected ("one level of nesting", YAGNI). Every nested step maps to its existing ActionType — **zero new approval knobs**.

- [ ] **Step 4: Implement execution** — the run loop evaluates `until` via the same wait_probe condition path (single evaluation, not a wait); `Repeat` iterates until condition-true or `max_iterations`; a condition *error* aborts the whole exec (first-failure-abort semantics unchanged). **Ref latch**: after any executed `Navigate` (or `Dialog` that navigated), set `refs_stale = true`; any later step carrying `ref_id` fails pre-dispatch with message ending "refs may be stale; take a fresh snapshot" — the batch-internal analogue of T6.

- [ ] **Step 5: Wall-clock** — unchanged 600s budget; iteration steps measure `elapsed` between steps exactly as today. No budget-table change needed (`browser_exec` already at 630s in `BUILTIN_TOOL_BUDGETS_MS` — verify with `rg -n "browser_exec" src/**/budget*.rs`).

- [ ] **Step 6: Verify + commit**

Run: `CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib browser_tools::exec 2>&1 | tail -8`
```bash
git add -A && git commit -m "browser: exec DSL gains repeat/if steps and an in-batch ref staleness latch"
```

**Merge checkpoint B (line 2 complete):** full `--lib` run + `cargo clippy -p alephcore -- -D warnings` → report to orchestrator → **orchestrator merges line-2 state; line 1 rebases onto it before starting T5.**

---

### Task 5: Structured error contract + nextActions (B1) — LINE 1

**Files:**
- Create: `src/builtin_tools/browser_tools/recovery.rs`
- Modify: `src/builtin_tools/browser_tools/mod.rs` (`pub(crate) mod recovery;` + call at the `backend_error_text` chokepoint)
- Test: `recovery.rs` tests + census test in `mod.rs`

**Interfaces:**
- Consumes: `BrowserError` (all variants incl. new `TabGone`), `StaleReason`
- Produces:
  ```rust
  #[serde(rename_all = "snake_case")]
  pub enum BrowserFailureCategory {
      NavigateBlocked, SecretInInput, StaleRef, TabGone, EngineBusy,
      UnsupportedByEngine, UnsupportedByDriver, WaitTimeout, SelectorMiss,
      DialogPending, Transport, ApprovalRequired, BudgetExhausted,
      EffectNotDelivered, // T3's probe verdict: dispatch claimed success, no event arrived
      Unknown,
  }

  pub struct NextAction {
      pub tool: &'static str,
      pub reason: String,
      pub params_hint: serde_json::Value,
      pub safety: NextActionSafety, // SafeToRetry | NeedsUserDecision | ChangesPage
  }

  pub fn classify(err: &BrowserError) -> BrowserFailureCategory;
  pub fn recovery_for(err: &BrowserError) -> Recovery; // { category, next_actions }
  ```
  Wire shape (additive): `"recovery": {"category": "stale_ref", "next_actions": [{"tool": "browser_snapshot", ...}]}`.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn every_browser_error_variant_has_a_category() {
    // Construct one instance per variant; classify() must never panic and
    // must return Unknown ONLY for genuinely unclassifiable input (判据 §8).
    assert_eq!(classify(&BrowserError::TabGone { tab_id: "t".into(), last_url: None }),
               BrowserFailureCategory::TabGone);
    assert_eq!(classify(&BrowserError::StaleRef { ref_id: "e1".into(), reason: StaleReason::Navigated }),
               BrowserFailureCategory::StaleRef);
    assert_eq!(classify(&BrowserError::EngineBusy { .. }), BrowserFailureCategory::EngineBusy);
    // …one assertion per variant…
}

#[test]
fn unknown_is_not_a_dumping_ground() {
    // NavigationFailed must NOT classify as Unknown just because nobody
    // wrote an arm — it is NavigateBlocked.
    assert_eq!(classify(&BrowserError::NavigationFailed("x".into())),
               BrowserFailureCategory::NavigateBlocked);
}
```

- [ ] **Step 2: Verify fail.**

- [ ] **Step 3: Implement `classify`** — exhaustive `match` (no wildcard arm; a new `BrowserError` variant must fail compilation here — that IS the exhaustiveness guard).

- [ ] **Step 4: Implement the next-action registry** — a `const` table, one entry per category that has a sane recovery. `StaleRef → browser_snapshot`; `TabGone → browser_tabs{action:"list"}`; `EngineBusy → browser_session{action:"capabilities"}`; `UnsupportedByEngine → browser_session{action:"switch_engine"}`; `WaitTimeout → browser_snapshot`; `EffectNotDelivered → browser_snapshot`（最常见成因是目标元素已移动/变化，重拍快照优先于盲目重试）。Entries carry static `params_hint` templates only (no session state).

- [ ] **Step 5: The census (判据: no ghost tools)**

```rust
// mod.rs tests
#[test]
fn recovery_registry_entries_name_real_tools() {
    let names = all_builtin_tool_names(); // the SAME table registration uses —
    // find it first (rg "BUILTIN_TOOL_DEFINITIONS|const NAME" src/builtin_tools)
    // and derive from it; do not hand-copy a list (判据 §5).
    for entry in recovery::REGISTRY {
        assert!(names.contains(&entry.tool),
            "recovery suggests `{}` which is not a registered tool", entry.tool);
    }
}
```
Falsify it: temporarily point one entry at `browser_nonexistent` → must go red.

- [ ] **Step 6: Wire at the chokepoint** — in `mod.rs`, wherever a tool currently formats `backend_error_text` into its `{success:false, message}` output, add the `recovery` key via one helper `recovery::attach(output_json, err)`. 26 tools: wire by editing the chokepoints they already share, NOT 26 hand edits — if a tool bypasses the chokepoint, that bypass is a finding to fix, not a place to hand-wire.

- [ ] **Step 7: Verify + commit**

Run: `CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib browser_tools 2>&1 | tail -8`
```bash
git add -A && git commit -m "browser: structured recovery contract (category + nextActions) at the egress chokepoints"
```

---

### Task 6: Ref pre-dispatch precheck (B4, single-tool layer) — LINE 1

**Files:**
- Modify: `src/builtin_tools/browser_tools/mod.rs` (`make_backend_and_tab_guarded` caller sites get a precheck helper)
- Modify: `src/builtin_tools/browser_tools/{click,type_text,fill_form,select,hover}.rs`
- Modify: `src/browser/engine/capability.rs` (`ref_precheck` row, verbs named in doc comment)
- Test: per-tool tests with FakeBackend + the new capability census

**Interfaces:**
- Consumes: `RefTable::resolve(&RefId) -> Result<RefEntry, StaleReason>` (refs.rs:183), `TabRegistry::last_url` (T1), `recovery::Recovery` (T5)
- Produces: `pub(crate) async fn precheck_ref(backend, tab_id, ref_id) -> Result<(), BrowserError>` — returns the existing `BrowserError::StaleRef` early, before any side effect.

- [ ] **Step 1: Failing tests (FakeBackend)**

```rust
#[test]
fn stale_ref_is_refused_before_any_page_side_effect() {
    // FakeBackend records call order; assert NO click/dispatch call was
    // recorded when the precheck fails.
}

#[test]
fn cross_profile_ref_is_unknown_not_a_panic() {
    // is_minted_shape true, absent from this profile's table →
    // StaleReason::Unknown → stale_ref recovery (Review Focus #3).
}

#[test]
fn url_drift_since_snapshot_is_named() {
    // last_url (T1) differs from the tab's current url → the recovery message
    // says "page navigated since snapshot" and names browser_snapshot.
}
```

- [ ] **Step 2: Verify fail.**

- [ ] **Step 3: Implement** — precheck runs only when the resolved backend is the cdp one (it owns the RefTable); other backends skip (capability row `ref_precheck` says so — honest asymmetry, not a silent gap). Placement: after `make_backend_and_tab_guarded`, before the action dispatch, in each of the five tools.

- [ ] **Step 4: Capability row + census** — `ref_precheck: Cap` on `EngineCapabilities`; doc comment names browser_click/browser_type/browser_fill_form/browser_select/browser_hover.

- [ ] **Step 5: Verify + commit**

Run: `CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib browser_tools 2>&1 | tail -8`
```bash
git add -A && git commit -m "browser: pre-dispatch ref staleness precheck on ref-taking tools"
```

---

### Task 7: High-value-controls hint on truncated snapshots (B5) — LINE 1

**Files:**
- Modify: `src/browser/page_state/render.rs` (truncation path)
- Test: `render.rs` tests

**Interfaces:**
- Consumes: the interactivity/role decision in `page_state/roles.rs` (derive, don't hand-list — 判据 §5); existing truncation point in render
- Produces: when truncation cuts interactive elements, append:
  ```
  Omitted high-value controls:
  - textbox "Search" [ref=e12]
  - button "Sign in" [ref=e13]
  …and 7 more
  ```
  ≤20 entries, ≤2048 bytes; refs remain valid (they're minted, just not rendered).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn truncated_snapshot_lists_omitted_interactive_controls_with_live_refs() {
    // Build a PageState with 30 interactive nodes, render with a tiny budget,
    // assert the section exists, entries carry [ref=eN], and every listed ref
    // resolves in the RefTable (the list must never name a dead ref).
}

#[test]
fn omitted_section_bounds_itself_and_counts_the_rest() {
    // >20 omitted controls → exactly 20 lines + "…and N more" with honest N.
}

#[test]
fn untruncated_snapshot_has_no_omitted_section() { /* section absent, not empty */ }
```

- [ ] **Step 2: Verify fail.**

- [ ] **Step 3: Implement** — the role set comes from the same table/谓词 that decides interactivity in `roles.rs`; if that decision is scattered, the task is to make it one function first, then consume it (that unification is in-scope; a hand-copied list is not). Section bytes count against the same snapshot budget — never exceed `max_chars` because of the hint.

- [ ] **Step 4: Verify + commit**

Run: `CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib page_state 2>&1 | tail -5`
```bash
git add -A && git commit -m "browser: truncated snapshots name their omitted high-value controls"
```

---

### Task 8: Docs + full verification (LINE 1, after T5–T7 merge)

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md` (§3.12 new round entry; 附录 D/E only if a *new criterion shape* emerged)
- Modify: `qa/README.md` (attach-stage extension note; caps rows `ref_precheck`/`effect_probe`)
- Modify: `docs/superpowers/plans/2026-09-24-browser-native-tools-r1.md` (check the boxes)

- [ ] **Step 1: FL §3.12 entry** — round summary: gap-analysis conclusion, what landed (B1–B5 + exec DSL + Task-14 debt), 刻意不做 list (JS sandbox, pi tool surfaces, for_each_ref, Electron, source-lookup, cross-restart tab identity), the two new capability rows. 触发器进附录 E only for genuinely new shapes.

- [ ] **Step 2: qa/README.md** — `browser_managed attach` now also asserts post-re-attach tab identity; `browser_dual caps` probes the two new rows. Do not copy stage counts into CLAUDE.md (that drifted once already).

- [ ] **Step 3: Full verification set (seven commands, in order; MemAvailable check before each cargo run)**

```
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
just test-shared
just _stage-shell-placeholders && cargo clippy --workspace --all-targets
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo test -p alephcore --lib
```

- [ ] **Step 4: Commit + report**

```bash
git add -A && git commit -m "docs: browser round-1 locator entry, qa stage notes, plan ledger"
```
Report to orchestrator: task ledger (done/skipped/deferred), any Review-Focus finding that surprised, and the deferred-debt list for FL.
