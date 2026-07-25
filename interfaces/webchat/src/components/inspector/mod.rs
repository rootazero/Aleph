//! Contextual inspector — the single-agent body of the right-side workspace pane.
//!
//! [`InspectorSurface`] reads [`crate::state::layout::WorkspaceState::selected`]
//! and routes each [`InspectorTarget`] variant to its dedicated surface
//! component. It replaces the old tool-only `ToolDetailView`, whose body now
//! lives verbatim in [`tool_surface`].
//!
//! The router is a closed `match` over the target enum — the sibling of
//! `tool_card::render_body`'s per-kind dispatch. Adding a surface is one arm
//! here plus one component module; nothing else changes (OCP / R10-thin).

mod plan_surface;
mod reasoning_surface;
mod run_meta_surface;
mod tool_surface;

use plan_surface::PlanInspector;
use reasoning_surface::ReasoningInspector;
use run_meta_surface::RunMetaInspector;
use tool_surface::ToolInspector;

use crate::i18n::{t, use_i18n};
use crate::state::inspector::InspectorTarget;
use crate::state::layout::WorkspaceState;
use leptos::prelude::*;

/// The pane body in single-agent mode. Routes the current selection to a
/// surface; renders the empty-state hint when nothing is selected.
#[component]
#[must_use]
pub fn InspectorSurface() -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let i18n = use_i18n();

    move || match workspace.selected.get() {
        None => view! {
            <div class="h-full flex flex-col items-center justify-center
                        text-center text-text-tertiary gap-3 py-12 px-6">
                <p class="text-sm font-medium text-text-secondary">
                    {t!(i18n, common.workspace_pane)}
                </p>
                <p class="text-xs max-w-[28ch] leading-relaxed">
                    {t!(i18n, common.workspace_detail_empty)}
                </p>
            </div>
        }
        .into_any(),
        Some(InspectorTarget::Tool { run_id, tool_id }) => {
            view! { <ToolInspector run_id tool_id /> }.into_any()
        }
        Some(InspectorTarget::RunMeta { run_id }) => {
            view! { <RunMetaInspector run_id /> }.into_any()
        }
        Some(InspectorTarget::Reasoning { run_id }) => {
            view! { <ReasoningInspector run_id /> }.into_any()
        }
        Some(InspectorTarget::Plan { run_id }) => {
            view! { <PlanInspector run_id /> }.into_any()
        }
    }
}
