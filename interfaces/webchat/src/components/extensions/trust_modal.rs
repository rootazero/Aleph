//! Trust disclosure modal (R6 centered modal) — the system-enforced trust gate.
//! Reads `store.disclosure`; renders the risk verdict, kv rows, the FULL untruncated
//! command that will run, and an ack checkbox (only when `ack_required`). Continue is
//! `disabled` until the ack box is checked when required.
use leptos::prelude::*;

use crate::i18n::{t, use_i18n};
use crate::views::extensions::model::risk_banner_class;
use crate::views::extensions::StoreState;

#[component]
#[must_use]
pub fn TrustModal(
    #[prop(into)] on_continue: Callback<()>,
    #[prop(into)] on_cancel: Callback<()>,
) -> impl IntoView {
    let Some(store) = use_context::<StoreState>() else {
        return ().into_any();
    };
    let i18n = use_i18n();
    // Ack checkbox state — reset whenever a new disclosure arrives.
    let ack = RwSignal::new(false);
    Effect::new(move || {
        store.disclosure.track();
        ack.set(false);
    });

    view! {
        <Show when=move || store.disclosure.get().is_some()>
            {move || {
                let Some(d) = store.disclosure.get() else {
                    return ().into_any();
                };
                let secrets_count = d.secrets.len();
                view! {
                    <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
                        <div
                            class="aleph-scrim absolute inset-0 bg-black/40"
                            on:click=move |_| on_cancel.run(())
                        ></div>
                        <div class="glass relative w-[480px] max-w-[94vw] bg-surface-overlay/90 border border-border rounded-xl shadow-xl flex flex-col max-h-[88vh]">
                            <header class="px-5 pt-4 pb-2">
                                <p class="font-mono text-[10px] uppercase tracking-wider text-text-tertiary">{d.tier.clone()}</p>
                                <h2 class="font-serif text-xl text-text-primary leading-tight">{t!(i18n, extensions.what_it_reaches)}</h2>
                            </header>
                            <div class="flex-1 overflow-y-auto px-5 pb-2 space-y-3 text-sm">
                                // verdict banner
                                <div class=format!("p-3 rounded border text-sm {}", risk_banner_class(&d.risk))>
                                    <strong>{d.one_line.clone()}</strong>
                                </div>
                                // kv rows
                                <div class="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
                                    <span class="text-text-tertiary">{t!(i18n, extensions.trust_label)}</span>
                                    <span class="text-text-secondary">{d.tier.clone()}</span>
                                    {d.version.clone().map(|v| view! {
                                        <span class="text-text-tertiary">{t!(i18n, extensions.version)}</span>
                                        <span class="text-text-secondary font-mono">{v}</span>
                                    })}
                                    {d.sha256.clone().map(|_| view! {
                                        <span class="text-text-tertiary">{t!(i18n, extensions.trust.integrity_label)}</span>
                                        <span class="text-success">{t!(i18n, extensions.trust.integrity_verified)}</span>
                                    })}
                                    {(secrets_count > 0).then(|| view! {
                                        <span class="text-text-tertiary">"🔑"</span>
                                        <span class="text-text-secondary">{secrets_count}</span>
                                    })}
                                </div>
                                // command that will run — FULL, untruncated (security: never clamp)
                                {d.command_display.clone().map(|cmd| view! {
                                    <details class="mt-1">
                                        <summary class="text-xs text-text-secondary cursor-pointer">{t!(i18n, extensions.command_label)}</summary>
                                        <pre class="font-mono text-xs bg-surface-sunken p-2 rounded mt-1 whitespace-pre-wrap break-all">{cmd}</pre>
                                    </details>
                                })}
                                // ack checkbox — ONLY when required
                                {d.ack_required.then(|| view! {
                                    <label class="flex items-start gap-2 p-2 bg-warning-subtle rounded text-xs cursor-pointer">
                                        <input
                                            type="checkbox"
                                            class="mt-0.5"
                                            prop:checked=move || ack.get()
                                            on:change=move |ev| ack.set(event_target_checked(&ev))
                                        />
                                        <span>{t!(i18n, extensions.ack)}</span>
                                    </label>
                                })}
                            </div>
                            <footer class="px-5 py-3 border-t border-border flex gap-2 justify-end">
                                <button
                                    class="px-4 py-2 bg-surface-sunken text-text-secondary rounded-lg text-sm hover:bg-surface-raised"
                                    on:click=move |_| on_cancel.run(())
                                >
                                    {t!(i18n, extensions.cancel)}
                                </button>
                                <button
                                    class="px-4 py-2 bg-primary text-white rounded-lg text-sm hover:bg-primary-hover disabled:opacity-40 disabled:cursor-not-allowed"
                                    disabled=move || d.ack_required && !ack.get()
                                    on:click=move |_| on_continue.run(())
                                >
                                    {t!(i18n, extensions.continue_install)}
                                </button>
                            </footer>
                        </div>
                    </div>
                }
            }}
        </Show>
    }
}
