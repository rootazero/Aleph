//! Persisted Panel layout: order, visibility and the active tab.
//!
//! A layout stores only stable `PanelTabKey`s and plain data. It never stores
//! descriptors, entries or permissions, so restoring a layout can rearrange
//! what the current snapshot offers but can never make a tab mountable.

use aleph_protocol::panel::PanelTabKey;
use serde::{Deserialize, Serialize};

use super::registry::TabRegistry;

/// Current on-disk layout schema.
pub const LAYOUT_VERSION: u32 = 1;

const STORAGE_PREFIX: &str = "aleph.panel.layout.v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PanelLayout {
    pub version: u32,
    pub order: Vec<PanelTabKey>,
    pub hidden: Vec<PanelTabKey>,
    pub active: Option<PanelTabKey>,
}

/// One position in the restored tab strip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutSlot {
    /// The tab is enabled in the current snapshot and may be mounted.
    Live(PanelTabKey),
    /// The layout names a tab the snapshot no longer offers as enabled.
    /// Renderers show a placeholder and never execute it.
    Unavailable(PanelTabKey),
}

/// Why a stored layout was replaced by the default layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutRepair {
    Corrupt,
    UnknownVersion(u32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoredLayout {
    /// The layout to persist back; includes newly offered tabs.
    pub layout: PanelLayout,
    /// Visible tabs in display order.
    pub slots: Vec<LayoutSlot>,
    pub repair: Option<LayoutRepair>,
}

/// Storage key for one endpoint and, when the Gateway provides one, one
/// opaque subject. Without a subject every user of this device shares the
/// endpoint's layout.
pub fn layout_storage_key(endpoint: &str, subject: Option<&str>) -> String {
    match subject {
        Some(subject) => format!("{STORAGE_PREFIX}:{endpoint}:{subject}"),
        None => format!("{STORAGE_PREFIX}:{endpoint}"),
    }
}

impl PanelLayout {
    pub fn default_for(registry: &TabRegistry) -> Self {
        let order: Vec<PanelTabKey> = registry
            .ordered()
            .into_iter()
            .map(|descriptor| descriptor.key)
            .collect();
        Self {
            version: LAYOUT_VERSION,
            active: order.first().cloned(),
            order,
            hidden: Vec::new(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("layout holds only strings")
    }

    /// Swap `key` with its neighbour one step towards the front (`forward`)
    /// or the back of the order. Returns whether the order changed.
    pub fn move_one(&mut self, key: &PanelTabKey, forward: bool) -> bool {
        let Some(index) = self.order.iter().position(|candidate| candidate == key) else {
            return false;
        };
        let target = if forward {
            index.checked_sub(1)
        } else {
            Some(index + 1).filter(|next| *next < self.order.len())
        };
        match target {
            Some(target) => {
                self.order.swap(index, target);
                true
            }
            None => false,
        }
    }

    /// Hide or show a tab the layout already orders. Hiding the active tab
    /// clears `active`; `restore` then picks the first live slot.
    pub fn set_hidden(&mut self, key: &PanelTabKey, hidden: bool) -> bool {
        if !self.order.contains(key) || self.hidden.contains(key) == hidden {
            return false;
        }
        if hidden {
            self.hidden.push(key.clone());
            if self.active.as_ref() == Some(key) {
                self.active = None;
            }
        } else {
            self.hidden.retain(|candidate| candidate != key);
        }
        true
    }

    /// Record `key` as active. Only `restore` decides whether it is live.
    pub fn select(&mut self, key: &PanelTabKey) -> bool {
        if !self.order.contains(key) || self.hidden.contains(key) {
            return false;
        }
        self.active = Some(key.clone());
        true
    }
}

/// Restore a stored layout against the current registry. A damaged or
/// unknown layout resets to the default and reports why; it never touches
/// sessions, plugins or anything outside presentation.
pub fn restore(raw: Option<&str>, registry: &TabRegistry) -> RestoredLayout {
    let default = || PanelLayout::default_for(registry);
    let (stored, repair) = match raw.map(parse) {
        None => (default(), None),
        Some(Ok(layout)) => (layout, None),
        Some(Err(repair)) => (default(), Some(repair)),
    };

    let live: Vec<PanelTabKey> = registry
        .ordered()
        .into_iter()
        .map(|descriptor| descriptor.key)
        .collect();

    let mut order: Vec<PanelTabKey> = Vec::new();
    for key in stored.order.into_iter().chain(live.iter().cloned()) {
        if !order.contains(&key) {
            order.push(key);
        }
    }
    let mut hidden: Vec<PanelTabKey> = Vec::new();
    for key in stored.hidden {
        if !hidden.contains(&key) {
            hidden.push(key);
        }
    }

    let slots: Vec<LayoutSlot> = order
        .iter()
        .filter(|key| !hidden.contains(key))
        .map(|key| {
            if live.contains(key) {
                LayoutSlot::Live(key.clone())
            } else {
                LayoutSlot::Unavailable(key.clone())
            }
        })
        .collect();

    let first_live = || {
        slots.iter().find_map(|slot| match slot {
            LayoutSlot::Live(key) => Some(key.clone()),
            LayoutSlot::Unavailable(_) => None,
        })
    };
    let active = match stored.active {
        Some(key) if slots.contains(&LayoutSlot::Live(key.clone())) => Some(key),
        _ => first_live(),
    };

    RestoredLayout {
        layout: PanelLayout {
            version: LAYOUT_VERSION,
            order,
            hidden,
            active,
        },
        slots,
        repair,
    }
}

fn parse(raw: &str) -> Result<PanelLayout, LayoutRepair> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|_| LayoutRepair::Corrupt)?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or(LayoutRepair::Corrupt)?;
    if version != u64::from(LAYOUT_VERSION) {
        return Err(LayoutRepair::UnknownVersion(
            u32::try_from(version).unwrap_or(u32::MAX),
        ));
    }
    serde_json::from_value(value).map_err(|_| LayoutRepair::Corrupt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::panel::{
        PanelTabDescriptor, PanelTabStatus, PanelUiTrust, PANEL_UI_API_MAJOR,
    };

    fn key(plugin: &str, tab: &str) -> PanelTabKey {
        PanelTabKey {
            plugin_id: plugin.into(),
            tab_id: tab.into(),
        }
    }

    fn registry(tabs: &[(&str, &str, i32, PanelTabStatus)]) -> TabRegistry {
        let mut registry = TabRegistry::default();
        for (plugin, tab, order, status) in tabs {
            registry
                .insert_or_replace(PanelTabDescriptor {
                    key: key(plugin, tab),
                    label: (*tab).into(),
                    icon: "layout".into(),
                    order: *order,
                    entry: format!("tabs/{tab}.js"),
                    api_major: PANEL_UI_API_MAJOR,
                    trust: PanelUiTrust::Verified,
                    permissions: vec![],
                    status: status.clone(),
                    status_detail: None,
                })
                .unwrap();
        }
        registry
    }

    fn three() -> TabRegistry {
        registry(&[
            ("p", "a", 0, PanelTabStatus::Enabled),
            ("p", "b", 1, PanelTabStatus::Enabled),
            ("p", "c", 2, PanelTabStatus::Enabled),
        ])
    }

    fn stored(
        order: &[PanelTabKey],
        hidden: &[PanelTabKey],
        active: Option<PanelTabKey>,
    ) -> String {
        PanelLayout {
            version: LAYOUT_VERSION,
            order: order.to_vec(),
            hidden: hidden.to_vec(),
            active,
        }
        .to_json()
    }

    #[test]
    fn restore_none_gives_default_order() {
        let restored = restore(None, &three());
        assert_eq!(restored.repair, None);
        assert_eq!(restored.layout, PanelLayout::default_for(&three()));
        assert_eq!(
            restored.slots,
            vec![
                LayoutSlot::Live(key("p", "a")),
                LayoutSlot::Live(key("p", "b")),
                LayoutSlot::Live(key("p", "c")),
            ]
        );
        assert_eq!(restored.layout.active, Some(key("p", "a")));
    }

    #[test]
    fn restore_corrupt_json_resets_with_repair_corrupt() {
        for raw in ["{", "[]", r#"{"version":1,"order":"x"}"#, r#"{"order":[]}"#] {
            let restored = restore(Some(raw), &three());
            assert_eq!(restored.repair, Some(LayoutRepair::Corrupt), "{raw}");
            assert_eq!(restored.layout, PanelLayout::default_for(&three()));
        }
    }

    #[test]
    fn restore_unknown_version_resets_with_repair() {
        let raw = r#"{"version":2,"order":[],"hidden":[],"active":null}"#;
        let restored = restore(Some(raw), &three());
        assert_eq!(restored.repair, Some(LayoutRepair::UnknownVersion(2)));
        assert_eq!(restored.layout, PanelLayout::default_for(&three()));
    }

    #[test]
    fn restore_keeps_missing_key_as_unavailable() {
        let raw = stored(
            &[key("gone", "x"), key("p", "b")],
            &[],
            Some(key("gone", "x")),
        );
        let restored = restore(Some(&raw), &three());
        assert_eq!(restored.slots[0], LayoutSlot::Unavailable(key("gone", "x")));
        assert_eq!(restored.slots[1], LayoutSlot::Live(key("p", "b")));
        // The placeholder can never become active.
        assert_eq!(restored.layout.active, Some(key("p", "b")));
        assert!(restored.layout.order.contains(&key("gone", "x")));
    }

    #[test]
    fn restore_treats_disabled_tab_as_unavailable() {
        let registry = registry(&[
            ("p", "a", 0, PanelTabStatus::Enabled),
            ("p", "b", 1, PanelTabStatus::Disabled),
        ]);
        let raw = stored(&[key("p", "b"), key("p", "a")], &[], Some(key("p", "b")));
        let restored = restore(Some(&raw), &registry);
        assert_eq!(
            restored.slots,
            vec![
                LayoutSlot::Unavailable(key("p", "b")),
                LayoutSlot::Live(key("p", "a")),
            ]
        );
        assert_eq!(restored.layout.active, Some(key("p", "a")));
    }

    #[test]
    fn restore_appends_new_keys_at_end() {
        let raw = stored(&[key("p", "c"), key("p", "a")], &[], Some(key("p", "c")));
        let restored = restore(Some(&raw), &three());
        assert_eq!(
            restored.layout.order,
            vec![key("p", "c"), key("p", "a"), key("p", "b")]
        );
        assert_eq!(restored.layout.active, Some(key("p", "c")));
    }

    #[test]
    fn hidden_keys_are_not_live_slots() {
        let raw = stored(&[], &[key("p", "b")], None);
        let restored = restore(Some(&raw), &three());
        assert_eq!(
            restored.slots,
            vec![
                LayoutSlot::Live(key("p", "a")),
                LayoutSlot::Live(key("p", "c"))
            ]
        );
        assert_eq!(restored.layout.hidden, vec![key("p", "b")]);
    }

    #[test]
    fn active_pointing_at_hidden_or_missing_falls_back_to_first_live() {
        for active in [key("p", "b"), key("gone", "x")] {
            let raw = stored(&[], &[key("p", "b")], Some(active));
            let restored = restore(Some(&raw), &three());
            assert_eq!(restored.layout.active, Some(key("p", "a")));
        }
        let everything_hidden = stored(&[], &[key("p", "a"), key("p", "b"), key("p", "c")], None);
        assert_eq!(
            restore(Some(&everything_hidden), &three()).layout.active,
            None
        );
    }

    #[test]
    fn duplicate_keys_collapse() {
        let raw = stored(
            &[key("p", "a"), key("p", "a")],
            &[key("p", "b"), key("p", "b")],
            None,
        );
        let restored = restore(Some(&raw), &three());
        assert_eq!(
            restored.layout.order,
            vec![key("p", "a"), key("p", "b"), key("p", "c")]
        );
        assert_eq!(restored.layout.hidden, vec![key("p", "b")]);
    }

    #[test]
    fn storage_key_formats() {
        assert_eq!(
            layout_storage_key("wss://h:1", Some("ab")),
            "aleph.panel.layout.v1:wss://h:1:ab"
        );
        assert_eq!(
            layout_storage_key("wss://h:1", None),
            "aleph.panel.layout.v1:wss://h:1"
        );
    }

    #[test]
    fn move_one_swaps_neighbours_and_stops_at_the_ends() {
        let mut layout = PanelLayout::default_for(&three());
        assert!(!layout.move_one(&key("p", "a"), true));
        assert!(layout.move_one(&key("p", "a"), false));
        assert_eq!(
            layout.order,
            vec![key("p", "b"), key("p", "a"), key("p", "c")]
        );
        assert!(layout.move_one(&key("p", "c"), true));
        assert_eq!(
            layout.order,
            vec![key("p", "b"), key("p", "c"), key("p", "a")]
        );
        assert!(!layout.move_one(&key("p", "a"), false));
        assert!(!layout.move_one(&key("p", "missing"), false));
    }

    #[test]
    fn hiding_the_active_tab_hands_activity_to_the_first_live_slot() {
        let mut layout = PanelLayout::default_for(&three());
        assert!(layout.set_hidden(&key("p", "a"), true));
        assert!(!layout.set_hidden(&key("p", "a"), true));
        assert_eq!(layout.active, None);
        assert!(!layout.select(&key("p", "a")));

        let restored = restore(Some(&layout.to_json()), &three());
        assert_eq!(restored.layout.active, Some(key("p", "b")));
        assert!(!restored.slots.contains(&LayoutSlot::Live(key("p", "a"))));

        let mut layout = restored.layout;
        assert!(layout.set_hidden(&key("p", "a"), false));
        assert!(layout.select(&key("p", "a")));
        let restored = restore(Some(&layout.to_json()), &three());
        assert_eq!(restored.layout.active, Some(key("p", "a")));
    }

    #[test]
    fn edits_ignore_keys_the_layout_does_not_order() {
        let mut layout = PanelLayout::default_for(&three());
        let before = layout.clone();
        assert!(!layout.set_hidden(&key("p", "missing"), true));
        assert!(!layout.select(&key("p", "missing")));
        assert_eq!(layout, before);
    }
}
