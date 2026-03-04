//! Persona registry — manages preset + runtime personas.
//!
//! The registry loads persona definitions from configuration and provides
//! resolution of [`PersonaSource`] references into concrete [`Persona`] instances.

use std::collections::HashMap;

use crate::config::types::PersonaConfig;

use super::protocol::{GroupChatError, Persona, PersonaSource};

/// Registry that manages preset personas loaded from configuration
/// and resolves [`PersonaSource`] references into concrete [`Persona`] instances.
pub struct PersonaRegistry {
    presets: HashMap<String, Persona>,
}

impl PersonaRegistry {
    /// Build a registry from a slice of [`PersonaConfig`].
    ///
    /// Each config is converted to a [`Persona`] and indexed by its `id`.
    /// If duplicate IDs exist, the last one wins.
    pub fn from_configs(configs: &[PersonaConfig]) -> Self {
        let presets = configs
            .iter()
            .map(|cfg| (cfg.id.clone(), persona_from_config(cfg)))
            .collect();
        Self { presets }
    }

    /// Look up a preset persona by ID.
    pub fn get(&self, id: &str) -> Option<&Persona> {
        self.presets.get(id)
    }

    /// Returns the number of preset personas in the registry.
    pub fn len(&self) -> usize {
        self.presets.len()
    }

    /// Returns `true` if the registry contains no preset personas.
    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }

    /// Resolve a list of [`PersonaSource`] references into concrete [`Persona`] instances.
    ///
    /// - `Preset(id)` — looked up in the registry; returns [`GroupChatError::PersonaNotFound`]
    ///   if the ID is not registered.
    /// - `Inline(persona)` — used directly as-is.
    pub fn resolve(&self, sources: &[PersonaSource]) -> Result<Vec<Persona>, GroupChatError> {
        sources
            .iter()
            .map(|source| match source {
                PersonaSource::Preset(id) => self
                    .get(id)
                    .cloned()
                    .ok_or_else(|| GroupChatError::PersonaNotFound(id.clone())),
                PersonaSource::Inline(persona) => Ok(persona.clone()),
            })
            .collect()
    }

    /// Clear the registry and rebuild from new configs (e.g., after hot-reload).
    pub fn reload(&mut self, configs: &[PersonaConfig]) {
        self.presets.clear();
        for cfg in configs {
            self.presets
                .insert(cfg.id.clone(), persona_from_config(cfg));
        }
    }

    /// Return all preset personas in the registry.
    ///
    /// The order is not guaranteed (HashMap iteration order).
    pub fn list_presets(&self) -> Vec<&Persona> {
        self.presets.values().collect()
    }
}

/// Convert a [`PersonaConfig`] into a [`Persona`].
fn persona_from_config(cfg: &PersonaConfig) -> Persona {
    Persona {
        id: cfg.id.clone(),
        name: cfg.name.clone(),
        system_prompt: cfg.system_prompt.clone(),
        provider: cfg.provider.clone(),
        model: cfg.model.clone(),
        thinking_level: cfg.thinking_level.clone(),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_persona_configs() -> Vec<PersonaConfig> {
        vec![
            PersonaConfig {
                id: "architect".into(),
                name: "架构师".into(),
                system_prompt: "You are an architect".into(),
                provider: Some("claude".into()),
                model: Some("claude-sonnet-4-20250514".into()),
                thinking_level: None,
            },
            PersonaConfig {
                id: "pm".into(),
                name: "产品经理".into(),
                system_prompt: "You are a product manager".into(),
                provider: None,
                model: None,
                thinking_level: None,
            },
        ]
    }

    #[test]
    fn test_load_from_configs() {
        let configs = sample_persona_configs();
        let registry = PersonaRegistry::from_configs(&configs);

        assert_eq!(registry.len(), 2);
        assert!(!registry.is_empty());

        let arch = registry.get("architect").expect("architect should exist");
        assert_eq!(arch.id, "architect");
        assert_eq!(arch.name, "架构师");
        assert_eq!(arch.system_prompt, "You are an architect");
        assert_eq!(arch.provider.as_deref(), Some("claude"));
        assert_eq!(arch.model.as_deref(), Some("claude-sonnet-4-20250514"));

        let pm = registry.get("pm").expect("pm should exist");
        assert_eq!(pm.id, "pm");
        assert_eq!(pm.name, "产品经理");
        assert!(pm.provider.is_none());
        assert!(pm.model.is_none());

        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_resolve_preset() {
        let configs = sample_persona_configs();
        let registry = PersonaRegistry::from_configs(&configs);

        let sources = vec![PersonaSource::Preset("architect".into())];
        let resolved = registry.resolve(&sources).expect("resolve should succeed");

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].id, "architect");
        assert_eq!(resolved[0].name, "架构师");
    }

    #[test]
    fn test_resolve_inline() {
        let registry = PersonaRegistry::from_configs(&[]);

        let inline_persona = Persona {
            id: "custom".into(),
            name: "Custom Expert".into(),
            system_prompt: "You are a custom expert".into(),
            provider: None,
            model: None,
            thinking_level: Some("high".into()),
        };
        let sources = vec![PersonaSource::Inline(inline_persona)];
        let resolved = registry.resolve(&sources).expect("resolve should succeed");

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].id, "custom");
        assert_eq!(resolved[0].name, "Custom Expert");
        assert_eq!(resolved[0].thinking_level.as_deref(), Some("high"));
    }

    #[test]
    fn test_resolve_preset_not_found() {
        let registry = PersonaRegistry::from_configs(&[]);

        let sources = vec![PersonaSource::Preset("nonexistent".into())];
        let result = registry.resolve(&sources);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, GroupChatError::PersonaNotFound(ref id) if id == "nonexistent"),
            "expected PersonaNotFound, got: {err:?}"
        );
    }

    #[test]
    fn test_resolve_mixed() {
        let configs = sample_persona_configs();
        let registry = PersonaRegistry::from_configs(&configs);

        let inline_persona = Persona {
            id: "reviewer".into(),
            name: "Code Reviewer".into(),
            system_prompt: "You review code".into(),
            provider: None,
            model: None,
            thinking_level: None,
        };

        let sources = vec![
            PersonaSource::Preset("pm".into()),
            PersonaSource::Inline(inline_persona),
            PersonaSource::Preset("architect".into()),
        ];
        let resolved = registry.resolve(&sources).expect("resolve should succeed");

        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0].id, "pm");
        assert_eq!(resolved[1].id, "reviewer");
        assert_eq!(resolved[2].id, "architect");
    }

    #[test]
    fn test_reload() {
        let configs = sample_persona_configs();
        let mut registry = PersonaRegistry::from_configs(&configs);
        assert_eq!(registry.len(), 2);
        assert!(registry.get("architect").is_some());

        // Reload with a different set
        let new_configs = vec![PersonaConfig {
            id: "designer".into(),
            name: "设计师".into(),
            system_prompt: "You are a designer".into(),
            provider: None,
            model: None,
            thinking_level: None,
        }];
        registry.reload(&new_configs);

        assert_eq!(registry.len(), 1);
        assert!(registry.get("architect").is_none(), "old preset should be gone");
        assert!(registry.get("designer").is_some(), "new preset should exist");
    }

    #[test]
    fn test_list_presets() {
        let configs = sample_persona_configs();
        let registry = PersonaRegistry::from_configs(&configs);

        let presets = registry.list_presets();
        assert_eq!(presets.len(), 2);

        let ids: Vec<&str> = {
            let mut v: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();
            v.sort();
            v
        };
        assert_eq!(ids, vec!["architect", "pm"]);
    }
}
