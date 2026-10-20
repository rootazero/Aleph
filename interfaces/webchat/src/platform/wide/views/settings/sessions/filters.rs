// Session filter controls

use leptos::prelude::*;

use super::view::SessionFilterParams;

#[component]
#[must_use]
pub fn SessionFilters(
    on_apply: impl Fn(SessionFilterParams) + 'static + Copy,
    current_params: RwSignal<SessionFilterParams>,
) -> impl IntoView {
    let _ = current_params;
    let agent_filter = RwSignal::new(String::new());
    let workspace_filter = RwSignal::new(String::new());
    let start_date_filter = RwSignal::new(String::new());
    let end_date_filter = RwSignal::new(String::new());
    let compression_filter = RwSignal::new(String::new());

    let apply_filters = move |_| {
        let agent = agent_filter.get();
        let workspace = workspace_filter.get();
        let start = start_date_filter.get();
        let end = end_date_filter.get();
        let compression = compression_filter.get();

        let params = SessionFilterParams {
            agent_name: if agent.is_empty() { None } else { Some(agent) },
            workspace: if workspace.is_empty() {
                None
            } else {
                Some(workspace)
            },
            start_date: if start.is_empty() { None } else { Some(start) },
            end_date: if end.is_empty() { None } else { Some(end) },
            compression_status: if compression.is_empty() {
                None
            } else {
                Some(compression)
            },
            limit: 20,
            offset: 0,
        };

        on_apply(params);
    };

    let clear_filters = move |_| {
        agent_filter.set(String::new());
        workspace_filter.set(String::new());
        start_date_filter.set(String::new());
        end_date_filter.set(String::new());
        compression_filter.set(String::new());

        on_apply(SessionFilterParams {
            limit: 20,
            offset: 0,
            ..Default::default()
        });
    };

    view! {
        <div class="flex flex-col gap-3">
            <div class="flex gap-4 items-end">
                <div class="flex flex-col gap-1 flex-1">
                    <label class="text-sm font-medium text-text-primary">"Agent:"</label>
                    <input
                        type="text"
                        class="px-3 py-2 border border-border rounded text-sm bg-surface focus:outline-none focus:border-primary"
                        placeholder="Filter by agent name"
                        prop:value=move || agent_filter.get()
                        on:input=move |ev| {
                            agent_filter.set(event_target_value(&ev));
                        }
                    />
                </div>

                <div class="flex flex-col gap-1 flex-1">
                    <label class="text-sm font-medium text-text-primary">"Workspace:"</label>
                    <input
                        type="text"
                        class="px-3 py-2 border border-border rounded text-sm bg-surface focus:outline-none focus:border-primary"
                        placeholder="Filter by workspace"
                        prop:value=move || workspace_filter.get()
                        on:input=move |ev| {
                            workspace_filter.set(event_target_value(&ev));
                        }
                    />
                </div>

                <div class="flex flex-col gap-1 flex-1">
                    <label class="text-sm font-medium text-text-primary">"Compression:"</label>
                    <select
                        class="px-3 py-2 border border-border rounded text-sm bg-surface focus:outline-none focus:border-primary"
                        on:change=move |ev| {
                            compression_filter.set(event_target_value(&ev));
                        }
                    >
                        <option value="" selected=move || compression_filter.get().is_empty()>
                            "All"
                        </option>
                        <option value="pending" selected=move || compression_filter.get() == "pending">
                            "Pending"
                        </option>
                        <option value="compressed" selected=move || compression_filter.get() == "compressed">
                            "Compressed"
                        </option>
                        <option value="archived" selected=move || compression_filter.get() == "archived">
                            "Archived"
                        </option>
                    </select>
                </div>
            </div>

            <div class="flex gap-4 items-end">
                <div class="flex flex-col gap-1 flex-1">
                    <label class="text-sm font-medium text-text-primary">"Start Date:"</label>
                    <input
                        type="date"
                        class="px-3 py-2 border border-border rounded text-sm bg-surface focus:outline-none focus:border-primary"
                        prop:value=move || start_date_filter.get()
                        on:input=move |ev| {
                            start_date_filter.set(event_target_value(&ev));
                        }
                    />
                </div>

                <div class="flex flex-col gap-1 flex-1">
                    <label class="text-sm font-medium text-text-primary">"End Date:"</label>
                    <input
                        type="date"
                        class="px-3 py-2 border border-border rounded text-sm bg-surface focus:outline-none focus:border-primary"
                        prop:value=move || end_date_filter.get()
                        on:input=move |ev| {
                            end_date_filter.set(event_target_value(&ev));
                        }
                    />
                </div>

                <div class="flex gap-2">
                    <button
                        class="px-4 py-2 border border-primary bg-primary text-white rounded text-sm hover:bg-primary/90 transition-colors"
                        on:click=apply_filters
                    >
                        "Apply Filters"
                    </button>
                    <button
                        class="px-4 py-2 border border-border bg-surface text-text-primary rounded text-sm hover:bg-surface-raised transition-colors"
                        on:click=clear_filters
                    >
                        "Clear"
                    </button>
                </div>
            </div>
        </div>
    }
}
