use leptos::prelude::*;

use crate::api::extensions::ExtensionEntry;
use crate::components::extensions::labels::{kind_label, trust_label};
use crate::i18n::{t, use_i18n};
use crate::views::extensions::model::{kind_badge_class, trust_dot_class};
use crate::views::extensions::StoreState;

#[component]
#[must_use]
pub fn ExtensionCard(entry: ExtensionEntry) -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();
    let e = entry.clone();
    let select = move |_| store.selected.set(Some(e.clone()));

    let badge_cls = format!(
        "px-1.5 py-0.5 rounded text-[10px] font-mono font-bold uppercase tracking-wider whitespace-nowrap flex-shrink-0 {}",
        kind_badge_class(&entry.kind)
    );
    let glyph = entry.icon.clone().unwrap_or_else(|| {
        entry
            .name
            .chars()
            .next()
            .map(|c| c.to_string())
            .unwrap_or_default()
    });
    let author = entry.author.clone().unwrap_or_default();
    let installed = entry.installed;
    let source_label = entry.source_label.clone();
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
                <span class="text-xs text-text-tertiary whitespace-nowrap flex-shrink-0">{trust_text}</span>
                {
                    let source_label = source_label.clone();
                    (!source_label.is_empty()).then(move || view! {
                        <span class="text-xs text-text-tertiary truncate">"· "{t!(i18n, extensions.via)}" "{source_label}</span>
                    })
                }
                <span class="flex-1"></span>
                {{
                    let install_entry = entry.clone();
                    move || if installed {
                        view! { <span class="px-3 py-1 rounded-lg text-xs bg-success-subtle text-success whitespace-nowrap flex-shrink-0">{t!(i18n, extensions.installed)}</span> }.into_any()
                    } else {
                        let install_entry = install_entry.clone();
                        view! { <button class="px-3 py-1 rounded-lg text-xs bg-primary text-white hover:bg-primary-hover whitespace-nowrap flex-shrink-0" on:click=move |ev| { ev.stop_propagation(); store.start_install(install_entry.clone()); }>{t!(i18n, extensions.install)}</button> }.into_any()
                    }
                }}
            </div>
        </div>
    }
}
