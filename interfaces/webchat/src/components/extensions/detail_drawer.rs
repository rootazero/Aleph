use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::extensions::{DisclosurePayload, ExtensionsApi, SecretDisclosure};
use crate::components::extensions::labels::{category_label, kind_label, trust_label};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use crate::views::extensions::model::{kind_badge_class, risk_banner_class, trust_dot_class};
use crate::views::extensions::StoreState;

#[component]
#[must_use]
pub fn ExtensionDetailDrawer() -> impl IntoView {
    let Some(state) = use_context::<DashboardState>() else {
        return ().into_any();
    };
    let Some(store) = use_context::<StoreState>() else {
        return ().into_any();
    };
    let i18n = use_i18n();

    let disclosure = RwSignal::new(Option::<DisclosurePayload>::None);
    let disc_loading = RwSignal::new(false);
    let disc_error = RwSignal::new(Option::<String>::None);
    let post_install = RwSignal::new(Option::<String>::None);

    // Lazy-load disclosure when an entry is selected.
    Effect::new(move || {
        if let Some(entry) = store.selected.get() {
            disclosure.set(None);
            post_install.set(None);
            disc_error.set(None);
            disc_loading.set(true);
            let id = entry.id.clone();
            let i18n = i18n;
            spawn_local(async move {
                match ExtensionsApi::disclosure(&state, id).await {
                    Ok((d, _findings, pi)) => {
                        disclosure.set(Some(d));
                        post_install.set(pi);
                        disc_loading.set(false);
                    }
                    Err(e) => {
                        disc_error.set(Some(crate::components::admin_refusal::settings_load_error(
                            i18n,
                            &e,
                            |e| e.to_string(),
                        )));
                        disc_loading.set(false);
                    }
                }
            });
        }
    });

    let close = move |_| store.selected.set(None);

    view! {
        <Show when=move || store.selected.get().is_some()>
            {move || {
                let Some(entry) = store.selected.get() else {
                    return ().into_any();
                };
                let badge_cls = format!("px-1.5 py-0.5 rounded text-[10px] font-mono font-bold uppercase whitespace-nowrap flex-shrink-0 {}", kind_badge_class(&entry.kind));
                let kind_text = kind_label(i18n, &entry.kind);
                let trust_text = trust_label(i18n, &entry.trust_tier);
                let category_text = category_label(i18n, &entry.category);
                view! {
                    <div class="fixed inset-0 z-[60] flex justify-end">
                        <div class="aleph-scrim absolute inset-0 bg-black/30" on:click=close></div>
                        <aside class="glass relative w-[480px] max-w-[94vw] h-full bg-surface-overlay/90 border-l border-border shadow-xl flex flex-col">
                            <header class="px-4 py-3 border-b border-border flex items-start justify-between gap-2">
                                <div class="flex items-center gap-3 min-w-0">
                                    <div class="w-12 h-12 rounded-lg bg-primary-subtle flex items-center justify-center text-xl flex-shrink-0">
                                        {entry.icon.clone().unwrap_or_else(|| entry.name.chars().next().map(|c| c.to_string()).unwrap_or_default())}
                                    </div>
                                    <div class="min-w-0">
                                        <div class="flex items-center gap-2">
                                            <span class="font-serif text-lg text-text-primary truncate">{entry.name.clone()}</span>
                                            <span class=badge_cls>{kind_text}</span>
                                        </div>
                                        <p class="text-xs text-text-tertiary truncate">{entry.author.clone().unwrap_or_default()}</p>
                                        {(!entry.source_label.is_empty()).then({
                                            let s = entry.source_label.clone();
                                            move || view! { <p class="text-xs text-text-tertiary truncate">{t!(i18n, extensions.via)}" "{s}</p> }
                                        })}
                                    </div>
                                </div>
                                <button class="text-text-tertiary hover:text-text-primary" on:click=close>"✕"</button>
                            </header>
                            <div class="flex-1 overflow-y-auto p-4 space-y-4 text-sm">
                                // stat row
                                <div class="grid grid-cols-3 gap-2 py-2 border-y border-border-subtle text-center">
                                    <div><p class="text-xs text-text-tertiary uppercase tracking-wider">{t!(i18n, extensions.version)}</p><p class="font-mono">{entry.version.clone().unwrap_or_else(|| "—".into())}</p></div>
                                    <div><p class="text-xs text-text-tertiary uppercase tracking-wider">{t!(i18n, extensions.category_label)}</p><p>{category_text}</p></div>
                                    <div><p class="text-xs text-text-tertiary uppercase tracking-wider">{t!(i18n, extensions.trust_label)}</p><p class="flex items-center justify-center gap-1 whitespace-nowrap"><span class=format!("inline-block w-2 h-2 rounded-full flex-shrink-0 {}", trust_dot_class(&entry.trust_tier))></span>{trust_text}</p></div>
                                </div>
                                // what it does — full untruncated description (spec §11 injection-hardening: no clamp here)
                                <div><h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider mb-1">{t!(i18n, extensions.what_it_does)}</h3><p class="text-text-secondary">{entry.description.clone()}</p></div>
                                // what it can reach (permissions from disclosure)
                                <div>
                                    <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider mb-1">{t!(i18n, extensions.what_it_reaches)}</h3>
                                    {move || if disc_loading.get() {
                                        view! { <p class="text-text-tertiary italic">{t!(i18n, extensions.loading_perms)}</p> }.into_any()
                                    } else if let Some(e) = disc_error.get() {
                                        view! { <p class="text-xs text-danger break-words">{e}</p> }.into_any()
                                    } else if let Some(d) = disclosure.get() {
                                        view! {
                                            <div class="space-y-2">
                                                <div class=format!("p-2 rounded border text-xs {}", risk_banner_class(&d.risk))>{d.one_line.clone()}</div>
                                                // command_display rendered in full — no truncation (spec §11)
                                                {d.command_display.clone().map(|cmd| view! { <div class="font-mono text-xs bg-surface-sunken p-2 rounded break-all">{cmd}</div> })}
                                                {(!d.secrets.is_empty()).then(|| view! {
                                                    <ul class="space-y-1">
                                                        {d.secrets.iter().map(|s: &SecretDisclosure| view! {
                                                            <li class="text-xs text-text-secondary">"🔑 "{s.name.clone()}{(!s.purpose.is_empty()).then(|| format!(" — {}", s.purpose))}</li>
                                                        }).collect_view()}
                                                    </ul>
                                                })}
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! { <p class="text-text-tertiary italic">{t!(i18n, extensions.no_perms)}</p> }.into_any()
                                    }}
                                </div>
                                // setup guidance (post_install) — persistent, pre/post install
                                {move || post_install.get().map(|pi| view! {
                                    <div class="p-2 rounded border border-border text-xs text-text-secondary whitespace-pre-line">
                                        "⚙️ "{pi}
                                    </div>
                                })}
                            </div>
                            <footer class="px-4 py-3 border-t border-border flex gap-2">
                                {{
                                    let install_entry = entry.clone();
                                    view! { <button class="flex-1 px-4 py-2 bg-primary text-white rounded-lg hover:bg-primary-hover text-sm whitespace-nowrap" on:click=move |_| { store.start_install(install_entry.clone()); store.selected.set(None); }>{t!(i18n, extensions.install)}</button> }
                                }}
                                {entry.repo_url.clone().and_then(|url| {
                                    let safe = crate::components::markdown::sanitize_link_url(&url);
                                    if safe.starts_with("#disallowed-") {
                                        return None;
                                    }
                                    Some(view! { <a class="px-4 py-2 bg-surface-sunken text-text-secondary rounded-lg text-sm" href=safe target="_blank" rel="noopener">{t!(i18n, extensions.docs)}</a> })
                                })}
                            </footer>
                        </aside>
                    </div>
                }.into_any()
            }}
        </Show>
    }.into_any()
}
