//! App-shell host for the flex right pane.

use std::sync::Arc;

use crate::components::workspace_panel::WorkspacePanel;
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n, I18nCtx};
use crate::platform::wide::views::browser::BrowserWorkspace;
use crate::state::layout::WorkspaceState;
use leptos::prelude::*;
use leptos_router::hooks::use_location;

use super::{stage_one_registry, RightPaneRegistry, SlotCtx, SlotRenderCtx};

#[component]
#[must_use]
pub fn RightPane() -> impl IntoView {
    view! { <WorkspacePanel legacy=false /> }
}

/// Resolve selection from the same visible catalog used by the tab strip.
fn selected_slot(
    registry: &RightPaneRegistry,
    ctx: &SlotCtx,
    requested: Option<&str>,
) -> Option<&'static str> {
    let visible: Vec<_> = registry.iter_visible(ctx).collect();
    visible
        .iter()
        .find(|slot| Some(slot.id()) == requested && ctx.effective_open(slot.as_ref()))
        .or_else(|| {
            visible
                .iter()
                .find(|slot| ctx.effective_open(slot.as_ref()))
        })
        .map(|slot| slot.id())
}

/// Resource-key translation only; descriptor order/defaults live in the registry.
fn slot_label(key: &str, i18n: I18nCtx) -> String {
    match key {
        "slot.rps.subagent_tree" => t_string!(i18n, slot.rps.subagent_tree).to_string(),
        "slot.rps.runtime_agents" => t_string!(i18n, slot.rps.runtime_agents).to_string(),
        "slot.rps.artifacts" => t_string!(i18n, slot.rps.artifacts).to_string(),
        "slot.rps.tray" => t_string!(i18n, slot.rps.tray).to_string(),
        "slot.rps.capabilities" => t_string!(i18n, slot.rps.capabilities).to_string(),
        "slot.rps.browser" => t_string!(i18n, slot.rps.browser).to_string(),
        "slot.rps.approvals" => t_string!(i18n, slot.rps.approvals).to_string(),
        _ => key.to_owned(),
    }
}

/// Registry rendering inside the shared width/resizer shell.
#[component]
pub(crate) fn RegistryPane() -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let dash = expect_context::<DashboardState>();
    let browser = use_context::<BrowserWorkspace>();
    let i18n = use_i18n();
    let registry = StoredValue::new(stage_one_registry());
    let snapshot = move || SlotCtx {
        locale: i18n.get_locale(),
        mode: workspace.mode.get(),
        is_connected: dash.is_connected.get(),
        has_browser_binding: browser.is_some_and(|browser| browser.has_browser()),
        user_opened: workspace.user_opened_slots.get(),
        user_closed: workspace.user_closed_slots.get(),
        ..SlotCtx::default()
    };
    let visible = Memo::new(move |_| {
        registry.with_value(|r| {
            r.iter_visible(&snapshot())
                .map(|slot| slot.id())
                .collect::<Vec<_>>()
        })
    });
    let active = Memo::new(move |_| {
        registry.with_value(|r| selected_slot(r, &snapshot(), workspace.selected_slot.get()))
    });
    // The full-screen deep-link owns the tree presenter. Keep the artifacts
    // owner mounted behind the hidden shell, but suppress the selected tree
    // slot so two live tree owners can never coexist.
    let location = use_location();
    let full_screen_subagents = Memo::new(move |_| {
        crate::components::mode_sidebar::is_subagent_tree_route(&location.pathname.get())
    });
    let rendered_active = Memo::new(move |_| {
        if full_screen_subagents.get() {
            Some("rps.artifacts")
        } else {
            active.get()
        }
    });
    // Existing canvas/tool producers also write `body` directly while Split.
    // Skip the initial read so the registry's default remains the entry tab.
    Effect::new(move |previous: Option<()>| {
        let _ = workspace.body.get();
        if previous.is_some() {
            workspace.select_slot("rps.artifacts");
        }
    });

    view! {
        <div class="aleph-pane-top flex flex-wrap gap-1 px-3 py-2 border-b border-border text-xs shrink-0" role="tablist">
            <For each=move || visible.get() key=|id| *id children=move |id| {
                let slot = registry.with_value(|r| r.get(id).expect("registered visible slot"));
                view! {
                    <button type="button" role="tab" data-slot-id=id
                        aria-selected=move || active.get() == Some(id)
                        class=move || if active.get() == Some(id) {
                            "px-2 py-1 rounded bg-primary text-white"
                        } else {
                            "px-2 py-1 rounded text-text-secondary hover:text-text-primary"
                        }
                        on:click=move |_| workspace.select_slot(id)
                    >{move || slot_label(slot.label(), i18n)}</button>
                }
            } />
        </div>
        <For each=move || visible.get() key=|id| *id children=move |id| {
            let slot = registry.with_value(|r| r.get(id).expect("registered visible slot"));
            // The artifacts workspace contains Canvas's camera, undo and live
            // subscriptions. Keep this owner alive across outer tab switches.
            if id == "rps.artifacts" {
                view! {
                    <div class="flex-1 min-h-0 flex flex-col" data-slot-body=id role="tabpanel"
                        style:display=move || {
                            if rendered_active.get() == Some(id) { "flex" } else { "none" }
                        }>
                        {slot.render(SlotRenderCtx { snapshot: snapshot() })}
                    </div>
                }.into_any()
            } else {
                let slot: Arc<dyn super::RightPaneSlot> = slot;
                view! {
                    <Show when=move || rendered_active.get() == Some(id)>
                        <div class="flex-1 min-h-0 flex flex-col overflow-y-auto" data-slot-body=id role="tabpanel">
                            {slot.render(SlotRenderCtx { snapshot: snapshot() })}
                        </div>
                    </Show>
                }.into_any()
            }
        } />
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_defaults_and_visibility_control_selected_body() {
        let registry = stage_one_registry();
        let mut ctx = SlotCtx::default();
        assert_eq!(
            selected_slot(&registry, &ctx, None),
            Some("rps.subagent_tree")
        );
        assert_eq!(
            selected_slot(&registry, &ctx, Some("rps.browser")),
            Some("rps.subagent_tree")
        );
        ctx.user_opened.insert("rps.runtime_agents");
        assert_eq!(
            selected_slot(&registry, &ctx, Some("rps.runtime_agents")),
            Some("rps.runtime_agents")
        );
        ctx.user_closed.insert("rps.subagent_tree");
        assert_eq!(
            selected_slot(&registry, &ctx, Some("rps.subagent_tree")),
            Some("rps.runtime_agents")
        );
        ctx.user_opened.clear();
        assert_eq!(selected_slot(&registry, &ctx, None), Some("rps.artifacts"));
    }
}
