//! Generic PII section (mirrored on Search settings — config lives there).

use leptos::prelude::*;

use crate::api::SearchConfig;
use crate::i18n::{t, use_i18n};

#[component]
pub(super) fn PIISection(config: RwSignal<SearchConfig>) -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, settings.security.pii_protection)}</h2>

            <div class="space-y-4">
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        checked=move || config.get().pii_enabled
                        on:change=move |ev| {
                            let val = event_target_checked(&ev);
                            config.update(|cfg| cfg.pii_enabled = val);
                        }
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded"
                    />
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, settings.security.enable_pii)}</div>
                    </div>
                </label>

                <div class="ml-7 space-y-2 border-l-2 border-border pl-4">
                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || config.get().pii_scrub_email
                            on:change=move |ev| {
                                let val = event_target_checked(&ev);
                                config.update(|cfg| cfg.pii_scrub_email = val);
                            }
                            disabled=move || !config.get().pii_enabled
                            class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-text-secondary">{t!(i18n, settings.security.pii_email)}</span>
                    </label>

                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || config.get().pii_scrub_phone
                            on:change=move |ev| {
                                let val = event_target_checked(&ev);
                                config.update(|cfg| cfg.pii_scrub_phone = val);
                            }
                            disabled=move || !config.get().pii_enabled
                            class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-text-secondary">{t!(i18n, settings.security.pii_phone)}</span>
                    </label>

                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || config.get().pii_scrub_ssn
                            on:change=move |ev| {
                                let val = event_target_checked(&ev);
                                config.update(|cfg| cfg.pii_scrub_ssn = val);
                            }
                            disabled=move || !config.get().pii_enabled
                            class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-text-secondary">{t!(i18n, settings.security.pii_ssn)}</span>
                    </label>

                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || config.get().pii_scrub_credit_card
                            on:change=move |ev| {
                                let val = event_target_checked(&ev);
                                config.update(|cfg| cfg.pii_scrub_credit_card = val);
                            }
                            disabled=move || !config.get().pii_enabled
                            class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-text-secondary">{t!(i18n, settings.security.pii_credit_card)}</span>
                    </label>
                </div>
            </div>
        </div>
    }
}
