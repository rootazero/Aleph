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

impl MoaPresetStore {
    pub fn new(config: Arc<RwLock<Config>>, patcher: Arc<ConfigPatcher>) -> Self {
        Self { config, patcher }
    }

    pub async fn list(&self) -> MoaToml {
        self.config.read().await.moa.clone().unwrap_or_default()
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
            let mut remaining: Vec<&String> = moa_cfg
                .presets
                .keys()
                .filter(|k| k.as_str() != name)
                .collect();
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
        self.apply(serde_json::json!({ "default_preset": name }))
            .await
    }

    pub async fn set_save_traces(&self, on: bool) -> Result<PatchResult, MoaStoreError> {
        self.apply(serde_json::json!({ "save_traces": on })).await
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
        let backup = crate::config::backup::ConfigBackup::new(dir.path().join("backups"), 10);
        let patcher = Arc::new(ConfigPatcher::new(Arc::clone(&config), config_path, backup));
        (MoaPresetStore::new(config, patcher), dir)
    }

    fn preset(advisor: &str, agg: &str) -> MoaPreset {
        MoaPreset {
            enabled: true,
            advisors: vec![MoaSlot {
                provider: "openai".into(),
                model: advisor.into(),
            }],
            aggregator: MoaSlot {
                provider: "anthropic".into(),
                model: agg.into(),
            },
            fanout: MoaFanout::default(),
            advisor_timeout_secs: 120,
            advisor_max_tokens: None,
            advisor_temperature: None,
            aggregator_temperature: None,
        }
    }

    #[tokio::test]
    async fn save_then_list_roundtrips() {
        let _guard = crate::providers::moa::config_handle::moa_config_test_lock();
        let (store, _dir) = temp_store();
        store
            .save_preset("default", preset("gpt-5.5", "claude-opus-4-8"), true)
            .await
            .expect("save ok");
        let listed = store.list().await;
        assert!(listed.presets.contains_key("default"));
        assert_eq!(listed.default_preset.as_deref(), Some("default"));
    }

    #[tokio::test]
    async fn save_rejects_invalid_preset() {
        let _guard = moa_config_test_lock();
        let (store, _dir) = temp_store();
        // aggregator == advisor -> dedup validation error
        let bad = MoaPreset {
            aggregator: MoaSlot {
                provider: "openai".into(),
                model: "gpt-5.5".into(),
            },
            ..preset("gpt-5.5", "x")
        };
        let err = store.save_preset("p", bad, false).await.unwrap_err();
        assert!(matches!(err, MoaStoreError::Validation(_)));
    }

    #[tokio::test]
    async fn delete_only_preset_is_refused() {
        let _guard = moa_config_test_lock();
        let (store, _dir) = temp_store();
        store
            .save_preset("solo", preset("gpt-5.5", "claude-opus-4-8"), false)
            .await
            .unwrap();
        let err = store.delete_preset("solo").await.unwrap_err();
        assert!(matches!(err, MoaStoreError::OnlyPreset(_)));
    }
}
