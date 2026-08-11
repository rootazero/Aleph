//! Standing approvals — the surface where "Allow for this session" and
//! "Always allow" become visible and revocable.
//!
//! It lives under Policies because that is where the rest of "what may run
//! without asking me" lives: the execution tier, and the per-tool overrides.
//! A standing grant is the third answer to that question — the per-CALL one —
//! and the only one that used to have no page at all.
//!
//! Pure I/O (R4). The server decides which rows this caller may see and which
//! it may revoke; nothing here filters, and a row that arrives is a row that
//! can be revoked.

use crate::api::exec_grants::{ExecGrantsApi, GrantView};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Local-time-ish rendering of a grant's timestamp. Deliberately coarse: the
/// row is identified by its summary, not by the second it was granted.
fn granted_on(ms: u64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    date.to_locale_string("default", &wasm_bindgen::JsValue::UNDEFINED)
        .into()
}

#[component]
#[must_use]
pub fn StandingGrantsSection() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();

    let grants = RwSignal::new(Vec::<GrantView>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let notice = RwSignal::new(Option::<String>::None);
    // Bumped after a revoke so the list re-reads the server rather than
    // trusting a local removal: the authoritative answer to "what is still
    // standing" is the one the gate will consult.
    let reload = RwSignal::new(0_u32);

    let dash = state;
    Effect::new(move || {
        // Re-runs on reconnect as well as on the first paint: a page that only
        // loads once shows a stale (or empty) list for the rest of a session
        // that dropped its socket.
        if !dash.is_connected.get() {
            return;
        }
        let _ = reload.get();
        spawn_local(async move {
            loading.set(true);
            match ExecGrantsApi::list(&dash).await {
                Ok(rows) => {
                    grants.set(rows);
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| format!("Failed to load standing approvals: {e}"),
                    )));
                }
            }
            loading.set(false);
        });
    });

    let revoke = move |grant: GrantView| {
        spawn_local(async move {
            match ExecGrantsApi::revoke(&dash, &grant).await {
                Ok(()) => {
                    error.set(None);
                    notice.set(Some(
                        t_string!(i18n, settings.policies.grants_revoked).to_string(),
                    ));
                    reload.update(|n| *n += 1);
                }
                Err(e) => {
                    notice.set(None);
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to revoke: {e}")
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
                    {t!(i18n, settings.policies.grants_title)}
                </h2>
                <p class="text-xs text-text-tertiary mt-1">
                    {t!(i18n, settings.policies.grants_desc)}
                </p>
            </div>

            {move || error.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
            })}
            {move || notice.get().map(|m| view! {
                <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">{m}</div>
            })}

            {move || {
                if loading.get() {
                    return view! {
                        <div class="text-text-secondary py-4 text-center text-sm">
                            {t!(i18n, settings.policies.grants_loading)}
                        </div>
                    }.into_any();
                }
                let rows = grants.get();
                if rows.is_empty() {
                    return view! {
                        <div class="text-text-tertiary py-4 text-center text-sm">
                            {t!(i18n, settings.policies.grants_empty)}
                        </div>
                    }.into_any();
                }
                view! {
                    <div class="bg-surface-raised border border-border rounded-xl divide-y divide-border/50">
                        {rows.into_iter().map(|grant| {
                            let is_always = grant.scope == "always";
                            let summary = grant.summary.clone();
                            let tool = grant.tool.clone();
                            let when = granted_on(grant.granted_at_ms);
                            let row = grant.clone();
                            view! {
                                <div class="flex items-start justify-between gap-3 px-5 py-3">
                                    <div class="min-w-0">
                                        <div class="flex items-center gap-2">
                                            <span
                                                class="px-1.5 py-0.5 rounded text-[10px] font-semibold uppercase tracking-wide"
                                                class=("bg-warning-subtle", move || is_always)
                                                class=("text-warning", move || is_always)
                                                class=("bg-surface-sunken", move || !is_always)
                                                class=("text-text-secondary", move || !is_always)
                                            >
                                                {if is_always {
                                                    t_string!(i18n, settings.policies.grants_scope_always).to_string()
                                                } else {
                                                    t_string!(i18n, settings.policies.grants_scope_session).to_string()
                                                }}
                                            </span>
                                            <span class="text-sm font-medium text-text-primary">{tool}</span>
                                        </div>
                                        // The redacted line the human read on the card. Without it
                                        // this list is a list of hashes.
                                        <div class="font-mono text-xs text-text-secondary break-all mt-1">
                                            {summary}
                                        </div>
                                        <div class="text-[11px] text-text-tertiary mt-0.5">
                                            {t!(i18n, settings.policies.grants_granted_at)} " " {when}
                                        </div>
                                    </div>
                                    <button
                                        type="button"
                                        class="shrink-0 px-3 py-1 rounded border border-border text-xs text-text-secondary hover:bg-surface-sunken transition-colors"
                                        on:click=move |_| revoke(row.clone())
                                    >
                                        {t!(i18n, settings.policies.grants_revoke)}
                                    </button>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
