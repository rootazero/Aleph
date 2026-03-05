//! Vault Configuration View
//!
//! Provides UI for managing the vault master key:
//! - Vault status overview (file, key source, keychain availability, key counts)
//! - Store master key to OS keychain
//! - Verify / remove key from keychain
//! - Toggle vault encryption (enable/disable with master key input)

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{DisableVaultResult, MigrateKeysResult, VaultConfigApi, VaultStatus, VaultVerifyResult};
use crate::context::DashboardState;

#[component]
pub fn VaultView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let status = RwSignal::new(Option::<VaultStatus>::None);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    // Load status on mount / reconnect
    let load_status = move || {
        spawn_local(async move {
            loading.set(true);
            match VaultConfigApi::status(&state).await {
                Ok(s) => {
                    status.set(Some(s));
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load vault status: {}", e)));
                }
            }
            loading.set(false);
        });
    };

    create_effect(move |_| {
        if state.is_connected.get() {
            load_status();
        } else {
            loading.set(false);
        }
    });

    view! {
        <div class="flex-1 p-6 overflow-y-auto">
            <div class="max-w-4xl">
                <h1 class="text-2xl font-bold mb-2">"Vault"</h1>
                <p class="text-text-secondary mb-6">
                    "Manage the master key for encrypted secret storage. "
                    "The master key can be stored in your OS keychain (macOS Keychain, Windows Credential Manager, or Linux Secret Service) "
                    "or set via the ALEPH_MASTER_KEY environment variable."
                </p>

                {move || {
                    if loading.get() {
                        view! { <div class="text-text-tertiary">"Loading..."</div> }.into_any()
                    } else {
                        view! {
                            <div class="space-y-6">
                                {move || error.get().map(|e| view! {
                                    <div class="p-3 bg-danger-subtle text-danger rounded">
                                        {e}
                                    </div>
                                })}

                                <VaultStatusCard status=status />
                                <StoreKeySection status=status on_refresh=load_status />
                                <KeyManagementSection status=status on_refresh=load_status />
                                <VaultToggleSection status=status on_refresh=load_status />
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn VaultStatusCard(status: RwSignal<Option<VaultStatus>>) -> impl IntoView {
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-4">"Status"</h2>

            {move || status.get().map(|s| {
                let source_label = match s.master_key_source.as_deref() {
                    Some("env_var") => "Environment variable (ALEPH_MASTER_KEY)",
                    Some("keychain") => "OS Keychain",
                    _ => "Not configured",
                };
                let key_status_class = if s.master_key_configured {
                    "text-success"
                } else {
                    "text-warning"
                };

                view! {
                    <div class="space-y-3">
                        <div class="flex items-center justify-between">
                            <span class="text-text-secondary">"Master Key"</span>
                            <span class=key_status_class>
                                {if s.master_key_configured { "Configured" } else { "Not configured" }}
                            </span>
                        </div>
                        <div class="flex items-center justify-between">
                            <span class="text-text-secondary">"Key Source"</span>
                            <span class="text-text-primary">{source_label.to_string()}</span>
                        </div>
                        <div class="flex items-center justify-between">
                            <span class="text-text-secondary">"Vault File"</span>
                            <span class="text-text-primary">
                                {if s.vault_exists { "Exists" } else { "Not created yet" }}
                            </span>
                        </div>
                        <div class="flex items-center justify-between">
                            <span class="text-text-secondary">"Vault Path"</span>
                            <span class="text-text-tertiary text-xs font-mono">{s.vault_path.clone()}</span>
                        </div>
                        <div class="flex items-center justify-between">
                            <span class="text-text-secondary">"OS Keychain"</span>
                            <span class={if s.keychain_available { "text-success" } else { "text-warning" }}>
                                {if s.keychain_available { "Available" } else { "Unavailable" }}
                            </span>
                        </div>
                        <div class="flex items-center justify-between">
                            <span class="text-text-secondary">"Plaintext Keys"</span>
                            <span class="text-text-primary">{s.plaintext_key_count.to_string()}</span>
                        </div>
                        <div class="flex items-center justify-between">
                            <span class="text-text-secondary">"Encrypted Keys"</span>
                            <span class="text-text-primary">{s.encrypted_key_count.to_string()}</span>
                        </div>
                    </div>
                }
            })}
        </div>
    }
}

#[component]
fn StoreKeySection<F>(status: RwSignal<Option<VaultStatus>>, on_refresh: F) -> impl IntoView
where
    F: Fn() + Copy + Send + 'static,
{
    let state = expect_context::<DashboardState>();
    let key_input = RwSignal::new(String::new());
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);
    let show_key = RwSignal::new(false);

    let save_key = move |_| {
        let key = key_input.get();
        if key.is_empty() {
            save_error.set(Some("Master key cannot be empty".to_string()));
            return;
        }

        spawn_local(async move {
            saving.set(true);
            save_error.set(None);
            save_success.set(false);

            match VaultConfigApi::store_key(&state, key).await {
                Ok(()) => {
                    save_success.set(true);
                    key_input.set(String::new());
                    on_refresh();
                    set_timeout(
                        move || save_success.set(false),
                        std::time::Duration::from_secs(3),
                    );
                }
                Err(e) => {
                    save_error.set(Some(e));
                }
            }
            saving.set(false);
        });
    };

    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">"Store Master Key"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Save your master key to the OS keychain for seamless vault access without environment variables."
            </p>

            {move || {
                let keychain_available = status.get().map(|s| s.keychain_available).unwrap_or(false);
                if !keychain_available {
                    view! {
                        <div class="p-3 bg-warning-subtle border border-warning/20 rounded text-warning text-sm">
                            "OS keychain is not available. Use the ALEPH_MASTER_KEY environment variable instead."
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="space-y-4">
                            <div class="flex gap-2">
                                <div class="flex-1 relative">
                                    <input
                                        type=move || if show_key.get() { "text" } else { "password" }
                                        placeholder="Enter master key..."
                                        prop:value=move || key_input.get()
                                        on:input=move |ev| key_input.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary font-mono text-sm focus:border-primary focus:outline-none"
                                    />
                                </div>
                                <button
                                    on:click=move |_| show_key.set(!show_key.get())
                                    class="px-3 py-2 bg-surface-sunken border border-border rounded text-text-secondary hover:text-text-primary text-sm"
                                >
                                    {move || if show_key.get() { "Hide" } else { "Show" }}
                                </button>
                            </div>

                            {move || save_error.get().map(|e| view! {
                                <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                                    {e}
                                </div>
                            })}

                            {move || {
                                if save_success.get() {
                                    Some(view! {
                                        <div class="p-3 bg-success-subtle border border-success/20 rounded text-success text-sm">
                                            "Master key saved to keychain successfully"
                                        </div>
                                    })
                                } else {
                                    None
                                }
                            }}

                            <button
                                on:click=save_key
                                disabled=move || saving.get() || key_input.get().is_empty()
                                class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50"
                            >
                                {move || if saving.get() { "Saving..." } else { "Save to Keychain" }}
                            </button>
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn KeyManagementSection<F>(status: RwSignal<Option<VaultStatus>>, on_refresh: F) -> impl IntoView
where
    F: Fn() + Copy + Send + 'static,
{
    let state = expect_context::<DashboardState>();
    let verifying = RwSignal::new(false);
    let verify_result = RwSignal::new(Option::<VaultVerifyResult>::None);
    let deleting = RwSignal::new(false);
    let delete_error = RwSignal::new(Option::<String>::None);
    let confirm_delete = RwSignal::new(false);

    let verify_key = move |_| {
        spawn_local(async move {
            verifying.set(true);
            verify_result.set(None);
            match VaultConfigApi::verify(&state).await {
                Ok(result) => verify_result.set(Some(result)),
                Err(e) => verify_result.set(Some(VaultVerifyResult {
                    ok: false,
                    message: e,
                })),
            }
            verifying.set(false);
        });
    };

    let delete_key = move |_| {
        if !confirm_delete.get() {
            confirm_delete.set(true);
            return;
        }
        spawn_local(async move {
            deleting.set(true);
            delete_error.set(None);
            match VaultConfigApi::delete_key(&state).await {
                Ok(()) => {
                    confirm_delete.set(false);
                    on_refresh();
                }
                Err(e) => {
                    delete_error.set(Some(e));
                }
            }
            deleting.set(false);
        });
    };

    // Only show management when key is from keychain
    let show_management = move || {
        status
            .get()
            .map(|s| s.master_key_configured)
            .unwrap_or(false)
    };

    let is_keychain_source = move || {
        status
            .get()
            .map(|s| s.master_key_source.as_deref() == Some("keychain"))
            .unwrap_or(false)
    };

    view! {
        <div
            class="bg-surface-raised p-6 rounded-lg border border-border"
            style:display=move || if show_management() { "block" } else { "none" }
        >
            <h2 class="text-lg font-semibold mb-4">"Key Management"</h2>

            <div class="space-y-4">
                // Verify button (always available when key is configured)
                <div class="flex items-center gap-3">
                    <button
                        on:click=verify_key
                        disabled=move || verifying.get()
                        class="px-4 py-2 bg-info text-white rounded hover:bg-primary-hover disabled:opacity-50"
                    >
                        {move || if verifying.get() { "Verifying..." } else { "Verify Key" }}
                    </button>

                    {move || verify_result.get().map(|r| {
                        let class = if r.ok {
                            "p-3 bg-success-subtle border border-success/20 rounded text-success text-sm"
                        } else {
                            "p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm"
                        };
                        view! {
                            <div class=class>
                                {r.message}
                            </div>
                        }
                    })}
                </div>

                // Delete button (only when source is keychain)
                <div
                    class="pt-4 border-t border-border"
                    style:display=move || if is_keychain_source() { "block" } else { "none" }
                >
                    <p class="text-sm text-text-tertiary mb-3">
                        "Remove the master key from the OS keychain. The vault file and its contents will not be deleted."
                    </p>

                    {move || delete_error.get().map(|e| view! {
                        <div class="p-3 mb-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                            {e}
                        </div>
                    })}

                    <button
                        on:click=delete_key
                        disabled=move || deleting.get()
                        class="px-4 py-2 bg-danger text-white rounded hover:bg-danger disabled:opacity-50"
                    >
                        {move || {
                            if deleting.get() {
                                "Removing...".to_string()
                            } else if confirm_delete.get() {
                                "Click again to confirm".to_string()
                            } else {
                                "Remove from Keychain".to_string()
                            }
                        }}
                    </button>
                </div>
            </div>
        </div>
    }
}

/// Vault encryption toggle — enables or disables vault encryption.
///
/// State logic:
/// - plaintext > 0 && encrypted == 0 → "Enable Vault Encryption"
/// - encrypted > 0 && plaintext == 0 → "Disable Vault Encryption"
/// - plaintext > 0 && encrypted > 0 → "Encrypt Remaining Keys" (mixed)
/// - both 0 → hidden
#[component]
fn VaultToggleSection<F>(status: RwSignal<Option<VaultStatus>>, on_refresh: F) -> impl IntoView
where
    F: Fn() + Copy + Send + 'static,
{
    let state = expect_context::<DashboardState>();
    let key_input = RwSignal::new(String::new());
    let show_key = RwSignal::new(false);
    let operating = RwSignal::new(false);
    let op_error = RwSignal::new(Option::<String>::None);
    let confirm_action = RwSignal::new(false);
    let remove_from_keychain = RwSignal::new(false);
    let success_message = RwSignal::new(Option::<String>::None);
    let result_providers = RwSignal::new(Vec::<String>::new());

    #[derive(Clone, Copy, PartialEq)]
    enum ToggleMode {
        Enable,
        Disable,
        EncryptRemaining,
        Hidden,
    }

    let mode = move || {
        status.get().map(|s| {
            let pt = s.plaintext_key_count;
            let enc = s.encrypted_key_count;
            if pt == 0 && enc == 0 {
                ToggleMode::Hidden
            } else if pt > 0 && enc == 0 {
                ToggleMode::Enable
            } else if enc > 0 && pt == 0 {
                ToggleMode::Disable
            } else {
                ToggleMode::EncryptRemaining
            }
        }).unwrap_or(ToggleMode::Hidden)
    };

    let show_section = move || {
        mode() != ToggleMode::Hidden || success_message.get().is_some()
    };

    let do_action = move |_| {
        let key = key_input.get();
        if key.is_empty() {
            op_error.set(Some("Master key is required".to_string()));
            return;
        }

        if !confirm_action.get() {
            confirm_action.set(true);
            return;
        }

        let current_mode = mode();
        let remove_kc = remove_from_keychain.get();

        spawn_local(async move {
            operating.set(true);
            op_error.set(None);
            success_message.set(None);
            result_providers.set(Vec::new());
            confirm_action.set(false);

            match current_mode {
                ToggleMode::Enable | ToggleMode::EncryptRemaining => {
                    match VaultConfigApi::migrate_keys(&state, key).await {
                        Ok(result) => {
                            success_message.set(Some(format!(
                                "Encrypted {} provider key(s) into the vault.",
                                result.migrated_count
                            )));
                            result_providers.set(result.migrated_providers);
                            key_input.set(String::new());
                            on_refresh();
                        }
                        Err(e) => op_error.set(Some(e)),
                    }
                }
                ToggleMode::Disable => {
                    match VaultConfigApi::disable_vault(&state, key, remove_kc).await {
                        Ok(result) => {
                            let kc_msg = if result.keychain_removed {
                                " Master key removed from keychain."
                            } else {
                                ""
                            };
                            success_message.set(Some(format!(
                                "Restored {} provider key(s) to plaintext.{}",
                                result.restored_count, kc_msg
                            )));
                            result_providers.set(result.restored_providers);
                            key_input.set(String::new());
                            on_refresh();
                        }
                        Err(e) => op_error.set(Some(e)),
                    }
                }
                ToggleMode::Hidden => {}
            }
            operating.set(false);
        });
    };

    let section_title = move || match mode() {
        ToggleMode::Enable => "Enable Vault Encryption",
        ToggleMode::Disable => "Disable Vault Encryption",
        ToggleMode::EncryptRemaining => "Encrypt Remaining Keys",
        ToggleMode::Hidden => "",
    };

    let description = move || match mode() {
        ToggleMode::Enable => {
            let count = status.get().map(|s| s.plaintext_key_count).unwrap_or(0);
            format!(
                "Found {} provider(s) with plaintext API keys. Enter your master key to encrypt them into the vault.",
                count
            )
        }
        ToggleMode::Disable => {
            let count = status.get().map(|s| s.encrypted_key_count).unwrap_or(0);
            format!(
                "{} provider key(s) are encrypted in the vault. Enter your master key to restore them as plaintext in config.",
                count
            )
        }
        ToggleMode::EncryptRemaining => {
            let pt = status.get().map(|s| s.plaintext_key_count).unwrap_or(0);
            format!(
                "{} provider(s) still have plaintext keys. Enter your master key to encrypt the remaining keys.",
                pt
            )
        }
        ToggleMode::Hidden => String::new(),
    };

    let button_label = move || {
        if operating.get() {
            return "Processing...".to_string();
        }
        if confirm_action.get() {
            return "Click again to confirm".to_string();
        }
        match mode() {
            ToggleMode::Enable => "Enable Vault Encryption".to_string(),
            ToggleMode::Disable => "Disable Vault Encryption".to_string(),
            ToggleMode::EncryptRemaining => "Encrypt Remaining Keys".to_string(),
            ToggleMode::Hidden => String::new(),
        }
    };

    let button_class = move || {
        if mode() == ToggleMode::Disable {
            "px-4 py-2 bg-danger text-white rounded hover:bg-danger disabled:opacity-50"
        } else {
            "px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50"
        }
    };

    let is_disable_mode = move || mode() == ToggleMode::Disable;

    view! {
        <div
            class="bg-surface-raised p-6 rounded-lg border border-border"
            style:display=move || if show_section() { "block" } else { "none" }
        >
            <h2 class="text-lg font-semibold mb-2">{move || section_title()}</h2>

            {move || {
                if let Some(msg) = success_message.get() {
                    let providers = result_providers.get();
                    view! {
                        <div class="space-y-3">
                            <div class="p-3 bg-success-subtle border border-success/20 rounded text-success text-sm">
                                {msg}
                            </div>
                            {if !providers.is_empty() {
                                Some(view! {
                                    <ul class="list-disc list-inside text-sm text-text-secondary">
                                        {providers.iter().map(|p| {
                                            let name = p.clone();
                                            view! { <li>{name}</li> }
                                        }).collect::<Vec<_>>()}
                                    </ul>
                                })
                            } else {
                                None
                            }}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="space-y-4">
                            <p class="text-sm text-text-tertiary">
                                {move || description()}
                            </p>

                            // Master key input
                            <div class="flex gap-2">
                                <div class="flex-1 relative">
                                    <input
                                        type=move || if show_key.get() { "text" } else { "password" }
                                        placeholder="Enter master key..."
                                        prop:value=move || key_input.get()
                                        on:input=move |ev| {
                                            key_input.set(event_target_value(&ev));
                                            confirm_action.set(false);
                                        }
                                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary font-mono text-sm focus:border-primary focus:outline-none"
                                    />
                                </div>
                                <button
                                    on:click=move |_| show_key.set(!show_key.get())
                                    class="px-3 py-2 bg-surface-sunken border border-border rounded text-text-secondary hover:text-text-primary text-sm"
                                >
                                    {move || if show_key.get() { "Hide" } else { "Show" }}
                                </button>
                            </div>

                            // "Also remove from keychain" checkbox (disable mode only)
                            <div style:display=move || if is_disable_mode() { "block" } else { "none" }>
                                <label class="flex items-center gap-2 text-sm text-text-secondary cursor-pointer">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || remove_from_keychain.get()
                                        on:change=move |ev| remove_from_keychain.set(event_target_checked(&ev))
                                        class="rounded border-border"
                                    />
                                    "Also remove master key from OS keychain"
                                </label>
                            </div>

                            // Error
                            {move || op_error.get().map(|e| view! {
                                <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                                    {e}
                                </div>
                            })}

                            // Action button
                            <button
                                on:click=do_action
                                disabled=move || operating.get() || key_input.get().is_empty()
                                class=move || button_class()
                            >
                                {move || button_label()}
                            </button>
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}
