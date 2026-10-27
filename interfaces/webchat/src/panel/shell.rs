use super::{
    loader::{PanelLoadStatus, PanelLoader},
    projection::PanelProjection,
    sdk::PanelSdk,
};
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use aleph_protocol::panel::PanelTabKey;
use leptos::{html, prelude::*, task::spawn_local};

#[component]
pub fn PanelShell(projection: PanelProjection, initial_tab: Option<PanelTabKey>) -> impl IntoView {
    let i18n = use_i18n();
    let loader = StoredValue::new_local(PanelLoader::new(projection.registry()));
    let host = loader.with_value(PanelLoader::host);
    provide_context(host.clone());
    let active = host.active;
    let revision = host.revision;
    active.set(initial_tab);
    let reconcile_host = host.clone();
    // Keep route selection and registry status in sync before attempting a mount.
    Effect::new(move |_| {
        revision.get();
        let current = active.get();
        let next = current.as_ref().and_then(|key| {
            let registry = reconcile_host.registry.lock().unwrap();
            registry
                .get(key)
                .filter(|descriptor| {
                    descriptor.status == aleph_protocol::panel::PanelTabStatus::Enabled
                })
                .map(|descriptor| descriptor.key.clone())
                .or_else(|| registry.active_after_removal(key))
        });
        if next != current {
            active.set(next);
        }
    });
    // Ignore same-key SDK navigation; explicit retry has its own attempt signal.
    let selected = Memo::new(move |_| active.get());
    let state = expect_context::<DashboardState>();
    let sidebar = NodeRef::<html::Div>::new();
    let content = NodeRef::<html::Div>::new();
    let status = RwSignal::new(PanelLoadStatus::Idle);
    let retry = RwSignal::new(0_u64);
    let shell_host = host.clone();
    let shell_kind = move || {
        let Some(key) = active.get() else {
            return "panel".to_string();
        };
        shell_host
            .registry
            .lock()
            .unwrap()
            .get(&key)
            .map(|descriptor| descriptor.key.tab_id.clone())
            .unwrap_or_else(|| "panel".to_string())
    };

    on_cleanup(move || {
        let _ = loader.try_update_value(|loader| {
            let _ = loader.deactivate();
        });
    });
    Effect::new(move |_| {
        revision.get();
        retry.get();
        let key = selected.get();
        let (Some(sidebar), Some(content)) = (sidebar.get(), content.get()) else {
            return;
        };
        let Some(key) = key else {
            loader.update_value(|loader| {
                let _ = loader.deactivate();
            });
            status.set(loader.with_value(PanelLoader::status));
            return;
        };
        let sdk = PanelSdk::new(state, key.clone());
        let future = loader.write_value().activate(
            key,
            (*sidebar).clone().into(),
            (*content).clone().into(),
            sdk,
        );
        status.set(loader.with_value(PanelLoader::status));
        spawn_local(async move {
            let _ = future.await;
            // Old futures may finish after Shell owner disposal. Read current
            // loader status, never publish a captured old attempt's outcome.
            if let Some(current) = loader.try_with_value(PanelLoader::status) {
                let _ = status.try_set(current);
            }
        });
    });

    view! {
        <section class="h-full flex flex-col" data-panel-shell=shell_kind>
            <nav aria-label=move || t_string!(i18n, panel_spike.tabs) class="flex gap-2 p-3">
                {move || {
                    revision.get();
                    host.registry
                        .lock()
                        .unwrap()
                        .rows()
                        .into_iter()
                        .map(|row| {
                            let key = row.key;
                            let selected = key.clone();
                            let enabled = row.status == aleph_protocol::panel::PanelTabStatus::Enabled;
                            let detail = row.status_detail.map(|detail| format!(" ({detail})"));
                            view! {
                                <button type="button" disabled=move || !enabled
                                    aria-pressed=move || active.get().as_ref() == Some(&selected)
                                    aria-disabled=(!enabled).to_string()
                                    on:click=move |_| active.set(Some(key.clone()))>
                                    {row.label}{detail}
                                </button>
                            }
                        })
                        .collect_view()
                }}
                <button type="button" on:click=move |_| active.set(None)>{move || t_string!(i18n, panel_spike.close)}</button>
            </nav>
            <div role="status" aria-live="polite">
                {move || match status.get() {
                    PanelLoadStatus::Idle => t_string!(i18n, panel_spike.select).to_string(),
                    PanelLoadStatus::Loading { .. } => t_string!(i18n, common.loading).to_string(),
                    PanelLoadStatus::Mounted { .. } => String::new(),
                    PanelLoadStatus::Failed { error, .. } => format!("{}: {error}", t_string!(i18n, panel_spike.failed)),
                }}
            </div>
            <Show when=move || matches!(status.get(), PanelLoadStatus::Failed { .. })>
                <button type="button" on:click=move |_| retry.update(|attempt| *attempt += 1)>{move || t_string!(i18n, common.retry)}</button>
            </Show>
            <div class="flex flex-1 min-h-0">
                <div node_ref=sidebar data-panel-sidebar-root="" class="w-64 shrink-0 overflow-auto"></div>
                <div node_ref=content data-panel-content-root="" class="flex-1 min-w-0 overflow-auto"></div>
            </div>
        </section>
    }
}
