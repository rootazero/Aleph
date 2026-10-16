//! Pure visibility inputs and the object-safe slot rendering boundary.

use std::collections::BTreeSet;

use leptos::prelude::AnyView;

use crate::i18n::Locale;
use crate::state::layout::LayoutMode;

/// An owned, read-only snapshot supplied by the host for one visibility pass.
///
/// The host must advance `visible_version` whenever an input changes. These
/// values are projections of existing state, never another state store. Run ids
/// use the existing Panel wire representation (`String`).
#[derive(Clone, Default)]
pub struct SlotCtx {
    pub locale: Locale,
    pub mode: LayoutMode,
    pub is_connected: bool,
    pub active_run_id: Option<String>,
    pub visible_version: u64,
    pub user_opened: BTreeSet<&'static str>,
    pub user_closed: BTreeSet<&'static str>,
    pub capability_revision_loaded: Option<u64>,
    pub capability_fetch_attempted: bool,
    pub has_browser_binding: bool,
    pub has_pending_approvals: bool,
}

impl SlotCtx {
    /// User-open wins over user-close, which wins over the slot's default.
    pub fn effective_open(&self, slot: &dyn RightPaneSlot) -> bool {
        if self.user_opened.contains(slot.id()) {
            return true;
        }
        if self.user_closed.contains(slot.id()) {
            return false;
        }
        slot.default_open()
    }
}

/// Rendering receives its own snapshot, separate from the pure visibility API.
///
/// Placeholder bodies need no services. Later slots add their existing service
/// and lifecycle-scoped subscription adapters here, rather than in the registry.
#[derive(Clone, Default)]
pub struct SlotRenderCtx {
    pub snapshot: SlotCtx,
}

/// A stable slot descriptor with no workspace, persistence or subscription cache.
///
/// `id`, `label`, `icon`, `order`, and `default_open` must be immutable. Labels
/// and optional icons are resource keys, not translated text. `visible` must be
/// pure. Rendering must work even for a hidden slot; effects and subscriptions
/// belong to the mounted Leptos owner and must end when that owner unmounts.
pub trait RightPaneSlot {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn icon(&self) -> Option<&'static str>;
    fn order(&self) -> u16;
    fn visible(&self, ctx: &SlotCtx) -> bool;
    fn default_open(&self) -> bool;
    fn render(&self, ctx: SlotRenderCtx) -> AnyView;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::right_pane::stage_one_registry;

    #[test]
    fn effective_open_three_way_decision() {
        let registry = stage_one_registry();
        let open = registry.get("rps.subagent_tree").unwrap();
        let closed = registry.get("rps.browser").unwrap();
        let mut ctx = SlotCtx::default();
        assert!(ctx.effective_open(open.as_ref()));
        assert!(!ctx.effective_open(closed.as_ref()));
        ctx.user_closed.insert(open.id());
        assert!(!ctx.effective_open(open.as_ref()));
        ctx.user_opened.insert(open.id());
        assert!(ctx.effective_open(open.as_ref()));
        ctx.user_opened.insert(closed.id());
        assert!(ctx.effective_open(closed.as_ref()));
    }

    #[test]
    fn placeholder_render_accepts_empty_context_even_when_hidden() {
        let registry = stage_one_registry();
        for id in [
            "rps.subagent_tree",
            "rps.artifacts",
            "rps.capabilities",
            "rps.browser",
            "rps.approvals",
        ] {
            let slot = registry.get(id).unwrap();
            let _view = slot.render(SlotRenderCtx::default());
        }
    }
}
