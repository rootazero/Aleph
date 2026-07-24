//! iPhone Appearance detail screen — Theme / Accent / Material / Font Scale / Roundness /
//! Density. Reuses crate::appearance read_*/apply_* (local, instant; no API).

use crate::appearance::{
    apply_accent, apply_density, apply_font_scale, apply_material, apply_mode, apply_roundness,
    read_accent, read_density, read_font_scale, read_material, read_mode, read_roundness, Accent,
    Density, FontScale, Material, Roundness, ThemeMode,
};
use crate::platform::phone::shell::PhoneShell;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn PhoneAppearance() -> impl IntoView {
    view! {
        <PhoneShell title="Appearance" back="/settings">
            <SelectGroup
                header="主题"
                items=ThemeMode::ALL.to_vec()
                current=read_mode()
                label=|m: ThemeMode| m.label()
                on_pick=|m| apply_mode(m)
            />
            <AccentGroup/>
            <SelectGroup
                header="材质"
                items=Material::ALL.to_vec()
                current=read_material()
                label=|m: Material| m.label()
                on_pick=|m| apply_material(m)
            />
            <SelectGroup
                header="字号"
                items=FontScale::ALL.to_vec()
                current=read_font_scale()
                label=|m: FontScale| m.label()
                on_pick=|m| apply_font_scale(m)
            />
            <SelectGroup
                header="圆角"
                items=Roundness::ALL.to_vec()
                current=read_roundness()
                label=|m: Roundness| m.label()
                on_pick=|m| apply_roundness(m)
            />
            <SelectGroup
                header="紧凑度"
                items=Density::ALL.to_vec()
                current=read_density()
                label=|m: Density| m.label()
                on_pick=|m| apply_density(m)
            />
        </PhoneShell>
    }
}

/// One iOS single-select section: a `.list` whose rows show a checkmark on the
/// chosen value. Generic over any `Copy + PartialEq` appearance enum.
#[component]
fn SelectGroup<T, L, P>(
    header: &'static str,
    items: Vec<T>,
    current: T,
    label: L,
    on_pick: P,
) -> impl IntoView
where
    T: Copy + PartialEq + Send + Sync + 'static,
    L: Fn(T) -> &'static str + Copy + 'static,
    P: Fn(T) + Copy + 'static,
{
    let selected = RwSignal::new(current);
    view! {
        <div>
            <div class="list-header">{header}</div>
            <div class="list">
                {items.into_iter().map(|item| {
                    view! {
                        <div
                            class="cell"
                            class:cell-selected=move || selected.get() == item
                            on:click=move |_| { on_pick(item); selected.set(item); }
                        >
                            <div class="cell-body"><div class="cell-title">{label(item)}</div></div>
                            <svg class="cell-check" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>
                        </div>
                    }
                }).collect_view()}
            </div>
        </div>
    }
}

/// Accent picker — swatch row (mirrors the landing's Accent cell).
#[component]
fn AccentGroup() -> impl IntoView {
    let selected = RwSignal::new(read_accent());
    view! {
        <div>
            <div class="list-header">"主题色"</div>
            <div class="list">
                <div class="cell" style="align-items:center;">
                    <div class="cell-body"><div class="cell-title">"Accent"</div></div>
                    <div style="display:flex; align-items:center; gap:8px; flex:none;">
                        {Accent::ALL.into_iter().map(|a| {
                            let style = format!("width:26px; height:26px; background:{};", a.swatch());
                            view! {
                                <span
                                    class="swatch"
                                    class:swatch-active=move || selected.get() == a
                                    style=style
                                    title=a.label()
                                    on:click=move |_| { apply_accent(a); selected.set(a); }
                                ></span>
                            }
                        }).collect_view()}
                    </div>
                </div>
            </div>
        </div>
    }
}
