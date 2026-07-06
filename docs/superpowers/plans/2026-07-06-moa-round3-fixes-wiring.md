# MoA Round-3 Fixes & Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate MoA session-activation into a single arm/disarm source, close the silent-no-op gap on two unvalidated arm paths, restore a sticky MoA preset after a `/moa` one-shot, gate the `/moa` slash path behind operator role, and document the VESR aggregator-attribution choice.

**Architecture:** MoA activation ("arm") is currently inlined at 4 sites, 2 of which skip preset validation. Introduce `src/providers/moa/activation.rs` with three free functions (`arm_sticky`, `arm_one_shot`, `disarm`) as the single source; rewrite all 4 sites onto them. Extend the single-slot `SessionMoaPref` with a `restore` box so a one-shot can reinstate a displaced sticky. Add an operator-role gate at the `/moa` interception sites. All changes live in `src/providers/`, `src/builtin_tools/`, `src/gateway/`, `src/orchestrator/` — zero changes to `src/harness/` (R10).

**Tech Stack:** Rust, tokio, `crate::sync_primitives::RwLock`, existing MoA modules. Tests: `#[test]` / `#[tokio::test]` in-file `#[cfg(test)]` modules.

## Global Constraints

- **MSRV 1.95**; edition idioms already in the touched files — match surrounding style.
- **R10 (thin harness):** No file under `src/harness/` may be touched. F4 lives in `src/orchestrator/harness_bridge/runner_impl.rs` (attribution logic already there).
- **Cargo frugality (user rule):** Each task runs at most ONE targeted test via `cargo test -p alephcore <test_name> --lib`. NEVER run the full suite. A single `cargo check -p alephcore --lib` closes the plan (Task 6).
- **Poison handling:** all lock acquisitions use `.write().unwrap_or_else(|e| e.into_inner())` / `.read().unwrap_or_else(|e| e.into_inner())` — match `session_moa_handle.rs`.
- **Operator-role rule (canonical):** `None | Some("operator") => operator`, else not. Mirrors `TurnContext::caller_is_operator` (`src/tools/turn_context.rs:53`).
- **Preset-not-found message (canonical copy, reused verbatim):** `"MoA preset '{}' not found — use the moa tool (action='list') to see presets."` with `preset.unwrap_or("<default>")`.
- **Commit style:** `<scope>: <description>`, English. No push (user decides, per prior rounds).

---

### Task 1: F2 — `SessionMoaPref.restore` + one-shot stash + `take_for_run` reinstate

**Files:**
- Modify: `src/providers/session_moa_handle.rs`
- Test: `src/providers/session_moa_handle.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `SessionMoaPref { preset: Option<String>, one_shot: bool, restore: Option<Box<SessionMoaPref>> }`
  - `pub fn set_session_moa_one_shot(session_key: &str, preset: Option<String>)` — arms one-shot, stashing a displaced sticky into `restore`.
  - `take_for_run` (unchanged signature `pub fn take_for_run(session_key: &str) -> Option<SessionMoaPref>`) now reinstates a stashed sticky when consuming a one-shot.
  - `set_session_moa(session_key, preset, one_shot)` unchanged signature; now sets `restore: None`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/providers/session_moa_handle.rs`:

```rust
#[test]
fn one_shot_over_sticky_reinstates_sticky_on_consume() {
    let key = "test:moa:f2:reinstate";
    set_session_moa(key, Some("deep".to_string()), false); // sticky
    set_session_moa_one_shot(key, None); // one-shot over sticky
    // Slot now holds the one-shot, carrying the stashed sticky.
    let taken = take_for_run(key).unwrap();
    assert!(taken.one_shot);
    assert!(taken.preset.is_none());
    // Consuming the one-shot reinstates the sticky for the next run.
    let after = get_session_moa(key).unwrap();
    assert!(!after.one_shot);
    assert_eq!(after.preset.as_deref(), Some("deep"));
    assert!(after.restore.is_none());
    clear_session_moa(key);
}

#[test]
fn one_shot_over_empty_slot_removes_on_consume() {
    let key = "test:moa:f2:empty";
    set_session_moa_one_shot(key, Some("x".to_string()));
    let taken = take_for_run(key).unwrap();
    assert!(taken.one_shot);
    assert!(taken.restore.is_none());
    assert!(get_session_moa(key).is_none()); // nothing stashed → removed
}

#[test]
fn one_shot_over_one_shot_stashes_nothing() {
    let key = "test:moa:f2:double-oneshot";
    set_session_moa_one_shot(key, Some("a".to_string()));
    set_session_moa_one_shot(key, Some("b".to_string()));
    let taken = take_for_run(key).unwrap();
    assert_eq!(taken.preset.as_deref(), Some("b"));
    assert!(taken.restore.is_none());
    assert!(get_session_moa(key).is_none());
}

#[test]
fn explicit_clear_drops_stashed_sticky_too() {
    let key = "test:moa:f2:clear";
    set_session_moa(key, Some("deep".to_string()), false);
    set_session_moa_one_shot(key, None);
    clear_session_moa(key); // moa off wins — whole slot, stash included
    assert!(get_session_moa(key).is_none());
    assert!(take_for_run(key).is_none());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore session_moa_handle::tests::one_shot_over_sticky_reinstates_sticky_on_consume --lib`
Expected: FAIL — `set_session_moa_one_shot` not found / `restore` field missing.

- [ ] **Step 3: Add the `restore` field**

In `src/providers/session_moa_handle.rs`, replace the `SessionMoaPref` struct:

```rust
/// A session's MoA activation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionMoaPref {
    /// Preset name; `None` = the config `default_preset`.
    pub preset: Option<String>,
    /// `true` = applies to exactly one run, consumed by [`take_for_run`].
    pub one_shot: bool,
    /// A sticky pref displaced by a one-shot arm, reinstated when the one-shot
    /// is consumed by [`take_for_run`]. `None` for sticky prefs and for
    /// one-shots armed over an empty or one-shot slot. Boxed to keep the
    /// (self-referential) struct small.
    pub restore: Option<Box<SessionMoaPref>>,
}
```

- [ ] **Step 4: Set `restore: None` in `set_session_moa`**

Replace the body of `set_session_moa`:

```rust
/// Record (or overwrite) the session's MoA activation. Clears any stash
/// (a fresh explicit activation is not a one-shot displacement).
pub fn set_session_moa(session_key: &str, preset: Option<String>, one_shot: bool) {
    map().write().unwrap_or_else(|e| e.into_inner()).insert(
        session_key.to_string(),
        SessionMoaPref { preset, one_shot, restore: None },
    );
}
```

- [ ] **Step 5: Add `set_session_moa_one_shot`**

Insert after `set_session_moa`:

```rust
/// Arm a one-shot pref. If an existing STICKY pref occupies the slot, stash
/// it into `restore` so [`take_for_run`] reinstates it after consuming this
/// one-shot. Arming over an empty or one-shot slot stashes nothing.
pub fn set_session_moa_one_shot(session_key: &str, preset: Option<String>) {
    let mut guard = map().write().unwrap_or_else(|e| e.into_inner());
    let restore = guard
        .get(session_key)
        .filter(|p| !p.one_shot)
        .cloned()
        .map(Box::new);
    guard.insert(
        session_key.to_string(),
        SessionMoaPref { preset, one_shot: true, restore },
    );
}
```

- [ ] **Step 6: Reinstate stash in `take_for_run`**

Replace `take_for_run`:

```rust
/// Read for run construction. A `one_shot` pref is REMOVED — or, when it
/// carries a stashed sticky, REPLACED by that sticky — in the same write-lock
/// section it is read in (the single restore point).
#[must_use]
pub fn take_for_run(session_key: &str) -> Option<SessionMoaPref> {
    let mut guard = map().write().unwrap_or_else(|e| e.into_inner());
    let pref = guard.get(session_key).cloned()?;
    if pref.one_shot {
        match &pref.restore {
            Some(sticky) => {
                guard.insert(session_key.to_string(), (**sticky).clone());
            }
            None => {
                guard.remove(session_key);
            }
        }
    }
    Some(pref)
}
```

- [ ] **Step 7: Run the new tests + existing ones in this file**

Run: `cargo test -p alephcore session_moa_handle:: --lib`
Expected: PASS — the 4 new tests plus the 5 existing (`sticky_survives_take_for_run`, `one_shot_consumed_atomically`, `status_read_does_not_consume`, `restore_one_shot_refills_empty_slot_only`, `restore_after_sticky_take_is_a_noop`) all green. (Existing tests construct `SessionMoaPref` only via `set_session_moa`, so the new field needs no edits there.)

- [ ] **Step 8: Commit**

```bash
git add src/providers/session_moa_handle.rs
git commit -m "providers: SessionMoaPref.restore stash so /moa one-shot reinstates displaced sticky (F2)"
```

---

### Task 2: F1 core — `activation.rs` arm/disarm helpers

**Files:**
- Create: `src/providers/moa/activation.rs`
- Modify: `src/providers/moa/mod.rs`
- Test: `src/providers/moa/activation.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `session_moa_handle::{set_session_moa, set_session_moa_one_shot, clear_session_moa}` (Task 1), `session_model_handle::clear_session_model`, `super::get_moa_config`.
- Produces:
  - `pub fn arm_sticky(session_key: &str, preset: Option<String>) -> Result<String, String>`
  - `pub fn arm_one_shot(session_key: &str, preset: Option<String>) -> Result<String, String>`
  - `pub fn disarm(session_key: &str)`
  - All reachable as `crate::providers::moa::activation::*`.

- [ ] **Step 1: Write the failing tests**

Create `src/providers/moa/activation.rs` with ONLY the test module first (implementation added in Step 3). The tests reuse the process-global config test lock + `store_moa_config` pattern from `select_model.rs`:

```rust
//! Single source of MoA session-activation ("arm") logic, shared by every
//! entry point that arms/disarms MoA (the `moa` tool, the `select_model`
//! `moa:` pick, the `chat.send` `provider:"moa"` override, and the `/moa`
//! one-shot intercept). Resolves + validates the preset once, then mutates
//! the session handles with the canonical set-then-clear ordering.

use crate::providers::{session_model_handle, session_moa_handle};

/// Resolve a preset name against the live `[moa]` config, or an error string
/// naming the missing preset. Shared by the arm helpers below.
fn resolve_or_err(preset: Option<&str>) -> Result<String, String> {
    super::get_moa_config()
        .as_ref()
        .and_then(|cfg| cfg.resolve_preset(preset))
        .map(|(name, _)| name)
        .ok_or_else(|| {
            format!(
                "MoA preset '{}' not found — use the moa tool (action='list') to see presets.",
                preset.unwrap_or("<default>")
            )
        })
}

/// Arm sticky MoA: resolve+validate, write the sticky pref, and clear any
/// per-session model pick (selector-slot exclusivity). Returns the resolved
/// preset name (for user-facing messages) or an error string.
pub fn arm_sticky(session_key: &str, preset: Option<String>) -> Result<String, String> {
    let name = resolve_or_err(preset.as_deref())?;
    session_moa_handle::set_session_moa(session_key, preset, false);
    session_model_handle::clear_session_model(session_key);
    Ok(name)
}

/// Arm one-shot MoA (F2 stash semantics live in `set_session_moa_one_shot`).
/// Same resolution/exclusivity as [`arm_sticky`].
pub fn arm_one_shot(session_key: &str, preset: Option<String>) -> Result<String, String> {
    let name = resolve_or_err(preset.as_deref())?;
    session_moa_handle::set_session_moa_one_shot(session_key, preset);
    session_model_handle::clear_session_model(session_key);
    Ok(name)
}

/// Clear the session's MoA sticky. Idempotent. Called by the "normal model
/// pick" path (selector-slot exclusivity) before it sets the model handle.
pub fn disarm(session_key: &str) {
    session_moa_handle::clear_session_moa(session_key);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::moa::config_handle::moa_config_test_lock;

    fn deep_preset_config() -> crate::config::MoaToml {
        let mut cfg = crate::config::MoaToml::default();
        cfg.presets.insert(
            "deep".to_string(),
            crate::config::MoaPreset {
                enabled: true,
                advisors: vec![crate::config::MoaSlot {
                    provider: "openai".to_string(),
                    model: "gpt-5".to_string(),
                }],
                aggregator: crate::config::MoaSlot {
                    provider: "anthropic".to_string(),
                    model: "claude-opus-4-8".to_string(),
                },
                fanout: crate::config::MoaFanout::default(),
                advisor_timeout_secs: 120,
                advisor_max_tokens: None,
                advisor_temperature: None,
                aggregator_temperature: None,
            },
        );
        cfg
    }

    #[test]
    fn arm_sticky_ok_writes_sticky_and_clears_model() {
        let _guard = moa_config_test_lock();
        let key = "test:moa:activation:sticky-ok";
        crate::providers::moa::store_moa_config(Some(deep_preset_config()));
        session_model_handle::set_session_model(key, None, "gpt-5".to_string());

        let name = arm_sticky(key, Some("deep".to_string())).unwrap();
        assert_eq!(name, "deep");
        let pref = session_moa_handle::get_session_moa(key).unwrap();
        assert_eq!(pref.preset.as_deref(), Some("deep"));
        assert!(!pref.one_shot);
        assert!(session_model_handle::get_session_model(key).is_none());

        session_moa_handle::clear_session_moa(key);
        crate::providers::moa::store_moa_config(None);
    }

    #[test]
    fn arm_sticky_unknown_preset_errs_and_writes_nothing() {
        let _guard = moa_config_test_lock();
        let key = "test:moa:activation:sticky-ghost";
        crate::providers::moa::store_moa_config(Some(deep_preset_config()));

        let err = arm_sticky(key, Some("ghost".to_string())).unwrap_err();
        assert!(err.contains("'ghost' not found"), "got: {err}");
        assert!(session_moa_handle::get_session_moa(key).is_none());

        crate::providers::moa::store_moa_config(None);
    }

    #[test]
    fn arm_one_shot_ok_writes_one_shot() {
        let _guard = moa_config_test_lock();
        let key = "test:moa:activation:oneshot-ok";
        crate::providers::moa::store_moa_config(Some(deep_preset_config()));

        let name = arm_one_shot(key, Some("deep".to_string())).unwrap();
        assert_eq!(name, "deep");
        let pref = session_moa_handle::get_session_moa(key).unwrap();
        assert!(pref.one_shot);

        session_moa_handle::clear_session_moa(key);
        crate::providers::moa::store_moa_config(None);
    }

    #[test]
    fn disarm_clears_sticky() {
        let key = "test:moa:activation:disarm";
        session_moa_handle::set_session_moa(key, Some("deep".to_string()), false);
        disarm(key);
        assert!(session_moa_handle::get_session_moa(key).is_none());
    }
}
```

- [ ] **Step 2: Wire the module + run to verify failure**

In `src/providers/moa/mod.rs`, add the module declaration alongside the existing submodule declarations (all consumers are in-crate — `moa_manage.rs`, `select_model.rs`, `agent.rs`, `execute.rs`, `slash_command.rs` — so `pub(crate)` matches the `advisory_view`/`fan_out` precedent):

```rust
pub(crate) mod activation;
```

Run: `cargo test -p alephcore moa::activation::tests::arm_sticky_ok_writes_sticky_and_clears_model --lib`
Expected: FAIL to compile first if `store_moa_config` / `get_moa_config` / `config_handle::moa_config_test_lock` paths differ — if so, confirm the exact re-export names used in `src/builtin_tools/select_model.rs` tests (they use `crate::providers::moa::store_moa_config`, `crate::providers::moa::get_moa_config`, and `crate::providers::moa::config_handle::moa_config_test_lock`) and match them. Once compiling, tests should PASS (implementation is already in the file from Step 1).

> Note: this task writes implementation and tests together because the helpers are trivial pass-throughs whose only real logic (preset resolution) is exercised by the two `arm_sticky` tests. The `store_moa_config(None)` teardown keeps the process-global slot clean for other tests sharing `moa_config_test_lock()`.

- [ ] **Step 3: Run the full activation test module**

Run: `cargo test -p alephcore moa::activation::tests --lib`
Expected: PASS — all 4 tests.

- [ ] **Step 4: Commit**

```bash
git add src/providers/moa/activation.rs src/providers/moa/mod.rs
git commit -m "providers: moa/activation.rs single arm/disarm source with preset validation (F1 core)"
```

---

### Task 3: F1 sites 1+2 — `moa_manage::activate` + `select_model` use the helpers

**Files:**
- Modify: `src/builtin_tools/moa_manage.rs` (`activate`, ~lines 232-265)
- Modify: `src/builtin_tools/select_model.rs` (~lines 89-132)
- Test: existing tests in both files (behavior-equivalent; must stay green)

**Interfaces:**
- Consumes: `crate::providers::moa::activation::{arm_sticky, arm_one_shot, disarm}` (Task 2).
- Produces: no new public surface.

- [ ] **Step 1: Rewrite `moa_manage::activate`**

Replace the body of `activate` (`src/builtin_tools/moa_manage.rs:232`) from the `let moa_cfg = get_moa_config();` block through the `set_session_moa`/`clear_session_model` calls with the helper. New body:

```rust
async fn activate(&self, preset: Option<String>, one_shot: bool) -> Result<MoaManageOutput> {
    let Some(ctx) = current_turn_context() else {
        return Ok(no_turn_context_output());
    };
    let key = ctx.session_key.to_key_string();

    let armed = if one_shot {
        crate::providers::moa::activation::arm_one_shot(&key, preset)
    } else {
        crate::providers::moa::activation::arm_sticky(&key, preset)
    };

    match armed {
        Ok(name) => {
            let message = if one_shot {
                format!("MoA '{name}' active for this session for the next turn only")
            } else {
                format!("MoA '{name}' active for this session from the NEXT turn")
            };
            Ok(MoaManageOutput {
                success: true,
                message,
                data: Some(serde_json::json!({ "preset": name, "one_shot": one_shot })),
            })
        }
        Err(_msg) => Ok(MoaManageOutput {
            success: false,
            message: NO_PRESET_GUIDANCE.to_string(),
            data: None,
        }),
    }
}
```

> Preserves the existing `NO_PRESET_GUIDANCE` message on failure (do not swap in the raw `arm_*` error string — the tool's own guidance copy is richer). Remove the now-unused local `get_moa_config`/`resolve_preset`/`set_session_moa`/`clear_session_model` calls in this function; leave their `use`/imports if still used elsewhere in the file (check `activate`'s siblings `status`/`list`/`set_preset` before deleting any import).

- [ ] **Step 2: Rewrite `select_model.rs` moa branch + normal-pick clear**

In `src/builtin_tools/select_model.rs`, replace the `moa` arm block (the `if args.model == "moa" || args.model.starts_with("moa:") { ... }`) and the normal-pick clear:

```rust
        // Round-2 E3 / Round-3 F1: the selector is ONE slot — "moa:<preset>"
        // (or bare "moa") arms MoA sticky and clears any model pick; a normal
        // model pick clears MoA. Arm logic is the single source in
        // moa::activation.
        if args.model == "moa" || args.model.starts_with("moa:") {
            let preset = args
                .model
                .strip_prefix("moa:")
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            match crate::providers::moa::activation::arm_sticky(&key, preset) {
                Ok(name) => {
                    let message = format!(
                        "MoA preset '{name}' activated for this session (sticky); model pick \
                         cleared. Takes effect from the next turn."
                    );
                    notify_tool_result(Self::NAME, &message, true);
                    return Ok(SelectModelOutput {
                        ok: true,
                        model: args.model,
                        provider: None,
                        message,
                    });
                }
                Err(message) => {
                    notify_tool_result(Self::NAME, &message, false);
                    return Ok(SelectModelOutput {
                        ok: false,
                        model: args.model,
                        provider: None,
                        message,
                    });
                }
            }
        }

        // Normal pick: selector-slot exclusivity clears any MoA sticky.
        crate::providers::moa::activation::disarm(&key);
        session_model_handle::set_session_model(&key, args.provider.clone(), args.model.clone());
```

> The old code resolved the preset inline and built the same not-found message; `arm_sticky`'s `Err` string is the canonical copy (identical text), so the not-found branch is behavior-equivalent. Remove the now-unused inline `get_moa_config`/`resolve_preset`/`set_session_moa`/`clear_session_model`/`clear_session_moa` references in this arm; keep `session_model_handle` import (still used by the normal pick).

- [ ] **Step 3: Run both files' existing tests**

Run: `cargo test -p alephcore select_model:: --lib` then `cargo test -p alephcore moa_manage:: --lib`
Expected: PASS — in particular `select_model`'s `moa_prefix_activates_preset_and_clears_model_pick` and `moa_manage`'s `on_with_resolvable_preset_writes_sticky_session_handle` / `once_writes_one_shot_session_handle` / `on_with_no_presets_configured_gives_guidance` all green (they already store a config via the shared test lock, so `arm_sticky`/`arm_one_shot` resolve).

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/moa_manage.rs src/builtin_tools/select_model.rs
git commit -m "builtin_tools: route moa_manage/select_model arm through moa::activation (F1 sites 1-2)"
```

---

### Task 4: F1 site 3 — `apply_moa_selector_semantics` validation closure

**Files:**
- Modify: `src/gateway/handlers/agent.rs` (`apply_moa_selector_semantics`, ~lines 526-547)
- Test: `src/gateway/handlers/agent.rs` (`moa_override_arms_session_and_is_consumed` at ~1212; add a new ghost test)

**Interfaces:**
- Consumes: `crate::providers::moa::activation::{arm_sticky, disarm}`, `crate::builtin_tools::notify_tool_result`.
- Produces: `apply_moa_selector_semantics` — same signature, now validates the preset and notifies on failure instead of arming an unresolvable preset.

- [ ] **Step 1: Write the failing test (ghost preset must NOT arm)**

Add a new test to the `#[cfg(test)] mod tests` in `src/gateway/handlers/agent.rs`:

```rust
/// Round-3 F1: a `provider:"moa"` override naming an UNKNOWN preset must not
/// silently arm — it validates, notifies, and leaves the session unarmed.
#[test]
fn moa_override_unknown_preset_does_not_arm() {
    use crate::gateway::model_override::ModelOverride;
    use crate::providers::moa::config_handle::moa_config_test_lock;
    let _guard = moa_config_test_lock();
    let key = "test:moa:selector:ghost";
    crate::providers::moa::store_moa_config(Some({
        let mut cfg = crate::config::MoaToml::default();
        cfg.presets.insert(
            "deep".to_string(),
            crate::config::MoaPreset {
                enabled: true,
                advisors: vec![crate::config::MoaSlot {
                    provider: "openai".to_string(),
                    model: "gpt-5".to_string(),
                }],
                aggregator: crate::config::MoaSlot {
                    provider: "anthropic".to_string(),
                    model: "claude-opus-4-8".to_string(),
                },
                fanout: crate::config::MoaFanout::default(),
                advisor_timeout_secs: 120,
                advisor_max_tokens: None,
                advisor_temperature: None,
                aggregator_temperature: None,
            },
        );
        cfg
    }));

    let out = apply_moa_selector_semantics(
        key,
        Some(ModelOverride::Qualified {
            provider: "moa".into(),
            model: "ghost".into(),
        }),
    );
    assert!(out.is_none(), "override is still consumed (not passed through)");
    assert!(
        crate::providers::session_moa_handle::get_session_moa(key).is_none(),
        "unknown preset must not arm the session"
    );

    crate::providers::moa::store_moa_config(None);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore gateway::handlers::agent::tests::moa_override_unknown_preset_does_not_arm --lib`
Expected: FAIL — current code arms `"ghost"` unconditionally, so `get_session_moa(key)` is `Some`.

- [ ] **Step 3: Rewrite `apply_moa_selector_semantics`**

Replace the `match model_override { ... }` body (`src/gateway/handlers/agent.rs:531-546`):

```rust
    use crate::gateway::model_override::ModelOverride;
    match model_override {
        Some(ModelOverride::Qualified { provider, model }) if provider == "moa" => {
            // Round-3 F1: validate via the single arm source. An unknown preset
            // is NOT silently armed — notify the user and leave the session
            // unarmed (the override is still consumed, not passed through, so a
            // "moa" provider never rides the run path).
            if let Err(msg) =
                crate::providers::moa::activation::arm_sticky(session_key, Some(model))
            {
                crate::builtin_tools::notify_tool_result("moa", &msg, false);
            }
            None
        }
        Some(other) => {
            crate::providers::moa::activation::disarm(session_key);
            Some(other)
        }
        None => None,
    }
```

- [ ] **Step 4: Update the existing arm test to store a config**

The existing `moa_override_arms_session_and_is_consumed` (~line 1212) arms `"deep"`, which now requires a resolvable config. At the very top of that test body (before `set_session_model`), insert:

```rust
    use crate::providers::moa::config_handle::moa_config_test_lock;
    let _guard = moa_config_test_lock();
    crate::providers::moa::store_moa_config(Some({
        let mut cfg = crate::config::MoaToml::default();
        cfg.presets.insert(
            "deep".to_string(),
            crate::config::MoaPreset {
                enabled: true,
                advisors: vec![crate::config::MoaSlot {
                    provider: "openai".to_string(),
                    model: "gpt-5".to_string(),
                }],
                aggregator: crate::config::MoaSlot {
                    provider: "anthropic".to_string(),
                    model: "claude-opus-4-8".to_string(),
                },
                fanout: crate::config::MoaFanout::default(),
                advisor_timeout_secs: 120,
                advisor_max_tokens: None,
                advisor_temperature: None,
                aggregator_temperature: None,
            },
        );
        cfg
    }));
```

And at the end of that test (after the final `clear_session_moa(key)`), add:

```rust
    crate::providers::moa::store_moa_config(None);
```

> Both this test and the new ghost test mutate the process-global `[moa]` config, so both must hold `moa_config_test_lock()` — that shared lock serializes them (and the `select_model` / `activation` config tests) against each other.

- [ ] **Step 5: Run both agent.rs moa tests**

Run: `cargo test -p alephcore gateway::handlers::agent::tests::moa_override --lib`
Expected: PASS — both `moa_override_arms_session_and_is_consumed` and `moa_override_unknown_preset_does_not_arm`.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/agent.rs
git commit -m "gateway: validate moa selector override via moa::activation, notify on unknown preset (F1 site 3, closes silent no-op)"
```

---

### Task 5: F3 + F1 site 4 — `/moa` one-shot uses `arm_one_shot` + operator gate

**Files:**
- Modify: `src/tools/turn_context.rs` (add `role_is_operator`, refactor `caller_is_operator`)
- Modify: `src/gateway/execution_engine/execute.rs` (~lines 253-268, the Panel/CLI `/moa` intercept)
- Modify: `src/gateway/execution_engine/slash_command.rs` (~lines 147-164, the channel `direct_tool` "moa" intercept)
- Test: `src/tools/turn_context.rs` (`role_is_operator` table)

**Interfaces:**
- Consumes: `crate::providers::moa::activation::arm_one_shot`, `crate::builtin_tools::notify_tool_result`.
- Produces: `pub fn role_is_operator(role: Option<&str>) -> bool` in `src/tools/turn_context.rs`.

- [ ] **Step 1: Write the failing test for `role_is_operator`**

Add to the `#[cfg(test)] mod tests` in `src/tools/turn_context.rs`:

```rust
#[test]
fn role_is_operator_table() {
    assert!(role_is_operator(None), "absent role = local/operator");
    assert!(role_is_operator(Some("operator")));
    assert!(!role_is_operator(Some("guest")));
    assert!(!role_is_operator(Some("chat")));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore tools::turn_context::tests::role_is_operator_table --lib`
Expected: FAIL — `role_is_operator` not defined.

- [ ] **Step 3: Add `role_is_operator` and delegate `caller_is_operator` to it**

In `src/tools/turn_context.rs`, add a free function (place it above the `impl` or near `caller_is_operator`):

```rust
/// Canonical operator-role predicate for a raw `caller_role` string. `None`
/// (absent = loopback/local) and `"operator"` are operator; everything else
/// (chat-tier channels default to `"guest"`) is not.
#[must_use]
pub fn role_is_operator(role: Option<&str>) -> bool {
    matches!(role, None | Some("operator"))
}
```

Then refactor the existing method to delegate (single source):

```rust
    pub fn caller_is_operator(&self) -> bool {
        role_is_operator(self.caller_role.as_deref())
    }
```

- [ ] **Step 4: Run the predicate tests**

Run: `cargo test -p alephcore tools::turn_context::tests --lib`
Expected: PASS — `role_is_operator_table` plus the existing `caller_is_operator` tests (`ctx(Some("operator"))`, `ctx(None)`, `ctx(Some("guest"))`) stay green.

- [ ] **Step 5: Gate + rewrite the Panel/CLI `/moa` intercept (`execute.rs`)**

In `src/gateway/execution_engine/execute.rs`, replace the arm block inside the `if !prompt.trim_start().starts_with('/') { ... }` guard (~lines 260-266):

```rust
                if !prompt.trim_start().starts_with('/') {
                    let key = request.session_key.to_key_string();
                    // Round-3 F3: mirror the operator gate on the `moa` tool
                    // (method_authz). A non-operator (chat-tier channel) may
                    // not arm advisory via `/moa`; the prompt still runs as a
                    // normal turn (prefix stripped below).
                    if crate::tools::turn_context::role_is_operator(
                        request.metadata.get("caller_role").map(String::as_str),
                    ) {
                        // Round-3 F1: single arm source (one-shot). `None`
                        // preset = the config default; clears the model slot.
                        if let Err(msg) =
                            crate::providers::moa::activation::arm_one_shot(&key, None)
                        {
                            crate::builtin_tools::notify_tool_result("moa", &msg, false);
                        }
                    } else {
                        crate::builtin_tools::notify_tool_result(
                            "moa",
                            "MoA advisory requires operator; running normally.",
                            false,
                        );
                    }
                }
                request.input = prompt.to_string();
```

> `arm_one_shot` already clears the session model handle, so the separate `clear_session_model` call in the old block is dropped. `request.input` is still rewritten to the stripped prompt in every branch (operator, non-operator, and Err).

- [ ] **Step 6: Gate + rewrite the channel `/moa` intercept (`slash_command.rs`)**

In `src/gateway/execution_engine/slash_command.rs`, replace the arm block inside the `if !args.is_empty() && !args.trim_start().starts_with('/') { ... }` guard (~lines 154-160):

```rust
                    if !args.is_empty() && !args.trim_start().starts_with('/') {
                        let key = request.session_key.to_key_string();
                        // Round-3 F3: operator gate (mirror the `moa` tool).
                        if crate::tools::turn_context::role_is_operator(
                            request.metadata.get("caller_role").map(String::as_str),
                        ) {
                            // Round-3 F1: single arm source (one-shot).
                            if let Err(msg) =
                                crate::providers::moa::activation::arm_one_shot(&key, None)
                            {
                                crate::builtin_tools::notify_tool_result("moa", &msg, false);
                            }
                        } else {
                            crate::builtin_tools::notify_tool_result(
                                "moa",
                                "MoA advisory requires operator; running normally.",
                                false,
                            );
                        }
                    }
```

> The raw `/moa ...` text still reaches the agent loop on `Fallthrough` and is stripped by `moa_fallthrough_input` (unchanged) — so the non-operator path degrades to a normal turn exactly like a nested-slash `/moa`.

- [ ] **Step 7: Verify the two touched files compile via their existing pure-fn tests**

Run: `cargo test -p alephcore moa_fallthrough_input --lib`
Expected: PASS — the existing `moa_fallthrough_input_semantics` test compiles the edited `execute.rs`. (The arm-gating itself is integration-level and is verified at runtime QA, consistent with how round-2 verified these same intercept sites; the unit-testable logic is `role_is_operator`, covered in Step 4.)

- [ ] **Step 8: Commit**

```bash
git add src/tools/turn_context.rs src/gateway/execution_engine/execute.rs src/gateway/execution_engine/slash_command.rs
git commit -m "gateway: gate /moa one-shot behind operator role + route through moa::activation (F3 + F1 site 4)"
```

---

### Task 6: F4 comment + docs + final compile check

**Files:**
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs` (~line 402-404, the B8 attribution site)
- Modify: `docs/reference/FEATURE_LOCATOR.md` (§4.9 status line)

**Interfaces:** none.

- [ ] **Step 1: Add the F4 attribution comment**

In `src/orchestrator/harness_bridge/runner_impl.rs`, at the B8 attribution block (the `let (vesr_model_id, vesr_provider_id) = match &moa_aggregator_identity { ... }`), extend the existing `// Round-2 B8:` comment with an F4 line clarifying the deliberate choice:

```rust
        // Round-2 B8: when MoA is active the run's acting model is the
        // preset's aggregator — record THAT into routing experience, not the
        // pre-MoA directive/pin (which never served a token this run).
        // Round-3 F4: attributing a MoA-assisted success to the SOLO aggregator
        // model is deliberate — the aggregator is this run's actual executor;
        // the advisor-guidance uplift is not modeled separately in routing
        // experience (known, accepted attribution choice — metering is exact).
```

- [ ] **Step 2: Add the round-3 note to FEATURE_LOCATOR §4.9**

In `docs/reference/FEATURE_LOCATOR.md`, append to the §4.9 **状态** line (after the round-2 clause) a round-3 sentence:

```
；✅ 第三轮修复与连线（2026-07-06，spec `docs/superpowers/specs/2026-07-06-moa-round3-fixes-wiring-design.md`）——arm 逻辑收拢单一真源 `src/providers/moa/activation.rs`（4 处 arm 站点统一，闭合 `chat.send`/`/moa` 两路径的「选了不存在的 preset 静默无效」缺口，改为 `notify_tool_result` 报错）、`/moa` one-shot 结束后 reinstate 被覆盖的 sticky（`SessionMoaPref.restore` 压栈）、`/moa` slash 路径加 operator 门（`role_is_operator` 对齐 `method_authz`）、VESR 归因给聚合器为有意为之（注释锁定）。
```

Also update the §4.9 **代码锚点** line to mention `src/providers/moa/activation.rs`（`arm_sticky`/`arm_one_shot`/`disarm` 单一 arm 真源）and `src/tools/turn_context.rs::role_is_operator`（canonical operator 谓词）where the arm/role machinery is described.

- [ ] **Step 3: Final compile check (the single full check for the plan)**

Run: `cargo check -p alephcore --lib`
Expected: clean (no errors, no new warnings).

- [ ] **Step 4: Commit**

```bash
git add src/orchestrator/harness_bridge/runner_impl.rs docs/reference/FEATURE_LOCATOR.md
git commit -m "docs: F4 VESR attribution comment + FEATURE_LOCATOR round-3 note"
```

---

## Self-Review

**Spec coverage:**
- F1 (single arm source + close silent no-op): Task 2 (core), Task 3 (sites 1-2), Task 4 (site 3 = the closure), Task 5 (site 4). ✓
- F2 (one-shot restores sticky): Task 1. ✓
- F3 (operator gate on `/moa`): Task 5. ✓
- F4 (attribution comment): Task 6. ✓
- Docs (FEATURE_LOCATOR): Task 6. ✓

**Placeholder scan:** No TBD/TODO; every code step has concrete code; the one integration-level gap (arm-gating end-to-end) is explicitly called out with its rationale, not hidden behind "add a test". ✓

**Type consistency:** `arm_sticky`/`arm_one_shot` return `Result<String, String>`; `disarm` returns `()`; `set_session_moa_one_shot(&str, Option<String>)`; `role_is_operator(Option<&str>) -> bool`; `SessionMoaPref.restore: Option<Box<SessionMoaPref>>`. Names used consistently across Tasks 1-6. `notify_tool_result(&str, &str, bool)` matches `src/builtin_tools/mod.rs:316`. ✓

**Ordering:** Task 1 (handle primitives) → Task 2 (activation helpers consume them) → Tasks 3-5 (sites consume activation) → Task 6 (docs + final check). Each task independently testable and committable. ✓
