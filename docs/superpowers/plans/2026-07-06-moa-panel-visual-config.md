# MoA Panel 可视化配置 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 MoA 加一个 Panel 顶级设置页，用选择器（从已配置 provider 模型中挑，全局不可重复）可视化创建/编辑/删除 preset，写回 `config.toml` 并热更新。

**Architecture:** Panel（Leptos，纯 I/O）经新 `moa.*` JSON-RPC 调后端；后端把 preset 写逻辑抽成共享核心 `MoaPresetStore`，`moa` 对话工具与 RPC handler 都调它（熵减单源）；模型选项复用 `providers.catalog`。零代码进 `src/harness/`。

**Tech Stack:** Rust (tokio, serde, schemars) · JSON-RPC gateway · Leptos/WASM Panel · `ConfigPatcher` 热更新。

## Global Constraints

- MSRV 1.95；`cargo fmt` + `cargo clippy -D warnings` 干净。
- 提交信息英文 `<scope>: <description>`，无 attribution。
- 极度节制 cargo 调用：默认不跑全量测试，最多一次 `cargo test -p alephcore --lib`（本计划用 `cargo test` 定向单测；WASM 侧不编入 `--lib`，见 feedback-cargo-check-skips-test-code）。
- 所有代码改动在**新建 worktree 分支**中进行，不直接触碰 main。
- `PatchRequest` 字段是 `path`（不是 section）；热更新用 `store_moa_config(...)`。
- MoA 槽位类型：`MoaSlot { provider: String, model: String }`；`MoaPreset { enabled, advisors: Vec<MoaSlot>, aggregator: MoaSlot, fanout: MoaFanout, advisor_timeout_secs: u64, advisor_max_tokens: Option<u32>, advisor_temperature: Option<f32>, aggregator_temperature: Option<f32> }`；`MoaFanout::{PerIteration, UserTurn}`（serde snake_case）；`MoaToml { default_preset: Option<String>, save_traces: bool, presets: HashMap<String, MoaPreset> }`。
- Config 字段：`Config.moa: MoaToml`。

---

### Task 1: `MoaToml` 全局去重校验

**Files:**
- Modify: `src/config/types/moa.rs`（`validation_errors`，约 98–131 行区块 + 尾部 `#[cfg(test)]`）

**Interfaces:**
- Consumes: 无（纯类型层）。
- Produces: `MoaToml::validation_errors(&self) -> Vec<String>` 新增去重错误行，形如 `[moa.presets.{name}] duplicate slot (provider, model) — advisors and aggregator must all be distinct`。规范化比较键 = `(provider.trim().to_lowercase(), model.trim().to_lowercase())`。

- [ ] **Step 1: Write the failing tests**

在 `src/config/types/moa.rs` 的 `#[cfg(test)] mod tests` 末尾追加：

```rust
    fn slot(p: &str, m: &str) -> MoaSlot {
        MoaSlot { provider: p.into(), model: m.into() }
    }

    fn preset_with(advisors: Vec<MoaSlot>, aggregator: MoaSlot) -> MoaToml {
        MoaToml {
            presets: HashMap::from([(
                "p".to_string(),
                MoaPreset {
                    enabled: true,
                    advisors,
                    aggregator,
                    fanout: MoaFanout::default(),
                    advisor_timeout_secs: 120,
                    advisor_max_tokens: None,
                    advisor_temperature: None,
                    aggregator_temperature: None,
                },
            )]),
            ..MoaToml::default()
        }
    }

    #[test]
    fn duplicate_advisor_slots_rejected() {
        let cfg = preset_with(
            vec![slot("openai", "gpt-5.5"), slot("openai", "gpt-5.5")],
            slot("anthropic", "claude-opus-4-8"),
        );
        assert!(cfg.validation_errors().iter().any(|e| e.contains("duplicate slot")));
    }

    #[test]
    fn aggregator_equal_to_advisor_rejected() {
        let cfg = preset_with(
            vec![slot("openai", "gpt-5.5")],
            slot("openai", "gpt-5.5"),
        );
        assert!(cfg.validation_errors().iter().any(|e| e.contains("duplicate slot")));
    }

    #[test]
    fn dedup_is_case_and_whitespace_insensitive() {
        let cfg = preset_with(
            vec![slot("OpenAI", " gpt-5.5 ")],
            slot("openai", "gpt-5.5"),
        );
        assert!(cfg.validation_errors().iter().any(|e| e.contains("duplicate slot")));
    }

    #[test]
    fn all_distinct_slots_pass_dedup() {
        let cfg = preset_with(
            vec![slot("openai", "gpt-5.5"), slot("deepseek", "deepseek-v4")],
            slot("anthropic", "claude-opus-4-8"),
        );
        assert!(!cfg.validation_errors().iter().any(|e| e.contains("duplicate slot")));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib config::types::moa::tests::duplicate_advisor_slots_rejected config::types::moa::tests::aggregator_equal_to_advisor_rejected config::types::moa::tests::dedup_is_case_and_whitespace_insensitive`
Expected: FAIL（当前无去重逻辑，重复未被拒）。

- [ ] **Step 3: Add the dedup rule**

在 `validation_errors` 里，每个 preset 的槽位循环之后（`if preset.enabled && preset.advisors.is_empty()` 检查前后皆可）插入：

```rust
            // Global distinctness: every slot (all advisors + aggregator) must be
            // a unique (provider, model) after case/whitespace normalization.
            let mut seen = std::collections::HashSet::new();
            let mut all_slots: Vec<&MoaSlot> = preset.advisors.iter().collect();
            all_slots.push(&preset.aggregator);
            for slot in all_slots {
                let key = (
                    slot.provider.trim().to_lowercase(),
                    slot.model.trim().to_lowercase(),
                );
                if !seen.insert(key) {
                    errs.push(format!(
                        "[moa.presets.{name}] duplicate slot (provider, model) — \
                         advisors and aggregator must all be distinct"
                    ));
                    break;
                }
            }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib config::types::moa::tests`
Expected: PASS（含既有测试）。

- [ ] **Step 5: Commit**

```bash
git add src/config/types/moa.rs
git commit -m "config: reject duplicate (provider, model) slots within a MoA preset"
```

---

### Task 2: `MoaPresetStore` 共享写核心

**Files:**
- Create: `src/providers/moa/preset_store.rs`
- Modify: `src/providers/moa/mod.rs`（加 `pub mod preset_store;` 并 re-export）

**Interfaces:**
- Consumes: `MoaToml::validation_errors`（Task 1）；`ConfigPatcher::apply(PatchRequest) -> Result<PatchResult>`；`store_moa_config` / `get_moa_config`（`crate::providers::moa::config_handle`）。
- Produces:
  - `pub struct MoaPresetStore` with `pub fn new(config: Arc<RwLock<Config>>, patcher: Arc<ConfigPatcher>) -> Self`。
  - `pub async fn save_preset(&self, name: &str, preset: MoaPreset, make_default: bool) -> Result<PatchResult, MoaStoreError>`
  - `pub async fn delete_preset(&self, name: &str) -> Result<PatchResult, MoaStoreError>`
  - `pub async fn set_default(&self, name: &str) -> Result<PatchResult, MoaStoreError>`
  - `pub async fn set_save_traces(&self, on: bool) -> Result<PatchResult, MoaStoreError>`
  - `pub async fn list(&self) -> MoaToml`
  - `pub enum MoaStoreError { Validation(Vec<String>), Absent(String), OnlyPreset(String), Patch(String) }`，impl `Display`。

- [ ] **Step 1: Write the failing test**

Create `src/providers/moa/preset_store.rs`（先只放 imports + 测试，让它编译失败于缺失符号）：

```rust
//! Shared write core for `[moa]` presets — the single source of truth behind
//! both the `moa` tool and the `moa.*` gateway RPCs. Extracted from
//! moa_manage.rs so config-write logic lives in exactly one place.

use crate::config::patcher::{ConfigPatcher, PatchRequest, PatchResult};
use crate::config::{Config, MoaPreset, MoaToml};
use crate::providers::moa::config_handle::{get_moa_config, store_moa_config};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

pub struct MoaPresetStore {
    config: Arc<RwLock<Config>>,
    patcher: Arc<ConfigPatcher>,
}

#[derive(Debug)]
pub enum MoaStoreError {
    Validation(Vec<String>),
    Absent(String),
    OnlyPreset(String),
    Patch(String),
}

impl std::fmt::Display for MoaStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(errs) => write!(f, "{}", errs.join("; ")),
            Self::Absent(n) => write!(f, "Preset '{n}' does not exist"),
            Self::OnlyPreset(n) => write!(
                f,
                "Cannot delete '{n}': it is the only MoA preset. Create another first."
            ),
            Self::Patch(e) => write!(f, "Config patch failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MoaFanout, MoaSlot};
    use crate::providers::moa::config_handle::moa_config_test_lock;

    fn temp_store() -> (MoaPresetStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "").unwrap();
        let config = Arc::new(RwLock::new(Config::default()));
        let backup = crate::config::backup::ConfigBackup::new(dir.path().to_path_buf());
        let patcher = Arc::new(ConfigPatcher::new(
            Arc::clone(&config),
            config_path,
            backup,
        ));
        (MoaPresetStore::new(config, patcher), dir)
    }

    fn preset(advisor: &str, agg: &str) -> MoaPreset {
        MoaPreset {
            enabled: true,
            advisors: vec![MoaSlot { provider: "openai".into(), model: advisor.into() }],
            aggregator: MoaSlot { provider: "anthropic".into(), model: agg.into() },
            fanout: MoaFanout::default(),
            advisor_timeout_secs: 120,
            advisor_max_tokens: None,
            advisor_temperature: None,
            aggregator_temperature: None,
        }
    }

    #[tokio::test]
    async fn save_then_list_roundtrips() {
        let _g = moa_config_test_lock().lock().await;
        let (store, _dir) = temp_store();
        store.save_preset("default", preset("gpt-5.5", "claude-opus-4-8"), true)
            .await
            .expect("save ok");
        let listed = store.list().await;
        assert!(listed.presets.contains_key("default"));
        assert_eq!(listed.default_preset.as_deref(), Some("default"));
    }

    #[tokio::test]
    async fn save_rejects_invalid_preset() {
        let _g = moa_config_test_lock().lock().await;
        let (store, _dir) = temp_store();
        // aggregator == advisor -> dedup validation error
        let bad = MoaPreset {
            aggregator: MoaSlot { provider: "openai".into(), model: "gpt-5.5".into() },
            ..preset("gpt-5.5", "x")
        };
        let err = store.save_preset("p", bad, false).await.unwrap_err();
        assert!(matches!(err, MoaStoreError::Validation(_)));
    }

    #[tokio::test]
    async fn delete_only_preset_is_refused() {
        let _g = moa_config_test_lock().lock().await;
        let (store, _dir) = temp_store();
        store.save_preset("solo", preset("gpt-5.5", "claude-opus-4-8"), false)
            .await
            .unwrap();
        let err = store.delete_preset("solo").await.unwrap_err();
        assert!(matches!(err, MoaStoreError::OnlyPreset(_)));
    }
}
```

> 注：`ConfigBackup::new` / `moa_config_test_lock` 的确切签名照 `moa_manage.rs` 测试里的用法（Task-2 实施时先 `grep -n "ConfigBackup::new\|moa_config_test_lock\|create_test_patcher" src/` 对齐；若现成有 `create_test_patcher` helper 就直接复用它替换 `temp_store` 的 patcher 构造）。

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib providers::moa::preset_store::tests::save_then_list_roundtrips`
Expected: FAIL（`MoaPresetStore::new`/`save_preset`/`list` 未实现）。

- [ ] **Step 3: Implement the store**

在 `preset_store.rs` 的 `impl std::fmt::Display` 之后、`#[cfg(test)]` 之前插入实现（逻辑 = 从 `moa_manage.rs` 上提，去掉 `MoaManageOutput` 包装，改用 `Result`）：

```rust
impl MoaPresetStore {
    pub fn new(config: Arc<RwLock<Config>>, patcher: Arc<ConfigPatcher>) -> Self {
        Self { config, patcher }
    }

    pub async fn list(&self) -> MoaToml {
        self.config.read().await.moa.clone()
    }

    async fn hot_refresh(&self) {
        store_moa_config(self.config.read().await.moa.clone());
    }

    async fn apply(&self, patch: serde_json::Value) -> Result<PatchResult, MoaStoreError> {
        let request = PatchRequest {
            path: "moa".to_string(),
            patch,
            health_check: false,
            dry_run: false,
        };
        match self.patcher.apply(request).await {
            Ok(result) if result.success => {
                self.hot_refresh().await;
                Ok(result)
            }
            Ok(_) => Err(MoaStoreError::Patch("patch did not apply".to_string())),
            Err(e) => Err(MoaStoreError::Patch(e.to_string())),
        }
    }

    pub async fn save_preset(
        &self,
        name: &str,
        preset: MoaPreset,
        make_default: bool,
    ) -> Result<PatchResult, MoaStoreError> {
        // Layer-2 validation against a scratch config (recursion / empty-advisor
        // / global-dedup guards — same pipeline a TOML-parsed config runs).
        let mut scratch = MoaToml::default();
        scratch.presets.insert(name.to_string(), preset.clone());
        let errors = scratch.validation_errors();
        if !errors.is_empty() {
            return Err(MoaStoreError::Validation(errors));
        }

        let preset_json = serde_json::to_value(&preset)
            .map_err(|e| MoaStoreError::Patch(format!("serialize preset: {e}")))?;
        let mut presets_patch = serde_json::Map::new();
        presets_patch.insert(name.to_string(), preset_json);
        let mut patch = serde_json::json!({ "presets": presets_patch });
        if make_default {
            patch["default_preset"] = serde_json::json!(name);
        }
        self.apply(patch).await
    }

    pub async fn delete_preset(&self, name: &str) -> Result<PatchResult, MoaStoreError> {
        let moa_cfg = get_moa_config().unwrap_or_default();
        if !moa_cfg.presets.contains_key(name) {
            return Err(MoaStoreError::Absent(name.to_string()));
        }
        if moa_cfg.presets.len() == 1 {
            return Err(MoaStoreError::OnlyPreset(name.to_string()));
        }
        let mut presets_patch = serde_json::Map::new();
        presets_patch.insert(name.to_string(), serde_json::Value::Null);
        let mut patch = serde_json::json!({ "presets": presets_patch });
        // Deleted preset was default: reassign to alphabetically-first remaining.
        if moa_cfg.default_preset.as_deref() == Some(name) {
            let mut remaining: Vec<&String> =
                moa_cfg.presets.keys().filter(|k| k.as_str() != name).collect();
            remaining.sort();
            if let Some(next) = remaining.first() {
                patch["default_preset"] = serde_json::json!(next);
            }
        }
        self.apply(patch).await
    }

    pub async fn set_default(&self, name: &str) -> Result<PatchResult, MoaStoreError> {
        let moa_cfg = get_moa_config().unwrap_or_default();
        if !moa_cfg.presets.contains_key(name) {
            return Err(MoaStoreError::Absent(name.to_string()));
        }
        self.apply(serde_json::json!({ "default_preset": name })).await
    }

    pub async fn set_save_traces(&self, on: bool) -> Result<PatchResult, MoaStoreError> {
        self.apply(serde_json::json!({ "save_traces": on })).await
    }
}
```

- [ ] **Step 4: Register the module**

在 `src/providers/moa/mod.rs` 加：

```rust
pub mod preset_store;
pub use preset_store::{MoaPresetStore, MoaStoreError};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib providers::moa::preset_store::tests`
Expected: PASS（三条）。

- [ ] **Step 6: Commit**

```bash
git add src/providers/moa/preset_store.rs src/providers/moa/mod.rs
git commit -m "moa: extract MoaPresetStore as the single config-write core"
```

---

### Task 3: `moa` 工具委托共享核心（熵减）

**Files:**
- Modify: `src/builtin_tools/moa_manage.rs`（`set_preset` ~373–472、`delete_preset` ~474–559）

**Interfaces:**
- Consumes: `MoaPresetStore`（Task 2）。工具已持有 `self.config: Option<Arc<RwLock<Config>>>` 与 `self.config_patcher: Option<Arc<ConfigPatcher>>`。
- Produces: 行为不变的 `set_preset` / `delete_preset`（返回同样的 `MoaManageOutput`）；内联 patch 块删除。

- [ ] **Step 1: Run existing tool tests to capture green baseline**

Run: `cargo test -p alephcore --lib builtin_tools::moa_manage::tests`
Expected: PASS（记录当前通过用例，作回归基线）。

- [ ] **Step 2: Rewrite `set_preset` to delegate**

将 `set_preset` 主体（构建 `MoaPreset` 之后的全部内联校验+patch 逻辑）替换为：

```rust
        let preset = MoaPreset {
            enabled: true,
            advisors,
            aggregator,
            fanout: fanout.unwrap_or_default(),
            advisor_timeout_secs: advisor_timeout_secs.unwrap_or_else(default_advisor_timeout_secs),
            advisor_max_tokens,
            advisor_temperature,
            aggregator_temperature,
        };

        let (config, patcher) = match (&self.config, &self.config_patcher) {
            (Some(c), Some(p)) => (Arc::clone(c), Arc::clone(p)),
            _ => {
                return Ok(MoaManageOutput {
                    success: false,
                    message: "Config patcher not available".to_string(),
                    data: None,
                })
            }
        };
        let store = crate::providers::moa::MoaPresetStore::new(config, patcher);
        match store.save_preset(&name, preset, set_default.unwrap_or(false)).await {
            Ok(result) => Ok(MoaManageOutput {
                success: true,
                message: format!("Preset '{name}' saved ({} field change(s)).", result.diff.len()),
                data: Some(serde_json::to_value(&result).unwrap_or_default()),
            }),
            Err(crate::providers::moa::MoaStoreError::Validation(errors)) => Ok(MoaManageOutput {
                success: false,
                message: format!("Preset '{name}' rejected: {}", errors.join("; ")),
                data: Some(serde_json::json!({ "errors": errors })),
            }),
            Err(e) => Ok(MoaManageOutput {
                success: false,
                message: e.to_string(),
                data: None,
            }),
        }
```

- [ ] **Step 3: Rewrite `delete_preset` to delegate**

将 `delete_preset` 主体替换为：

```rust
        let (config, patcher) = match (&self.config, &self.config_patcher) {
            (Some(c), Some(p)) => (Arc::clone(c), Arc::clone(p)),
            _ => {
                return Ok(MoaManageOutput {
                    success: false,
                    message: "Config patcher not available".to_string(),
                    data: None,
                })
            }
        };
        let store = crate::providers::moa::MoaPresetStore::new(config, patcher);
        match store.delete_preset(&name).await {
            Ok(result) => Ok(MoaManageOutput {
                success: true,
                message: format!("Preset '{name}' deleted."),
                data: Some(serde_json::to_value(&result).unwrap_or_default()),
            }),
            Err(e) => Ok(MoaManageOutput {
                success: false,
                message: e.to_string(),
                data: None,
            }),
        }
```

> 删除随之变为未使用的 import（如 `PatchRequest`、`ConfigPatcher` 若不再引用），按编译器 warning 一并清理——只清你这次改动导致 unused 的。

- [ ] **Step 4: Run tool tests + build to verify parity**

Run: `cargo test -p alephcore --lib builtin_tools::moa_manage::tests`
Expected: PASS（与 Step 1 基线一致；错误信息文案可能微调，若某用例断言旧文案则同步更新断言）。

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/moa_manage.rs
git commit -m "moa: delegate tool set_preset/delete_preset to MoaPresetStore"
```

---

### Task 4: `moa.*` Gateway RPC handlers

**Files:**
- Create: `src/gateway/handlers/moa.rs`
- Modify: `src/gateway/handlers/mod.rs`（`pub mod moa;`）
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/settings.rs`（`register_config_handlers` 内注册 5 方法）
- Test: `src/gateway/handlers/moa.rs`（`#[cfg(test)]`，照 `providers/tests.rs` 模式）

**Interfaces:**
- Consumes: `MoaPresetStore`（Task 2）；handler 签名 `async fn(request: JsonRpcRequest, config: Arc<RwLock<Config>>, config_patcher: Arc<ConfigPatcher>) -> JsonRpcResponse`；`JsonRpcResponse::{success,error}`；错误码 `INVALID_PARAMS`、`INTERNAL_ERROR`。
- Produces: RPC 方法 `moa.listPresets`（读）、`moa.savePreset`、`moa.deletePreset`、`moa.setDefault`、`moa.setSaveTraces`（写）。

- [ ] **Step 1: Write the failing test**

Create `src/gateway/handlers/moa.rs`（先放 handler 骨架签名 + 测试，编译失败于未实现分支）。测试骨架：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::moa::config_handle::moa_config_test_lock;

    // Build a handler-ready (config, patcher) over a temp config.toml.
    // Reuse the same temp-store helper shape as preset_store tests.
    async fn ctx() -> (Arc<RwLock<Config>>, Arc<ConfigPatcher>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        let config = Arc::new(RwLock::new(Config::default()));
        let backup = crate::config::backup::ConfigBackup::new(dir.path().to_path_buf());
        let patcher = Arc::new(ConfigPatcher::new(Arc::clone(&config), path, backup));
        (config, patcher, dir)
    }

    #[tokio::test]
    async fn save_preset_persists_and_list_returns_it() {
        let _g = moa_config_test_lock().lock().await;
        let (config, patcher, _dir) = ctx().await;
        let params = serde_json::json!({
            "name": "default",
            "enabled": true,
            "advisors": [{"provider": "openai", "model": "gpt-5.5"}],
            "aggregator": {"provider": "anthropic", "model": "claude-opus-4-8"},
            "make_default": true
        });
        let req = JsonRpcRequest::with_id("moa.savePreset", Some(params), serde_json::json!(1));
        let resp = handle_save_preset(req, Arc::clone(&config), Arc::clone(&patcher)).await;
        assert!(resp.error.is_none(), "save should succeed: {:?}", resp.error);

        let list_req = JsonRpcRequest::with_id("moa.listPresets", None, serde_json::json!(2));
        let list_resp = handle_list_presets(list_req, Arc::clone(&config)).await;
        let v = list_resp.result.unwrap();
        assert!(v["presets"]["default"].is_object());
        assert_eq!(v["default_preset"], "default");
    }

    #[tokio::test]
    async fn save_preset_rejects_duplicate_slot() {
        let _g = moa_config_test_lock().lock().await;
        let (config, patcher, _dir) = ctx().await;
        let params = serde_json::json!({
            "name": "p",
            "enabled": true,
            "advisors": [{"provider": "openai", "model": "gpt-5.5"}],
            "aggregator": {"provider": "openai", "model": "gpt-5.5"}
        });
        let req = JsonRpcRequest::with_id("moa.savePreset", Some(params), serde_json::json!(1));
        let resp = handle_save_preset(req, config, patcher).await;
        let err = resp.error.expect("must reject duplicate slot");
        assert_eq!(err.code, INVALID_PARAMS);
    }
}
```

> `JsonRpcRequest::with_id` / `JsonRpcResponse` 字段（`.result`/`.error`/`.error.code`）照 `providers/tests.rs` 现有用法对齐。

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::handlers::moa::tests::save_preset_persists_and_list_returns_it`
Expected: FAIL（handlers 未实现）。

- [ ] **Step 3: Implement handlers**

`moa.rs` 顶部与 handler 实现：

```rust
//! MoA preset configuration RPC handlers. Thin I/O over MoaPresetStore — the
//! Panel's visual config talks to these; the `moa` tool shares the same core.

use crate::config::{Config, MoaFanout, MoaPreset, MoaSlot};
use crate::config::patcher::ConfigPatcher;
use crate::config::default_advisor_timeout_secs;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::providers::moa::{MoaPresetStore, MoaStoreError};
use crate::sync_primitives::Arc;
use serde::Deserialize;
use tokio::sync::RwLock;

#[derive(Debug, Deserialize)]
struct SavePresetParams {
    name: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    advisors: Vec<MoaSlot>,
    aggregator: MoaSlot,
    #[serde(default)]
    fanout: MoaFanout,
    #[serde(default = "default_advisor_timeout_secs")]
    advisor_timeout_secs: u64,
    #[serde(default)]
    advisor_max_tokens: Option<u32>,
    #[serde(default)]
    advisor_temperature: Option<f32>,
    #[serde(default)]
    aggregator_temperature: Option<f32>,
    #[serde(default)]
    make_default: bool,
}

const fn default_true() -> bool { true }

#[derive(Debug, Deserialize)]
struct NameParam { name: String }

#[derive(Debug, Deserialize)]
struct SaveTracesParam { on: bool }

fn parse<T: for<'de> Deserialize<'de>>(
    req: &JsonRpcRequest,
) -> Result<T, JsonRpcResponse> {
    let params = req.params.clone().ok_or_else(|| {
        JsonRpcResponse::error(req.id.clone(), INVALID_PARAMS, "Missing params".into())
    })?;
    serde_json::from_value(params).map_err(|e| {
        JsonRpcResponse::error(req.id.clone(), INVALID_PARAMS, format!("Invalid params: {e}"))
    })
}

/// Map a store error to the right JSON-RPC error code.
fn store_err_response(id: serde_json::Value, e: MoaStoreError) -> JsonRpcResponse {
    let code = match e {
        MoaStoreError::Validation(_) | MoaStoreError::Absent(_) | MoaStoreError::OnlyPreset(_) => {
            INVALID_PARAMS
        }
        MoaStoreError::Patch(_) => INTERNAL_ERROR,
    };
    JsonRpcResponse::error(id, code, e.to_string())
}

pub async fn handle_list_presets(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let moa = config.read().await.moa.clone();
    match serde_json::to_value(&moa) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

pub async fn handle_save_preset(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    config_patcher: Arc<ConfigPatcher>,
) -> JsonRpcResponse {
    let p: SavePresetParams = match parse(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let preset = MoaPreset {
        enabled: p.enabled,
        advisors: p.advisors,
        aggregator: p.aggregator,
        fanout: p.fanout,
        advisor_timeout_secs: p.advisor_timeout_secs,
        advisor_max_tokens: p.advisor_max_tokens,
        advisor_temperature: p.advisor_temperature,
        aggregator_temperature: p.aggregator_temperature,
    };
    let store = MoaPresetStore::new(config, config_patcher);
    match store.save_preset(&p.name, preset, p.make_default).await {
        Ok(result) => JsonRpcResponse::success(
            request.id,
            serde_json::to_value(&result).unwrap_or_default(),
        ),
        Err(e) => store_err_response(request.id, e),
    }
}

pub async fn handle_delete_preset(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    config_patcher: Arc<ConfigPatcher>,
) -> JsonRpcResponse {
    let p: NameParam = match parse(&request) { Ok(p) => p, Err(r) => return r };
    let store = MoaPresetStore::new(config, config_patcher);
    match store.delete_preset(&p.name).await {
        Ok(result) => JsonRpcResponse::success(request.id, serde_json::to_value(&result).unwrap_or_default()),
        Err(e) => store_err_response(request.id, e),
    }
}

pub async fn handle_set_default(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    config_patcher: Arc<ConfigPatcher>,
) -> JsonRpcResponse {
    let p: NameParam = match parse(&request) { Ok(p) => p, Err(r) => return r };
    let store = MoaPresetStore::new(config, config_patcher);
    match store.set_default(&p.name).await {
        Ok(result) => JsonRpcResponse::success(request.id, serde_json::to_value(&result).unwrap_or_default()),
        Err(e) => store_err_response(request.id, e),
    }
}

pub async fn handle_set_save_traces(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    config_patcher: Arc<ConfigPatcher>,
) -> JsonRpcResponse {
    let p: SaveTracesParam = match parse(&request) { Ok(p) => p, Err(r) => return r };
    let store = MoaPresetStore::new(config, config_patcher);
    match store.set_save_traces(p.on).await {
        Ok(result) => JsonRpcResponse::success(request.id, serde_json::to_value(&result).unwrap_or_default()),
        Err(e) => store_err_response(request.id, e),
    }
}
```

> `MoaSlot` / `MoaFanout` 需 `Deserialize`（已 derive）。若 `parse` 的泛型辅助与现有 handler 风格不符，可展开为逐 handler 的 `match request.params`（照 behavior_config.rs）。

- [ ] **Step 4: Register the module + methods**

`src/gateway/handlers/mod.rs` 加 `pub mod moa;`。

`settings.rs::register_config_handlers` 的 `use` 段加 `use alephcore::gateway::handlers::moa;`，并在 providers 注册块之后加：

```rust
    // MoA presets (visual config; shares MoaPresetStore with the `moa` tool)
    register_handler!(server, "moa.listPresets", moa::handle_list_presets, config);
    register_handler!(
        server, "moa.savePreset", moa::handle_save_preset, config, config_patcher
    );
    register_handler!(
        server, "moa.deletePreset", moa::handle_delete_preset, config, config_patcher
    );
    register_handler!(
        server, "moa.setDefault", moa::handle_set_default, config, config_patcher
    );
    register_handler!(
        server, "moa.setSaveTraces", moa::handle_set_save_traces, config, config_patcher
    );
```

> 授权对齐：写方法与 `providers.update`/`config.patch` 同属 Mutate lane + config-tier（远程 chat-tier 被既有 device-tier 机制拦，Panel 连接授权后即 operator——见 `method_authz.rs` 头注释）。**无需新增 per-method 授权代码**；实施时确认 `Lane::for_method` 对未知 `moa.*` 默认落 `Mutate`（fail-safe，见 lane.rs 注释），无需改动。

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::handlers::moa::tests`
Expected: PASS（两条）。

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/moa.rs src/gateway/handlers/mod.rs src/bin/aleph-server/commands/start/builder/handlers/settings.rs
git commit -m "gateway: add moa.* preset config RPCs backed by MoaPresetStore"
```

---

### Task 5: Panel API 层 `MoaApi`

**Files:**
- Create: `interfaces/webchat/src/api/moa.rs`
- Modify: `interfaces/webchat/src/api.rs`（`pub mod moa;`）

**Interfaces:**
- Consumes: `DashboardState::rpc_call(method, params) -> Result<Value, String>`（照 `api/config.rs` 用法）。
- Produces: `MoaApi` with `list_presets`、`save_preset`、`delete_preset`、`set_default`、`set_save_traces`；DTO `MoaPresetDto`、`MoaSlotDto`、`MoaConfigDto`（与后端 serde 形状一一对应）。

- [ ] **Step 1: Implement the API wrapper**

```rust
//! Panel-side wrapper for the `moa.*` gateway RPCs. Pure I/O (R4).

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MoaSlotDto {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoaPresetDto {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub advisors: Vec<MoaSlotDto>,
    pub aggregator: MoaSlotDto,
    #[serde(default)]
    pub fanout: String, // "per_iteration" | "user_turn"
    #[serde(default)]
    pub advisor_timeout_secs: u64,
    #[serde(default)]
    pub advisor_max_tokens: Option<u32>,
    #[serde(default)]
    pub advisor_temperature: Option<f32>,
    #[serde(default)]
    pub aggregator_temperature: Option<f32>,
}

fn yes() -> bool { true }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MoaConfigDto {
    #[serde(default)]
    pub default_preset: Option<String>,
    #[serde(default)]
    pub save_traces: bool,
    #[serde(default)]
    pub presets: HashMap<String, MoaPresetDto>,
}

pub struct MoaApi;

impl MoaApi {
    pub async fn list_presets(state: &DashboardState) -> Result<MoaConfigDto, String> {
        let v = state.rpc_call("moa.listPresets", Value::Null).await?;
        serde_json::from_value(v).map_err(|e| format!("parse moa config: {e}"))
    }

    pub async fn save_preset(
        state: &DashboardState,
        name: &str,
        preset: &MoaPresetDto,
        make_default: bool,
    ) -> Result<(), String> {
        let mut params = serde_json::to_value(preset).map_err(|e| e.to_string())?;
        params["name"] = serde_json::json!(name);
        params["make_default"] = serde_json::json!(make_default);
        state.rpc_call("moa.savePreset", params).await.map(|_| ())
    }

    pub async fn delete_preset(state: &DashboardState, name: &str) -> Result<(), String> {
        state.rpc_call("moa.deletePreset", serde_json::json!({ "name": name })).await.map(|_| ())
    }

    pub async fn set_default(state: &DashboardState, name: &str) -> Result<(), String> {
        state.rpc_call("moa.setDefault", serde_json::json!({ "name": name })).await.map(|_| ())
    }

    pub async fn set_save_traces(state: &DashboardState, on: bool) -> Result<(), String> {
        state.rpc_call("moa.setSaveTraces", serde_json::json!({ "on": on })).await.map(|_| ())
    }
}
```

> `MoaPresetDto` 的 `fanout` 用 string 兼容后端 snake_case enum；save 时后端 serde 会把 `"per_iteration"`/`"user_turn"` 解析回 `MoaFanout`。

- [ ] **Step 2: Register the module**

`interfaces/webchat/src/api.rs` 加 `pub mod moa;`（放在 `pub mod memory;` 附近，字母序）。

- [ ] **Step 3: Verify it compiles (WASM crate)**

Run: `cargo check -p aleph-webchat --target wasm32-unknown-unknown 2>/dev/null || cargo check -p aleph-webchat`
Expected: 编译通过（无该 target 时退化为普通 check）。

> 若 webchat crate 名不同，先 `grep -n "^name" interfaces/webchat/Cargo.toml` 确认。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/api/moa.rs interfaces/webchat/src/api.rs
git commit -m "webchat: add MoaApi wrapper for moa.* RPCs"
```

---

### Task 6: Panel MoA 设置页（列表 + 编辑器）

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/settings/moa/mod.rs`
- Create: `interfaces/webchat/src/platform/wide/views/settings/moa/preset_editor.rs`
- Create: `interfaces/webchat/src/platform/wide/views/settings/moa/options.rs`（纯去重逻辑 + 单测）
- Modify: `interfaces/webchat/src/platform/wide/views/settings/mod.rs`（`pub mod moa;` + re-export `MoaView` + 入口卡片 `href: "/settings/moa"`）
- Modify: 设置路由匹配处（与 `/settings/generation-providers` 同一 match/router，照现有 `GenerationProvidersView` 注册处新增 `MoaView` 分支）

**Interfaces:**
- Consumes: `MoaApi`（Task 5）；`ProvidersApi::catalog(state, CatalogView::…) -> Vec<CatalogEntry>`（`CatalogEntry { id, models, enabled, has_api_key, verified, .. }`）。
- Produces: `MoaView`（顶级设置页）；纯函数 `available_options(catalog, used) -> Vec<SlotOption>`。

- [ ] **Step 1: Write the failing test for the pure dedup logic**

Create `options.rs`：

```rust
//! Pure model-option logic for the MoA editor — kept out of the view so the
//! "already-used slots are filtered out" rule is unit-testable.

use crate::api::moa::MoaSlotDto;
use crate::api::providers::CatalogEntry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotOption {
    pub provider: String,
    pub model: String,
    pub label: String, // "provider / model"
}

fn norm(s: &str) -> String { s.trim().to_lowercase() }

/// Flatten the credential-aware catalog into (provider, model) options, minus
/// any slot already used elsewhere in the preset (global dedup). `keep` is the
/// slot currently bound to THIS selector (so editing a row still shows its own
/// value); pass None for a fresh row.
pub fn available_options(
    catalog: &[CatalogEntry],
    used: &[MoaSlotDto],
    keep: Option<&MoaSlotDto>,
) -> Vec<SlotOption> {
    let blocked: std::collections::HashSet<(String, String)> = used
        .iter()
        .filter(|s| keep != Some(*s))
        .map(|s| (norm(&s.provider), norm(&s.model)))
        .collect();

    let mut out = Vec::new();
    for entry in catalog.iter().filter(|e| e.enabled && e.has_api_key) {
        for model in &entry.models {
            let key = (norm(&entry.id), norm(model));
            if blocked.contains(&key) {
                continue;
            }
            out.push(SlotOption {
                provider: entry.id.clone(),
                model: model.clone(),
                label: format!("{} / {}", entry.id, model),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, models: &[&str]) -> CatalogEntry {
        CatalogEntry {
            id: id.into(),
            display_name: id.into(),
            default_model: models.first().copied().unwrap_or("").into(),
            base_url: String::new(),
            protocol: String::new(),
            color: String::new(),
            homepage: None,
            notes: None,
            modalities: vec![],
            models: models.iter().map(|m| (*m).into()).collect(),
            has_api_key: true,
            verified: true,
            enabled: true,
            is_default: false,
        }
    }

    #[test]
    fn used_slots_are_filtered_out() {
        let catalog = vec![entry("openai", &["gpt-5.5", "gpt-5-mini"])];
        let used = vec![MoaSlotDto { provider: "openai".into(), model: "gpt-5.5".into() }];
        let opts = available_options(&catalog, &used, None);
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].model, "gpt-5-mini");
    }

    #[test]
    fn kept_slot_remains_selectable_when_editing() {
        let catalog = vec![entry("openai", &["gpt-5.5"])];
        let mine = MoaSlotDto { provider: "openai".into(), model: "gpt-5.5".into() };
        let used = vec![mine.clone()];
        let opts = available_options(&catalog, &used, Some(&mine));
        assert_eq!(opts.len(), 1); // my own value stays available
    }

    #[test]
    fn providers_without_credentials_are_excluded() {
        let mut e = entry("openai", &["gpt-5.5"]);
        e.has_api_key = false;
        let opts = available_options(&[e], &[], None);
        assert!(opts.is_empty());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p aleph-webchat available_options 2>/dev/null || cargo test -p aleph-webchat options`
Expected: FAIL（模块未接入）。

- [ ] **Step 3: Wire `options.rs` into the moa module so it compiles**

Create `moa/mod.rs` 顶部先加 `mod options;` 并 `pub use options::{available_options, SlotOption};`（视图代码 Step 5 再补）。运行 Step 2 命令直到 PASS。

- [ ] **Step 4: Run the pure test to verify it passes**

Run: `cargo test -p aleph-webchat options`
Expected: PASS（三条）。

- [ ] **Step 5: Implement the list view + editor**

`moa/mod.rs`（`MoaView`）与 `preset_editor.rs` 照 `generation_providers/mod.rs`+`preset_setup.rs` 的 Leptos 惯用法实现，要点：

- `MoaView` 进入时并行 `MoaApi::list_presets` + `ProvidersApi::catalog`；缺任一 → 顶部错误条。
- 已配置模型（`catalog` 过滤 enabled+has_api_key 后拍平）计数 < 2 → 顶部提示"先去 Providers 配置更多模型"并禁用「新建 preset」。
- preset 卡片：名称、default 徽章、advisor chips（`provider / model`）、aggregator chip、enabled、编辑/删除按钮。删除走 `MoaApi::delete_preset` 后重拉。
- 顶部全局 `save_traces` 开关 → `MoaApi::set_save_traces`。
- `preset_editor.rs`：本地 `RwSignal<MoaPresetDto>` + 名称信号；每个模型下拉的选项 = `available_options(&catalog, &all_used_slots, Some(&this_slot))`；advisor 行可增删；高级折叠区（fanout 单选、`advisor_timeout_secs` 数字、`advisor_max_tokens`/温度可选输入）；enabled 开关；设为默认复选。保存前本地校验（名称非空、enabled 时 ≥1 advisor、`available_options` 天然挡重复）；调 `MoaApi::save_preset`，失败时把服务端错误串显示在表单顶部、不关表单；成功后重拉列表并关表单。

> 该视图为 Leptos/WASM，渲染细节以 `generation_providers` 为准（`view!` 宏、`RwSignal`、`Suspense`/`Resource`、按钮/表单样式类名一致）。实施时先通读那两文件再落笔，保证风格与既有设置页一致（surgical，勿引入新样式体系）。

- [ ] **Step 6: Register the view + route + sidebar card**

- `settings/mod.rs`：加 `pub mod moa;`、`pub use moa::MoaView;`，并在设置入口卡片列表加一张（`title: "MoA"`, `href: "/settings/moa"`, `body: "多模型持续咨询：配置 advisor / aggregator preset"`）。
- 设置路由匹配处：照 `GenerationProvidersView`/`ProvidersView` 的注册点新增 `"/settings/moa" => MoaView` 分支（先 `grep -rn "GenerationProvidersView\|settings/generation-providers" interfaces/webchat/src` 定位那个 match/Routes 块）。

- [ ] **Step 7: Verify the WASM crate compiles**

Run: `cargo check -p aleph-webchat`
Expected: 编译通过。

- [ ] **Step 8: Manual walkthrough (honest coverage note)**

视图渲染无自动化 E2E（WASM 较重）。手动走查清单（`just dev` 后浏览器打开 `/settings/moa`）：
1. 无 preset → 显示空态与「新建」；已配置模型 < 2 → 新建禁用 + 提示。
2. 新建 preset：advisor 下拉选一个后，同一模型在 aggregator 下拉消失（去重生效）。
3. 保存 → 卡片出现；刷新页面仍在（config.toml 已落盘 + 热更新）。
4. 设为默认 → 徽章移动；删除非唯一 preset 成功，删唯一 preset 被拒并提示。
5. 制造重复（若绕过 UI）→ 保存被服务端拒、表单顶部报错、表单不关。

- [ ] **Step 9: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/settings/moa interfaces/webchat/src/platform/wide/views/settings/mod.rs
git commit -m "webchat: add MoA visual preset config settings page"
```

---

### Task 7: 文档同步 — FEATURE_LOCATOR §4.9

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`（§4.9 状态行）

**Interfaces:** 无代码接口。

- [ ] **Step 1: Append the Round-4 record**

在 §4.9「状态」行尾追加一句（中文，双语文档惯例）：

```
；✅ 第四轮 · Panel 可视化配置（2026-07-06，spec `docs/superpowers/specs/2026-07-06-moa-panel-visual-config-design.md`）——新增顶级设置页 `/settings/moa`（`interfaces/webchat/src/platform/wide/views/settings/moa/`）用选择器从 `providers.catalog` 挑 advisor/aggregator（全局去重，`available_options` 纯函数）、preset 写核心抽出为 `src/providers/moa/preset_store.rs::MoaPresetStore`（`moa` 工具与新 `moa.*` RPC 共享，熵减）、`moa.{listPresets,savePreset,deletePreset,setDefault,setSaveTraces}` gateway RPC（`src/gateway/handlers/moa.rs`）、`MoaToml::validation_errors` 增全局槽位去重。
```

- [ ] **Step 2: Commit**

```bash
git add docs/reference/FEATURE_LOCATOR.md
git commit -m "docs: record MoA round-4 panel visual config in FEATURE_LOCATOR"
```

---

## 完成标准

- Task 1–4/7：`cargo test -p alephcore --lib`（定向到 `config::types::moa`、`providers::moa::preset_store`、`builtin_tools::moa_manage`、`gateway::handlers::moa`）全绿。
- Task 5–6：`cargo check -p aleph-webchat` 通过；`options` 纯函数单测绿；Task 6 手动走查清单通过。
- `moa` 对话工具行为不回归（Task 3 基线）。
- 无死代码：`moa_manage.rs` 内联 patch 块已删除，写路径单一真源 `MoaPresetStore`。
- 全程在 worktree 分支，未触碰 main。
