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

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

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
    // Exclude the synthetic `moa` pseudo-provider row (id/protocol "moa") that
    // `providers.catalog` appends when presets exist — picking it as an advisor
    // or aggregator would build a recursive MoaSlot{provider:"moa"} the server
    // rejects at save time. Filter it here so it never appears as an option.
    for entry in catalog
        .iter()
        .filter(|e| e.enabled && e.has_api_key && e.id != "moa")
    {
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
        use aleph_protocol::providers::{ModelSource, RosterModel};
        CatalogEntry {
            id: id.into(),
            display_name: id.into(),
            default_model: models.first().copied().unwrap_or("").into(),
            base_url: String::new(),
            protocol: String::new(),
            color: String::new(),
            homepage: None,
            notes: None,
            signup_url: None,
            fallback_models: vec![],
            default_aux_model: None,
            aliases: vec![],
            modalities: vec![],
            models: models.iter().map(|m| (*m).into()).collect(),
            has_api_key: true,
            verified: true,
            enabled: true,
            is_default: false,
            auth_kind: crate::api::AuthKind::ApiKey,
            capabilities: None,
            cost: None,
            endpoint: "cloud".into(),
            lifecycle: crate::api::ModelLifecycle::default(),
            requires_explicit_model: false,
            discoverable: true,
            roster: models
                .iter()
                .map(|m| RosterModel::new(*m, ModelSource::Configured))
                .collect(),
        }
    }

    #[test]
    fn used_slots_are_filtered_out() {
        let catalog = vec![entry("openai", &["gpt-5.5", "gpt-5-mini"])];
        let used = vec![MoaSlotDto {
            provider: "openai".into(),
            model: "gpt-5.5".into(),
        }];
        let opts = available_options(&catalog, &used, None);
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].model, "gpt-5-mini");
    }

    #[test]
    fn kept_slot_remains_selectable_when_editing() {
        let catalog = vec![entry("openai", &["gpt-5.5"])];
        let mine = MoaSlotDto {
            provider: "openai".into(),
            model: "gpt-5.5".into(),
        };
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

    #[test]
    fn synthetic_moa_pseudo_provider_is_excluded() {
        // providers.catalog appends an id="moa" row (enabled + has_api_key) whose
        // models are preset names; it must never be a selectable advisor/aggregator.
        let catalog = vec![
            entry("openai", &["gpt-5.5"]),
            entry("moa", &["my-preset", "other-preset"]),
        ];
        let opts = available_options(&catalog, &[], None);
        assert!(opts.iter().all(|o| o.provider != "moa"));
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].provider, "openai");
    }
}
