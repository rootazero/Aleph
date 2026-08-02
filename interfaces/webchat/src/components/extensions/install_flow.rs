//! Install state machine (Task 8). Owns the multi-step trust→configure→verify flow
//! behind the trust gate. The in-flight extension's id AND entry persist on
//! `StoreState` (`install_id`/`install_entry`/`install_missing`) so no step depends
//! on `store.selected` — the user may close the drawer mid-flow.
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;

use crate::api::extensions::{ExtensionsApi, InstallResult};
use crate::components::json_schema_form::{fields_from, JsonSchemaForm};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use crate::views::extensions::browse::load_catalog;
use crate::views::extensions::StoreState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStep {
    Hidden,
    Trust,
    Configure,
    Installing,
    Done,
    /// Installed, but post-install verification said the artifact is not healthy
    /// (MCP server not running or exposing zero tools; artifact missing on disk).
    /// Held open for the user instead of flashing the success toast.
    DoneUnhealthy,
    Failed,
}

/// Pure: `verify.ok` from an install result's verify payload. An absent or
/// unrecognised shape counts as healthy — never cry wolf over a payload we do
/// not understand.
#[must_use]
pub fn verify_ok(verify: &Value) -> bool {
    verify.get("ok").and_then(Value::as_bool).unwrap_or(true)
}

/// Human-readable reason from a failed verify payload.
#[must_use]
pub fn verify_detail(verify: &Value) -> Option<String> {
    verify
        .get("detail")
        .or_else(|| verify.get("error"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Pure: route an `extensions.install` result to its next UI step.
///
/// A `Done` whose verification failed routes to `DoneUnhealthy`: the backend runs
/// a real post-install check (server up, ≥1 tool, artifact on disk) and its
/// verdict used to be carried to the Panel and dropped — every install read as
/// "Installed ✓" even when the server never came up.
#[must_use]
pub fn next_step(result: &InstallResult) -> InstallStep {
    match result {
        InstallResult::NeedsAck { .. } => InstallStep::Trust,
        InstallResult::Missing { .. } => InstallStep::Configure,
        InstallResult::Done { verify, .. } => {
            if verify_ok(verify) {
                InstallStep::Done
            } else {
                InstallStep::DoneUnhealthy
            }
        }
    }
}

/// Call install with current values+ack and route to the next step. Uses the
/// in-flight `id` (passed by the caller from `store.install_id`), never `selected`.
fn drive_install(state: DashboardState, store: StoreState, id: String, ack: bool) {
    store.installing.set(true);
    store.install_error.set(None);
    let values = Value::Object(store.config_values.get_untracked());
    spawn_local(async move {
        match ExtensionsApi::install(&state, id, values, ack).await {
            Ok(result) => {
                match &result {
                    InstallResult::NeedsAck { disclosure, .. } => {
                        store.disclosure.set(Some(disclosure.clone()));
                    }
                    InstallResult::Missing { missing } => {
                        store.install_missing.set(missing.clone());
                    }
                    InstallResult::Done { verify, .. } => {
                        store.install_warning.set(if verify_ok(verify) {
                            None
                        } else {
                            Some(verify_detail(verify).unwrap_or_default())
                        });
                    }
                }
                store.install_step.set(next_step(&result));
                store.installing.set(false);
            }
            Err(e) => {
                store.install_error.set(Some(e));
                store.install_step.set(InstallStep::Failed);
                store.installing.set(false);
            }
        }
    });
}

#[component]
#[must_use]
pub fn InstallFlow() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();

    // start_install set a target → fire the first install probe (no values, no ack).
    Effect::new(move || {
        if let Some(entry) = store.install_target.get() {
            store.install_target.set(None); // consume
            store.install_step.set(InstallStep::Installing);
            drive_install(state, store, entry.id.clone(), false);
        }
    });

    // On reaching Done: refresh the catalog so cards flip to "Installed", then close
    // the flow. Writing `install_step = Hidden` settles the effect on its re-run.
    Effect::new(move || {
        if store.install_step.get() == InstallStep::Done {
            load_catalog(state, store, i18n, true);
            store.disclosure.set(None);
            store.install_missing.set(Vec::new());
            store.install_step.set(InstallStep::Hidden);
        }
    });

    // Unhealthy is still installed, so the cards must flip — but the panel stays
    // open for the user to read the reason (no `Hidden` write here).
    Effect::new(move || {
        if store.install_step.get() == InstallStep::DoneUnhealthy {
            load_catalog(state, store, i18n, true);
        }
    });

    let close = move || {
        store.install_step.set(InstallStep::Hidden);
        store.disclosure.set(None);
        store.install_error.set(None);
        store.install_warning.set(None);
    };

    // Trust "Continue" → re-install with the in-flight id and ack=true.
    let on_trust_continue = move || {
        if let Some(id) = store.install_id.get_untracked() {
            drive_install(state, store, id, true);
        }
    };
    // Configure "Install & verify" → re-install with values + ack=true.
    let on_configure_submit = move || {
        if let Some(id) = store.install_id.get_untracked() {
            drive_install(state, store, id, true);
        }
    };

    view! {
        // Trust step
        <Show when=move || store.install_step.get() == InstallStep::Trust>
            <crate::components::extensions::trust_modal::TrustModal
                on_continue=Callback::new(move |()| on_trust_continue())
                on_cancel=Callback::new(move |()| close())
            />
        </Show>

        // Configure step — JsonSchemaForm over fields_from(schema, secrets, missing)
        <Show when=move || store.install_step.get() == InstallStep::Configure>
            {move || {
                let entry = store.install_entry.get();
                let secrets = store.disclosure.get().map(|d| d.secrets).unwrap_or_default();
                let missing = store.install_missing.get();
                let fields = fields_from(
                    entry.as_ref().and_then(|e| e.config_schema.as_ref()),
                    &secrets,
                    &missing,
                );
                view! {
                    <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
                        <div class="aleph-scrim absolute inset-0 bg-black/40" on:click=move |_| close()></div>
                        <div class="glass relative w-[480px] max-w-[94vw] bg-surface-overlay/90 border border-border rounded-xl shadow-xl flex flex-col max-h-[88vh]">
                            <header class="px-5 pt-4 pb-2">
                                <h2 class="font-serif text-xl text-text-primary leading-tight">{t!(i18n, extensions.configure_title)}</h2>
                            </header>
                            <div class="flex-1 overflow-y-auto px-5 py-2">
                                <JsonSchemaForm fields=fields values=store.config_values />
                            </div>
                            <footer class="px-5 py-3 border-t border-border flex gap-2 justify-end">
                                <button
                                    class="px-4 py-2 bg-surface-sunken text-text-secondary rounded-lg text-sm hover:bg-surface-raised"
                                    on:click=move |_| close()
                                >
                                    {t!(i18n, extensions.cancel)}
                                </button>
                                <button
                                    class="px-4 py-2 bg-primary text-white rounded-lg text-sm hover:bg-primary-hover"
                                    on:click=move |_| on_configure_submit()
                                >
                                    {t!(i18n, extensions.install_and_verify)}
                                </button>
                            </footer>
                        </div>
                    </div>
                }
            }}
        </Show>

        // Installing step — lightweight overlay/spinner
        <Show when=move || store.install_step.get() == InstallStep::Installing>
            <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
                <div class="aleph-scrim absolute inset-0 bg-black/40"></div>
                <div class="glass relative bg-surface-overlay/90 border border-border rounded-xl shadow-xl px-6 py-5 flex items-center gap-3">
                    <div class="animate-spin rounded-full h-5 w-5 border-b-2 border-primary"></div>
                    <span class="text-sm text-text-secondary">{t!(i18n, extensions.installing)}</span>
                </div>
            </div>
        </Show>

        // Done step — transient success toast (the close/refresh runs in the Effect above)
        <Show when=move || store.install_step.get() == InstallStep::Done>
            <div class="fixed inset-0 z-50 flex items-center justify-center p-4 pointer-events-none">
                <div class="glass bg-surface-overlay/90 border border-border rounded-xl shadow-xl px-6 py-4 text-sm text-success">
                    {t!(i18n, extensions.install_done)}
                </div>
            </div>
        </Show>

        // Installed-but-unhealthy — held open with the verifier's reason, so a
        // server that never came up is not reported as a plain success.
        <Show when=move || store.install_step.get() == InstallStep::DoneUnhealthy>
            <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
                <div class="aleph-scrim absolute inset-0 bg-black/40" on:click=move |_| close()></div>
                <div class="glass relative w-[420px] max-w-[94vw] bg-surface-overlay/90 border border-border rounded-xl shadow-xl flex flex-col">
                    <div class="px-5 py-4 space-y-2">
                        <h2 class="font-serif text-lg text-warning">{t!(i18n, extensions.install_unhealthy)}</h2>
                        <p class="text-sm text-text-secondary">{t!(i18n, extensions.install_unhealthy_hint)}</p>
                        {move || store.install_warning.get().filter(|d| !d.is_empty()).map(|d| view! {
                            <p class="text-xs font-mono text-text-tertiary break-words">{d}</p>
                        })}
                    </div>
                    <footer class="px-5 py-3 border-t border-border flex justify-end">
                        <button
                            class="px-4 py-2 bg-surface-sunken text-text-secondary rounded-lg text-sm hover:bg-surface-raised"
                            on:click=move |_| close()
                        >
                            {t!(i18n, extensions.cancel)}
                        </button>
                    </footer>
                </div>
            </div>
        </Show>

        // Failed step — show the error with a Close
        <Show when=move || store.install_step.get() == InstallStep::Failed>
            <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
                <div class="aleph-scrim absolute inset-0 bg-black/40" on:click=move |_| close()></div>
                <div class="glass relative w-[420px] max-w-[94vw] bg-surface-overlay/90 border border-border rounded-xl shadow-xl flex flex-col">
                    <div class="px-5 py-4 space-y-2">
                        <h2 class="font-serif text-lg text-danger">{t!(i18n, extensions.install_failed)}</h2>
                        <p class="text-sm text-text-secondary break-words">
                            {move || store.install_error.get().unwrap_or_default()}
                        </p>
                    </div>
                    <footer class="px-5 py-3 border-t border-border flex justify-end">
                        <button
                            class="px-4 py-2 bg-surface-sunken text-text-secondary rounded-lg text-sm hover:bg-surface-raised"
                            on:click=move |_| close()
                        >
                            {t!(i18n, extensions.cancel)}
                        </button>
                    </footer>
                </div>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::extensions::{DisclosurePayload, InstallResult};

    fn disc(ack: bool) -> DisclosurePayload {
        DisclosurePayload {
            tier: "community".into(),
            risk: "runs_commands".into(),
            one_line: "x".into(),
            command_display: None,
            secrets: vec![],
            version: None,
            sha256: None,
            ack_required: ack,
        }
    }

    #[test]
    fn needs_ack_goes_to_trust() {
        let r = InstallResult::NeedsAck {
            disclosure: disc(true),
            injection_findings: vec![],
        };
        assert_eq!(next_step(&r), InstallStep::Trust);
    }
    #[test]
    fn missing_goes_to_configure() {
        let r = InstallResult::Missing {
            missing: vec!["TOKEN".into()],
        };
        assert_eq!(next_step(&r), InstallStep::Configure);
    }
    fn done_with(verify: Value) -> InstallResult {
        InstallResult::Done {
            outcome: Value::Null,
            verify,
            pin: Value::Null,
            injection_findings: vec![],
        }
    }

    #[test]
    fn done_goes_to_done() {
        assert_eq!(
            next_step(&done_with(
                serde_json::json!({"ok": true, "detail": "running; 7 tools"})
            )),
            InstallStep::Done
        );
    }

    /// The backend really verifies the install; a failed verdict must not render
    /// as "Installed ✓". This is the wire that used to be dropped.
    #[test]
    fn done_with_failed_verify_goes_to_unhealthy() {
        let r =
            done_with(serde_json::json!({"ok": false, "detail": "running but exposes 0 tools"}));
        assert_eq!(next_step(&r), InstallStep::DoneUnhealthy);
        let InstallResult::Done { verify, .. } = &r else {
            unreachable!()
        };
        assert_eq!(
            verify_detail(verify).as_deref(),
            Some("running but exposes 0 tools")
        );
    }

    /// An absent or unfamiliar verify payload (older server) is treated as
    /// healthy — the flow must not invent a warning it cannot substantiate.
    #[test]
    fn unknown_verify_shape_is_treated_as_healthy() {
        assert_eq!(next_step(&done_with(Value::Null)), InstallStep::Done);
        assert_eq!(
            next_step(&done_with(serde_json::json!({"something": 1}))),
            InstallStep::Done
        );
        assert!(verify_ok(&Value::Null));
        assert!(verify_detail(&Value::Null).is_none());
    }

    /// The legacy `{"ok":false,"error":...}` shape still yields a reason.
    #[test]
    fn verify_detail_falls_back_to_error_key() {
        let v = serde_json::json!({"ok": false, "error": "mcp manager unavailable"});
        assert!(!verify_ok(&v));
        assert_eq!(
            verify_detail(&v).as_deref(),
            Some("mcp manager unavailable")
        );
    }
}
