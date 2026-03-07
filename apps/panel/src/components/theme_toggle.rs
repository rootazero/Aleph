// apps/panel/src/components/theme_toggle.rs
//
// Theme mode toggle: System / Light / Dark. Persists choice to localStorage.
//
use leptos::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq)]
enum ThemeMode {
    System,
    Light,
    Dark,
}

impl ThemeMode {
    fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[component]
pub fn ThemeToggle() -> impl IntoView {
    let initial = {
        let window = web_sys::window().unwrap();
        let storage: Option<web_sys::Storage> = window.local_storage().ok().flatten();
        match storage.as_ref().and_then(|s: &web_sys::Storage| s.get_item("aleph-theme").ok()).flatten().as_deref() {
            Some("light") => ThemeMode::Light,
            Some("dark") => ThemeMode::Dark,
            _ => ThemeMode::System,
        }
    };
    let (theme, set_theme) = signal(initial);

    let toggle = move |_| {
        let next = theme.get().next();
        set_theme.set(next);

        let window = web_sys::window().unwrap();
        let document = window.document().unwrap();
        let html = document.document_element().unwrap();
        let class_list = html.class_list();

        let _ = class_list.remove_2("dark", "light");

        let storage: Option<web_sys::Storage> = window.local_storage().ok().flatten();
        match next {
            ThemeMode::Light => {
                let _ = class_list.add_1("light");
                if let Some(s) = &storage { let _ = s.set_item("aleph-theme", "light"); }
            }
            ThemeMode::Dark => {
                let _ = class_list.add_1("dark");
                if let Some(s) = &storage { let _ = s.set_item("aleph-theme", "dark"); }
            }
            ThemeMode::System => {
                if let Some(s) = &storage { let _ = s.remove_item("aleph-theme"); }
            }
        }
    };

    let icon = move || match theme.get() {
        ThemeMode::System => view! {
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
                <line x1="8" y1="21" x2="16" y2="21" />
                <line x1="12" y1="17" x2="12" y2="21" />
            </svg>
        }.into_any(),
        ThemeMode::Light => view! {
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="5" />
                <line x1="12" y1="1" x2="12" y2="3" />
                <line x1="12" y1="21" x2="12" y2="23" />
                <line x1="4.22" y1="4.22" x2="5.64" y2="5.64" />
                <line x1="18.36" y1="18.36" x2="19.78" y2="19.78" />
                <line x1="1" y1="12" x2="3" y2="12" />
                <line x1="21" y1="12" x2="23" y2="12" />
                <line x1="4.22" y1="19.78" x2="5.64" y2="18.36" />
                <line x1="18.36" y1="5.64" x2="19.78" y2="4.22" />
            </svg>
        }.into_any(),
        ThemeMode::Dark => view! {
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
            </svg>
        }.into_any(),
    };

    view! {
        <button
            on:click=toggle
            class="flex items-center gap-1.5 px-2 py-1.5 rounded-lg text-text-secondary hover:text-text-primary hover:bg-surface-sunken transition-all duration-200"
            title=move || format!("Theme: {}", theme.get().label())
        >
            {icon}
            <span class="text-xs font-medium">{move || theme.get().label()}</span>
        </button>
    }
}
