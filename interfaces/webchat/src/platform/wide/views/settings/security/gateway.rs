//! Network-access scope settings (gateway bind address).

use leptos::prelude::*;

use crate::api::SecurityConfig;
use crate::i18n::{t, t_string, use_i18n};

#[component]
pub(super) fn NetworkAccessSection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, settings.security.network_access)}</h2>

            <div class="space-y-4">
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        {t!(i18n, settings.security.network_scope)}
                    </label>
                    <select
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.network_access = event_target_value(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary"
                    >
                        <option
                            value="localhost"
                            selected=move || config.get().map(|c| c.network_access == "localhost").unwrap_or(true)
                        >
                            {t!(i18n, settings.security.localhost_only)}
                        </option>
                        <option
                            value="allnetworks"
                            selected=move || config.get().map(|c| c.network_access == "allnetworks").unwrap_or(false)
                        >
                            {t!(i18n, settings.security.all_networks)}
                        </option>
                    </select>
                    <p class="text-xs text-text-tertiary mt-1">
                        {move || {
                            let is_all = config.get().map(|c| c.network_access == "allnetworks").unwrap_or(false);
                            if is_all {
                                t_string!(i18n, settings.security.all_networks_desc).to_string()
                            } else {
                                t_string!(i18n, settings.security.localhost_only_desc).to_string()
                            }
                        }}
                    </p>
                </div>
            </div>
        </div>
    }
}
