//
// Theme picker — a popover that controls two orthogonal axes:
//   • Mode   : System / Light / Dark / Vibrant (translucent glass)
//   • Accent : Mauve / Ocean / Forest / Sunset / Rose
//
// Mode drives the `dark` / `light` / `translucent` classes on <html>;
// accent drives the `data-accent` attribute. Both persist to
// localStorage (`aleph-theme`, `aleph-accent`) and are re-applied on
// boot by `init_theme()` in lib.rs.
//
use leptos::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeMode {
    System,
    Light,
    Dark,
    Vibrant,
}

impl ThemeMode {
    fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Light => "明亮",
            Self::Dark => "暗黑",
            Self::Vibrant => "玻璃",
        }
    }

    /// localStorage value, or `None` for System (which clears the key).
    fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
            Self::Vibrant => Some("translucent"),
        }
    }
}

/// Accent palette: (id, label, swatch color).
/// "mauve" is the base theme — selecting it clears `data-accent`.
const ACCENTS: &[(&str, &str, &str)] = &[
    ("mauve", "魅紫", "oklch(0.60 0.13 310)"),
    ("ocean", "海蓝", "oklch(0.58 0.13 250)"),
    ("forest", "森绿", "oklch(0.55 0.12 150)"),
    ("sunset", "暖橙", "oklch(0.66 0.135 60)"),
    ("rose", "玫瑰", "oklch(0.62 0.15 15)"),
];

/// Read the current mode from localStorage.
fn read_mode() -> ThemeMode {
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten());
    match storage
        .and_then(|s| s.get_item("aleph-theme").ok().flatten())
        .as_deref()
    {
        Some("light") => ThemeMode::Light,
        Some("dark") => ThemeMode::Dark,
        Some("translucent") => ThemeMode::Vibrant,
        _ => ThemeMode::System,
    }
}

/// Read the current accent id from localStorage (defaults to "mauve").
fn read_accent() -> String {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("aleph-accent").ok().flatten())
        .filter(|a| ACCENTS.iter().any(|(id, ..)| id == a))
        .unwrap_or_else(|| "mauve".to_string())
}

/// Apply a mode to <html> and persist it.
fn apply_mode(mode: ThemeMode) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(html) = document.document_element() else {
        return;
    };

    let class_list = html.class_list();
    let _ = class_list.remove_3("dark", "light", "translucent");
    match mode {
        ThemeMode::Light => {
            let _ = class_list.add_1("light");
        }
        ThemeMode::Dark => {
            let _ = class_list.add_1("dark");
        }
        ThemeMode::Vibrant => {
            let _ = class_list.add_2("dark", "translucent");
        }
        ThemeMode::System => {}
    }

    if let Some(storage) = window.local_storage().ok().flatten() {
        match mode.storage_value() {
            Some(value) => {
                let _ = storage.set_item("aleph-theme", value);
            }
            None => {
                let _ = storage.remove_item("aleph-theme");
            }
        }
    }
}

/// Apply an accent to <html> and persist it.
fn apply_accent(id: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(html) = document.document_element() else {
        return;
    };

    if id == "mauve" {
        let _ = html.remove_attribute("data-accent");
    } else {
        let _ = html.set_attribute("data-accent", id);
    }

    if let Some(storage) = window.local_storage().ok().flatten() {
        if id == "mauve" {
            let _ = storage.remove_item("aleph-accent");
        } else {
            let _ = storage.set_item("aleph-accent", id);
        }
    }
}

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
    let start_fn = js_sys::Reflect::get(document.as_ref(), &JsValue::from_str("startViewTransition"))
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
        let vw = window.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
        let vh = window.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
        let (x, y) = reveal_origin(client_x, client_y, vw, vh);
        let style = html.style();
        let _ = style.set_property("--theme-x", &x);
        let _ = style.set_property("--theme-y", &y);
        let _ = html.class_list().add_1("theme-switching");
    }

    // startViewTransition(cb) snapshots the old DOM, runs cb synchronously to
    // mutate the theme, then animates from old → new snapshot.
    let cb = Closure::once_into_js(move || apply());
    let _ = start_fn.call1(document.as_ref(), &cb);
}

#[component]
pub fn ThemeToggle() -> impl IntoView {
    let mode = RwSignal::new(read_mode());
    let accent = RwSignal::new(read_accent());
    let open = RwSignal::new(false);

    let current_swatch = move || {
        let id = accent.get();
        ACCENTS
            .iter()
            .find(|(aid, ..)| *aid == id)
            .map(|(.., color)| color.to_string())
            .unwrap_or_else(|| "oklch(0.60 0.13 310)".to_string())
    };

    view! {
        <div class="relative">
            // Trigger
            <button
                on:click=move |_| open.update(|v| *v = !*v)
                class="flex items-center gap-1.5 px-2 py-1.5 rounded-lg text-text-secondary
                       hover:text-text-primary hover:bg-surface-sunken transition-all duration-200"
                title="主题"
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
                <div class="theme-picker-popover glass-surface glass animate-pop-in absolute top-full right-0 mt-2 z-50 w-56
                            rounded-xl border border-border bg-surface-overlay/90 shadow-xl p-3 space-y-3"
                     style="transform-origin: top right;"
                >
                    // Mode
                    <div>
                        <p class="px-1 pb-1.5 text-[10px] font-semibold uppercase tracking-wider text-text-tertiary">
                            "外观"
                        </p>
                        <div class="grid grid-cols-2 gap-1">
                            {[ThemeMode::System, ThemeMode::Light, ThemeMode::Dark, ThemeMode::Vibrant]
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
                                            class=move || {
                                                let base = "px-2 py-1.5 rounded-lg text-xs font-medium transition-colors";
                                                if is_active() {
                                                    format!("{base} bg-primary text-white")
                                                } else {
                                                    format!("{base} text-text-secondary hover:bg-surface-sunken")
                                                }
                                            }
                                        >
                                            {m.label()}
                                        </button>
                                    }
                                })
                                .collect::<Vec<_>>()}
                        </div>
                    </div>

                    // Accent
                    <div>
                        <p class="px-1 pb-1.5 text-[10px] font-semibold uppercase tracking-wider text-text-tertiary">
                            "强调色"
                        </p>
                        <div class="flex items-center justify-between px-1">
                            {ACCENTS.iter().map(|(id, label, color)| {
                                let id = id.to_string();
                                let id_for_click = id.clone();
                                let color = color.to_string();
                                let is_active = {
                                    let id = id.clone();
                                    move || accent.get() == id
                                };
                                view! {
                                    <button
                                        on:click=move |ev: web_sys::MouseEvent| {
                                            let id = id_for_click.clone();
                                            let x = ev.client_x() as f64;
                                            let y = ev.client_y() as f64;
                                            animated_apply(x, y, move || apply_accent(&id));
                                            accent.set(id_for_click.clone());
                                        }
                                        title=*label
                                        class="flex flex-col items-center gap-1 group"
                                    >
                                        <span
                                            class=move || {
                                                let base = "w-6 h-6 rounded-full transition-transform group-hover:scale-110";
                                                if is_active() {
                                                    format!("{base} ring-2 ring-offset-2 ring-offset-surface-overlay ring-text-primary")
                                                } else {
                                                    format!("{base} ring-1 ring-border")
                                                }
                                            }
                                            style=format!("background: {}", color)
                                        />
                                    </button>
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
