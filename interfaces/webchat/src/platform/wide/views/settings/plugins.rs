use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::I18nContext;
use serde_json::json;

use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n, Locale};

/// The list row is `aleph_protocol::plugins::PluginRow` — the same type the
/// server builds the response from and the CLI decodes it with.
///
/// The hand-written DTO this replaces omitted `status` and `status_detail`, so
/// the Panel could show a plugin as "off" but never why: refused by policy,
/// shadowed by a higher-scope copy, and failed-to-parse all rendered as an
/// unlit toggle, identical to a plugin the operator had simply switched off.
pub use aleph_protocol::plugins::{PluginRow as PluginInfo, PluginRuntimeStatus};

/// The browse rows, likewise shared with the server that builds them and the
/// CLI that renders them.
use aleph_protocol::plugins::{
    MarketplaceAddParams, MarketplaceBrowseParams, MarketplaceBrowseResult,
    MarketplaceInstallParams, MarketplaceListResult, MarketplacePluginRow, MarketplaceRemoveParams,
    MarketplaceRow, MarketplaceUpdateParams,
};

/// Load plugins list from Gateway
fn load_plugins(
    // Localised copy for a refused load — the caller already holds the
    // context; `I18nContext` is `Copy`, so it rides along like the signals do.
    i18n: I18nContext<Locale>,
    state: DashboardState,
    plugins: RwSignal<Vec<PluginInfo>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) {
    loading.set(true);
    error.set(None);
    spawn_local(async move {
        match state.rpc_call("plugins.list", json!({})).await {
            Ok(result) => {
                if let Some(list) = result.get("plugins") {
                    if let Ok(parsed) = serde_json::from_value::<Vec<PluginInfo>>(list.clone()) {
                        plugins.set(parsed);
                    }
                }
                loading.set(false);
            }
            Err(e) => {
                error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| format!("Failed to load plugins: {e}"),
                )));
                loading.set(false);
            }
        }
    });
}

/// Load the registered marketplaces — the *sources* an install-by-name is
/// looked up in, which is a different question from what any of them contains
/// (`plugin.marketplace.browse`).
///
/// `error` is a separate signal from the rows on purpose. A refusal is not an
/// empty list: folding an `Err` into an empty `Vec` is how "you are not an
/// admin" renders as "you have no marketplaces", and the operator then adds a
/// duplicate of one that is already there.
fn load_marketplaces(
    i18n: I18nContext<Locale>,
    state: DashboardState,
    rows: RwSignal<Vec<MarketplaceRow>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) {
    loading.set(true);
    error.set(None);
    spawn_local(async move {
        match state.rpc_call("plugin.marketplace.list", json!({})).await {
            Ok(result) => {
                match serde_json::from_value::<MarketplaceListResult>(result) {
                    Ok(listing) => {
                        rows.set(listing.marketplaces);
                    }
                    Err(e) => {
                        // A shape we cannot read is also not an empty list.
                        //
                        // Routed through the same classifier as the refusal arm
                        // below even though a decode failure is never a
                        // refusal: on a non-refusal string the wrapper is a
                        // byte-for-byte no-op, and that is exactly why the rule
                        // it satisfies has no allowlist to grow.
                        rows.set(Vec::new());
                        error.set(Some(crate::components::admin_refusal::settings_load_error(
                            i18n,
                            &e.to_string(),
                            |e| format!("Failed to read marketplace list: {e}"),
                        )));
                    }
                }
                loading.set(false);
            }
            Err(e) => {
                rows.set(Vec::new());
                error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| format!("Failed to load marketplaces: {e}"),
                )));
                loading.set(false);
            }
        }
    });
}

#[component]
#[must_use]
pub fn PluginsView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let plugins = RwSignal::new(Vec::<PluginInfo>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let show_install_dialog = RwSignal::new(false);

    // Load plugins when connected
    Effect::new(move || {
        if state.is_connected.get() {
            load_plugins(i18n, state, plugins, loading, error);
        } else {
            loading.set(false);
        }
    });

    view! {
        <div class="flex-1 px-6 pb-6 overflow-y-auto bg-surface aleph-content-top">
            <div class="max-w-3xl space-y-6">
                // Page Header
                <div class="flex items-center justify-between">
                    <div>
                        <h1 class="text-2xl font-semibold text-text-primary mb-1">
                            {t!(i18n, settings.plugins.title)}
                        </h1>
                        <p class="text-sm text-text-secondary">
                            {t!(i18n, settings.plugins.description)}
                        </p>
                    </div>
                    <div class="flex items-center gap-2">
                        <button
                            class="px-3 py-1.5 bg-surface-sunken text-text-secondary rounded hover:bg-surface-sunken text-sm"
                            on:click=move |_| {
                                load_plugins(i18n, state, plugins, loading, error);
                            }
                        >
                            {t!(i18n, settings.plugins.refresh)}
                        </button>
                        <button
                            class="px-3 py-1.5 bg-primary text-white rounded hover:bg-primary-hover text-sm"
                            on:click=move |_| show_install_dialog.set(true)
                        >
                            {t!(i18n, settings.plugins.install_plugin)}
                        </button>
                    </div>
                </div>

                // Error Message
                {move || error.get().map(|err| view! {
                    <div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm">
                        {err}
                    </div>
                })}

                // Installed Plugins Section
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">
                        {move || format!("{} ({})", t_string!(i18n, settings.plugins.installed_count), plugins.get().len())}
                    </h2>

                    {move || {
                        if loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
                                </div>
                            }.into_any()
                        } else if plugins.get().is_empty() {
                            view! {
                                <div class="text-center py-12 border border-dashed border-border rounded">
                                    <div class="text-4xl mb-4">"🔌"</div>
                                    <p class="text-text-secondary">{t!(i18n, settings.plugins.no_plugins)}</p>
                                    <p class="text-xs text-text-tertiary mt-1">
                                        {t!(i18n, settings.plugins.no_plugins_hint)}
                                    </p>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="space-y-3">
                                    <For
                                        each=move || plugins.get()
                                        key=|plugin| plugin.name.clone()
                                        children=move |plugin| {
                                            view! {
                                                <PluginCard
                                                    plugin=plugin
                                                    plugins=plugins
                                                    loading=loading
                                                    error=error
                                                />
                                            }
                                        }
                                    />
                                </div>
                            }.into_any()
                        }
                    }}
                </div>

                // Marketplaces Section
                <MarketplacesSection />

                // Info Box
                <div class="p-4 bg-primary-subtle border border-primary/20 rounded">
                    <div class="flex items-start gap-2">
                        <span class="text-info text-sm">"ℹ️"</span>
                        <span class="text-sm text-info">
                            {t!(i18n, settings.plugins.info_text)}
                        </span>
                    </div>
                </div>
            </div>

            // Install Dialog
            <Show when=move || show_install_dialog.get()>
                <InstallPluginDialog
                    on_close=move || show_install_dialog.set(false)
                    plugins=plugins
                    loading=loading
                    error=error
                />
            </Show>
        </div>
    }
}

/// The registered marketplaces, with the add/remove pair the Panel had no way
/// to reach.
///
/// `plugin.marketplace.add` and `.remove` were registered, admin-classed and
/// spoken by `interfaces/cli` — a binary the release workflow never builds. On
/// a desktop App the only client that used them is not installed, so adding a
/// third-party marketplace meant finding the embedded `aleph-server` inside the
/// .app bundle and running it from a terminal.
///
/// Every row carries the server's own `removable` verdict rather than a local
/// comparison against `"aleph-official"`: the built-in marketplace is always
/// listed and can never be removed, and on a fresh install it is the only row
/// there is — so a Remove button rendered unconditionally would fail on the
/// only thing on screen.
#[component]
fn MarketplacesSection() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let rows = RwSignal::new(Vec::<MarketplaceRow>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let source = RwSignal::new(String::new());
    let busy = RwSignal::new(false);

    Effect::new(move || {
        if state.is_connected.get() {
            load_marketplaces(i18n, state, rows, loading, error);
        } else {
            loading.set(false);
        }
    });

    // Read the input before the await, deliberately: by the time the call
    // returns this section may be gone, and a signal read after an `.await` is
    // the disposed-read hazard the guard in this crate exists for.
    let add = move || {
        let src = source.get_untracked().trim().to_string();
        if src.is_empty() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        // Name omitted: the server derives it from the source, and a second
        // derivation here would be a second answer to what the row is called.
        let params = serde_json::to_value(MarketplaceAddParams {
            source: src,
            name: None,
        })
        .unwrap_or_else(|_| json!({}));
        spawn_local(async move {
            let outcome = state.rpc_call("plugin.marketplace.add", params).await;
            match outcome {
                Ok(added) => {
                    source.set(String::new());
                    // `plugin.marketplace.add` registers; it does not fetch.
                    // The shipped `aleph-server plugin marketplace add` syncs
                    // immediately after, and an operator who adds a source here
                    // and then finds it empty has no way to guess that a
                    // differently-named button in another dialog is what fills
                    // it. Composed from the two documented calls rather than
                    // folded into the handler, so `add` keeps one meaning for
                    // every client — and so this surface ends up where the
                    // shipped subcommand already leaves it.
                    //
                    // The name comes from the add response because the server
                    // derives it from the source; deriving it again here would
                    // be a second answer to what the row is called.
                    let name = added
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    let sync = serde_json::to_value(MarketplaceUpdateParams { name })
                        .unwrap_or_else(|_| json!({}));
                    let sync_failed = state
                        .rpc_call("plugin.marketplace.update", sync)
                        .await
                        .err();
                    busy.set(false);
                    // Reload first: it clears `error` on its way in, so a note
                    // set before it would be wiped by the very refresh that is
                    // meant to show the new row.
                    load_marketplaces(i18n, state, rows, loading, error);
                    if let Some(e) = sync_failed {
                        // Registered but not fetched is a real state, and
                        // saying so beats an empty catalogue with no reason.
                        error.set(Some(
                            crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                                format!("Added, but fetching its contents failed: {e}")
                            }),
                        ));
                    }
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to add marketplace: {e}")
                        }),
                    ));
                }
            }
        });
    };

    let remove = move |name: String| {
        busy.set(true);
        error.set(None);
        let params =
            serde_json::to_value(MarketplaceRemoveParams { name }).unwrap_or_else(|_| json!({}));
        spawn_local(async move {
            let outcome = state.rpc_call("plugin.marketplace.remove", params).await;
            busy.set(false);
            match outcome {
                Ok(_) => load_marketplaces(i18n, state, rows, loading, error),
                Err(e) => {
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to remove marketplace: {e}")
                        }),
                    ));
                }
            }
        });
    };

    view! {
        <div class="space-y-4">
            <div>
                <h2 class="text-lg font-medium text-text-primary">
                    {t!(i18n, settings.plugins.marketplaces_title)}
                </h2>
                <p class="text-sm text-text-secondary">
                    {t!(i18n, settings.plugins.marketplaces_desc)}
                </p>
            </div>

            {move || error.get().map(|err| view! {
                <div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm">
                    {err}
                </div>
            })}

            <div class="flex items-center gap-2">
                <input
                    type="text"
                    class="flex-1 px-3 py-1.5 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                    placeholder=t_string!(i18n, settings.plugins.marketplace_source_placeholder).to_string()
                    prop:value=move || source.get()
                    on:input=move |ev| source.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            add();
                        }
                    }
                />
                <button
                    class="px-3 py-1.5 bg-primary text-white rounded hover:bg-primary-hover text-sm disabled:opacity-50"
                    disabled=move || busy.get() || source.get().trim().is_empty()
                    on:click=move |_| add()
                >
                    {move || if busy.get() {
                        t_string!(i18n, settings.plugins.marketplace_adding).to_string()
                    } else {
                        t_string!(i18n, settings.plugins.marketplace_add).to_string()
                    }}
                </button>
            </div>

            {move || {
                match marketplace_list_state(
                    loading.get(),
                    state.is_connected.get(),
                    error.get().is_some(),
                    rows.get().is_empty(),
                ) {
                    MarketplaceListState::Waiting => view! {
                        <p class="text-sm text-text-tertiary">
                            {t!(i18n, settings.plugins.marketplaces_loading)}
                        </p>
                    }.into_any(),
                    MarketplaceListState::Silent => ().into_any(),
                    MarketplaceListState::Empty => view! {
                        <p class="text-sm text-text-tertiary">
                            {t!(i18n, settings.plugins.marketplaces_none)}
                        </p>
                    }.into_any(),
                    MarketplaceListState::Rows => {
                    view! {
                        <div class="space-y-2">
                            <For
                                each=move || rows.get()
                                key=|row| row.name.clone()
                                children=move |row| {
                                    let name = row.name.clone();
                                    let removable = row.removable;
                                    let reason = row.unremovable_reason.clone();
                                    view! {
                                        <div class="flex items-center justify-between p-3 bg-surface-raised border border-border rounded">
                                            <div class="min-w-0">
                                                <div class="flex items-center gap-2">
                                                    <span class="text-sm font-medium text-text-primary">
                                                        {row.name.clone()}
                                                    </span>
                                                    <span class="px-1.5 py-0.5 text-xs rounded bg-surface-sunken text-text-tertiary">
                                                        {row.source_type.clone()}
                                                    </span>
                                                </div>
                                                <p class="text-xs text-text-secondary mt-1 truncate">
                                                    {row.source.clone()}
                                                </p>
                                            </div>
                                            {if removable {
                                                let name = name.clone();
                                                view! {
                                                    <button
                                                        class="p-1.5 text-danger hover:bg-danger-subtle rounded flex-shrink-0 ml-4 disabled:opacity-50"
                                                        title=t_string!(i18n, settings.plugins.marketplace_remove).to_string()
                                                        disabled=move || busy.get()
                                                        on:click=move |_| remove(name.clone())
                                                    >
                                                        "🗑️"
                                                    </button>
                                                }.into_any()
                                            } else {
                                                // No button, and the server's
                                                // own words for why — not a
                                                // silently missing control.
                                                view! {
                                                    <span
                                                        class="text-xs text-text-tertiary flex-shrink-0 ml-4"
                                                        title=reason.clone().unwrap_or_default()
                                                    >
                                                        {t!(i18n, settings.plugins.marketplace_builtin)}
                                                    </span>
                                                }.into_any()
                                            }}
                                        </div>
                                    }
                                }
                            />
                        </div>
                    }.into_any()
                    }
                }
            }}
        </div>
    }
}

/// What the marketplace list area renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarketplaceListState {
    /// Still connecting, or the first load has not answered.
    Waiting,
    /// Nothing here — the banner above already says why.
    Silent,
    /// A load that succeeded and really came back empty.
    Empty,
    /// The registrations themselves.
    Rows,
}

/// Decide it in one place, because the order of these four questions is the
/// whole content of the decision.
///
/// The refusal arm used to be asked **before** the rows, so a failed *write*
/// (an add whose source does not resolve, a remove the server refused) wiped
/// the registrations off the screen: the rows were still in the signal and
/// still correct, and the operator saw "add failed" plus an empty list, which
/// reads as "and it ate my marketplaces". The comment on that arm said what it
/// meant — the banner already explains it, so a second line saying "none
/// registered" would contradict it — but suppressing that *line* is not
/// suppressing the *list*.
///
/// A failed **load** needs no special case here: `load_marketplaces` empties
/// the rows on both of its error paths, so the refusal arm still gets its
/// turn, which is the behaviour that arm was written for.
///
/// Found by the first real-browser run of this screen (`qa/plugins/run.sh
/// panel`), with three registrations on disk and none on screen.
const fn marketplace_list_state(
    loading: bool,
    connected: bool,
    has_error: bool,
    rows_empty: bool,
) -> MarketplaceListState {
    if loading || !connected {
        MarketplaceListState::Waiting
    } else if !rows_empty {
        MarketplaceListState::Rows
    } else if has_error {
        MarketplaceListState::Silent
    } else {
        MarketplaceListState::Empty
    }
}

#[cfg(test)]
mod tests {
    use super::{marketplace_list_state, MarketplaceListState};

    /// The case the browser found: a write failed, the registrations are
    /// intact, and they must still be on screen.
    #[test]
    fn a_failed_write_does_not_erase_the_registrations() {
        assert_eq!(
            marketplace_list_state(false, true, true, false),
            MarketplaceListState::Rows
        );
    }

    /// A failed *load* empties the rows itself, so the banner is the only
    /// thing on screen — saying "none registered" underneath it would be the
    /// UI inventing an answer the server never gave.
    #[test]
    fn a_failed_load_says_nothing_beyond_its_banner() {
        assert_eq!(
            marketplace_list_state(false, true, true, true),
            MarketplaceListState::Silent
        );
    }

    /// Only a load that succeeded and really came back empty may say so. In
    /// practice the built-in marketplace is always in a real response, so this
    /// state means the server answered with nothing at all.
    #[test]
    fn only_a_successful_empty_load_claims_there_are_none() {
        assert_eq!(
            marketplace_list_state(false, true, false, true),
            MarketplaceListState::Empty
        );
    }

    /// Not connected outranks everything: rows from a previous connection are
    /// not evidence about this one, and neither is their absence.
    #[test]
    fn waiting_outranks_every_other_answer() {
        for has_error in [false, true] {
            for rows_empty in [false, true] {
                assert_eq!(
                    marketplace_list_state(true, true, has_error, rows_empty),
                    MarketplaceListState::Waiting
                );
                assert_eq!(
                    marketplace_list_state(false, false, has_error, rows_empty),
                    MarketplaceListState::Waiting
                );
            }
        }
    }
}

#[component]
fn PluginCard(
    plugin: PluginInfo,
    plugins: RwSignal<Vec<PluginInfo>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let enabled = RwSignal::new(plugin.enabled);
    let deleting = RwSignal::new(false);
    let toggling = RwSignal::new(false);
    let plugin_name = StoredValue::new(plugin.name.clone());

    let usage_summary = plugin.usage.clone();
    let status_note = match (plugin.status, plugin.status_detail.clone()) {
        (PluginRuntimeStatus::Loaded | PluginRuntimeStatus::Disabled, _) => None,
        (status, Some(detail)) => Some(format!("{}: {detail}", status.label())),
        (status, None) => Some(status.label().to_string()),
    };
    let description = if plugin.description.is_empty() {
        "No description".to_string()
    } else {
        plugin.description.clone()
    };

    view! {
        <div class="p-4 bg-surface-raised border border-border rounded">
            <div class="flex items-start justify-between">
                <div class="flex items-start gap-3">
                    <div class="w-10 h-10 rounded bg-primary-subtle flex items-center justify-center flex-shrink-0">
                        <span class="text-primary">"🔌"</span>
                    </div>
                    <div>
                        <div class="flex items-center gap-2">
                            <span class="text-sm font-medium text-text-primary">
                                {plugin.name}
                            </span>
                            <span class="text-xs text-text-tertiary">
                                {format!("v{}", plugin.version)}
                            </span>
                        </div>
                        <p class="text-xs text-text-secondary mt-1">
                            {description}
                        </p>
                        // A plugin that is not `loaded` needs its reason next
                        // to it: "off" with no explanation points the operator
                        // at a toggle that, for a blocked or errored plugin,
                        // cannot change the outcome.
                        {status_note.map(|note| view! {
                            <p class="text-xs text-warning mt-1">{note}</p>
                        })}
                        <div class="flex items-center gap-1 mt-2 text-xs text-text-tertiary">
                            <span>"📦"</span>
                            <span>{t!(i18n, settings.plugins.git_repository)}</span>
                            <crate::components::usage_badge::UsageBadge usage=usage_summary />
                        </div>
                    </div>
                </div>

                <div class="flex items-center gap-2 flex-shrink-0 ml-4">
                    {move || {
                        if deleting.get() {
                            view! {
                                <div class="animate-spin rounded-full h-4 w-4 border-b-2 border-text-secondary"></div>
                            }.into_any()
                        } else {
                            view! {
                                <button
                                    class="p-1.5 text-danger hover:bg-danger-subtle rounded"
                                    title=t_string!(i18n, settings.plugins.remove).to_string()
                                    on:click=move |_| {
                                        deleting.set(true);
                                        let name = plugin_name.get_value();
                                        spawn_local(async move {
                                            match state.rpc_call("plugins.uninstall", json!({ "name": name })).await {
                                                Ok(_) => {
                                                    load_plugins(i18n, state, plugins, loading, error);
                                                }
                                                Err(e) => {
                                                    error.set(Some(crate::components::admin_refusal::settings_write_error(
                                                        i18n,
                                                        &e,
                                                        |e| format!("Failed to delete plugin: {e}"),
                                                    )));
                                                    deleting.set(false);
                                                }
                                            }
                                        });
                                    }
                                >
                                    "🗑️"
                                </button>
                            }.into_any()
                        }
                    }}
                    {move || {
                        if toggling.get() {
                            view! {
                                <div class="animate-spin rounded-full h-4 w-4 border-b-2 border-text-secondary"></div>
                            }.into_any()
                        } else {
                            view! {
                                <label class="relative inline-flex items-center cursor-pointer">
                                    <input
                                        type="checkbox"
                                        class="sr-only peer"
                                        checked=move || enabled.get()
                                        on:change=move |ev| {
                                            let new_val = event_target_checked(&ev);
                                            enabled.set(new_val);
                                            toggling.set(true);
                                            let name = plugin_name.get_value();
                                            let method = if new_val { "plugins.enable" } else { "plugins.disable" };
                                            spawn_local(async move {
                                                match state.rpc_call(method, json!({ "name": name })).await {
                                                    Ok(_) => {
                                                        toggling.set(false);
                                                    }
                                                    Err(e) => {
                                                        error.set(Some(crate::components::admin_refusal::settings_write_error(
                                                            i18n,
                                                            &e,
                                                            |e| format!("Failed to toggle plugin: {e}"),
                                                        )));
                                                        enabled.set(!new_val);
                                                        toggling.set(false);
                                                    }
                                                }
                                            });
                                        }
                                    />
                                    <div class="w-11 h-6 bg-surface-sunken peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/30 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-primary"></div>
                                </label>
                            }.into_any()
                        }
                    }}
                </div>
            </div>
        </div>
    }
}

#[component]
fn InstallPluginDialog(
    on_close: impl Fn() + 'static + Copy,
    plugins: RwSignal<Vec<PluginInfo>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let url = RwSignal::new(String::new());
    let installing = RwSignal::new(false);
    let dialog_error = RwSignal::new(Option::<String>::None);

    // Browse state.
    let query = RwSignal::new(String::new());
    let browse_rows = RwSignal::new(Vec::<MarketplacePluginRow>::new());
    let browse_problems =
        RwSignal::new(Vec::<aleph_protocol::plugins::MarketplaceProblemRow>::new());
    let browsing = RwSignal::new(false);
    let refreshing = RwSignal::new(false);

    // Which ids are already on this machine. Read from the page's own
    // `plugins.list` rather than answered a second time by the browse call:
    // the registry is the one thing that knows a plugin is installed-but-
    // blocked or installed-but-shadowed, and a second answer would disagree
    // with it in exactly those cases.
    let installed_ids = move || {
        plugins
            .get()
            .into_iter()
            .map(|p| p.name)
            .collect::<std::collections::HashSet<_>>()
    };

    // The query goes to the server rather than filtering `browse_rows` here.
    // The substring predicate already exists server-side for the CLI, and a
    // second copy in the Panel is a second answer to "does this row match".
    // One local manifest read per keystroke is the price; browsing never
    // fetches from the network (that is `refresh_index` below).
    //
    // It takes the query as an argument rather than reading the signal, so no
    // caller ends up reading a signal after an `.await`. `refresh_index` is
    // exactly that caller: it re-browses once the fetch returns, and by then
    // this dialog may have been closed. `disposed_reads` cannot see that
    // hazard — its scanner reads the literal text inside a `spawn_local` block
    // and does not follow a named closure — so the shape has to be right
    // rather than merely unflagged.
    let run_browse = move |q: String| {
        browsing.set(true);
        let params = serde_json::to_value(MarketplaceBrowseParams {
            marketplace: None,
            query: (!q.trim().is_empty()).then(|| q.trim().to_string()),
        })
        .unwrap_or_else(|_| json!({}));
        spawn_local(async move {
            match state.rpc_call("plugin.marketplace.browse", params).await {
                Ok(result) => {
                    match serde_json::from_value::<MarketplaceBrowseResult>(result) {
                        Ok(listing) => {
                            browse_rows.set(listing.plugins);
                            browse_problems.set(listing.problems);
                        }
                        Err(e) => {
                            browse_rows.set(Vec::new());
                            browse_problems.set(vec![
                                aleph_protocol::plugins::MarketplaceProblemRow {
                                    marketplace: String::new(),
                                    reason: e.to_string(),
                                },
                            ]);
                        }
                    }
                    browsing.set(false);
                }
                Err(e) => {
                    // A refusal is not an empty marketplace. Routing it
                    // through the same classifier the rest of this page uses
                    // keeps "you are not an admin" from rendering as "there
                    // are no plugins".
                    browse_rows.set(Vec::new());
                    browse_problems.set(vec![aleph_protocol::plugins::MarketplaceProblemRow {
                        marketplace: String::new(),
                        reason: crate::components::admin_refusal::settings_load_error(
                            i18n,
                            &e,
                            |e| format!("Failed to browse marketplace: {e}"),
                        ),
                    }]);
                    browsing.set(false);
                }
            }
        });
    };

    // Fetching is the network call, and it is the operator's to make: a browse
    // that silently git-pulls turns opening a dialog into network I/O.
    let refresh_index = move || {
        refreshing.set(true);
        // Read before the await, deliberately: refreshing re-browses with the
        // query as of the button press, which is also what an operator expects
        // a button to do.
        let q = query.get_untracked();
        spawn_local(async move {
            let outcome = state.rpc_call("plugin.marketplace.update", json!({})).await;
            refreshing.set(false);
            if let Err(e) = outcome {
                browse_problems.set(vec![aleph_protocol::plugins::MarketplaceProblemRow {
                    marketplace: String::new(),
                    reason: crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("Failed to refresh marketplace index: {e}")
                    }),
                }]);
                return;
            }
            run_browse(q);
        });
    };

    // First fill. The dialog is only mounted once the operator opens it, so
    // the socket is up by then; `rpc_call`'s bounded wait covers the rest.
    run_browse(String::new());

    // Install one browsed row. Addressed by `{name, marketplace}` rather than
    // by the bare name: browse rows know which marketplace they came from, and
    // sending it is what keeps a name that exists in two of them from being
    // refused as ambiguous.
    let install_row = move |name: String, marketplace: String| {
        installing.set(true);
        dialog_error.set(None);
        let params = serde_json::to_value(MarketplaceInstallParams {
            name,
            marketplace: Some(marketplace),
            scope: None,
        })
        .unwrap_or_else(|_| json!({}));
        spawn_local(async move {
            match state.rpc_call("plugin.marketplace.install", params).await {
                Ok(_) => {
                    installing.set(false);
                    load_plugins(i18n, state, plugins, loading, error);
                }
                Err(e) => {
                    dialog_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to install: {e}")
                        }),
                    ));
                    installing.set(false);
                }
            }
        });
    };

    let handle_install = move |_| {
        if url.get().trim().is_empty() {
            return;
        }
        installing.set(true);
        dialog_error.set(None);
        let install_url = url.get().trim().to_string();
        spawn_local(async move {
            // `plugin.install` (singular), not `plugins.install` (plural).
            // The plural one is the git-clone-only handler; the singular one
            // classifies the source server-side and routes a bare name to
            // `plugin.marketplace.install`. Panel spoke only the plural
            // namespace, which is why no marketplace plugin could be installed
            // from here — the name went to a git clone and failed.
            match state
                .rpc_call(
                    "plugin.install",
                    json!({
                        "source": install_url,
                    }),
                )
                .await
            {
                Ok(_) => {
                    installing.set(false);
                    load_plugins(i18n, state, plugins, loading, error);
                    on_close();
                }
                Err(e) => {
                    dialog_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to install: {e}")
                        }),
                    ));
                    installing.set(false);
                }
            }
        });
    };

    view! {
        <div class="aleph-scrim fixed inset-0 bg-black/50 flex items-center justify-center z-50">
            <div class="glass bg-surface-overlay/85 border border-border rounded-lg p-6 max-w-md w-full mx-4">
                <h2 class="text-lg font-semibold text-text-primary mb-2">{t!(i18n, settings.plugins.install_plugin_dialog_title)}</h2>
                <p class="text-sm text-text-secondary mb-4">
                    {t!(i18n, settings.plugins.install_plugin_dialog_desc)}
                </p>

                <div class="space-y-4">
                    // One field, because the server accepts one thing and
                    // classifies it itself.
                    //
                    // This used to be a three-option <select> — Git Repository
                    // / ZIP Archive / Local Folder — whose value was read by
                    // nothing but the label and placeholder below it. All
                    // three branches sent the same `plugins.install {url}`,
                    // the git-clone-only handler, so picking "ZIP Archive" or
                    // "Local Folder" changed the hint text and then git-cloned
                    // whatever was typed. Neither had a handler to reach:
                    // `plugins.installFromZip` takes base64 file bytes rather
                    // than a URL, and no RPC installs from a local directory
                    // at all. A control that collects a field nothing decides
                    // on is worse than no control — it reads as a capability.
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">
                            {t!(i18n, settings.plugins.source_label)}
                        </label>
                        <input
                            type="text"
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                            placeholder="my-plugin  or  https://github.com/user/plugin.git"
                            value=move || url.get()
                            on:input=move |ev| url.set(event_target_value(&ev))
                        />
                        <p class="mt-2 text-xs text-text-secondary">
                            {t!(i18n, settings.plugins.source_hint)}
                        </p>
                    </div>

                    {move || dialog_error.get().map(|err| view! {
                        <div class="flex items-center gap-2 text-danger text-sm">
                            <span>"⚠️"</span>
                            <span>{err}</span>
                        </div>
                    })}

                    // Browsing. Installing by name worked before this; finding
                    // the name did not — `search_plugin` matched an exact id
                    // and nothing listed a marketplace's contents, so the field
                    // above could only be used by someone who already knew what
                    // to type.
                    <div class="border-t border-border pt-4">
                        <div class="flex items-center justify-between mb-2">
                            <span class="text-sm font-medium text-text-secondary">
                                {t!(i18n, settings.plugins.browse_title)}
                            </span>
                            <button
                                class="text-xs px-2 py-1 rounded bg-surface-sunken text-text-secondary hover:bg-surface-overlay disabled:opacity-50"
                                disabled=move || refreshing.get()
                                on:click=move |_| refresh_index()
                            >
                                {move || if refreshing.get() {
                                    t_string!(i18n, settings.plugins.browse_refreshing).to_string()
                                } else {
                                    t_string!(i18n, settings.plugins.browse_refresh).to_string()
                                }}
                            </button>
                        </div>

                        <input
                            type="text"
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm mb-2"
                            placeholder=move || t_string!(i18n, settings.plugins.browse_search_placeholder).to_string()
                            value=move || query.get()
                            on:input=move |ev| {
                                let q = event_target_value(&ev);
                                query.set(q.clone());
                                run_browse(q);
                            }
                        />

                        // Every marketplace that could not be read, named. An
                        // empty catalogue with nothing said means "no matches";
                        // dropping these would make it also mean "never
                        // synced", and only one of those is something the
                        // operator should keep typing at.
                        <For
                            each=move || browse_problems.get()
                            key=|p| p.marketplace.clone()
                            let:problem
                        >
                            <div class="text-xs text-warning mb-1">
                                {format!("{}: {}", problem.marketplace, problem.reason)}
                            </div>
                        </For>

                        <div class="max-h-56 overflow-y-auto space-y-1">
                            {move || {
                                if browsing.get() {
                                    return view! {
                                        <div class="text-xs text-text-secondary py-2">
                                            {t!(i18n, settings.plugins.browse_loading)}
                                        </div>
                                    }.into_any();
                                }
                                let rows = browse_rows.get();
                                if rows.is_empty() && browse_problems.get().is_empty() {
                                    return view! {
                                        <div class="text-xs text-text-secondary py-2">
                                            {t!(i18n, settings.plugins.browse_empty)}
                                        </div>
                                    }.into_any();
                                }
                                let installed = installed_ids();
                                rows.into_iter().map(|row| {
                                    let already = installed.contains(&row.name);
                                    let reason = row.unavailable_reason.clone();
                                    let installable = row.installable;
                                    let name = row.name.clone();
                                    let marketplace = row.marketplace.clone();
                                    view! {
                                        <div class="flex items-start justify-between gap-2 px-2 py-1.5 rounded hover:bg-surface-sunken">
                                            <div class="min-w-0">
                                                <div class="text-sm text-text-primary truncate">
                                                    {row.name.clone()}
                                                    <span class="text-xs text-text-secondary ml-1">
                                                        {format!("@{}", row.marketplace)}
                                                    </span>
                                                    {(!row.version.is_empty()).then(|| view! {
                                                        <span class="text-xs text-text-secondary ml-1">{row.version.clone()}</span>
                                                    })}
                                                </div>
                                                {(!row.description.is_empty()).then(|| view! {
                                                    <div class="text-xs text-text-secondary truncate">{row.description.clone()}</div>
                                                })}
                                                // Shown for exactly the rows the install call refuses,
                                                // using the server's own predicate — the button beside
                                                // it is disabled by the same bit.
                                                {reason.map(|r| view! {
                                                    <div class="text-xs text-warning">{r}</div>
                                                })}
                                            </div>
                                            {if already {
                                                view! {
                                                    <span class="text-xs text-text-secondary shrink-0 px-2 py-1">
                                                        {t!(i18n, settings.plugins.browse_installed)}
                                                    </span>
                                                }.into_any()
                                            } else {
                                                view! {
                                                    <button
                                                        class="text-xs shrink-0 px-2 py-1 rounded bg-primary text-white hover:bg-primary-hover disabled:opacity-50"
                                                        disabled=move || !installable || installing.get()
                                                        on:click=move |_| install_row(name.clone(), marketplace.clone())
                                                    >
                                                        {t!(i18n, settings.plugins.install)}
                                                    </button>
                                                }.into_any()
                                            }}
                                        </div>
                                    }
                                }).collect_view().into_any()
                            }}
                        </div>
                    </div>
                </div>

                <div class="flex gap-2 mt-6">
                    <button
                        class="flex-1 px-4 py-2 bg-surface-sunken text-text-secondary rounded hover:bg-surface-sunken text-sm"
                        on:click=move |_| on_close()
                    >
                        {t!(i18n, settings.plugins.cancel)}
                    </button>
                    <button
                        class="flex-1 px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover text-sm disabled:opacity-50"
                        disabled=move || url.get().trim().is_empty() || installing.get()
                        on:click=handle_install
                    >
                        {move || if installing.get() { t_string!(i18n, settings.plugins.installing).to_string() } else { t_string!(i18n, settings.plugins.install).to_string() }}
                    </button>
                </div>
            </div>
        </div>
    }
}
