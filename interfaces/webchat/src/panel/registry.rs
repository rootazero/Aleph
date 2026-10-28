//! Pure tab membership and enabled mount projections; no loading or UI effects.

use aleph_protocol::panel::{PanelTabDescriptor, PanelTabKey, PanelTabStatus, PANEL_UI_API_MAJOR};

/// Only the host's explicit built-in initialization may populate this namespace.
pub const BUILTIN_NAMESPACE: &str = "aleph";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryError {
    BuiltInShadowed,
    DuplicateKey,
    InvalidEntry,
}

/// Small, key-indexed registry. Linear lookup uses the protocol key's equality
/// without requiring Hash/Ord on the shared wire contract or a second key format.
#[derive(Clone, Debug, Default)]
pub struct TabRegistry {
    entries: Vec<PanelTabDescriptor>,
    asset_integrities: Vec<Option<String>>,
    source_revision: u64,
}

impl TabRegistry {
    /// Initialize host-owned built-ins, rejecting duplicates rather than overwriting.
    /// This is a host boundary, not an entry point for untrusted plugin descriptors.
    pub fn new(builtins: Vec<PanelTabDescriptor>) -> Result<Self, RegistryError> {
        let mut registry = Self::default();
        for descriptor in builtins {
            registry.insert_builtin(descriptor)?;
        }
        Ok(registry)
    }

    pub(crate) fn insert_builtin(
        &mut self,
        descriptor: PanelTabDescriptor,
    ) -> Result<(), RegistryError> {
        validate_descriptor(&descriptor)?;
        if descriptor.key.plugin_id != BUILTIN_NAMESPACE {
            return Err(RegistryError::InvalidEntry);
        }
        if self.get(&descriptor.key).is_some() {
            return Err(RegistryError::DuplicateKey);
        }
        self.entries.push(descriptor);
        self.asset_integrities.push(None);
        Ok(())
    }

    pub fn source_revision(&self) -> u64 {
        self.source_revision
    }

    pub(crate) fn set_source_revision(&mut self, revision: u64) {
        self.source_revision = revision;
    }

    pub(crate) fn set_asset_integrity(&mut self, key: &PanelTabKey, integrity: Option<String>) {
        if let Some(index) = self.entries.iter().position(|entry| &entry.key == key) {
            self.asset_integrities[index] = integrity;
        }
    }

    pub(crate) fn asset_integrity(&self, key: &PanelTabKey) -> Option<&str> {
        self.entries
            .iter()
            .position(|entry| &entry.key == key)
            .and_then(|index| self.asset_integrities[index].as_deref())
    }

    /// Explicit plugin registration/replacement; reserved built-ins cannot be shadowed.
    pub fn insert_or_replace(
        &mut self,
        descriptor: PanelTabDescriptor,
    ) -> Result<(), RegistryError> {
        validate_descriptor(&descriptor)?;
        if descriptor.key.plugin_id == BUILTIN_NAMESPACE {
            return Err(RegistryError::BuiltInShadowed);
        }
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.key == descriptor.key)
        {
            self.entries[index] = descriptor;
            self.asset_integrities[index] = None;
            return Ok(());
        }
        self.entries.push(descriptor);
        self.asset_integrities.push(None);
        Ok(())
    }

    pub fn remove(&mut self, key: &PanelTabKey) -> bool {
        if let Some(index) = self.entries.iter().position(|entry| &entry.key == key) {
            self.entries.remove(index);
            self.asset_integrities.remove(index);
            return true;
        }
        false
    }

    pub fn get(&self, key: &PanelTabKey) -> Option<&PanelTabDescriptor> {
        self.entries.iter().find(|entry| &entry.key == key)
    }

    /// Status rows remain queryable; `ordered` keeps only entries that may be mounted.
    pub fn rows(&self) -> Vec<PanelTabDescriptor> {
        let mut entries = self.entries.clone();
        entries.sort_by_cached_key(|entry| (entry.order, entry.key.qualified()));
        entries
    }

    /// Only enabled entries may be mounted.
    pub fn ordered(&self) -> Vec<PanelTabDescriptor> {
        self.rows()
            .into_iter()
            .filter(|entry| entry.status == PanelTabStatus::Enabled)
            .collect()
    }

    /// Select the first enabled entry in projection order, excluding the removed key.
    /// Works both before and after removal, including when no enabled entry remains.
    pub fn active_after_removal(&self, removed: &PanelTabKey) -> Option<PanelTabKey> {
        self.ordered()
            .into_iter()
            .find(|entry| &entry.key != removed)
            .map(|entry| entry.key)
    }
}

pub(crate) fn validate_descriptor(descriptor: &PanelTabDescriptor) -> Result<(), RegistryError> {
    if [&descriptor.key.plugin_id, &descriptor.key.tab_id, &descriptor.label, &descriptor.entry]
        .iter().any(|value| value.trim().is_empty())
        // The protocol's qualified display key uses a literal colon separator.
        || descriptor.key.plugin_id.contains(':')
        || descriptor.key.tab_id.contains(':')
        || descriptor.entry.starts_with('/')
        || descriptor.entry.contains('\\')
        || descriptor.entry.contains(':')
        || descriptor.entry.split('/').any(|segment| segment == "..")
        || descriptor.api_major != PANEL_UI_API_MAJOR
    {
        return Err(RegistryError::InvalidEntry);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{RegistryError, TabRegistry};
    use aleph_protocol::panel::{
        PanelTabDescriptor, PanelTabKey, PanelTabStatus, PanelUiTrust, PANEL_UI_API_MAJOR,
    };

    fn descriptor(plugin_id: &str, tab_id: &str, order: i32) -> PanelTabDescriptor {
        PanelTabDescriptor {
            key: PanelTabKey {
                plugin_id: plugin_id.into(),
                tab_id: tab_id.into(),
            },
            label: "Overview".into(),
            icon: "layout".into(),
            order,
            entry: "tabs/overview.js".into(),
            api_major: PANEL_UI_API_MAJOR,
            trust: PanelUiTrust::Verified,
            permissions: vec![],
            status: PanelTabStatus::Enabled,
            status_detail: None,
        }
    }

    #[test]
    fn ordered_uses_order_then_qualified_key() {
        for entries in [
            vec![
                ("z", "overview", 0),
                ("a", "z", 0),
                ("a", "a", 0),
                ("b", "first", -1),
            ],
            vec![
                ("b", "first", -1),
                ("a", "a", 0),
                ("a", "z", 0),
                ("z", "overview", 0),
            ],
        ] {
            let mut registry = TabRegistry::default();
            for (plugin, tab, order) in entries {
                registry
                    .insert_or_replace(descriptor(plugin, tab, order))
                    .unwrap();
            }
            assert_eq!(
                registry
                    .ordered()
                    .iter()
                    .map(|row| row.key.qualified())
                    .collect::<Vec<_>>(),
                ["b:first", "a:a", "a:z", "z:overview"]
            );
        }
    }

    #[test]
    fn plugin_cannot_shadow_reserved_builtin() {
        let builtin = descriptor("aleph", "terminal", 0);
        let mut registry = TabRegistry::new(vec![builtin.clone()]).unwrap();
        let mut shadow = builtin.clone();
        shadow.label = "Imposter".into();
        assert_eq!(
            registry.insert_or_replace(shadow),
            Err(RegistryError::BuiltInShadowed)
        );
        assert_eq!(registry.get(&builtin.key), Some(&builtin));
        assert_eq!(
            TabRegistry::default().insert_or_replace(descriptor("aleph", "new", 0)),
            Err(RegistryError::BuiltInShadowed)
        );
    }

    #[test]
    fn replacement_is_explicit_and_preserves_one_entry() {
        let builtin = descriptor("aleph", "terminal", 0);
        assert!(matches!(
            TabRegistry::new(vec![builtin.clone(), builtin]),
            Err(RegistryError::DuplicateKey)
        ));
        let mut registry = TabRegistry::default();
        let original = descriptor("plugin", "overview", 0);
        registry.insert_or_replace(original.clone()).unwrap();
        registry.set_asset_integrity(
            &original.key,
            Some("sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into()),
        );
        assert_eq!(
            registry.asset_integrity(&original.key),
            Some("sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
        );
        let replacement = PanelTabDescriptor {
            label: "Replaced".into(),
            order: -1,
            ..original
        };
        registry.insert_or_replace(replacement.clone()).unwrap();
        assert_eq!(registry.get(&replacement.key), Some(&replacement));
        assert_eq!(registry.asset_integrity(&replacement.key), None);
        assert_eq!(registry.ordered(), vec![replacement.clone()]);
        assert!(registry.remove(&replacement.key));
        assert!(!registry.remove(&replacement.key));
        assert_eq!(registry.get(&replacement.key), None);
        assert_eq!(registry.asset_integrity(&replacement.key), None);
    }

    #[test]
    fn disabled_entries_are_not_ordered_for_mount() {
        let mut registry = TabRegistry::default();
        let enabled = descriptor("plugin", "enabled", 0);
        registry.insert_or_replace(enabled.clone()).unwrap();
        for status in [
            PanelTabStatus::Disabled,
            PanelTabStatus::Incompatible,
            PanelTabStatus::Failed,
        ] {
            let row = PanelTabDescriptor {
                status,
                ..descriptor("plugin", "hidden", -10)
            };
            registry.insert_or_replace(row.clone()).unwrap();
            assert_eq!(registry.get(&row.key), Some(&row));
            assert_eq!(registry.ordered(), vec![enabled.clone()]);
            assert_eq!(registry.active_after_removal(&enabled.key), None);
        }
        registry
            .insert_or_replace(descriptor("plugin", "hidden", -10))
            .unwrap();
        assert_eq!(registry.ordered().len(), 2);
    }

    #[test]
    fn active_after_removal_falls_back_to_first_enabled_tab() {
        let builtin = descriptor("aleph", "home", -100);
        let mut registry = TabRegistry::new(vec![builtin.clone()]).unwrap();
        let active = descriptor("plugin", "active", -200);
        registry.insert_or_replace(active.clone()).unwrap();
        registry
            .insert_or_replace(descriptor("plugin", "other", 0))
            .unwrap();
        // Also exclude the removed key before the caller mutates the registry.
        assert_eq!(
            registry.active_after_removal(&active.key),
            Some(builtin.key.clone())
        );
        assert!(registry.remove(&active.key));
        assert_eq!(
            registry.active_after_removal(&active.key),
            Some(builtin.key.clone())
        );
        assert!(registry.remove(&builtin.key));
        let other = descriptor("plugin", "other", 0).key;
        assert_eq!(
            registry.active_after_removal(&builtin.key),
            Some(other.clone())
        );
        assert!(registry.remove(&other));
        assert_eq!(registry.active_after_removal(&other), None);
    }

    #[test]
    fn invalid_entries_do_not_insert_or_replace() {
        let original = descriptor("plugin", "overview", 0);
        let mut registry = TabRegistry::default();
        registry.insert_or_replace(original.clone()).unwrap();
        let mut invalid = vec![];
        for blank in ["", "  "] {
            invalid.push(PanelTabDescriptor {
                key: PanelTabKey {
                    plugin_id: blank.into(),
                    ..original.key.clone()
                },
                ..original.clone()
            });
            invalid.push(PanelTabDescriptor {
                key: PanelTabKey {
                    tab_id: blank.into(),
                    ..original.key.clone()
                },
                ..original.clone()
            });
            invalid.push(PanelTabDescriptor {
                label: blank.into(),
                ..original.clone()
            });
            invalid.push(PanelTabDescriptor {
                entry: blank.into(),
                ..original.clone()
            });
        }
        for major in [0, PANEL_UI_API_MAJOR + 1] {
            invalid.push(PanelTabDescriptor {
                api_major: major,
                ..original.clone()
            });
        }
        invalid.push(descriptor("a:b", "c", 0));
        invalid.push(descriptor("a", "b:c", 0));
        for row in invalid {
            assert_eq!(
                registry.insert_or_replace(row),
                Err(RegistryError::InvalidEntry)
            );
            assert_eq!(registry.ordered(), vec![original.clone()]);
        }
        assert!(matches!(
            TabRegistry::new(vec![original]),
            Err(RegistryError::InvalidEntry)
        ));
        let mut invalid_builtin = descriptor("aleph", "home", 0);
        invalid_builtin.entry.clear();
        assert!(matches!(
            TabRegistry::new(vec![invalid_builtin]),
            Err(RegistryError::InvalidEntry)
        ));
    }
}
