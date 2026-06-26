//! iPhone Settings landing screen — 1:1 rebuild of the aleph-mobile design
//! (`docs/design-system/aleph-mobile/screens/exported/Aleph Settings.dc.html`).
//!
//! Rendered inside `PhoneShell` which provides the full-screen overlay, top bar,
//! and tab bar. Mounted only at <640px from `app.rs`'s `SettingsRouter`.
//! I/O-only (R4): cells/tabs navigate to existing routes; displayed values are
//! static placeholders for v1 (see spec §6). The faux status bar / dynamic
//! island / home indicator in the mockup are device chrome (OS-drawn on real
//! hardware) and are intentionally omitted.

pub mod appearance;
pub mod connection;
pub mod embeddings;
pub mod model_route;
pub mod providers;

use crate::platform::phone::shell::PhoneShell;
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

#[component]
#[must_use]
pub fn PhoneSettings() -> impl IntoView {
    let navigate = use_navigate();
    // `use_navigate` returns a Clone-able Fn; each handler gets its own clone.
    let go = move |path: &'static str| {
        let navigate = navigate.clone();
        move |_| navigate(path, NavigateOptions::default())
    };

    view! {
        <PhoneShell title="Settings">
            // Connection group
            <div>
                <div class="list-header">"连接"</div>
                <div class="list">
                    <div class="cell" on:click=go("/settings/network")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M5 12.5a7 7 0 0 1 14 0"></path>
                                <path d="M2 9a11 11 0 0 1 20 0"></path>
                                <path d="M8.5 16a4 4 0 0 1 7 0"></path>
                                <circle cx="12" cy="19.5" r="1" fill="currentColor"></circle>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Connection"</div></div>
                        <span class="cell-value mono" style="font-size:13px;">"remote · 10.10.10.4"</span>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                </div>
            </div>

            // AI group
            <div>
                <div class="list-header">"AI"</div>
                <div class="list">
                    <div class="cell" on:click=go("/settings/providers")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M12 3l1.6 3.8L17.5 8l-3.9 1.2L12 13l-1.6-3.8L6.5 8l3.9-1.2z"></path>
                                <path d="M6 15l.8 2 .8-2 .8 2-.8-2zM18 16l.7 1.8.7-1.8-.7 1.8z"></path>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Providers"</div></div>
                        <span class="cell-value">"Anthropic"</span>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                    <div class="cell" on:click=go("/settings/embedding-providers")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <circle cx="12" cy="12" r="3"></circle>
                                <circle cx="5" cy="6" r="1.6"></circle>
                                <circle cx="19" cy="7" r="1.6"></circle>
                                <circle cx="6" cy="18" r="1.6"></circle>
                                <circle cx="18" cy="17" r="1.6"></circle>
                                <path d="M9.6 10.4 6.4 7M14.4 10.6 17.6 8M9.8 14 6.9 16.6M14.2 13.8 17 16"></path>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Embeddings"</div></div>
                        <span class="cell-value mono" style="font-size:13px;">"text-embedding-3"</span>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                    <div class="cell" on:click=go("/settings/model-route")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M6 3v6a6 6 0 0 0 12 0V3"></path>
                                <path d="M6 21v-2a6 6 0 0 1 12 0v2"></path>
                                <line x1="4" y1="3" x2="20" y2="3"></line>
                                <line x1="4" y1="21" x2="20" y2="21"></line>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Model route"</div></div>
                        <span class="cell-value">"Opus 4.8"</span>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                </div>
            </div>

            // Appearance group
            <div>
                <div class="list-header">"外观"</div>
                <div class="list">
                    <div class="cell" on:click=go("/settings/appearance")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <circle cx="12" cy="12" r="9"></circle>
                                <path d="M12 3a9 9 0 0 0 0 18z" fill="currentColor" stroke="none"></path>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Theme"</div></div>
                        <span class="cell-value">"System"</span>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                    <div class="cell" style="align-items:center;" on:click=go("/settings/appearance")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <circle cx="13.5" cy="6.5" r="1.3"></circle>
                                <circle cx="17.5" cy="10.5" r="1.3"></circle>
                                <circle cx="8.5" cy="7.5" r="1.3"></circle>
                                <circle cx="6.5" cy="12.5" r="1.3"></circle>
                                <path d="M12 3a9 9 0 1 0 0 18c1 0 1.5-.8 1.5-1.6 0-1.2-1-1.6-1-2.6 0-.8.7-1.3 1.6-1.3H16a5 5 0 0 0 5-5c0-4.4-4-7.5-9-7.5z"></path>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Accent"</div></div>
                        <div style="display:flex; align-items:center; gap:8px; flex:none;">
                            <span class="swatch swatch-active" style="width:26px; height:26px; background:oklch(0.55 0.120 310);" title="Mauve"></span>
                            <span class="swatch" style="width:26px; height:26px; background:oklch(0.55 0.130 250);" title="Ocean"></span>
                            <span class="swatch" style="width:26px; height:26px; background:oklch(0.53 0.115 150);" title="Forest"></span>
                            <span class="swatch" style="width:26px; height:26px; background:oklch(0.62 0.135 60);" title="Sunset"></span>
                            <span class="swatch" style="width:26px; height:26px; background:oklch(0.57 0.150 15);" title="Rose"></span>
                        </div>
                    </div>
                    <div class="cell" on:click=go("/settings/appearance")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <rect x="3" y="3" width="18" height="18" rx="4"></rect>
                                <path d="M3 9a9 6 0 0 0 18 0"></path>
                                <path d="M3 14a9 5 0 0 0 18 0"></path>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Material"</div></div>
                        <span class="cell-value">"Luxe"</span>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                </div>
            </div>
        </PhoneShell>
    }
}
