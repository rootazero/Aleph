//! One immutable catalog; mounted bodies own their live subscriptions.

mod artifacts;
mod runtime_agents;
mod subagent_tree;
mod tray;

use std::sync::Arc;

use leptos::prelude::*;

use super::{RightPaneRegistry, RightPaneSlot, SlotCtx, SlotRenderCtx};

struct SlotDescriptor {
    id: &'static str,
    label: &'static str,
    order: u16,
    default_open: bool,
    visible: fn(&SlotCtx) -> bool,
    render: fn(SlotRenderCtx) -> AnyView,
}

impl RightPaneSlot for SlotDescriptor {
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
    fn render(&self, ctx: SlotRenderCtx) -> AnyView {
        (self.render)(ctx)
    }
}

pub(super) fn register_stage_one_slots(registry: &mut RightPaneRegistry) {
    for slot in [
        SlotDescriptor {
            id: "rps.subagent_tree",
            label: "slot.rps.subagent_tree",
            order: 100,
            default_open: true,
            visible: |_| true,
            render: subagent_tree::render,
        },
        SlotDescriptor {
            id: "rps.runtime_agents",
            label: "slot.rps.runtime_agents",
            order: 150,
            default_open: false,
            visible: |_| true,
            render: runtime_agents::render,
        },
        SlotDescriptor {
            id: "rps.artifacts",
            label: "slot.rps.artifacts",
            order: 200,
            default_open: true,
            visible: |_| true,
            render: artifacts::render,
        },
        SlotDescriptor {
            id: "rps.tray",
            label: "slot.rps.tray",
            order: 250,
            default_open: false,
            visible: |_| true,
            render: tray::render,
        },
        SlotDescriptor {
            id: "rps.capabilities",
            label: "slot.rps.capabilities",
            order: 300,
            default_open: false,
            visible: |ctx| {
                ctx.capability_revision_loaded.is_some() || ctx.capability_fetch_attempted
            },
            render: |_| ().into_any(),
        },
        SlotDescriptor {
            id: "rps.browser",
            label: "slot.rps.browser",
            order: 400,
            default_open: false,
            visible: |ctx| ctx.has_browser_binding,
            render: |_| ().into_any(),
        },
        SlotDescriptor {
            id: "rps.approvals",
            label: "slot.rps.approvals",
            order: 500,
            default_open: false,
            visible: |ctx| ctx.has_pending_approvals,
            render: |_| ().into_any(),
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
        let ctx = SlotCtx {
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
                (
                    "rps.runtime_agents",
                    "slot.rps.runtime_agents",
                    150,
                    false,
                    None
                ),
                ("rps.artifacts", "slot.rps.artifacts", 200, true, None),
                ("rps.tray", "slot.rps.tray", 250, false, None),
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
        assert!(registry.get("rps.runtime_agents").is_some());
    }
}
