//! Appearance settings page.
//!
//! Surfaces the client-side appearance axes (theme mode, accent, material,
//! font scale, roundness, density) that previously lived only in the topbar
//! `ThemeToggle` popover (mode/accent) or had no UI at all (font scale,
//! roundness). All read/apply/persist logic is delegated to
//! [`crate::appearance`] — this view is pure I/O: render the choices, apply
//! on click, no business logic.

use crate::appearance::{
    apply_accent, apply_density, apply_font_scale, apply_material, apply_mode, apply_roundness,
    read_accent, read_density, read_font_scale, read_material, read_mode, read_roundness, Accent,
    Density, FontScale, Material, Roundness, ThemeMode,
};
use crate::components::ui::SwatchButton;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;

#[component]
#[must_use]
pub fn AppearanceView() -> impl IntoView {
    let i18n = use_i18n();
    let mode = RwSignal::new(read_mode());
    let accent = RwSignal::new(read_accent());
    let material = RwSignal::new(read_material());
    let font_scale = RwSignal::new(read_font_scale());
    let roundness = RwSignal::new(read_roundness());
    let density = RwSignal::new(read_density());

    let reset = move |_| {
        apply_mode(ThemeMode::System);
        apply_accent(Accent::Mauve);
        apply_material(Material::Luxe);
        apply_font_scale(FontScale::Default);
        apply_roundness(Roundness::Default);
        apply_density(Density::Compact);
        mode.set(ThemeMode::System);
        accent.set(Accent::Mauve);
        material.set(Material::Luxe);
        font_scale.set(FontScale::Default);
        roundness.set(Roundness::Default);
        density.set(Density::Compact);
    };

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-4xl mx-auto">
            <div class="mb-8">
                <h1 class="text-3xl font-bold mb-2 text-text-primary">{t!(i18n, appearance.title)}</h1>
                <p class="text-text-secondary">
                    {t!(i18n, appearance.subtitle)}
                </p>
            </div>

            <div class="space-y-6">
                // --- Theme mode -------------------------------------------------
                <SettingCard title=Signal::derive(move || t_string!(i18n, appearance.section.mode).to_string())
                    desc=Signal::derive(move || t_string!(i18n, appearance.desc.mode).to_string())>
                    <div class="flex flex-wrap gap-2">
                        {ThemeMode::ALL.into_iter().map(|m| {
                            let active = move || mode.get() == m;
                            view! {
                                <ChoiceButton
                                    label=Signal::derive(move || m.label(i18n))
                                    active=Signal::derive(active)
                                    on_pick=move || { apply_mode(m); mode.set(m); }
                                />
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </SettingCard>

                // --- Material --------------------------------------------------
                <SettingCard title=Signal::derive(move || t_string!(i18n, appearance.section.material).to_string())
                    desc=Signal::derive(move || t_string!(i18n, appearance.desc.material).to_string())>
                    <div class="flex flex-wrap gap-4">
                        {Material::ALL.into_iter().map(|m| {
                            let active = move || material.get() == m;
                            view! {
                                <SwatchButton
                                    background=m.preview()
                                    face="w-14 h-9 rounded-lg transition-transform group-hover:scale-105"
                                    ring_offset="ring-offset-surface-raised"
                                    title=Signal::derive(move || m.label(i18n))
                                    label=Signal::derive(move || m.label(i18n))
                                    active=Signal::derive(active)
                                    on_pick=move |_| { apply_material(m); material.set(m); }
                                />
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </SettingCard>

                // --- Accent -----------------------------------------------------
                <SettingCard title=Signal::derive(move || t_string!(i18n, appearance.section.accent).to_string())
                    desc=Signal::derive(move || t_string!(i18n, appearance.desc.accent).to_string())>
                    <div class="flex flex-wrap gap-4">
                        {Accent::ALL.into_iter().map(|a| {
                            let active = move || accent.get() == a;
                            view! {
                                <SwatchButton
                                    background=a.swatch()
                                    face="w-9 h-9 rounded-full transition-transform group-hover:scale-110"
                                    ring_offset="ring-offset-surface-raised"
                                    title=Signal::derive(move || a.label(i18n))
                                    label=Signal::derive(move || a.label(i18n))
                                    active=Signal::derive(active)
                                    on_pick=move |_| { apply_accent(a); accent.set(a); }
                                />
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </SettingCard>

                // --- Font scale -------------------------------------------------
                <SettingCard title=Signal::derive(move || t_string!(i18n, appearance.section.font).to_string())
                    desc=Signal::derive(move || t_string!(i18n, appearance.desc.font).to_string())>
                    <div class="flex flex-wrap gap-2">
                        {FontScale::ALL.into_iter().map(|f| {
                            let active = move || font_scale.get() == f;
                            view! {
                                <ChoiceButton
                                    label=Signal::derive(move || f.label(i18n))
                                    active=Signal::derive(active)
                                    on_pick=move || { apply_font_scale(f); font_scale.set(f); }
                                />
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </SettingCard>

                // --- Roundness --------------------------------------------------
                <SettingCard title=Signal::derive(move || t_string!(i18n, appearance.section.radius).to_string())
                    desc=Signal::derive(move || t_string!(i18n, appearance.desc.radius).to_string())>
                    <div class="flex flex-wrap gap-2">
                        {Roundness::ALL.into_iter().map(|r| {
                            let active = move || roundness.get() == r;
                            view! {
                                <ChoiceButton
                                    label=Signal::derive(move || r.label(i18n))
                                    active=Signal::derive(active)
                                    on_pick=move || { apply_roundness(r); roundness.set(r); }
                                />
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </SettingCard>

                // --- Density ----------------------------------------------------
                <SettingCard title=Signal::derive(move || t_string!(i18n, appearance.section.density).to_string())
                    desc=Signal::derive(move || t_string!(i18n, appearance.desc.density).to_string())>
                    <div class="flex flex-wrap gap-2">
                        {Density::ALL.into_iter().map(|d| {
                            let active = move || density.get() == d;
                            view! {
                                <ChoiceButton
                                    label=Signal::derive(move || d.label(i18n))
                                    active=Signal::derive(active)
                                    on_pick=move || { apply_density(d); density.set(d); }
                                />
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </SettingCard>

                // --- Live preview ----------------------------------------------
                <div class="bg-surface-raised border border-border rounded-xl p-6">
                    <h2 class="text-sm font-semibold text-text-tertiary uppercase tracking-wider mb-3">{t!(i18n, appearance.preview.heading)}</h2>
                    <div class="flex items-center gap-3 flex-wrap">
                        <button class="px-4 py-2 bg-primary text-white rounded-lg text-sm font-medium">{t!(i18n, appearance.preview.primary)}</button>
                        <button class="px-4 py-2 bg-surface-sunken border border-border rounded-lg text-sm text-text-primary">{t!(i18n, appearance.preview.secondary)}</button>
                        <span class="px-3 py-1 rounded-full bg-primary-subtle text-primary text-xs">{t!(i18n, appearance.preview.tag)}</span>
                        <input
                            class="px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm text-text-primary"
                            placeholder=move || t_string!(i18n, appearance.preview.input).to_string()
                        />
                    </div>
                </div>

                // --- Reset ------------------------------------------------------
                <div class="flex justify-end">
                    <button
                        on:click=reset
                        class="px-4 py-2 text-sm font-medium text-text-secondary hover:text-text-primary
                               border border-border rounded-lg hover:bg-surface-sunken transition-colors"
                    >
                        {t!(i18n, appearance.reset)}
                    </button>
                </div>
            </div>
        </div>
    }
}

/// A titled settings card matching the styling used across the settings pages.
#[component]
fn SettingCard(
    #[prop(into)] title: Signal<String>,
    #[prop(into)] desc: Signal<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-xl font-semibold text-text-primary mb-1">{title}</h2>
            <p class="text-sm text-text-secondary mb-4">{desc}</p>
            {children()}
        </div>
    }
}

/// A segmented-choice pill. `active` drives the highlighted state; `on_pick`
/// applies the choice (mutating the DOM + persisting) on click.
#[component]
fn ChoiceButton(
    #[prop(into)] label: Signal<String>,
    #[prop(into)] active: Signal<bool>,
    on_pick: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        <button
            on:click=move |_| on_pick()
            aria-pressed=move || active.get().to_string()
            class=move || {
                let base = "px-3 py-1.5 rounded-lg text-sm font-medium transition-colors border";
                if active.get() {
                    format!("{base} bg-primary text-white border-primary")
                } else {
                    format!("{base} text-text-secondary border-border hover:bg-surface-sunken hover:text-text-primary")
                }
            }
        >
            {label}
        </button>
    }
}
