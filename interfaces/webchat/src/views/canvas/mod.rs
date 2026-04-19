mod breadcrumb;
mod detail_panel;
mod graph_canvas;
mod toolbar;

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::graph::GraphApi;
use crate::canvas_engine::adapter::{adapt_graph_response, NoteDetailResponse};
use leptos::callback::Callback;

use crate::canvas_engine::interaction::CanvasEvent;
use crate::canvas_engine::types::{BreadcrumbEntry, ViewMode};
use crate::context::DashboardState;

use breadcrumb::Breadcrumb;
use detail_panel::DetailPanel;
use graph_canvas::{GraphCanvas, GraphState};
use toolbar::CanvasToolbar;

#[component]
pub fn CanvasView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Reactive signals
    let (view_mode, set_view_mode) = signal(ViewMode::Global { top_k: 100 });
    let (selected_node, set_selected_node) = signal(None::<String>);
    let (node_detail, set_node_detail) = signal(None::<NoteDetailResponse>);
    let (breadcrumb_entries, set_breadcrumb) = signal(Vec::<BreadcrumbEntry>::new());
    let search_query = RwSignal::new(String::new());

    // Non-reactive 60fps state
    let graph_state = Rc::new(RefCell::new(GraphState::new()));

    // Load data when connected or view mode changes
    let gs_load = graph_state.clone();
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }
        let mode = view_mode.get();
        let gs = gs_load.clone();

        spawn_local(async move {
            let result = match &mode {
                ViewMode::Global { top_k } => GraphApi::query(&state, *top_k, vec![]).await,
                ViewMode::Local {
                    center_node_id,
                    depth,
                } => GraphApi::neighbors(&state, center_node_id, *depth, 200).await,
            };

            match result {
                Ok(response) => {
                    let (nodes, edges) = adapt_graph_response(&response);
                    let mut gs = gs.borrow_mut();
                    gs.nodes = nodes;
                    gs.edges = edges;
                    gs.layout.wake();
                    gs.selected_node = None;
                    gs.hovered_node = None;
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to load graph: {}", e).into());
                }
            }
        });
    });

    // Fetch node detail when selection changes
    Effect::new(move || {
        let node_id = selected_node.get();
        match node_id {
            Some(id) => {
                spawn_local(async move {
                    match GraphApi::node_detail(&state, &id).await {
                        Ok(detail) => set_node_detail.set(Some(detail)),
                        Err(e) => {
                            web_sys::console::error_1(
                                &format!("Failed to load node detail: {}", e).into(),
                            );
                            set_node_detail.set(None);
                        }
                    }
                });
            }
            None => {
                set_node_detail.set(None);
            }
        }
    });

    // Callbacks — must be Send+Sync for Callback::new, so no Rc<RefCell<_>> capture.
    let on_event = move |event: CanvasEvent| match event {
        CanvasEvent::SelectNode(id) => {
            set_selected_node.set(Some(id));
        }
        CanvasEvent::DeselectNode => {
            set_selected_node.set(None);
        }
        CanvasEvent::EnterLocalView(id) => {
            // Push breadcrumb (use id as display name; detail fetch will supply the real name)
            set_breadcrumb.update(|entries| {
                entries.push(BreadcrumbEntry {
                    node_id: id.clone(),
                    node_name: id.clone(),
                });
            });
            set_view_mode.set(ViewMode::Local {
                center_node_id: id,
                depth: 2,
            });
            set_selected_node.set(None);
        }
        CanvasEvent::HoverNode(_) => {
            // Hover is handled in GraphState directly
        }
        _ => {}
    };

    let on_toggle_mode = move || {
        let current = view_mode.get();
        match current {
            ViewMode::Global { .. } => {
                // Stay in global (no-op unless there's a selected node)
            }
            ViewMode::Local { .. } => {
                set_view_mode.set(ViewMode::Global { top_k: 100 });
                set_breadcrumb.set(vec![]);
                set_selected_node.set(None);
            }
        }
    };

    let on_search = move |query: String| {
        if query.is_empty() {
            return;
        }
        spawn_local(async move {
            match GraphApi::search(&state, &query, 20).await {
                Ok(response) => {
                    if let Some(first) = response.results.first() {
                        let id = first.id.clone();
                        let name = first.name.clone();
                        set_breadcrumb.set(vec![BreadcrumbEntry {
                            node_id: id.clone(),
                            node_name: name,
                        }]);
                        set_view_mode.set(ViewMode::Local {
                            center_node_id: id,
                            depth: 2,
                        });
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Search failed: {}", e).into());
                }
            }
        });
    };

    let on_breadcrumb_navigate = move |target: Option<String>| match target {
        None => {
            // Go back to global
            set_view_mode.set(ViewMode::Global { top_k: 100 });
            set_breadcrumb.set(vec![]);
            set_selected_node.set(None);
        }
        Some(id) => {
            // Truncate breadcrumb to the navigated node
            set_breadcrumb.update(|entries| {
                if let Some(pos) = entries.iter().position(|e| e.node_id == id) {
                    entries.truncate(pos + 1);
                }
            });
            set_view_mode.set(ViewMode::Local {
                center_node_id: id,
                depth: 2,
            });
            set_selected_node.set(None);
        }
    };

    let is_local = move || matches!(view_mode.get(), ViewMode::Local { .. });
    let has_detail = move || node_detail.get().is_some();

    view! {
        <div class="flex flex-col h-full">
            <CanvasToolbar
                view_mode=view_mode
                search_query=search_query
                on_toggle_mode=on_toggle_mode
                on_search=on_search
            />

            {move || is_local().then(|| view! {
                <Breadcrumb
                    entries=breadcrumb_entries
                    on_navigate=on_breadcrumb_navigate
                />
            })}

            <div class="flex flex-1 overflow-hidden">
                <div class="flex-1 relative bg-[#0a0a0f]">
                    <GraphCanvas
                        graph_state=graph_state.clone()
                        on_event=Callback::new(on_event)
                    />
                </div>

                {move || has_detail().then(|| view! {
                    <DetailPanel detail=node_detail />
                })}
            </div>
        </div>
    }
}
