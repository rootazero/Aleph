//! Immutable projection of the configured roster and published runtime readiness.
//! No credentials, provider instances, or independent registry live here.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::SessionModelPref;
use crate::config::Config;
use crate::providers::{model_catalog, presets};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelSelectionError {
    Unavailable,
    Unknown {
        model: String,
        provider: Option<String>,
    },
    Ambiguous {
        model: String,
        providers: Vec<String>,
    },
    Unauthenticated {
        provider: String,
    },
    Unsupported {
        provider: String,
        model: String,
    },
}

impl std::fmt::Display for ModelSelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => write!(f, "Selectable model publication is unavailable; model unchanged."),
            Self::Unknown { model, provider } => write!(f, "Unknown model '{model}' or provider {provider:?}; model unchanged."),
            Self::Ambiguous { model, providers } => write!(f, "Model '{model}' is ambiguous across providers: {}; qualify the provider; model unchanged.", providers.join(", ")),
            Self::Unauthenticated { provider } => write!(f, "Provider '{provider}' has no published usable authentication; model unchanged."),
            Self::Unsupported { provider, model } => write!(f, "Provider '{provider}' or model '{model}' is not currently supported/available; model unchanged."),
        }
    }
}

#[derive(Clone, Debug)]
struct SelectableProvider {
    models: BTreeSet<String>,
    discovered: BTreeSet<String>,
    discovery_transport_current: bool,
    discovery_vault_backed: bool,
    authenticated: bool,
    available: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SelectableModelsSnapshot {
    // The exact names the run builder can resolve. Reload must not silently
    // grant a newly registered provider absent from that routing map.
    pub(super) routable: BTreeSet<String>,
    providers: BTreeMap<String, SelectableProvider>,
}

impl SelectableModelsSnapshot {
    /// Project the same config/preset/discovery roster used by the catalog.
    /// `available` and `authenticated` are published construction/registry
    /// results, not guesses made by the selector. Neither retains credentials.
    pub fn from_config(
        config: &Config,
        routable: BTreeSet<String>,
        available: &BTreeSet<String>,
        authenticated: impl Fn(&str) -> bool,
    ) -> Self {
        let providers = config
            .providers
            .iter()
            .map(|(name, pc)| {
                let preset = presets::get_merged_preset(name, &config.presets_override);
                let base_url = pc
                    .base_url
                    .as_deref()
                    .or_else(|| preset.as_ref().map(|p| p.base_url.as_str()));
                let mut base: Vec<String> = pc
                    .all_models()
                    .iter()
                    .flat_map(|ids| ids.split(','))
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .collect();
                base.extend(pc.model_defs.iter().map(|def| def.id.clone()));
                if base.is_empty() {
                    if let Some(preset) = &preset {
                        base.push(preset.default_model.clone());
                    }
                }
                let models = presets::model_roster(
                    name,
                    base,
                    model_catalog::ModelSource::Configured,
                    &[],
                    base_url,
                )
                .into_iter()
                .map(|model| model.id)
                .collect();
                (
                    name.clone(),
                    SelectableProvider {
                        models,
                        discovered: BTreeSet::new(),
                        discovery_transport_current: false,
                        discovery_vault_backed: pc.api_key.is_none(),
                        authenticated: authenticated(name),
                        available: pc.enabled
                            && routable.contains(name)
                            && available.contains(name),
                    },
                )
            })
            .collect();
        Self {
            routable,
            providers,
        }
    }

    /// Certify only transports installed in the named run path. Config-only
    /// rebuilds preserve the previous certificate instead of issuing one.
    pub fn certify_discovery_transports(mut self, certified: impl Fn(&str) -> bool) -> Self {
        for (provider, entry) in &mut self.providers {
            entry.discovery_transport_current =
                entry.available && entry.authenticated && certified(provider);
        }
        self
    }

    pub fn replace_discovered(
        &self,
        provider: &str,
        models: &[model_catalog::discovery::DiscoveredModel],
    ) -> Self {
        let mut next = self.clone();
        if let Some(entry) = next.providers.get_mut(provider) {
            if entry.available && entry.authenticated && entry.discovery_transport_current {
                entry.discovered = models.iter().map(|model| model.id.clone()).collect();
            }
        }
        next
    }

    pub(super) fn preserve_discovered(&mut self, previous: &Self) {
        for (name, entry) in &mut self.providers {
            // Registry refresh alone does not replace the run builder's named
            // transports. Only a new boot publication certifies them again.
            if let Some(prior) = previous.providers.get(name) {
                entry.discovery_vault_backed = prior.discovery_vault_backed;
                entry.discovery_transport_current = prior.discovery_transport_current;
                entry.discovered.clone_from(&prior.discovered);
            } else {
                entry.discovery_transport_current = false;
            }
        }
    }

    pub(super) fn revoke_discovered(&mut self, affected: impl Fn(&str) -> bool, vault_only: bool) {
        for (name, entry) in &mut self.providers {
            if affected(name) && (!vault_only || entry.discovery_vault_backed) {
                entry.discovered.clear();
                entry.discovery_transport_current = false;
            }
        }
    }

    /// Return only bindings this publication can select, in provider/model order.
    pub fn choices(&self) -> Vec<aleph_protocol::providers::ChatModelChoice> {
        let mut choices = Vec::new();
        for (provider, entry) in &self.providers {
            if !entry.available || !entry.authenticated {
                continue;
            }
            let provider_label = presets::get_preset(provider)
                .and_then(|preset| preset.display_name)
                .unwrap_or(provider);
            choices.extend(entry.models.union(&entry.discovered).map(|model| {
                aleph_protocol::providers::ChatModelChoice {
                    provider: provider.clone(),
                    model: model.clone(),
                    provider_label: provider_label.to_owned(),
                    model_label: model.clone(),
                }
            }));
        }
        choices
    }

    /// Runtime readiness in this publication, used when another provider reloads.
    pub fn provider_is_available(&self, provider: &str) -> bool {
        self.providers
            .get(provider)
            .is_some_and(|entry| entry.available)
    }

    pub fn resolve(
        &self,
        provider: Option<&str>,
        model: &str,
    ) -> Result<SessionModelPref, ModelSelectionError> {
        // An explicit provider argument preserves slash-containing vendor IDs.
        // Otherwise a configured prefix is the qualified form; opaque vendor
        // ids (e.g. org/model) still resolve as a whole bare model id.
        let (provider, model) = if provider.is_none() {
            model
                .split_once('/')
                .filter(|(p, _)| self.providers.contains_key(*p))
                .map_or((None, model), |(p, m)| (Some(p), m))
        } else {
            (provider, model)
        };
        match provider {
            Some(name) => self.resolve_qualified(name, model),
            None => self.resolve_raw(model),
        }
    }

    /// Resolve the exact wire ID, without interpreting a slash as shorthand.
    pub fn resolve_raw(&self, model: &str) -> Result<SessionModelPref, ModelSelectionError> {
        let matches: Vec<_> = self
            .providers
            .iter()
            .filter(|(_, entry)| {
                entry.models.contains(model)
                    || (entry.available && entry.authenticated && entry.discovered.contains(model))
            })
            .map(|(name, _)| name.as_str())
            .collect();
        match matches.as_slice() {
            [] => Err(ModelSelectionError::Unknown {
                model: model.into(),
                provider: None,
            }),
            [name] => self.resolve_qualified(name, model),
            _ => Err(ModelSelectionError::Ambiguous {
                model: model.into(),
                providers: matches.into_iter().map(str::to_owned).collect(),
            }),
        }
    }

    fn resolve_qualified(
        &self,
        provider: &str,
        model: &str,
    ) -> Result<SessionModelPref, ModelSelectionError> {
        let Some(entry) = self.providers.get(provider) else {
            return Err(ModelSelectionError::Unknown {
                model: model.into(),
                provider: Some(provider.into()),
            });
        };
        if !entry.authenticated {
            return Err(ModelSelectionError::Unauthenticated {
                provider: provider.into(),
            });
        }
        if !entry.available || !(entry.models.contains(model) || entry.discovered.contains(model)) {
            return Err(ModelSelectionError::Unsupported {
                provider: provider.into(),
                model: model.into(),
            });
        }
        Ok(SessionModelPref {
            provider: Some(provider.into()),
            model: model.into(),
        })
    }
}
