//! Appearance settings page.
//!
//! Surfaces the four client-side appearance axes (theme mode, accent, font
//! scale, roundness) that previously lived only in the topbar `ThemeToggle`
//! popover (mode/accent) or had no UI at all (font scale, roundness). All
//! read/apply/persist logic is delegated to [`crate::appearance`] — this view
//! is pure I/O: render the choices, apply on click, no business logic.

use crate::appearance::{
    apply_accent, apply_font_scale, apply_material, apply_mode, apply_roundness, read_accent,
    read_font_scale, read_material, read_mode, read_roundness, Accent, FontScale, Material,
    Roundness, ThemeMode,
};
use crate::components::ui::SwatchButton;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn AppearanceView() -> impl IntoView {
    let mode = RwSignal::new(read_mode());
    let accent = RwSignal::new(read_accent());
    let material = RwSignal::new(read_material());
    let font_scale = RwSignal::new(read_font_scale());
    let roundness = RwSignal::new(read_roundness());

    let reset = move |_| {
        apply_mode(ThemeMode::System);
        apply_accent(Accent::Mauve);
        apply_material(Material::Luxe);
        apply_font_scale(FontScale::Default);
        apply_roundness(Roundness::Default);
        mode.set(ThemeMode::System);
        accent.set(Accent::Mauve);
        material.set(Material::Luxe);
        font_scale.set(FontScale::Default);
        roundness.set(Roundness::Default);
    };

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-4xl mx-auto">
            <div class="mb-8">
                <h1 class="text-3xl font-bold mb-2 text-text-primary">"外观"</h1>
                <p class="text-text-secondary">
                    "调整主题、强调色、字号与圆角。所有设置保存在本机浏览器，立即生效。"
                </p>
            </div>

            <div class="space-y-6">
                // --- Theme mode -------------------------------------------------
                <SettingCard title="主题模式" desc="界面明暗。「跟随系统」交由操作系统决定。">
                    <div class="flex flex-wrap gap-2">
                        {ThemeMode::ALL.into_iter().map(|m| {
                            let active = move || mode.get() == m;
                            view! {
                                <ChoiceButton
                                    label=m.label()
                                    active=Signal::derive(active)
                                    on_pick=move || { apply_mode(m); mode.set(m); }
                                />
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </SettingCard>

                // --- Material --------------------------------------------------
                <SettingCard title="材质" desc="界面的玻璃材质风格：奢华磨砂克制内敛，液态玻璃通透鲜活，极光浓雾色彩弥漫。">
                    <div class="flex flex-wrap gap-4">
                        {Material::ALL.into_iter().map(|m| {
                            let active = move || material.get() == m;
                            view! {
                                <SwatchButton
                                    background=m.preview()
                                    face="w-14 h-9 rounded-lg transition-transform group-hover:scale-105"
                                    ring_offset="ring-offset-surface-raised"
                                    title=m.label()
                                    label=m.label()
                                    active=Signal::derive(active)
                                    on_pick=move |_| { apply_material(m); material.set(m); }
                                />
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </SettingCard>

                // --- Accent -----------------------------------------------------
                <SettingCard title="强调色" desc="按钮、链接与高亮采用的主色调。">
                    <div class="flex flex-wrap gap-4">
                        {Accent::ALL.into_iter().map(|a| {
                            let active = move || accent.get() == a;
                            view! {
                                <SwatchButton
                                    background=a.swatch()
                                    face="w-9 h-9 rounded-full transition-transform group-hover:scale-110"
                                    ring_offset="ring-offset-surface-raised"
                                    title=a.label()
                                    label=a.label()
                                    active=Signal::derive(active)
                                    on_pick=move |_| { apply_accent(a); accent.set(a); }
                                />
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </SettingCard>

                // --- Font scale -------------------------------------------------
                <SettingCard title="字号" desc="整体界面文字缩放，便于无障碍阅读。">
                    <div class="flex flex-wrap gap-2">
                        {FontScale::ALL.into_iter().map(|f| {
                            let active = move || font_scale.get() == f;
                            view! {
                                <ChoiceButton
                                    label=f.label()
                                    active=Signal::derive(active)
                                    on_pick=move || { apply_font_scale(f); font_scale.set(f); }
                                />
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </SettingCard>

                // --- Roundness --------------------------------------------------
                <SettingCard title="圆角" desc="卡片、按钮与输入框的边角弧度。">
                    <div class="flex flex-wrap gap-2">
                        {Roundness::ALL.into_iter().map(|r| {
                            let active = move || roundness.get() == r;
                            view! {
                                <ChoiceButton
                                    label=r.label()
                                    active=Signal::derive(active)
                                    on_pick=move || { apply_roundness(r); roundness.set(r); }
                                />
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </SettingCard>

                // --- Live preview ----------------------------------------------
                <div class="bg-surface-raised border border-border rounded-xl p-6">
                    <h2 class="text-sm font-semibold text-text-tertiary uppercase tracking-wider mb-3">"预览"</h2>
                    <div class="flex items-center gap-3 flex-wrap">
                        <button class="px-4 py-2 bg-primary text-white rounded-lg text-sm font-medium">"主按钮"</button>
                        <button class="px-4 py-2 bg-surface-sunken border border-border rounded-lg text-sm text-text-primary">"次按钮"</button>
                        <span class="px-3 py-1 rounded-full bg-primary-subtle text-primary text-xs">"标签"</span>
                        <input
                            class="px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm text-text-primary"
                            placeholder="输入框"
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
                        "恢复默认"
                    </button>
                </div>
            </div>
        </div>
    }
}

/// A titled settings card matching the styling used across the settings pages.
#[component]
fn SettingCard(title: &'static str, desc: &'static str, children: Children) -> impl IntoView {
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
    label: &'static str,
    #[prop(into)] active: Signal<bool>,
    on_pick: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        <button
            on:click=move |_| on_pick()
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
