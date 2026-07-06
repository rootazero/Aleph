//! Shared test helpers for the `deps_builder` submodules.

#[cfg(test)]
use crate::config::types::{FallbackProviderToml, ProviderConfig};
#[cfg(test)]
use crate::config::Config;

#[cfg(test)]
pub(crate) fn cfg_with_fallback(
    fb: Option<FallbackProviderToml>,
    providers: Vec<(&str, ProviderConfig)>,
) -> Config {
    let mut providers_map: std::collections::HashMap<String, ProviderConfig> =
        std::collections::HashMap::new();
    for (k, v) in providers {
        providers_map.insert(k.to_string(), v);
    }
    Config {
        fallback_provider: fb,
        providers: providers_map,
        ..Config::default()
    }
}
