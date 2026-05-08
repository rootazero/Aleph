# Phase-6 配置驱动装配 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `AgentHarnessRunner` 的 5 个 Phase-6 占位字段（`guardrails` / `fallback_llm` / `stall_config` / `consecutive_failure_cap` / `turn_timeout`）从硬编码 `None` 替换为 `aleph.toml` 配置加载的真实值，让 Stage 5a/5b + P0 rescue 在 production 真实生效。

**Architecture:** 三个新顶层 toml section（`[guardrails]` / `[stability]` / `[fallback_provider]`），三个 private builder 函数都装在 `orchestrator_init.rs`（不引入 trait/struct）。每个 builder 接受 `&Config`，缺 section → None；`build_fallback_llm` 通过 by-name 引用现有 `[providers.<key>]` + 自指检测。`src/harness/` 不动，`agent.rs` 行数不变。

**Tech Stack:** Rust 1.x stable + serde + toml + schemars (现有 Aleph deps)；no new crate.

**Reference:**
- Design doc: `docs/superpowers/specs/2026-05-08-phase6-config-wiring-design.md` (commit `ecd547a6c`)
- Stage 7 audit: `docs/superpowers/specs/2026-05-08-harness-stage7-audit-report.md`
- Master spec: `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` § Stage 7

---

## File Structure

| 文件 | 操作 | 责任 |
|------|------|------|
| `src/config/types/phase6_wiring.rs` | **新建** | 三个 schema struct (`GuardrailsToml`, `StabilityToml`, `FallbackProviderToml`) + 单元测试 |
| `src/config/types/mod.rs` | **修改** | re-export `phase6_wiring::*` |
| `src/config/structs.rs` | **修改** | `Config` 顶层加三个 `Option<XxxToml>` 字段 + Default impl |
| `src/bin/aleph-server/commands/start/orchestrator_init.rs` | **修改** | 加 `&Config` + `&str` (primary_provider_key) 参数；加三个 `build_*` 私有函数；line 130-134 的 `None` 改为真值；模块底部加 `#[cfg(test)] mod tests` |
| `src/bin/aleph-server/commands/start/mod.rs` | **修改** | line 1126 附近：`read().await.clone()` 取 `Config` 快照；调用点透传 `&cfg_snapshot` 和 `default_provider_key` |
| `CHANGELOG.md` | **修改** (P6-6) | `[Unreleased]` 加 Phase-6 entry |
| `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` | **修改** (P6-6) | Stage 7/Phase-6 状态翻 🟢 Shipped |
| `docs/superpowers/specs/2026-05-08-harness-stage7-init-audit-plan.md` | **修改** (P6-6) | 加 "Phase-6 closed" 收尾段 |

---

## Task 0 (P6-1): Plan doc commit

**Status:** This plan document IS the P6-1 commit content. After writing-plans skill completes, the user will commit it as:

```bash
git add docs/superpowers/specs/2026-05-08-phase6-config-wiring-plan.md
git commit -m "harness: Phase-6 P6-1 plan doc — config-driven wiring task list"
```

No code changes. Skip to Task 1 (P6-2) for implementation.

---

## Task 1 (P6-2): Schema — three new toml sections

**Files:**
- Create: `src/config/types/phase6_wiring.rs`
- Modify: `src/config/types/mod.rs` (add `pub mod phase6_wiring;` + `pub use phase6_wiring::*;`)
- Modify: `src/config/structs.rs` (add 3 fields to `Config` + 3 fields to `Default for Config`)
- Test: same file `phase6_wiring.rs` `#[cfg(test)] mod tests`

### Step 1.1: Create `phase6_wiring.rs` with three schema struct + roundtrip tests

- [ ] **Write the file:**

```rust
// src/config/types/phase6_wiring.rs
//! Phase-6 wiring schema — three top-level toml sections that flip
//! Stage 5a/5b + P0 rescue from None placeholders to live values.
//!
//! Missing section → corresponding `Config` field stays `None` →
//! AgentHarnessRunner field stays `None` → behavior identical to
//! Stage 7 ship (commit c2cd8d293) main HEAD.
//!
//! Wired into `AgentHarnessRunner` by `orchestrator_init::build_*` helpers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[guardrails]` — single switch wiring `PiiSecretsGuardrail::from_globals()`
/// onto Input + Output + ToolCall trait surfaces. Phase-6 has only one real
/// `GuardrailImpl`; future detectors (e.g. content_safety) extend this struct
/// additively without breaking existing toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct GuardrailsToml {
    #[serde(default)]
    pub enabled: bool,
}

/// `[stability]` — P0 rescue knobs (stall watchdog + failure cap + per-turn
/// timeout). Each field is `Option<u64>` so callers can opt into a subset;
/// missing fields stay None. `stall_timeout_secs` is the trigger that builds
/// `StallConfig`; `stall_check_interval_secs` falls back to
/// `StallConfig::default().check_interval` (30s) when the timeout is set.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct StabilityToml {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_check_interval_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consecutive_failure_cap: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_timeout_secs: Option<u64>,
}

/// `[fallback_provider]` — Stage 5b single-step fallback. References an
/// existing `[providers.<provider>]` entry by toml key; `ProviderConfig`
/// is *not* inlined here. Self-reference (provider == primary toml key)
/// is detected at build time and yields `None` with a warn log.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct FallbackProviderToml {
    pub provider: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_toml_yields_none_for_three_sections() {
        // Phase-6 acceptance #2: missing section → None for the matching
        // AgentHarnessRunner field. We assert at the schema level here:
        // an empty toml string deserializes the three Option<XxxToml>
        // fields on Config to None.
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default)]
            guardrails: Option<GuardrailsToml>,
            #[serde(default)]
            stability: Option<StabilityToml>,
            #[serde(default)]
            fallback_provider: Option<FallbackProviderToml>,
        }
        let p: Probe = toml::from_str("").expect("empty toml parses");
        assert!(p.guardrails.is_none());
        assert!(p.stability.is_none());
        assert!(p.fallback_provider.is_none());
    }

    #[test]
    fn full_toml_yields_three_some() {
        let toml_str = r#"
[guardrails]
enabled = true

[stability]
stall_timeout_secs = 300
stall_check_interval_secs = 30
consecutive_failure_cap = 8
turn_timeout_secs = 300

[fallback_provider]
provider = "openai-mini"
"#;
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default)]
            guardrails: Option<GuardrailsToml>,
            #[serde(default)]
            stability: Option<StabilityToml>,
            #[serde(default)]
            fallback_provider: Option<FallbackProviderToml>,
        }
        let p: Probe = toml::from_str(toml_str).expect("toml parses");
        assert_eq!(p.guardrails, Some(GuardrailsToml { enabled: true }));
        assert_eq!(
            p.stability,
            Some(StabilityToml {
                stall_timeout_secs: Some(300),
                stall_check_interval_secs: Some(30),
                consecutive_failure_cap: Some(8),
                turn_timeout_secs: Some(300),
            })
        );
        assert_eq!(
            p.fallback_provider,
            Some(FallbackProviderToml {
                provider: "openai-mini".to_string()
            })
        );
    }
}
```

### Step 1.2: Wire `phase6_wiring` into `types/mod.rs`

- [ ] **Find existing exports** (read `src/config/types/mod.rs:40-80`):

```bash
grep -n "pub mod\|pub use" src/config/types/mod.rs | head -20
```

- [ ] **Add (placement: alphabetical, near `stop_hooks`):**

In `src/config/types/mod.rs`, locate the line `pub mod stop_hooks;` and add **after** it:

```rust
pub mod phase6_wiring;
```

Then locate the line `pub use stop_hooks::*;` (around line 74) and add **after** it:

```rust
pub use phase6_wiring::*;
```

### Step 1.3: Add three fields to `Config` (`src/config/structs.rs`)

- [ ] **Find the existing `stop_hooks` declaration** (line ~185):

```bash
grep -n "pub stop_hooks\|stop_hooks: Vec::new()" src/config/structs.rs
```

Expected: line 185 (declaration) + line 376 (Default).

- [ ] **Add three fields next to `stop_hooks`** (line ~185, immediately after the closing `;` of `pub stop_hooks: Vec<StopHookConfig>,`):

```rust
    /// Phase-6 wiring (#12) — single-switch guardrails section. When `Some`
    /// and `enabled = true`, the orchestrator wires `PiiSecretsGuardrail`
    /// onto Input + Output + ToolCall surfaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guardrails: Option<GuardrailsToml>,
    /// Phase-6 wiring (#12) — P0 rescue knobs (stall / consecutive failure
    /// cap / per-turn timeout). Each sub-field is independently optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stability: Option<StabilityToml>,
    /// Phase-6 wiring (#12) — Stage 5b single-step fallback provider. Refers
    /// to an existing `[providers.<key>]` entry by toml key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_provider: Option<FallbackProviderToml>,
```

- [ ] **Add three fields to `Default for Config`** (line ~376, immediately after `stop_hooks: Vec::new(),`):

```rust
            guardrails: None,
            stability: None,
            fallback_provider: None,
```

### Step 1.4: Run schema tests — verify compile + roundtrip

- [ ] **Run:**

```bash
cargo test -p alephcore --lib config::types::phase6_wiring -- --nocapture
```

Expected:
```
running 2 tests
test config::types::phase6_wiring::tests::empty_toml_yields_none_for_three_sections ... ok
test config::types::phase6_wiring::tests::full_toml_yields_three_some ... ok
```

- [ ] **Run full lib test as compile check:**

```bash
cargo test -p alephcore --lib --no-run
```

Expected: builds clean (no errors). Existing tests unaffected.

### Step 1.5: R10 self-check + commit

- [ ] **Verify harness untouched:**

```bash
wc -l src/harness/agent.rs
ls src/harness/
```

Expected: agent.rs == 1520 lines; 9 .rs files in `src/harness/`.

- [ ] **Commit:**

```bash
git add src/config/types/phase6_wiring.rs src/config/types/mod.rs src/config/structs.rs
git commit -m "harness: Phase-6 P6-2 schema — guardrails/stability/fallback_provider toml

Three top-level Option<XxxToml> sections on Config, each Default::default()
to None so the wiring path stays a no-op until P6-3/4/5 read them. Two
roundtrip tests in phase6_wiring::tests lock the empty-toml-and-full-toml
serde contract.

No runtime behavior change yet — orchestrator_init.rs still hardcodes None
on the five AgentHarnessRunner Phase-6 fields."
```

---

## Task 2 (P6-3): Wire `[guardrails]` → `AgentHarnessRunner.guardrails`

**Files:**
- Modify: `src/bin/aleph-server/commands/start/orchestrator_init.rs` (add `&Config` parameter, `build_guardrail_registry`, change line 130 `guardrails: None` → `guardrails`, add `#[cfg(test)] mod tests`)
- Modify: `src/bin/aleph-server/commands/start/mod.rs` (line ~1126: clone Config snapshot from `app_config`, pass `&cfg_snapshot` to `initialize_orchestrator`)

### Step 2.1: Plumb `&Config` through `initialize_orchestrator` signature + caller

- [ ] **Find the function signature** (`orchestrator_init.rs:39-52`):

```bash
grep -n "pub(in crate::commands::start) async fn initialize_orchestrator" src/bin/aleph-server/commands/start/orchestrator_init.rs
```

- [ ] **Add imports near top of file** (after the existing `use alephcore::...` block):

```rust
use alephcore::config::types::{
    FallbackProviderToml, GuardrailsToml, StabilityToml,
};
use alephcore::config::Config;
```

- [ ] **Modify signature** — add `config: &Config` and `primary_provider_key: &str` as the **first two parameters** (these are the most stable/zero-cost args):

Find:
```rust
pub(in crate::commands::start) async fn initialize_orchestrator(
    agent_registry: Arc<alephcore::agents::AgentRegistry>,
```

Replace with:
```rust
pub(in crate::commands::start) async fn initialize_orchestrator(
    config: &Config,
    primary_provider_key: &str,
    agent_registry: Arc<alephcore::agents::AgentRegistry>,
```

- [ ] **Modify caller at `src/bin/aleph-server/commands/start/mod.rs` line ~1126**:

Find:
```rust
        let stop_hook_configs = app_config.read().await.stop_hooks.clone();
        match initialize_orchestrator(
            orchestrator_agent_registry,
            session_service,
```

Replace with:
```rust
        let cfg_snapshot = app_config.read().await.clone();
        let stop_hook_configs = cfg_snapshot.stop_hooks.clone();
        let primary_provider_key = cfg_snapshot
            .general
            .default_provider
            .clone()
            .unwrap_or_default();
        match initialize_orchestrator(
            &cfg_snapshot,
            &primary_provider_key,
            orchestrator_agent_registry,
            session_service,
```

- [ ] **Compile check** (no test code yet — just signature plumbing):

```bash
cargo check -p alephcore
cargo check -p aleph-server
```

Expected: clean. `config` and `primary_provider_key` are unused in the body — that's expected for now (Step 2.2 fixes it).

### Step 2.2: Write the failing test for `build_guardrail_registry`

- [ ] **Add at the bottom of `orchestrator_init.rs`** (no `mod tests` exists yet):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use alephcore::config::Config;
    use alephcore::config::types::GuardrailsToml;

    fn cfg_with_guardrails(g: Option<GuardrailsToml>) -> Config {
        Config {
            guardrails: g,
            ..Config::default()
        }
    }

    #[test]
    fn guardrails_missing_section_returns_none() {
        let cfg = Config::default();
        let r = build_guardrail_registry(&cfg);
        assert!(r.is_none(), "missing [guardrails] should yield None");
    }

    #[test]
    fn guardrails_disabled_returns_none() {
        let cfg = cfg_with_guardrails(Some(GuardrailsToml { enabled: false }));
        let r = build_guardrail_registry(&cfg);
        assert!(r.is_none(), "[guardrails] enabled=false should yield None");
    }

    #[test]
    fn guardrails_enabled_wires_pii_secrets() {
        let cfg = cfg_with_guardrails(Some(GuardrailsToml { enabled: true }));
        let r = build_guardrail_registry(&cfg).expect("enabled=true should yield Some");
        assert_eq!(r.input_count(), 1);
        assert_eq!(r.output_count(), 1);
        assert_eq!(r.tool_call_count(), 1);
    }
}
```

- [ ] **Run — expect FAIL on `build_guardrail_registry` not defined:**

```bash
cargo test -p aleph-server --lib commands::start::orchestrator_init::tests -- --nocapture 2>&1 | head -30
```

Expected: `error[E0425]: cannot find function 'build_guardrail_registry' in this scope`.

### Step 2.3: Implement `build_guardrail_registry`

- [ ] **Add at the bottom of `orchestrator_init.rs`** (above the `#[cfg(test)]` block):

```rust
// =============================================================================
// Phase-6 wiring helpers (Stage 7 init audit closure)
// =============================================================================

/// Build the optional `GuardrailRegistry` from `[guardrails]`. Phase-6 wiring
/// for Stage 5a/5b. Missing section, or `enabled = false`, returns `None`.
/// When `enabled = true`, wires the single existing `PiiSecretsGuardrail`
/// onto Input + Output + ToolCall surfaces (one struct, three traits).
fn build_guardrail_registry(
    config: &alephcore::config::Config,
) -> Option<Arc<alephcore::guardrails::GuardrailRegistry>> {
    let g = config.guardrails.as_ref()?;
    if !g.enabled {
        return None;
    }
    let pii = Arc::new(alephcore::guardrails::PiiSecretsGuardrail::from_globals());
    Some(Arc::new(
        alephcore::guardrails::GuardrailRegistry::builder()
            .with_input(pii.clone())
            .with_output(pii.clone())
            .with_tool_call(pii)
            .build(),
    ))
}
```

> **Note on imports:** `alephcore::guardrails::PiiSecretsGuardrail` and `GuardrailRegistry` are already re-exported at `alephcore::guardrails`. If `cargo check` complains about visibility, fall back to fully-qualified paths in the function body (already shown above).

- [ ] **Wire it into `AgentHarnessRunner`** — locate `let harness = Arc::new(AgentHarnessRunner {` and replace `guardrails: None,` with:

```rust
        guardrails: build_guardrail_registry(config),
```

- [ ] **Run tests — expect PASS:**

```bash
cargo test -p aleph-server --lib commands::start::orchestrator_init::tests -- --nocapture
```

Expected:
```
running 3 tests
test ... guardrails_missing_section_returns_none ... ok
test ... guardrails_disabled_returns_none ... ok
test ... guardrails_enabled_wires_pii_secrets ... ok
```

### Step 2.4: Stage 7 init_audit tests + lib regression

- [ ] **Run init_audit tests (must not regress):**

```bash
cargo test -p alephcore --lib orchestrator::tests::init_audit
```

Expected: 3 tests pass (`cold_start_emits_all_seam_events`, `init_seams_emitted_in_declared_order_with_correct_configured_flags`, `default_trait_impl_is_noop`).

- [ ] **Run alephcore lib full:**

```bash
cargo test -p alephcore --lib
```

Expected: green (or at most the documented baseline failures noted in `project_baseline_test_failures.md` — those are pre-existing).

### Step 2.5: R10 self-check + commit

- [ ] **R10 verify:**

```bash
wc -l src/harness/agent.rs
ls src/harness/ | wc -l
```

Expected: `1520 src/harness/agent.rs` and `9` files (count includes `tests/` dir).

- [ ] **Commit:**

```bash
git add src/bin/aleph-server/commands/start/orchestrator_init.rs \
        src/bin/aleph-server/commands/start/mod.rs
git commit -m "harness: Phase-6 P6-3 wire [guardrails] → AgentHarnessRunner.guardrails

build_guardrail_registry(&Config) -> Option<Arc<GuardrailRegistry>> reads
[guardrails] enabled=true and wires PiiSecretsGuardrail::from_globals() onto
Input + Output + ToolCall surfaces (one struct, three traits). Missing
section or enabled=false yields None; behavior identical to Stage 7 HEAD.

Adds &Config + primary_provider_key plumbing through initialize_orchestrator
signature; caller in start/mod.rs clones a Config snapshot under app_config
read lock once at boot.

R10: src/harness/agent.rs unchanged (1520 lines)."
```

---

## Task 3 (P6-4): Wire `[fallback_provider]` → `AgentHarnessRunner.fallback_llm`

**Files:**
- Modify: `src/bin/aleph-server/commands/start/orchestrator_init.rs` (add `build_fallback_llm`, change line 131 `fallback_llm: None`, add 5 tests)

### Step 3.1: Write the failing tests for `build_fallback_llm`

- [ ] **Append to the existing `mod tests` in `orchestrator_init.rs`:**

```rust
    use alephcore::config::types::FallbackProviderToml;
    use alephcore::config::types::ProviderConfig;
    use std::collections::HashMap;

    fn mock_provider_config() -> ProviderConfig {
        let mut pc = ProviderConfig::test_config("mock-model");
        pc.protocol = Some("mock".to_string());
        pc
    }

    fn cfg_with_fallback(
        fb: Option<FallbackProviderToml>,
        providers: Vec<(&str, ProviderConfig)>,
    ) -> Config {
        let mut providers_map: HashMap<String, ProviderConfig> = HashMap::new();
        for (k, v) in providers {
            providers_map.insert(k.to_string(), v);
        }
        Config {
            fallback_provider: fb,
            providers: providers_map,
            ..Config::default()
        }
    }

    #[test]
    fn fallback_missing_section_returns_none() {
        let cfg = Config::default();
        assert!(build_fallback_llm(&cfg, "anthropic").is_none());
    }

    #[test]
    fn fallback_self_reference_returns_none() {
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                provider: "anthropic".to_string(),
            }),
            vec![("anthropic", mock_provider_config())],
        );
        assert!(
            build_fallback_llm(&cfg, "anthropic").is_none(),
            "self-reference must yield None"
        );
    }

    #[test]
    fn fallback_unknown_name_returns_none() {
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                provider: "ghost".to_string(),
            }),
            vec![],
        );
        assert!(build_fallback_llm(&cfg, "anthropic").is_none());
    }

    #[test]
    fn fallback_valid_name_returns_some() {
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                provider: "mock".to_string(),
            }),
            vec![("mock", mock_provider_config())],
        );
        let r = build_fallback_llm(&cfg, "anthropic");
        assert!(r.is_some(), "valid by-name reference must yield Some");
    }

    #[test]
    fn fallback_create_provider_failure_returns_none() {
        // Construct a ProviderConfig that create_provider rejects:
        // protocol = Some("__bogus_protocol__") falls through every match arm
        // and ends in "Unknown protocol" Err.
        let mut bad = ProviderConfig::test_config("bad");
        bad.protocol = Some("__bogus_protocol__".to_string());
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                provider: "bad".to_string(),
            }),
            vec![("bad", bad)],
        );
        assert!(build_fallback_llm(&cfg, "anthropic").is_none());
    }
```

- [ ] **Run — expect FAIL on `build_fallback_llm` not defined:**

```bash
cargo test -p aleph-server --lib commands::start::orchestrator_init::tests::fallback -- --nocapture 2>&1 | head -30
```

Expected: `error[E0425]: cannot find function 'build_fallback_llm' in this scope`.

### Step 3.2: Implement `build_fallback_llm`

- [ ] **Add to the Phase-6 helpers block in `orchestrator_init.rs`** (immediately after `build_guardrail_registry`):

```rust
/// Build the optional Stage 5b single-step fallback provider from
/// `[fallback_provider]`. Phase-6 wiring. Behaviors:
/// - Missing section, or `provider == primary_provider_key` (self-reference),
///   yields `None`.
/// - `provider` not present in `[providers]` map yields `None` + warn log.
/// - `create_provider` failure yields `None` + warn log (e.g. unknown protocol).
fn build_fallback_llm(
    config: &alephcore::config::Config,
    primary_provider_key: &str,
) -> Option<Arc<dyn alephcore::providers::AiProvider>> {
    let fb = config.fallback_provider.as_ref()?;
    if fb.provider == primary_provider_key {
        tracing::warn!(
            provider = %fb.provider,
            "fallback_provider self-reference; disabling"
        );
        return None;
    }
    let pc = match config.providers.get(&fb.provider) {
        Some(c) => c.clone(),
        None => {
            tracing::warn!(
                provider = %fb.provider,
                "fallback_provider not found in [providers]; disabling"
            );
            return None;
        }
    };
    match alephcore::providers::create_provider(&fb.provider, pc) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(
                provider = %fb.provider,
                error = %e,
                "fallback_provider create_provider failed; disabling"
            );
            None
        }
    }
}
```

- [ ] **Wire it into `AgentHarnessRunner`** — replace `fallback_llm: None,` with:

```rust
        fallback_llm: build_fallback_llm(config, primary_provider_key),
```

- [ ] **Run tests — expect PASS:**

```bash
cargo test -p aleph-server --lib commands::start::orchestrator_init::tests::fallback -- --nocapture
```

Expected: 5 fallback tests pass.

### Step 3.3: lib regression + R10 + commit

- [ ] **Run alephcore lib full:**

```bash
cargo test -p alephcore --lib
cargo test -p aleph-server --lib commands::start::orchestrator_init::tests -- --nocapture
```

Expected: 8 orchestrator_init tests pass (3 guardrails + 5 fallback). Lib green.

- [ ] **R10:**

```bash
wc -l src/harness/agent.rs
```

Expected: 1520.

- [ ] **Commit:**

```bash
git add src/bin/aleph-server/commands/start/orchestrator_init.rs
git commit -m "harness: Phase-6 P6-4 wire [fallback_provider] → fallback_llm

build_fallback_llm(&Config, primary_key) -> Option<Arc<dyn AiProvider>>
reads [fallback_provider] provider = \"<key>\" and constructs the secondary
via create_provider(name, providers[name].clone()). Missing section,
self-reference (provider == primary_key), unknown name, or create_provider
error all warn-and-disable to None.

Activates Stage 5b single-step retry-on-Transient seam (commit 27f303c64)
for the first time in production. FailoverProvider remains a separate
N-tier path applied to default_provider — unaffected.

R10: src/harness/agent.rs unchanged (1520 lines)."
```

---

## Task 4 (P6-5): Wire `[stability]` → `stall_config` + `consecutive_failure_cap` + `turn_timeout`

**Files:**
- Modify: `src/bin/aleph-server/commands/start/orchestrator_init.rs` (add `build_stability_triple`, change lines 132-134, add 4 tests)

### Step 4.1: Write the failing tests for `build_stability_triple`

- [ ] **Append to the existing `mod tests`:**

```rust
    use alephcore::config::types::StabilityToml;
    use alephcore::harness::deps::StallConfig;
    use std::time::Duration;

    fn cfg_with_stability(s: Option<StabilityToml>) -> Config {
        Config {
            stability: s,
            ..Config::default()
        }
    }

    #[test]
    fn stability_missing_section_all_none() {
        let cfg = Config::default();
        let (sc, cap, tt) = build_stability_triple(&cfg);
        assert!(sc.is_none());
        assert!(cap.is_none());
        assert!(tt.is_none());
    }

    #[test]
    fn stability_partial_only_turn_timeout() {
        let cfg = cfg_with_stability(Some(StabilityToml {
            turn_timeout_secs: Some(60),
            ..StabilityToml::default()
        }));
        let (sc, cap, tt) = build_stability_triple(&cfg);
        assert!(sc.is_none(), "no stall_timeout_secs → no StallConfig");
        assert!(cap.is_none());
        assert_eq!(tt, Some(Duration::from_secs(60)));
    }

    #[test]
    fn stability_stall_uses_default_check_interval() {
        let cfg = cfg_with_stability(Some(StabilityToml {
            stall_timeout_secs: Some(120),
            ..StabilityToml::default()
        }));
        let (sc, cap, tt) = build_stability_triple(&cfg);
        let sc = sc.expect("stall_timeout_secs=120 → Some(StallConfig)");
        assert_eq!(sc.timeout, Duration::from_secs(120));
        // missing stall_check_interval_secs → falls back to default (30s)
        assert_eq!(sc.check_interval, StallConfig::default().check_interval);
        assert!(cap.is_none());
        assert!(tt.is_none());
    }

    #[test]
    fn stability_full_section_all_some() {
        let cfg = cfg_with_stability(Some(StabilityToml {
            stall_timeout_secs: Some(300),
            stall_check_interval_secs: Some(15),
            consecutive_failure_cap: Some(8),
            turn_timeout_secs: Some(180),
        }));
        let (sc, cap, tt) = build_stability_triple(&cfg);
        let sc = sc.expect("full section → Some(StallConfig)");
        assert_eq!(sc.timeout, Duration::from_secs(300));
        assert_eq!(sc.check_interval, Duration::from_secs(15));
        assert_eq!(cap, Some(8));
        assert_eq!(tt, Some(Duration::from_secs(180)));
    }
```

- [ ] **Run — expect FAIL on `build_stability_triple` not defined:**

```bash
cargo test -p aleph-server --lib commands::start::orchestrator_init::tests::stability -- --nocapture 2>&1 | head -30
```

Expected: `cannot find function 'build_stability_triple'`.

### Step 4.2: Implement `build_stability_triple`

- [ ] **Add to the Phase-6 helpers block** (after `build_fallback_llm`):

```rust
/// Build the P0 rescue triple from `[stability]`. Phase-6 wiring. Each of the
/// three returned `Option`s is independent:
/// - `StallConfig` only constructed when `stall_timeout_secs` is `Some`;
///   `stall_check_interval_secs` falls back to `StallConfig::default()` (30s).
/// - `consecutive_failure_cap` is a bare `Option<usize>` (no derived state).
/// - `turn_timeout` wraps `turn_timeout_secs` in `Duration::from_secs`.
fn build_stability_triple(
    config: &alephcore::config::Config,
) -> (
    Option<alephcore::harness::deps::StallConfig>,
    Option<usize>,
    Option<std::time::Duration>,
) {
    let Some(s) = config.stability.as_ref() else {
        return (None, None, None);
    };
    let stall_config = s.stall_timeout_secs.map(|secs| {
        let mut sc = alephcore::harness::deps::StallConfig::default();
        sc.timeout = std::time::Duration::from_secs(secs);
        if let Some(ci) = s.stall_check_interval_secs {
            sc.check_interval = std::time::Duration::from_secs(ci);
        }
        sc
    });
    (
        stall_config,
        s.consecutive_failure_cap,
        s.turn_timeout_secs.map(std::time::Duration::from_secs),
    )
}
```

- [ ] **Wire it into `AgentHarnessRunner`** — find the three `None` placeholders for stall/cap/timeout and replace.

Find:
```rust
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
```

Replace with:
```rust
        stall_config: stall_cfg,
        consecutive_failure_cap: failure_cap,
        turn_timeout: turn_to,
```

- [ ] **Add the destructuring just above the `let harness = Arc::new(AgentHarnessRunner {` line:**

```rust
    let (stall_cfg, failure_cap, turn_to) = build_stability_triple(config);
```

- [ ] **Run tests — expect PASS:**

```bash
cargo test -p aleph-server --lib commands::start::orchestrator_init::tests::stability -- --nocapture
```

Expected: 4 stability tests pass.

### Step 4.3: Full lib regression + R10 + commit

- [ ] **Run all builder tests at once:**

```bash
cargo test -p aleph-server --lib commands::start::orchestrator_init::tests -- --nocapture
cargo test -p alephcore --lib
```

Expected: 12 tests pass (3 guardrails + 5 fallback + 4 stability). Lib green. init_audit tests still pass.

- [ ] **R10 final check:**

```bash
wc -l src/harness/agent.rs
ls src/harness/ | wc -l
```

Expected: 1520 lines; 9 entries (8 .rs files + tests/ dir).

- [ ] **Commit:**

```bash
git add src/bin/aleph-server/commands/start/orchestrator_init.rs
git commit -m "harness: Phase-6 P6-5 wire [stability] → P0 rescue triple

build_stability_triple(&Config) -> (Option<StallConfig>, Option<usize>,
Option<Duration>) reads [stability]'s four fields and synthesizes the three
HarnessDeps targets:
- stall_config: Some only when stall_timeout_secs set; check_interval
  defaults to StallConfig::default().check_interval (30s) when omitted.
- consecutive_failure_cap: bare Option<usize>.
- turn_timeout: Option<Duration> from turn_timeout_secs.

Activates the P0 rescue trio (stall watchdog + failure cap + per-turn
timeout) for the first time in production. Closes the last three Stage 7
init audit gaps.

R10: src/harness/agent.rs unchanged (1520 lines)."
```

---

## Task 5 (P6-6): Ship — CHANGELOG + spec status flips

**Files:**
- Modify: `CHANGELOG.md` (add Phase-6 entry to `[Unreleased]`)
- Modify: `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` (Stage 7 / Phase-6 status flip 🟡 → 🟢)
- Modify: `docs/superpowers/specs/2026-05-08-harness-stage7-init-audit-plan.md` (append "Phase-6 closed" section)

### Step 5.1: CHANGELOG entry

- [ ] **Read current `[Unreleased]` section** (top of `CHANGELOG.md`):

```bash
head -40 CHANGELOG.md
```

- [ ] **Add under `### Added` (or create the section if missing):**

```markdown
- **Phase-6 config-driven wiring (Stage 7 closure):** `[guardrails] enabled = true`,
  `[stability] {stall_timeout_secs, stall_check_interval_secs, consecutive_failure_cap,
  turn_timeout_secs}`, `[fallback_provider] provider = "<key>"`. Three private builders in
  `orchestrator_init.rs` turn the five `AgentHarnessRunner` Phase-6 placeholders into
  live values; missing section ≡ None ≡ pre-Phase-6 main HEAD behavior. Activates
  Stage 5a guardrails, Stage 5b single-step fallback, and the P0 rescue trio (stall
  watchdog / consecutive failure cap / per-turn timeout) for the first time in
  production. R10: `src/harness/agent.rs` unchanged at 1520 lines.
```

### Step 5.2: Master spec status flip

- [ ] **Find Stage 7 status row** in `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md`:

```bash
grep -n "Stage 7\|Phase-6\|🟡\|🟢" docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md | head -20
```

- [ ] **Flip status indicator** for Stage 7 / Phase-6 from 🟡 (pending Phase-6) → 🟢 (Shipped). Exact line content varies — match the existing pattern. Add a one-line note: `Phase-6 wiring closed at <commit-shortsha>; all five AgentHarnessRunner fields opt-in via aleph.toml.`

### Step 5.3: Stage 7 plan closure section

- [ ] **Append to bottom of `docs/superpowers/specs/2026-05-08-harness-stage7-init-audit-plan.md`:**

```markdown

---

## Phase-6 Closed (2026-05-08)

Stage 7 left five `AgentHarnessRunner` fields hardcoded to `None` with a
PHASE-6 marker. Phase-6 closed those gaps in 5 commits (P6-2 schema → P6-3
guardrails → P6-4 fallback → P6-5 stability → P6-6 docs). The five fields
are now `Option<T>` driven by three top-level `aleph.toml` sections; missing
section preserves Stage 7 ship behavior exactly. See
`2026-05-08-phase6-config-wiring-design.md` for design and
`2026-05-08-phase6-config-wiring-plan.md` for task list.
```

### Step 5.4: Final tests + commit

- [ ] **Confirm green baseline one more time** (sanity before docs commit):

```bash
cargo test -p alephcore --lib
cargo test -p aleph-server --lib commands::start::orchestrator_init::tests
```

Expected: green.

- [ ] **Commit:**

```bash
git add CHANGELOG.md \
        docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md \
        docs/superpowers/specs/2026-05-08-harness-stage7-init-audit-plan.md
git commit -m "harness: Phase-6 P6-6 ship — CHANGELOG + master spec 🟢 + Stage 7 plan closure

Phase-6 wiring complete. AgentHarnessRunner's five Phase-6 placeholder fields
(guardrails / fallback_llm / stall_config / consecutive_failure_cap /
turn_timeout) now opt-in via three aleph.toml sections ([guardrails] /
[stability] / [fallback_provider]). Stage 7 init audit closed; 12-module
roadmap §Phase-6 marked 🟢 Shipped."
```

---

## Acceptance Verification (after Task 5)

Run end-to-end smoke as the final verification:

- [ ] **All builder tests:**
```bash
cargo test -p aleph-server --lib commands::start::orchestrator_init::tests
```
Expected: 12 tests pass.

- [ ] **Stage 7 init audit non-regression:**
```bash
cargo test -p alephcore --lib orchestrator::tests::init_audit
```
Expected: 3 tests pass.

- [ ] **Schema roundtrip:**
```bash
cargo test -p alephcore --lib config::types::phase6_wiring
```
Expected: 2 tests pass.

- [ ] **Lib full green:**
```bash
cargo test -p alephcore --lib
```
Expected: green (modulo pre-existing baseline failures in `project_baseline_test_failures.md`).

- [ ] **R10 cap + harness file count:**
```bash
wc -l src/harness/agent.rs
ls src/harness/
```
Expected: `1520 src/harness/agent.rs`; 9 entries.

- [ ] **Boot-time smoke (manual, optional):** With a fully-populated `aleph.toml` (three sections), run `cargo run --bin aleph-server start` and grep the log for the existing `harness deps assembled guardrails=true fallback_llm=true stall_config=true consecutive_failure_cap=true turn_timeout=true` line at session start.

---

## Risk Register (recap from design § 13)

| Risk | Mitigation |
|------|-----------|
| `Config::clone()` cost in caller (`mod.rs:~1126`) | One-time at boot; HashMap clones acceptable |
| `PiiEngine::global()` returns `None` in test env | `PiiSecretsGuardrail` already handles this internally |
| `create_provider("mock", ...)` requires `protocol = Some("mock")` | Test fixture `mock_provider_config()` sets it explicitly |
| `__bogus_protocol__` test relies on `create_provider` returning `Err` | Verified at design-doc-time via `src/providers/mod.rs:193-199` "Unknown protocol" branch |
| R10 cap regression | `wc -l src/harness/agent.rs` step at end of every commit |

---

## Plan Self-Review

**Spec coverage:** Each design § maps to a task:
- design § 5 (Schema) → Task 1 (P6-2)
- design § 6.1 (build_guardrail_registry) → Task 2 (P6-3)
- design § 6.2 (build_fallback_llm) → Task 3 (P6-4)
- design § 6.3 (build_stability_triple) → Task 4 (P6-5)
- design § 7 (Wiring point) → spread across Tasks 2/3/4 (signature in Task 2, slot fills in 2/3/4)
- design § 8 (Test matrix #1-#12) → 12 tests across Tasks 2/3/4
- design § 9 (Commit split) → 5 implementation commits (P6-2 ... P6-6)
- design § 10 (Acceptance ① ... ⑥) → final Acceptance Verification block
- design § 11 (Red-line self-check) → R10 step at end of every commit
- design § 12 (O1/O2/O3) → resolved before plan: O1 = `app_config.general.default_provider.clone().unwrap_or_default()`; O2 = `&Config` snapshot; O3 = `MockProvider` via `protocol = Some("mock")`

**Placeholder scan:** ✅ No TBD/TODO. Every step has runnable code or shell command. The CHANGELOG entry text is fully written; the master-spec status-flip step uses match-and-replace pattern (the only step where exact line content varies — kept intentional because the spec evolves between brainstorm and ship).

**Type consistency:** ✅
- `GuardrailsToml` / `StabilityToml` / `FallbackProviderToml` named identically across schema (Task 1), test fixtures (Tasks 2/3/4), and design § 5.2.
- `build_guardrail_registry(&Config)` signature stable across Task 2 impl and design § 6.1.
- `build_fallback_llm(&Config, primary_provider_key: &str)` stable across Task 3 impl and design § 6.2 (post self-review fix `primary_key` → `primary_provider_key`).
- `build_stability_triple(&Config) -> (Option<StallConfig>, Option<usize>, Option<Duration>)` triple-tuple stable across Task 4 impl, test asserts, and design § 6.3.
- Test method names (`input_count`, `output_count`, `tool_call_count`) match `GuardrailRegistry` impl (`src/guardrails/registry.rs:52-60`).

Plan is self-consistent and ready to execute.
