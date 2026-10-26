//! Disposable host frame for a modular Panel tab.
//! Static descriptors are host projections; the loaded module remains the implementation authority.

use super::{
    loader::{PanelLoadStatus, PanelLoader},
    registry::TabRegistry,
    sdk::PanelSdk,
};
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use aleph_protocol::panel::{
    PanelTabDescriptor, PanelTabKey, PanelTabStatus, PanelUiTrust, PANEL_UI_API_MAJOR,
};
use leptos::{html, prelude::*, task::spawn_local};

#[component]
pub fn PanelShell() -> impl IntoView {
    let i18n = use_i18n();
    let loader = StoredValue::new_local(PanelLoader::new(
        TabRegistry::new(vec![builtin_descriptor(
            t_string!(i18n, panel_spike.pilot).to_string(),
        )])
        .expect("static panel descriptor"),
    ));
    let host = loader.with_value(PanelLoader::host);
    provide_context(host.clone());
    let active = host.active;
    // Ignore same-key SDK navigation; explicit retry has its own attempt signal.
    let selected = Memo::new(move |_| active.get());
    let rows = host.registry.lock().unwrap().ordered();
    let state = expect_context::<DashboardState>();
    let sidebar = NodeRef::<html::Div>::new();
    let content = NodeRef::<html::Div>::new();
    let status = RwSignal::new(PanelLoadStatus::Idle);
    let retry = RwSignal::new(0_u64);
    let shell_kind = if cfg!(feature = "panel-tab-spike") {
        "spike"
    } else {
        "terminal"
    };

    on_cleanup(move || {
        let _ = loader.try_update_value(|loader| {
            let _ = loader.deactivate();
        });
    });
    Effect::new(move |_| {
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
                {rows.into_iter().map(|row| {
                    let key = row.key;
                    let selected = key.clone();
                    view! {
                        <button type="button" aria-pressed=move || active.get().as_ref() == Some(&selected)
                            on:click=move |_| active.set(Some(key.clone()))>{row.label}</button>
                    }
                }).collect_view()}
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

#[cfg(feature = "panel-tab-spike")]
fn builtin_descriptor(label: String) -> PanelTabDescriptor {
    PanelTabDescriptor {
        key: PanelTabKey {
            plugin_id: "aleph".into(),
            tab_id: "terminal-pilot".into(),
        },
        label,
        icon: "terminal".into(),
        order: 0,
        // The build can project a different asset root without changing the loader.
        entry: option_env!("ALEPH_PANEL_PILOT_ENTRY")
            .unwrap_or("tabs/terminal-pilot/terminal-pilot.js")
            .into(),
        api_major: PANEL_UI_API_MAJOR,
        trust: PanelUiTrust::Verified,
        permissions: vec!["sessions.list".into()],
        status: PanelTabStatus::Enabled,
        status_detail: None,
    }
}

#[cfg(not(feature = "panel-tab-spike"))]
fn builtin_descriptor(label: String) -> PanelTabDescriptor {
    PanelTabDescriptor {
        key: PanelTabKey {
            plugin_id: "aleph".into(),
            tab_id: "terminal".into(),
        },
        label,
        icon: "terminal".into(),
        order: 0,
        entry: option_env!("PANEL_TERMINAL_ENTRY")
            .unwrap_or("./tabs/terminal/dev/terminal.js")
            .into(),
        api_major: PANEL_UI_API_MAJOR,
        trust: PanelUiTrust::Verified,
        permissions: [
            "pty.list",
            "pty.spawn",
            "pty.attach",
            "pty.input",
            "pty.resize",
            "pty.close",
            "config.get",
            "events.subscribe:pty.screen",
            "events.subscribe:pty.exit",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
        status: PanelTabStatus::Enabled,
        status_detail: None,
    }
}
