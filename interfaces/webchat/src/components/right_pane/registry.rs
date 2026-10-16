//! Construction-only registration and read-only catalog queries.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use super::{RightPaneSlot, SlotCtx};

/// The closed set of registration failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    DuplicateSlotId(&'static str),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSlotId(id) => write!(f, "duplicate right-pane slot id: {id}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// The sole catalog of slot descriptors, sorted at query time.
///
/// Register during construction, then share via `Arc<RightPaneRegistry>`. There
/// is no interior mutability, serialized state, layout state or subscription
/// cache here. Runtime UI decisions belong to `WorkspaceState`.
#[derive(Default)]
pub struct RightPaneRegistry {
    entries: BTreeMap<&'static str, Arc<dyn RightPaneSlot>>,
}

impl RightPaneRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, slot: Arc<dyn RightPaneSlot>) -> Result<(), RegistryError> {
        match self.entries.entry(slot.id()) {
            std::collections::btree_map::Entry::Occupied(entry) => {
                Err(RegistryError::DuplicateSlotId(entry.key()))
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(slot);
                Ok(())
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn RightPaneSlot>> {
        self.entries.get(id).cloned()
    }

    pub fn iter_visible(&self, ctx: &SlotCtx) -> impl Iterator<Item = Arc<dyn RightPaneSlot>> {
        let mut visible: Vec<_> = self
            .entries
            .values()
            .filter(|slot| slot.visible(ctx))
            .cloned()
            .collect();
        visible.sort_by_key(|slot| (slot.order(), slot.id()));
        visible.into_iter()
    }

    pub fn default_open(&self) -> BTreeSet<&'static str> {
        self.entries
            .values()
            .filter(|slot| slot.default_open())
            .map(|slot| slot.id())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use leptos::prelude::*;

    use super::*;

    struct TestSlot {
        id: &'static str,
        order: u16,
        default_open: bool,
        needs_connection: bool,
    }

    impl RightPaneSlot for TestSlot {
        fn id(&self) -> &'static str {
            self.id
        }
        fn label(&self) -> &'static str {
            "slot.test"
        }
        fn icon(&self) -> Option<&'static str> {
            None
        }
        fn order(&self) -> u16 {
            self.order
        }
        fn visible(&self, ctx: &SlotCtx) -> bool {
            !self.needs_connection || ctx.is_connected
        }
        fn default_open(&self) -> bool {
            self.default_open
        }
        fn render(&self, _ctx: super::super::SlotRenderCtx) -> AnyView {
            ().into_any()
        }
    }

    fn slot(id: &'static str, order: u16, default_open: bool) -> Arc<dyn RightPaneSlot> {
        Arc::new(TestSlot {
            id,
            order,
            default_open,
            needs_connection: false,
        })
    }

    #[test]
    fn duplicate_registration_keeps_the_original_descriptor_and_defaults() {
        let mut registry = RightPaneRegistry::new();
        let original = slot("rps.test", 100, true);
        registry.register(Arc::clone(&original)).unwrap();
        assert_eq!(
            registry.register(slot("rps.test", 0, false)),
            Err(RegistryError::DuplicateSlotId("rps.test"))
        );
        assert!(Arc::ptr_eq(&original, &registry.get("rps.test").unwrap()));
        assert_eq!(registry.default_open(), BTreeSet::from(["rps.test"]));
        assert_eq!(registry.iter_visible(&SlotCtx::default()).count(), 1);
    }

    #[test]
    fn iteration_sorts_by_order_then_stable_id_not_registration_order() {
        let mut registry = RightPaneRegistry::new();
        for entry in [
            slot("rps.a_last", u16::MAX, false),
            slot("rps.z_tie", 100, false),
            slot("rps.b_tie", 100, false),
            slot("rps.z_first", 0, false),
        ] {
            registry.register(entry).unwrap();
        }
        let ids: Vec<_> = registry
            .iter_visible(&SlotCtx::default())
            .map(|s| s.id())
            .collect();
        assert_eq!(ids, ["rps.z_first", "rps.b_tie", "rps.z_tie", "rps.a_last"]);
    }

    #[test]
    fn visibility_is_recomputed_from_each_snapshot_without_removing_entries() {
        let mut registry = RightPaneRegistry::new();
        registry.register(slot("rps.always", 100, false)).unwrap();
        registry
            .register(Arc::new(TestSlot {
                id: "rps.connected",
                order: 200,
                default_open: true,
                needs_connection: true,
            }))
            .unwrap();
        let mut ctx = SlotCtx::default();
        assert_eq!(registry.iter_visible(&ctx).count(), 1);
        assert_eq!(registry.iter_visible(&ctx).count(), 1);
        assert!(registry.get("rps.connected").is_some());
        ctx.is_connected = true;
        ctx.visible_version += 1;
        assert_eq!(registry.iter_visible(&ctx).count(), 2);
        ctx.is_connected = false;
        ctx.visible_version += 1;
        assert_eq!(registry.iter_visible(&ctx).count(), 1);
    }

    #[test]
    fn defaults_include_hidden_slots_and_return_an_independent_set() {
        let mut registry = RightPaneRegistry::new();
        registry.register(slot("rps.closed", 100, false)).unwrap();
        registry
            .register(Arc::new(TestSlot {
                id: "rps.hidden",
                order: 200,
                default_open: true,
                needs_connection: true,
            }))
            .unwrap();
        assert_eq!(registry.iter_visible(&SlotCtx::default()).count(), 1);
        let mut defaults = registry.default_open();
        assert_eq!(defaults, BTreeSet::from(["rps.hidden"]));
        defaults.clear();
        defaults.insert("rps.foreign");
        assert_eq!(registry.default_open(), BTreeSet::from(["rps.hidden"]));
    }

    #[test]
    fn shared_lookups_keep_identity_without_mutating_the_catalog() {
        let mut registry = RightPaneRegistry::new();
        let original = slot("rps.test", 100, false);
        registry.register(Arc::clone(&original)).unwrap();
        let shared = Arc::new(registry);
        let first = shared.get("rps.test").unwrap();
        let second = Arc::clone(&shared).get("rps.test").unwrap();
        assert!(Arc::ptr_eq(&original, &first));
        assert!(Arc::ptr_eq(&first, &second));
        drop(first);
        drop(second);
        assert!(shared.get("rps.missing").is_none());
        assert!(shared.get("RPS.TEST").is_none());
        assert_eq!(shared.iter_visible(&SlotCtx::default()).count(), 1);
        assert!(shared.default_open().is_empty());
    }

    #[test]
    fn empty_registry_has_no_lookup_iteration_or_defaults() {
        let registry = RightPaneRegistry::new();
        assert!(registry.get("rps.missing").is_none());
        assert_eq!(registry.iter_visible(&SlotCtx::default()).count(), 0);
        assert!(registry.default_open().is_empty());
    }
}
