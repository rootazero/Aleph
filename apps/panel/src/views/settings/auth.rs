//! Token Authentication Settings View
//!
//! Visual configuration for the three-layer auth system:
//! - Shared token display / regeneration
//! - Active HTTP sessions management
//! - Auth mode indicator

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{AuthTokenApi, AuthTokenInfo, SessionInfo};
use crate::context::DashboardState;

#[component]
pub fn AuthView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let token_info = RwSignal::new(Option::<AuthTokenInfo>::None);
    let sessions = RwSignal::new(Vec::<SessionInfo>::new());
    let session_count = RwSignal::new(0u64);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    // Load data on mount / reconnect
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                loading.set(true);
                error.set(None);

                // Load token info
                match AuthTokenApi::show_token(&state).await {
                    Ok(info) => token_info.set(Some(info)),
                    Err(e) => error.set(Some(format!("Failed to load token: {}", e))),
                }

                // Load sessions
                match AuthTokenApi::list_sessions(&state).await {
                    Ok(resp) => {
                        session_count.set(resp.count);
                        sessions.set(resp.sessions);
                    }
                    Err(e) => {
                        if error.get().is_none() {
                            error.set(Some(format!("Failed to load sessions: {}", e)));
                        }
                    }
                }

                loading.set(false);
            });
        } else {
            loading.set(false);
        }
    });

    view! {
        <div class="flex-1 p-6 overflow-y-auto">
            <div class="max-w-4xl">
                <div class="mb-8">
                    <h1 class="text-2xl font-bold text-text-primary">"Token Authentication"</h1>
                    <p class="text-text-secondary mt-1">
                        "Manage access tokens and active sessions for Panel UI and API access"
                    </p>
                </div>

                {move || {
                    if loading.get() {
                        view! {
                            <div class="flex items-center gap-2 text-text-tertiary py-8">
                                <svg class="animate-spin h-4 w-4" viewBox="0 0 24 24" fill="none">
                                    <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
                                    <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                                </svg>
                                "Loading authentication settings..."
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="space-y-6">
                                {move || error.get().map(|e| view! {
                                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                                        {e}
                                    </div>
                                })}

                                <SharedTokenSection
                                    token_info=token_info
                                    error=error
                                />
                                <ActiveSessionsSection
                                    sessions=sessions
                                    session_count=session_count
                                    error=error
                                />
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// =============================================================================
// Shared Token Section
// =============================================================================

#[component]
fn SharedTokenSection(
    token_info: RwSignal<Option<AuthTokenInfo>>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let visible = RwSignal::new(false);
    let regenerating = RwSignal::new(false);
    let show_confirm = RwSignal::new(false);
    let copied = RwSignal::new(false);

    let regenerate = move || {
        show_confirm.set(false);
        regenerating.set(true);
        spawn_local(async move {
            match AuthTokenApi::reset_token(&state).await {
                Ok(info) => {
                    token_info.set(Some(info));
                    visible.set(true); // Show the new token
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to regenerate token: {}", e)));
                }
            }
            regenerating.set(false);
        });
    };

    let copy_token = move || {
        if let Some(info) = token_info.get() {
            if let Some(token) = info.token {
                // Use js_sys::eval for clipboard access to avoid extra web-sys features
                let js = format!("navigator.clipboard.writeText('{}')", token.replace('\'', "\\'"));
                let _ = js_sys::eval(&js);
                copied.set(true);
                set_timeout(
                    move || copied.set(false),
                    std::time::Duration::from_secs(2),
                );
            }
        }
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <div class="flex items-center justify-between mb-4">
                <div>
                    <h2 class="text-lg font-semibold text-text-primary">"Shared Access Token"</h2>
                    <p class="text-sm text-text-tertiary mt-1">
                        "Used for Panel login, API access (Bearer header), and WebSocket authentication"
                    </p>
                </div>
                <div class="flex items-center gap-2 px-3 py-1 rounded-full bg-success-subtle text-success text-xs font-medium">
                    <svg class="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>
                    </svg>
                    "Active"
                </div>
            </div>

            // Token display
            <div class="mb-4">
                <label class="block text-sm font-medium text-text-secondary mb-2">"Current Token"</label>
                <div class="flex items-center gap-2">
                    <div class="flex-1 px-4 py-2.5 bg-surface-sunken border border-border rounded-lg font-mono text-sm select-all">
                        {move || {
                            match token_info.get() {
                                Some(info) => match info.token {
                                    Some(t) if visible.get() => t,
                                    Some(_) => "••••••••••••••••••••••••••••••••••••••••••".to_string(),
                                    None => "Token not in memory — check ~/.aleph/data/.shared_token".to_string(),
                                },
                                None => "Loading...".to_string(),
                            }
                        }}
                    </div>

                    // Toggle visibility
                    <button
                        on:click=move |_| visible.update(|v| *v = !*v)
                        class="p-2.5 bg-surface-sunken border border-border rounded-lg hover:bg-surface text-text-secondary hover:text-text-primary transition-colors"
                        title=move || if visible.get() { "Hide token" } else { "Show token" }
                    >
                        {move || if visible.get() {
                            view! {
                                <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                    <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/>
                                    <line x1="1" y1="1" x2="23" y2="23"/>
                                </svg>
                            }.into_any()
                        } else {
                            view! {
                                <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                    <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/>
                                    <circle cx="12" cy="12" r="3"/>
                                </svg>
                            }.into_any()
                        }}
                    </button>

                    // Copy button
                    <button
                        on:click=move |_| copy_token()
                        class="p-2.5 bg-surface-sunken border border-border rounded-lg hover:bg-surface text-text-secondary hover:text-text-primary transition-colors"
                        title="Copy token"
                    >
                        {move || if copied.get() {
                            view! {
                                <svg class="w-4 h-4 text-success" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                    <polyline points="20 6 9 17 4 12"/>
                                </svg>
                            }.into_any()
                        } else {
                            view! {
                                <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                    <rect x="9" y="9" width="13" height="13" rx="2"/>
                                    <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                                </svg>
                            }.into_any()
                        }}
                    </button>
                </div>
            </div>

            // Regenerate token
            <div class="pt-4 border-t border-border">
                {move || if show_confirm.get() {
                    view! {
                        <div class="flex items-center gap-3 p-3 bg-warning-subtle border border-warning/20 rounded-lg">
                            <svg class="w-5 h-5 text-warning flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/>
                                <line x1="12" y1="9" x2="12" y2="13"/>
                                <line x1="12" y1="17" x2="12.01" y2="17"/>
                            </svg>
                            <div class="flex-1">
                                <p class="text-sm font-medium text-text-primary">"Regenerate token?"</p>
                                <p class="text-xs text-text-tertiary">"All API clients using the current token will lose access."</p>
                            </div>
                            <div class="flex gap-2">
                                <button
                                    on:click=move |_| show_confirm.set(false)
                                    class="px-3 py-1.5 text-sm bg-surface border border-border rounded-lg hover:bg-surface-raised"
                                >
                                    "Cancel"
                                </button>
                                <button
                                    on:click=move |_| regenerate()
                                    class="px-3 py-1.5 text-sm bg-danger text-white rounded-lg hover:opacity-90"
                                >
                                    "Confirm"
                                </button>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <button
                            on:click=move |_| show_confirm.set(true)
                            prop:disabled=move || regenerating.get()
                            class="flex items-center gap-2 px-4 py-2 text-sm bg-surface-sunken border border-border rounded-lg hover:bg-surface text-text-secondary hover:text-text-primary transition-colors disabled:opacity-50"
                        >
                            {move || if regenerating.get() {
                                view! {
                                    <svg class="animate-spin w-4 h-4" viewBox="0 0 24 24" fill="none">
                                        <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
                                        <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                                    </svg>
                                    "Regenerating..."
                                }.into_any()
                            } else {
                                view! {
                                    <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                        <polyline points="23 4 23 10 17 10"/>
                                        <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/>
                                    </svg>
                                    "Regenerate Token"
                                }.into_any()
                            }}
                        </button>
                    }.into_any()
                }}
            </div>
        </div>
    }
}

// =============================================================================
// Active Sessions Section
// =============================================================================

#[component]
fn ActiveSessionsSection(
    sessions: RwSignal<Vec<SessionInfo>>,
    session_count: RwSignal<u64>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let revoke_session = move |session_id: String| {
        spawn_local(async move {
            match AuthTokenApi::revoke_session(&state, &session_id).await {
                Ok(()) => {
                    // Reload sessions
                    match AuthTokenApi::list_sessions(&state).await {
                        Ok(resp) => {
                            session_count.set(resp.count);
                            sessions.set(resp.sessions);
                        }
                        Err(e) => error.set(Some(format!("Failed to reload sessions: {}", e))),
                    }
                }
                Err(e) => error.set(Some(format!("Failed to revoke session: {}", e))),
            }
        });
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <div class="flex items-center justify-between mb-4">
                <div>
                    <h2 class="text-lg font-semibold text-text-primary">"Active Sessions"</h2>
                    <p class="text-sm text-text-tertiary mt-1">
                        "Browser sessions authenticated via shared token login"
                    </p>
                </div>
                <div class="px-3 py-1 rounded-full bg-info-subtle text-info text-xs font-medium">
                    {move || format!("{} active", session_count.get())}
                </div>
            </div>

            <div class="space-y-2">
                {move || {
                    let sess_list = sessions.get();
                    if sess_list.is_empty() {
                        view! {
                            <div class="text-center py-8 text-text-tertiary">
                                <svg class="w-8 h-8 mx-auto mb-2 opacity-50" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
                                    <rect x="3" y="11" width="18" height="11" rx="2"/>
                                    <path d="M7 11V7a5 5 0 0 1 10 0v4"/>
                                </svg>
                                <p class="text-sm">"No active sessions"</p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="space-y-2">
                                {sess_list.into_iter().map(|session| {
                                    let sid_revoke = session.session_id.clone();
                                    view! {
                                        <SessionCard
                                            session=session
                                            on_revoke=move || revoke_session(sid_revoke.clone())
                                        />
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn SessionCard<F>(
    session: SessionInfo,
    on_revoke: F,
) -> impl IntoView
where
    F: Fn() + 'static,
{
    let created = format_timestamp(session.created_at);
    let expires = format_timestamp(session.expires_at);
    let last_used = format_timestamp(session.last_used_at);
    let short_id = format!("{}...", session.session_id.get(..8).unwrap_or(&session.session_id));

    view! {
        <div class="flex items-center justify-between p-4 bg-surface-sunken rounded-lg border border-border">
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                    <svg class="w-4 h-4 text-text-tertiary flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <rect x="2" y="3" width="20" height="14" rx="2"/>
                        <line x1="8" y1="21" x2="16" y2="21"/>
                        <line x1="12" y1="17" x2="12" y2="21"/>
                    </svg>
                    <span class="font-mono text-sm text-text-primary">{short_id.clone()}</span>
                    <span class="text-xs px-1.5 py-0.5 rounded bg-success-subtle text-success">"active"</span>
                </div>
                <div class="flex gap-4 mt-1.5 text-xs text-text-tertiary">
                    <span>"Created: " {created}</span>
                    <span>"Last used: " {last_used}</span>
                    <span>"Expires: " {expires}</span>
                </div>
            </div>
            <button
                on:click=move |_| on_revoke()
                class="ml-4 px-3 py-1.5 text-xs bg-danger/10 text-danger border border-danger/20 rounded-lg hover:bg-danger hover:text-white transition-colors"
            >
                "Revoke"
            </button>
        </div>
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn format_timestamp(ms: i64) -> String {
    // Convert ms epoch to human-readable relative or absolute time
    let now_ms = js_sys::Date::now() as i64;
    let diff_ms = now_ms - ms;

    if diff_ms < 0 {
        // Future — show absolute for expires_at
        let remaining = (-diff_ms) / 1000;
        if remaining < 3600 {
            format!("in {}m", remaining / 60)
        } else if remaining < 86400 {
            format!("in {}h", remaining / 3600)
        } else {
            format!("in {}d", remaining / 86400)
        }
    } else if diff_ms < 60_000 {
        "just now".to_string()
    } else if diff_ms < 3_600_000 {
        format!("{}m ago", diff_ms / 60_000)
    } else if diff_ms < 86_400_000 {
        format!("{}h ago", diff_ms / 3_600_000)
    } else {
        format!("{}d ago", diff_ms / 86_400_000)
    }
}
