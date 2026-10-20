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
                let discovered = base_url
                    .and_then(|url| model_catalog::cached_models(name, url))
                    .map(|listing| {
                        listing
                            .models
                            .into_iter()
                            .map(|model| model.id)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let models = presets::model_roster(
                    name,
                    base,
                    model_catalog::ModelSource::Configured,
                    &discovered,
                    base_url,
                )
                .into_iter()
                .map(|model| model.id)
                .collect();
                (
                    name.clone(),
                    SelectableProvider {
                        models,
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
        let provider = match provider {
            Some(name) => name,
            None => {
                let matches: Vec<_> = self
                    .providers
                    .iter()
                    .filter(|(_, p)| p.models.contains(model))
                    .map(|(name, _)| name.as_str())
                    .collect();
                match matches.as_slice() {
                    [] => {
                        return Err(ModelSelectionError::Unknown {
                            model: model.into(),
                            provider: None,
                        })
                    }
                    [name] => *name,
                    _ => {
                        return Err(ModelSelectionError::Ambiguous {
                            model: model.into(),
                            providers: matches.iter().map(|p| (*p).to_owned()).collect(),
                        })
                    }
                }
            }
        };
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
        if !entry.available || !entry.models.contains(model) {
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
