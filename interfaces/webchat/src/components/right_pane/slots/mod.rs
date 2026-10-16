//! Stage-one placeholders only. Named bodies and service adapters arrive in
//! Tasks 3–6; background subagent trees never stand in for runtime agents.

use std::sync::Arc;

use leptos::prelude::*;

use super::{RightPaneRegistry, RightPaneSlot, SlotCtx, SlotRenderCtx};

struct PlaceholderSlot {
    id: &'static str,
    label: &'static str,
    order: u16,
    default_open: bool,
    visible: fn(&SlotCtx) -> bool,
}

impl RightPaneSlot for PlaceholderSlot {
    fn id(&self) -> &'static str {
        self.id
    }

    fn label(&self) -> &'static str {
        self.label
    }

    fn icon(&self) -> Option<&'static str> {
        None
    }

    fn order(&self) -> u16 {
        self.order
    }

    fn visible(&self, ctx: &SlotCtx) -> bool {
        (self.visible)(ctx)
    }

    fn default_open(&self) -> bool {
        self.default_open
    }

    fn render(&self, _ctx: SlotRenderCtx) -> AnyView {
        // TODO: replace placeholders with the existing bodies in Tasks 3–6.
        ().into_any()
    }
}

pub(super) fn register_stage_one_slots(registry: &mut RightPaneRegistry) {
    // This is the only catalog. Defaults and tab order are derived from it.
    for slot in [
        PlaceholderSlot {
            id: "rps.subagent_tree",
            label: "slot.rps.subagent_tree",
            order: 100,
            default_open: true,
            visible: |_| true,
        },
        PlaceholderSlot {
            id: "rps.artifacts",
            label: "slot.rps.artifacts",
            order: 200,
            default_open: true,
            visible: |_| true,
        },
        PlaceholderSlot {
            id: "rps.capabilities",
            label: "slot.rps.capabilities",
            order: 300,
            default_open: false,
            visible: |ctx| {
                ctx.capability_revision_loaded.is_some() || ctx.capability_fetch_attempted
            },
        },
        PlaceholderSlot {
            id: "rps.browser",
            label: "slot.rps.browser",
            order: 400,
            default_open: false,
            visible: |ctx| ctx.has_browser_binding,
        },
        PlaceholderSlot {
            id: "rps.approvals",
            label: "slot.rps.approvals",
            order: 500,
            default_open: false,
            visible: |ctx| ctx.has_pending_approvals,
        },
    ] {
        registry
            .register(Arc::new(slot))
            .expect("right pane registry");
    }
}

#[cfg(test)]
mod tests {
    use super::super::{stage_one_registry, SlotCtx};

    #[test]
    fn stage_one_catalog_has_exact_ids_order_labels_and_defaults() {
        let registry = stage_one_registry();
        let mut ctx = SlotCtx {
            capability_fetch_attempted: true,
            has_browser_binding: true,
            has_pending_approvals: true,
            ..SlotCtx::default()
        };
        let actual: Vec<_> = registry
            .iter_visible(&ctx)
            .map(|slot| {
                (
                    slot.id(),
                    slot.label(),
                    slot.order(),
                    slot.default_open(),
                    slot.icon(),
                )
            })
            .collect();
        assert_eq!(
            actual,
            [
                (
                    "rps.subagent_tree",
                    "slot.rps.subagent_tree",
                    100,
                    true,
                    None
                ),
                ("rps.artifacts", "slot.rps.artifacts", 200, true, None),
                (
                    "rps.capabilities",
                    "slot.rps.capabilities",
                    300,
                    false,
                    None
                ),
                ("rps.browser", "slot.rps.browser", 400, false, None),
                ("rps.approvals", "slot.rps.approvals", 500, false, None),
            ]
        );
        assert_eq!(
            registry.default_open().into_iter().collect::<Vec<_>>(),
            ["rps.artifacts", "rps.subagent_tree"]
        );
        assert!(registry.get("rps.runtime_agents").is_none());
        assert!(registry.get("rps.tray").is_none());
        ctx.capability_fetch_attempted = false;
        ctx.has_browser_binding = false;
        ctx.has_pending_approvals = false;
        assert_eq!(
            registry
                .iter_visible(&ctx)
                .map(|s| s.id())
                .collect::<Vec<_>>(),
            ["rps.subagent_tree", "rps.artifacts"]
        );
    }

    #[test]
    fn conditional_visibility_uses_independent_snapshot_inputs() {
        let registry = stage_one_registry();
        let mut ctx = SlotCtx::default();
        for id in ["rps.capabilities", "rps.browser", "rps.approvals"] {
            assert!(!registry.get(id).unwrap().visible(&ctx));
        }
        let capabilities = registry.get("rps.capabilities").unwrap();
        ctx.capability_revision_loaded = Some(0);
        ctx.visible_version += 1;
        assert!(capabilities.visible(&ctx));
        ctx.capability_revision_loaded = None;
        ctx.capability_fetch_attempted = true;
        ctx.visible_version += 1;
        assert!(capabilities.visible(&ctx));
        assert!(!registry.get("rps.browser").unwrap().visible(&ctx));
        assert!(!registry.get("rps.approvals").unwrap().visible(&ctx));
        ctx.has_browser_binding = true;
        ctx.visible_version += 1;
        assert!(registry.get("rps.browser").unwrap().visible(&ctx));
        assert!(!registry.get("rps.approvals").unwrap().visible(&ctx));
        ctx.has_pending_approvals = true;
        ctx.visible_version += 1;
        assert!(registry.get("rps.approvals").unwrap().visible(&ctx));
    }
}
