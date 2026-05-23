//! PairingModal — auto-triggered modal that drives the `wizard.*` RPC handshake
//! when `auth.connect` returns `pairing_required`.
//!
//! Flow:
//!   1. DashboardState.pairing_required becomes Some(_)
//!   2. Modal appears; calls wizard.start { wizard_type: "pairing" }
//!   3. First wizard.next returns the welcome note  (auto-advanced by PairingFlow)
//!   4. Second wizard.next returns the confirm step with the pairing code
//!   5. User clicks "Approve" → wizard.answer { step_id, value: true }
//!   6. wizard.next returns done=true with data.token
//!   7. set_pairing_token saves to localStorage; reconnect() re-authenticates

use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Modal overlay that is conditionally rendered whenever `DashboardState.pairing_required`
/// is `Some(_)`.  Mount this at the App root level so it overlays everything.
#[component]
pub fn PairingModal() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let pairing = state.pairing_required;

    move || {
        if pairing.get().is_some() {
            view! { <PairingModalInner /> }.into_any()
        } else {
            ().into_any()
        }
    }
}

/// Inner modal rendered while pairing is in progress.
#[component]
fn PairingModalInner() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Wizard state signals (local to this modal instance)
    let session_id = RwSignal::new(Option::<String>::None);
    let pairing_code = RwSignal::new(Option::<String>::None);
    let step_id = RwSignal::new(Option::<String>::None);
    let error_msg = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(true);
    let approving = RwSignal::new(false);
    let done = RwSignal::new(false);

    // --- Start the wizard on mount ---
    spawn_local(async move {
        match state
            .rpc_call(
                "wizard.start",
                serde_json::json!({ "wizard_type": "pairing" }),
            )
            .await
        {
            Err(e) => {
                error_msg.set(Some(format!("Failed to start pairing wizard: {e}")));
                loading.set(false);
            }
            Ok(resp) => {
                // Store session_id
                let sid = resp
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                session_id.set(Some(sid.clone()));

                // wizard.start already returns the first step (welcome note).
                // PairingFlow auto-advances past the note, so we need to call
                // wizard.next once to get the confirm step with the code.
                match state
                    .rpc_call(
                        "wizard.next",
                        serde_json::json!({ "session_id": sid }),
                    )
                    .await
                {
                    Err(e) => {
                        error_msg.set(Some(format!("Failed to get pairing step: {e}")));
                        loading.set(false);
                    }
                    Ok(next_resp) => {
                        // Extract pairing code from the confirm step message
                        if let Some(step) = next_resp.get("step") {
                            // step.id is "pairing-approve"
                            let sid_val = step
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("pairing-approve")
                                .to_string();
                            step_id.set(Some(sid_val));

                            // Message contains "本机配对码：XXXXXX\n..."
                            let code = step
                                .get("message")
                                .and_then(|v| v.as_str())
                                .and_then(|msg| {
                                    // Extract code between "配对码：" and "\n"
                                    let prefix = "配对码：";
                                    msg.find(prefix).map(|idx| {
                                        let rest = &msg[idx + prefix.len()..];
                                        rest.split('\n').next().unwrap_or(rest).trim().to_string()
                                    })
                                });
                            pairing_code.set(code);
                        }
                        loading.set(false);
                    }
                }
            }
        }
    });

    // --- Approve handler ---
    let on_approve = move |_| {
        let sid = session_id.get();
        let sid_val = match &sid {
            Some(s) if !s.is_empty() => s.clone(),
            _ => {
                error_msg.set(Some("No active wizard session".to_string()));
                return;
            }
        };
        let step_id_val = step_id
            .get()
            .unwrap_or_else(|| "pairing-approve".to_string());

        approving.set(true);
        error_msg.set(None);

        spawn_local(async move {
            // Answer the confirm step
            match state
                .rpc_call(
                    "wizard.answer",
                    serde_json::json!({
                        "session_id": sid_val.clone(),
                        "step_id": step_id_val,
                        "value": true
                    }),
                )
                .await
            {
                Err(e) => {
                    error_msg.set(Some(format!("Approval failed: {e}")));
                    approving.set(false);
                }
                Ok(resp) => {
                    // wizard.answer returns WizardNextResult; check for done + data.token
                    let is_done = resp.get("done").and_then(|v| v.as_bool()).unwrap_or(false);
                    if is_done {
                        if let Some(token) = resp
                            .get("data")
                            .and_then(|d| d.get("token"))
                            .and_then(|t| t.as_str())
                        {
                            // Persist token and clear modal
                            state.set_pairing_token(token.to_string());
                            done.set(true);

                            // Reconnect with the new device token
                            let state_clone = state;
                            spawn_local(async move {
                                if let Err(e) = state_clone.reconnect().await {
                                    web_sys::console::error_1(
                                        &format!("Reconnect after pairing failed: {e}").into(),
                                    );
                                }
                            });
                        } else {
                            // Done but no token — show error
                            let err = resp
                                .get("error")
                                .and_then(|e| e.as_str())
                                .unwrap_or("Pairing completed but no token was returned")
                                .to_string();
                            error_msg.set(Some(err));
                            approving.set(false);
                        }
                    } else {
                        error_msg.set(Some("Unexpected: wizard not done after approval".to_string()));
                        approving.set(false);
                    }
                }
            }
        });
    };

    // --- Cancel handler ---
    let on_cancel = move |_| {
        let sid = session_id.get();
        if let Some(s) = sid.filter(|s| !s.is_empty()) {
            spawn_local(async move {
                let _ = state
                    .rpc_call("wizard.cancel", serde_json::json!({ "session_id": s }))
                    .await;
            });
        }
        // Clear modal regardless of cancel RPC result
        state.pairing_required.set(None);
    };

    view! {
        // Backdrop
        <div class="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
            // Modal card
            <div class="bg-surface border border-border rounded-xl p-8 max-w-md w-full mx-4 shadow-2xl">
                // Header
                <div class="flex items-center gap-3 mb-6">
                    <div class="w-10 h-10 rounded-full bg-primary/10 flex items-center justify-center flex-shrink-0">
                        <svg width="20" height="20" viewBox="0 0 24 24" fill="none"
                            stroke="currentColor" stroke-width="2" stroke-linecap="round"
                            stroke-linejoin="round" class="text-primary">
                            <rect x="3" y="11" width="18" height="11" rx="2" ry="2"/>
                            <path d="M7 11V7a5 5 0 0 1 10 0v4"/>
                        </svg>
                    </div>
                    <div>
                        <h2 class="text-lg font-semibold text-text-primary">"Device Pairing Required"</h2>
                        <p class="text-sm text-text-tertiary">"Approve this device to access Aleph"</p>
                    </div>
                </div>

                // Body: loading / code display / done states
                {move || {
                    if loading.get() {
                        view! {
                            <div class="flex items-center gap-3 py-6 text-text-secondary">
                                <div class="w-5 h-5 border-2 border-primary border-t-transparent rounded-full animate-spin" />
                                <span class="text-sm">"Starting pairing wizard…"</span>
                            </div>
                        }.into_any()
                    } else if done.get() {
                        view! {
                            <div class="flex items-center gap-3 py-6 text-success">
                                <svg width="20" height="20" viewBox="0 0 24 24" fill="none"
                                    stroke="currentColor" stroke-width="2.5">
                                    <polyline points="20 6 9 17 4 12"/>
                                </svg>
                                <span class="text-sm font-medium">"Pairing approved! Reconnecting…"</span>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="space-y-5">
                                // Pairing code display
                                <div class="bg-surface-sunken rounded-lg p-5 text-center border border-border">
                                    <p class="text-xs text-text-tertiary mb-2 uppercase tracking-widest">"Pairing Code"</p>
                                    {move || {
                                        match pairing_code.get() {
                                            Some(code) => view! {
                                                <span class="font-mono text-3xl font-bold text-primary tracking-widest">
                                                    {code}
                                                </span>
                                            }.into_any(),
                                            None => view! {
                                                <span class="text-text-tertiary text-sm">"Fetching code…"</span>
                                            }.into_any(),
                                        }
                                    }}
                                </div>

                                // Instructions
                                <p class="text-sm text-text-secondary">
                                    "Run the following command on the server to approve, then click "
                                    <strong class="text-text-primary">"Approve"</strong>
                                    " below:"
                                </p>
                                {move || {
                                    pairing_code.get().map(|code| view! {
                                        <div class="bg-surface-sunken rounded-lg p-3 font-mono text-xs text-text-secondary break-all">
                                            {format!("aleph-server pairing approve {}", code)}
                                        </div>
                                    })
                                }}

                                // Error message
                                {move || error_msg.get().map(|e| view! {
                                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                                        {e}
                                    </div>
                                })}

                                // Action buttons
                                <div class="flex gap-3 pt-1">
                                    <button
                                        class="flex-1 py-2.5 px-4 rounded-lg border border-border text-text-secondary hover:bg-surface-raised transition-colors text-sm"
                                        on:click=on_cancel
                                        disabled=move || approving.get()
                                    >
                                        "Cancel"
                                    </button>
                                    <button
                                        class="flex-1 py-2.5 px-4 rounded-lg bg-primary hover:bg-primary/90 text-white font-medium transition-colors text-sm disabled:opacity-50 disabled:cursor-not-allowed"
                                        on:click=on_approve
                                        disabled=move || approving.get() || session_id.get().is_none()
                                    >
                                        {move || if approving.get() {
                                            view! {
                                                <span class="flex items-center justify-center gap-2">
                                                    <span class="w-4 h-4 border-2 border-white border-t-transparent rounded-full animate-spin" />
                                                    "Approving…"
                                                </span>
                                            }.into_any()
                                        } else {
                                            view! { <span>"Approve"</span> }.into_any()
                                        }}
                                    </button>
                                </div>
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}
