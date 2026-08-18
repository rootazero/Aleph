//
// Theme picker — a topbar popover that controls three of the appearance axes:
//   • Mode     : System / Light / Dark
//   • Material : Luxe / Liquid / Aurora
//   • Accent   : Mauve / Ocean / Forest / Sunset / Rose
//
// The read/apply/persist logic + enums live in `crate::appearance` (the single
// source of appearance truth, shared with the Appearance settings page and the
// boot replay in `lib.rs`). This component only owns the popover UI and the
// circular View-Transition reveal animation.
//
use crate::appearance::{
    apply_accent, apply_material, apply_mode, read_accent, read_material, read_mode, Accent,
    Material, ThemeMode,
};
use crate::components::ui::SwatchButton;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

/// Convert a click position within the viewport into the CSS percentage
/// origin (`x%`, `y%`) for the circular theme reveal. Clamped to [0, 100];
/// a non-positive viewport span degrades to the centre (50%).
fn reveal_origin(client_x: f64, client_y: f64, vw: f64, vh: f64) -> (String, String) {
    let pct = |v: f64, span: f64| {
        if span <= 0.0 {
            50.0
        } else {
            (v / span * 100.0).clamp(0.0, 100.0)
        }
    };
    (
        format!("{:.2}%", pct(client_x, vw)),
        format!("{:.2}%", pct(client_y, vh)),
    )
}

/// Apply a theme change, animating it with a circular reveal via the
/// View Transition API when the browser supports it. `client_x/_y` are the
/// pointer coordinates of the originating click (the reveal origin). When
/// the API is unavailable the change is applied instantly (the `:root`
/// colour transition still cross-fades it).
fn animated_apply(client_x: f64, client_y: f64, apply: impl FnOnce() + 'static) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        apply();
        return;
    };

    // Feature-detect document.startViewTransition (Safari < 18, Firefox).
    let start_fn =
        js_sys::Reflect::get(document.as_ref(), &JsValue::from_str("startViewTransition"))
            .ok()
            .filter(JsValue::is_function)
            .and_then(|v| v.dyn_into::<js_sys::Function>().ok());

    let Some(start_fn) = start_fn else {
        apply();
        return;
    };

    // Mark the click origin + opt this <html> into the scoped reveal CSS.
    if let Some(html) = document
        .document_element()
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let vw = window
            .inner_width()
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let vh = window
            .inner_height()
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let (x, y) = reveal_origin(client_x, client_y, vw, vh);
        let style = html.style();
        let _ = style.set_property("--theme-x", &x);
        let _ = style.set_property("--theme-y", &y);
        let _ = html.class_list().add_1("theme-switching");
    }

    // startViewTransition(cb) snapshots the old DOM, runs cb synchronously to
    // mutate the theme, then animates from old → new snapshot.
    let cb = Closure::once_into_js(apply);
    let _ = start_fn.call1(document.as_ref(), &cb);
}

#[component]
#[must_use]
pub fn ThemeToggle() -> impl IntoView {
    let i18n = use_i18n();
    let mode = RwSignal::new(read_mode());
    let accent = RwSignal::new(read_accent());
    let material = RwSignal::new(read_material());
    let open = RwSignal::new(false);

    let current_swatch = move || accent.get().swatch().to_string();

    view! {
        <div class="relative">
            // Trigger
            <button
                on:click=move |_| open.update(|v| *v = !*v)
                class="flex items-center gap-1.5 px-2 py-1.5 rounded-lg text-text-secondary
                       hover:text-text-primary hover:bg-surface-sunken transition-all duration-200"
                title=move || t_string!(i18n, appearance.popover_title).to_string()
            >
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <circle cx="13.5" cy="6.5" r=".5" fill="currentColor" />
                    <circle cx="17.5" cy="10.5" r=".5" fill="currentColor" />
                    <circle cx="8.5" cy="7.5" r=".5" fill="currentColor" />
                    <circle cx="6.5" cy="12.5" r=".5" fill="currentColor" />
                    <path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688
                             0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1
                             1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z" />
                </svg>
                <span
                    class="w-2.5 h-2.5 rounded-full ring-1 ring-border"
                    style=move || format!("background: {}", current_swatch())
                />
            </button>

            // Click-outside catcher
            {move || open.get().then(|| view! {
                <div class="fixed inset-0 z-40" on:click=move |_| open.set(false) />
            })}

            // Popover
            <Show when=move || open.get()>
                <div class="theme-picker-popover glass animate-pop-in absolute top-full right-0 mt-2 z-50 w-56
                            rounded-xl border border-border bg-surface-overlay/85 shadow-xl p-3 space-y-3"
                     style="transform-origin: top right;"
                >
                    // Mode
                    <div>
                        <p class="px-1 pb-1.5 text-[10px] font-semibold uppercase tracking-wider text-text-tertiary">
                            {t!(i18n, appearance.title)}
                        </p>
                        <div class="grid grid-cols-3 gap-1">
                            {ThemeMode::ALL
                                .into_iter()
                                .map(|m| {
                                    let is_active = move || mode.get() == m;
                                    view! {
                                        <button
                                            on:click=move |ev: web_sys::MouseEvent| {
                                                let x = ev.client_x() as f64;
                                                let y = ev.client_y() as f64;
                                                animated_apply(x, y, move || apply_mode(m));
                                                mode.set(m);
                                            }
                                            aria-pressed=move || is_active().to_string()
                                            class=move || {
                                                let base = "px-1 py-1.5 rounded-lg text-xs font-medium transition-colors";
                                                if is_active() {
                                                    format!("{base} bg-primary text-white")
                                                } else {
                                                    format!("{base} text-text-secondary hover:bg-surface-sunken")
                                                }
                                            }
                                        >
                                            {move || m.label(i18n)}
                                        </button>
                                    }
                                })
                                .collect::<Vec<_>>()}
                        </div>
                    </div>

                    // Material
                    <div>
                        <p class="px-1 pb-1.5 text-[10px] font-semibold uppercase tracking-wider text-text-tertiary">
                            {t!(i18n, appearance.section.material)}
                        </p>
                        <div class="grid grid-cols-3 gap-1">
                            {Material::ALL
                                .into_iter()
                                .map(|m| {
                                    let is_active = move || material.get() == m;
                                    view! {
                                        <button
                                            on:click=move |ev: web_sys::MouseEvent| {
                                                let x = ev.client_x() as f64;
                                                let y = ev.client_y() as f64;
                                                animated_apply(x, y, move || apply_material(m));
                                                material.set(m);
                                            }
                                            aria-pressed=move || is_active().to_string()
                                            class=move || {
                                                // px-1: 4-CJK labels need the extra 8px in a third-width column (same as the Mode row)
                                                let base = "px-1 py-1.5 rounded-lg text-xs font-medium transition-colors";
                                                if is_active() {
                                                    format!("{base} bg-primary text-white")
                                                } else {
                                                    format!("{base} text-text-secondary hover:bg-surface-sunken")
                                                }
                                            }
                                        >
                                            {move || m.label(i18n)}
                                        </button>
                                    }
                                })
                                .collect::<Vec<_>>()}
                        </div>
                    </div>

                    // Accent
                    <div>
                        <p class="px-1 pb-1.5 text-[10px] font-semibold uppercase tracking-wider text-text-tertiary">
                            {t!(i18n, appearance.section.accent)}
                        </p>
                        <div class="flex items-center justify-between px-1">
                            {Accent::ALL.into_iter().map(|a| {
                                let is_active = move || accent.get() == a;
                                view! {
                                    <SwatchButton
                                        background=a.swatch()
                                        face="w-6 h-6 rounded-full transition-transform group-hover:scale-110"
                                        ring_offset="ring-offset-surface-overlay"
                                        title=Signal::derive(move || a.label(i18n))
                                        active=Signal::derive(is_active)
                                        on_pick=move |ev: web_sys::MouseEvent| {
                                            let x = ev.client_x() as f64;
                                            let y = ev.client_y() as f64;
                                            animated_apply(x, y, move || apply_accent(a));
                                            accent.set(a);
                                        }
                                    />
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    </div>
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::reveal_origin;

    #[test]
    fn maps_click_to_viewport_percentage() {
        let (x, y) = reveal_origin(480.0, 300.0, 1920.0, 1200.0);
        assert_eq!(x, "25.00%");
        assert_eq!(y, "25.00%");
    }

    #[test]
    fn clamps_out_of_bounds_clicks() {
        let (x, y) = reveal_origin(-50.0, 5000.0, 1000.0, 1000.0);
        assert_eq!(x, "0.00%");
        assert_eq!(y, "100.00%");
    }

    #[test]
    fn degrades_to_centre_when_viewport_unknown() {
        let (x, y) = reveal_origin(123.0, 456.0, 0.0, 0.0);
        assert_eq!(x, "50.00%");
        assert_eq!(y, "50.00%");
    }
}
