//! iPhone Settings — landing screen + phone screen classification for the whole
//! `/settings/**` tree.
//!
//! The landing is rendered from [`SETTINGS_GROUPS`] — the *same* const the
//! desktop settings sidebar renders from — so every settings page the desktop
//! exposes is reachable on the phone by construction, with the same grouping,
//! the same i18n labels and the same icons. It used to be a hand-written list of
//! five rows carrying design-mock values (`"Anthropic"`, `"Opus 4.8"`,
//! `"remote · 10.10.10.4"`, a hard-coded active accent swatch …) declared as
//! "static placeholders for v1". That made it a second source of navigation
//! truth *and* a set of numbers with no producer: 17 of the 22 desktop settings
//! pages had no phone entry at all, and every value shown was fiction. Both are
//! gone — a row shows a value only when [`live_value`] can name the code that
//! computes it.
//!
//! Screens come in two shapes, decided by [`screen_for_path`]:
//!   * **Native** — a hand-built iOS screen (Appearance / Connection /
//!     Embeddings / Model route / Providers). It brings its own `PhoneShell`.
//!   * **Wrapped** — the desktop view mounted inside a `PhoneShell` with a
//!     `‹ Settings` back button and the tab bar. Same move `PhoneDashboard`
//!     already makes for Overview / Logs / Tasks …; zero duplicated UI, and the
//!     page tracks the desktop automatically. Without it these paths rendered
//!     the 256 px desktop sidebar into a 390 px viewport with no way back.
//!
//! I/O-only (R4): the landing only navigates.

pub mod appearance;
pub mod connection;
pub mod embeddings;
pub mod model_route;
pub mod providers;

use crate::components::settings_sidebar::{SettingsTab, SETTINGS_GROUPS};
use crate::i18n::{use_i18n, Locale};
use crate::platform::phone::shell::PhoneShell;
use leptos::prelude::*;
use leptos_i18n::I18nContext;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

/// One of the hand-built iOS settings screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeScreen {
    Appearance,
    Connection,
    Embeddings,
    ModelRoute,
    Providers,
}

/// How the phone renders a `/settings…` path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneSettingsScreen {
    /// `/settings` — the sections landing.
    Landing,
    /// A hand-built iOS screen.
    Native(NativeScreen),
    /// The desktop view, wrapped in `PhoneShell`.
    Wrapped,
    /// Not a settings path — the settings router renders nothing.
    NotSettings,
}

/// Classify a URL path into the phone screen that serves it.
///
/// Pure (no reactive reads, no `window`) so the guard test below can assert that
/// **every** path in `SETTINGS_GROUPS` resolves to a real screen. That assertion
/// is the wire: adding a settings tab to the desktop sidebar without giving the
/// phone a screen now fails the test rather than silently stranding phone users.
#[must_use]
pub fn screen_for_path(path: &str) -> PhoneSettingsScreen {
    let path = path.trim_end_matches('/');
    match path {
        "/settings" => PhoneSettingsScreen::Landing,
        "/settings/appearance" => PhoneSettingsScreen::Native(NativeScreen::Appearance),
        "/settings/network" => PhoneSettingsScreen::Native(NativeScreen::Connection),
        "/settings/embedding-providers" => PhoneSettingsScreen::Native(NativeScreen::Embeddings),
        "/settings/model-route" => PhoneSettingsScreen::Native(NativeScreen::ModelRoute),
        "/settings/providers" => PhoneSettingsScreen::Native(NativeScreen::Providers),
        // Every other `/settings/**` path — including `/settings/channels/<id>`
        // — falls to the wrapped desktop view rather than to nothing, so an
        // unknown or deep-linked path is still escapable (back + tab bar).
        _ if path.starts_with("/settings/") => PhoneSettingsScreen::Wrapped,
        _ => PhoneSettingsScreen::NotSettings,
    }
}

/// Title for a wrapped settings screen. Resolved from `SETTINGS_GROUPS` so it is
/// the same string the desktop sidebar shows; channel platform pages (which are
/// not sidebar tabs) fall back to the Channels label, and anything unrecognised
/// to "Settings" — a wrapped page always has *a* title, never an empty bar.
#[must_use]
pub fn title_for_path(path: &str, i18n: I18nContext<Locale>) -> String {
    let path = path.trim_end_matches('/');
    if let Some(tab) = tab_for_path(path) {
        return tab.i18n_label(i18n);
    }
    if path.starts_with("/settings/channels/") {
        return SettingsTab::Channels.i18n_label(i18n);
    }
    "Settings".to_string()
}

/// The sidebar tab owning an exact path, if any.
fn tab_for_path(path: &str) -> Option<SettingsTab> {
    SETTINGS_GROUPS
        .iter()
        .flat_map(|g| g.tabs.iter())
        .copied()
        .find(|t| t.path() == path)
}

/// A live summary for a settings row, or `None` when nothing in this build can
/// compute one synchronously.
///
/// Deliberately sparse. The rule it enforces: **a value shown on the landing
/// must have a producer we can name.** Theme reads the same `read_mode()` the
/// Appearance screen writes; Connection reads the same host the Connection
/// screen prints. Everything else (provider name, embedding model, route model)
/// lives behind an async RPC and belongs on its own screen — a constant here
/// would be the fiction this module just deleted.
fn live_value(tab: SettingsTab) -> Option<String> {
    match tab {
        SettingsTab::Appearance => Some(crate::appearance::read_mode().label().to_string()),
        SettingsTab::Network => {
            let host = self::connection::current_host();
            if host.is_empty() {
                None
            } else if self::connection::is_loopback_host(&host) {
                Some("local".to_string())
            } else {
                Some(host)
            }
        }
        _ => None,
    }
}

#[component]
#[must_use]
pub fn PhoneSettings() -> impl IntoView {
    let i18n = use_i18n();
    let navigate = use_navigate();

    view! {
        <PhoneShell title="Settings">
            {SETTINGS_GROUPS.iter().map(|group| {
                let group_label = group.i18n_label(i18n);
                let rows = group.tabs.iter().copied().map(|tab| {
                    let path = tab.path();
                    let label = tab.i18n_label(i18n);
                    let icon = tab.icon_svg();
                    let value = live_value(tab);
                    // `use_navigate` returns a Clone-able Fn; each row gets its own.
                    let navigate = navigate.clone();
                    view! {
                        <div
                            class="cell"
                            on:click=move |_| navigate(path, NavigateOptions::default())
                        >
                            <span class="cell-leading">
                                <svg
                                    width="17" height="17" viewBox="0 0 24 24" fill="none"
                                    stroke="currentColor" stroke-width="1.8"
                                    stroke-linecap="round" stroke-linejoin="round"
                                    inner_html=icon
                                />
                            </span>
                            <div class="cell-body"><div class="cell-title">{label}</div></div>
                            {value.map(|v| view! { <span class="cell-value">{v}</span> })}
                            <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                        </div>
                    }
                }).collect_view();
                view! {
                    <div>
                        <div class="list-header">{group_label}</div>
                        <div class="list">{rows}</div>
                    </div>
                }
            }).collect_view()}
        </PhoneShell>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every settings page the desktop sidebar exposes resolves to a phone
    /// screen — and specifically not to the landing, which would be a routing
    /// loop.
    ///
    /// Honest about its own strength: the `Wrapped` fallback catches anything
    /// under `/settings/`, so this cannot fail for a tab whose path stays inside
    /// that namespace. What it does catch is a tab registered *outside* it,
    /// which would render nothing on a phone. The reachability of the rows is
    /// held by `landing_renders_from_the_shared_settings_groups_const` below —
    /// that is the assertion with weight, since a hand-written landing would
    /// leave this test green while stranding the user exactly as before.
    #[test]
    fn every_desktop_settings_tab_has_a_phone_screen() {
        for group in SETTINGS_GROUPS {
            for tab in group.tabs {
                let screen = screen_for_path(tab.path());
                assert_ne!(
                    screen,
                    PhoneSettingsScreen::NotSettings,
                    "settings tab {} has no phone screen",
                    tab.path()
                );
                assert_ne!(
                    screen,
                    PhoneSettingsScreen::Landing,
                    "settings tab {} collides with the landing",
                    tab.path()
                );
            }
        }
    }

    /// Every native screen must correspond to a real sidebar tab — a native
    /// screen for a path the desktop no longer routes is dead UI.
    #[test]
    fn every_native_screen_is_a_real_settings_tab() {
        for path in [
            "/settings/appearance",
            "/settings/network",
            "/settings/embedding-providers",
            "/settings/model-route",
            "/settings/providers",
        ] {
            assert!(
                tab_for_path(path).is_some(),
                "native phone screen {path} is not a desktop settings tab"
            );
            assert!(matches!(
                screen_for_path(path),
                PhoneSettingsScreen::Native(_)
            ));
        }
    }

    #[test]
    fn landing_and_trailing_slash() {
        assert_eq!(screen_for_path("/settings"), PhoneSettingsScreen::Landing);
        assert_eq!(screen_for_path("/settings/"), PhoneSettingsScreen::Landing);
        assert_eq!(
            screen_for_path("/settings/appearance/"),
            PhoneSettingsScreen::Native(NativeScreen::Appearance)
        );
    }

    /// Channel platform pages are not sidebar tabs but must still be escapable.
    #[test]
    fn channel_platform_pages_and_unknown_paths_are_wrapped() {
        assert_eq!(
            screen_for_path("/settings/channels/discord"),
            PhoneSettingsScreen::Wrapped
        );
        assert_eq!(
            screen_for_path("/settings/unknown-page"),
            PhoneSettingsScreen::Wrapped
        );
    }

    /// Does `src` (a copy of this file) still build the landing rows by walking
    /// the shared const, taking each row's route and label from the tab?
    ///
    /// Split out of the test so the check itself is falsifiable: mutating the
    /// real file to prove the assertion can fail would just stop it compiling,
    /// which is no signal at all. `landing_derivation_check_rejects_a_hand_written_list`
    /// below feeds it the shape this module replaced and requires a `false`.
    fn landing_is_derived(src: &str) -> bool {
        let Some((_, body)) = src.split_once("pub fn PhoneSettings()") else {
            return false;
        };
        body.contains("SETTINGS_GROUPS.iter()")
            && body.contains("tab.path()")
            && body.contains("tab.i18n_label(i18n)")
    }

    /// The landing must *derive* its rows from the shared const, not re-list
    /// them. This is the load-bearing guard of the module: reachability of all
    /// 22 pages is a property of that loop, so the thing worth pinning is that
    /// the loop is still there. A host test cannot mount a Leptos view, so the
    /// component source is the only available grip (same reason
    /// `phone_menu_renders_a_row_for_every_leaf` reads `menu.rs` as text).
    ///
    /// Reverting to a hand-written list fails here and nowhere else.
    #[test]
    fn landing_renders_from_the_shared_settings_groups_const() {
        assert!(
            landing_is_derived(include_str!("mod.rs")),
            "the settings landing no longer derives its rows from SETTINGS_GROUPS \
             — rows are hand-listed again, so a new desktop settings tab will be \
             invisible on phone"
        );
    }

    /// Proves the check above can say no. Without this it is a guard that has
    /// never been observed to fail — the failure mode this repo keeps hitting.
    #[test]
    fn landing_derivation_check_rejects_a_hand_written_list() {
        // Condensed from the landing as it was before this change: five rows,
        // literal labels, literal routes, no reference to the shared const.
        let hand_written = r#"
            pub fn PhoneSettings() -> impl IntoView {
                view! {
                    <PhoneShell title="Settings">
                        <div class="cell" on:click=go("/settings/network")>
                            <div class="cell-title">"Connection"</div>
                            <span class="cell-value">"remote · 10.10.10.4"</span>
                        </div>
                        <div class="cell" on:click=go("/settings/providers")>
                            <div class="cell-title">"Providers"</div>
                            <span class="cell-value">"Anthropic"</span>
                        </div>
                    </PhoneShell>
                }
            }
        "#;
        assert!(!landing_is_derived(hand_written));
        // A file with no landing at all is also not "derived".
        assert!(!landing_is_derived("fn unrelated() {}"));
    }

    #[test]
    fn non_settings_paths_are_not_claimed() {
        for path in ["/", "/memory", "/agents", "/dashboard", "/settingsfoo"] {
            assert_eq!(
                screen_for_path(path),
                PhoneSettingsScreen::NotSettings,
                "{path} must not be claimed by the settings router"
            );
        }
    }
}
