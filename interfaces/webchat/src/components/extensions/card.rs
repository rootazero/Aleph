use leptos::prelude::*;
use leptos_i18n::I18nContext;

use crate::api::extensions::ExtensionEntry;
use crate::i18n::{t, t_string, use_i18n, Locale};
use crate::views::extensions::model::{kind_badge_class, trust_dot_class};
use crate::views::extensions::StoreState;

/// Localize a `kind` string using literal i18n key paths.
/// The `t!` macro requires compile-time literal keys, so we match on the runtime string.
fn kind_label(i18n: I18nContext<Locale>, kind: &str) -> String {
    match kind {
        "skill" => t_string!(i18n, extensions.kind.skill).to_string(),
        "plugin" => t_string!(i18n, extensions.kind.plugin).to_string(),
        "mcp" => t_string!(i18n, extensions.kind.mcp).to_string(),
        _ => t_string!(i18n, extensions.kind.other).to_string(),
    }
}

/// Localize a `trust_tier` string using literal i18n key paths.
fn trust_label(i18n: I18nContext<Locale>, tier: &str) -> String {
    match tier {
        "official" => t_string!(i18n, extensions.trust.official).to_string(),
        "verified" => t_string!(i18n, extensions.trust.verified).to_string(),
        "community" => t_string!(i18n, extensions.trust.community).to_string(),
        _ => t_string!(i18n, extensions.trust.unverified).to_string(),
    }
}

#[component]
#[must_use]
pub fn ExtensionCard(entry: ExtensionEntry) -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();
    let e = entry.clone();
    let select = move |_| store.selected.set(Some(e.clone()));

    let badge_cls = format!(
        "px-1.5 py-0.5 rounded text-[10px] font-mono font-bold uppercase tracking-wider {}",
        kind_badge_class(&entry.kind)
    );
    let glyph = entry
        .icon
        .clone()
        .unwrap_or_else(|| entry.name.chars().next().map(|c| c.to_string()).unwrap_or_default());
    let author = entry.author.clone().unwrap_or_default();
    let installed = entry.installed;
    let kind_text = kind_label(i18n, &entry.kind);
    let trust_text = trust_label(i18n, &entry.trust_tier);
    let trust_cls = format!(
        "inline-block w-2 h-2 rounded-full {}",
        trust_dot_class(&entry.trust_tier)
    );

    view! {
        <div
            class="p-4 bg-surface-raised border border-border rounded-xl hover:border-primary/40 hover:shadow-md transition-all cursor-pointer flex flex-col gap-2"
            on:click=select.clone()
        >
            <div class="flex items-start gap-3">
                <div class="w-10 h-10 rounded-lg bg-primary-subtle flex items-center justify-center flex-shrink-0 text-lg">{glyph}</div>
                <div class="min-w-0 flex-1">
                    <div class="flex items-center gap-2">
                        <span class="font-serif text-base text-text-primary truncate">{entry.name.clone()}</span>
                        <span class=badge_cls>{kind_text}</span>
                    </div>
                    <p class="text-xs text-text-tertiary truncate">{author}</p>
                </div>
            </div>
            <p class="text-sm text-text-secondary line-clamp-2">{entry.description.clone()}</p>
            <div class="flex items-center gap-2 mt-1">
                <span class=trust_cls></span>
                <span class="text-xs text-text-tertiary">{trust_text}</span>
                <span class="flex-1"></span>
                {{
                    let install_select = select.clone();
                    move || if installed {
                        view! { <span class="px-3 py-1 rounded-lg text-xs bg-success-subtle text-success">{t!(i18n, extensions.installed)}</span> }.into_any()
                    } else {
                        view! { <button class="px-3 py-1 rounded-lg text-xs bg-primary text-white hover:bg-primary-hover" on:click=install_select.clone()>{t!(i18n, extensions.install)}</button> }.into_any()
                    }
                }}
            </div>
        </div>
    }
}
