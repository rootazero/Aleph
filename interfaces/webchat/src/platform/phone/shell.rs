//! platform/phone/shell.rs
//! Shared iOS chrome for phone screens: a full-screen `h-dvh` shell (top bar +
//! scroll body + bottom tab bar) and the tab bar itself. `h-dvh` (not inset-0)
//! keeps the tab bar above the mobile browser's bottom toolbar.

use crate::components::mode_sidebar::PanelMode;
use leptos::prelude::*;
use leptos_router::hooks::use_location;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

/// Bottom tab bar shared by every phone screen (landing + detail). Settings is
/// the active tab on all settings screens. I/O-only: each item navigates.
#[component]
#[must_use]
pub fn PhoneTabBar() -> impl IntoView {
    let navigate = use_navigate();
    let go = move |path: &'static str| {
        let navigate = navigate.clone();
        move |_| navigate(path, NavigateOptions::default())
    };
    let location = use_location();
    let mode = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));
    // Same derivation the More menu's Alerts row reads — one source, so the dot
    // and the row's number can never disagree about what is waiting.
    let badge = crate::platform::phone::more::alert_badge_count();
    view! {
        <div class="tabbar glass" style="flex:none;">
            <button class="tabitem" class:tabitem-active=move || mode.get() == PanelMode::Chat on:click=go("/")>
                <svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 11.5a8.4 8.4 0 0 1-8.5 8.5 8.7 8.7 0 0 1-4-1L3 20l1-5.5a8.4 8.4 0 0 1-1-4A8.4 8.4 0 0 1 11.5 2 8.4 8.4 0 0 1 21 11.5z"></path></svg>
                "Chat"
            </button>
            <button class="tabitem" class:tabitem-active=move || mode.get() == PanelMode::Memory on:click=go("/memory")>
                <svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="6" cy="7" r="2.4"></circle><circle cx="18" cy="8" r="2.4"></circle><circle cx="11" cy="17" r="2.4"></circle><path d="M8 8.4l1.5 6.4M15.8 9.6L12.6 15.6"></path></svg>
                "Memory"
            </button>
            <button class="tabitem" class:tabitem-active=move || mode.get() == PanelMode::Agents on:click=go("/agents")>
                <svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="8" r="4"></circle><path d="M5 21a7 7 0 0 1 14 0"></path></svg>
                "Agents"
            </button>
            <button class="tabitem" class:tabitem-active=move || mode.get() == PanelMode::Settings on:click=go("/settings")>
                <svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-2.7 1.1V21a2 2 0 1 1-4 0v-.1A1.6 1.6 0 0 0 6.6 19l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1A1.6 1.6 0 0 0 4 13.6H4a2 2 0 1 1 0-4h.1A1.6 1.6 0 0 0 5 6.6l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1A1.6 1.6 0 0 0 10.4 4V4a2 2 0 1 1 4 0v.1A1.6 1.6 0 0 0 17 5l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0 1.1 2.7H21a2 2 0 1 1 0 4h-.1a1.6 1.6 0 0 0-1.5 1z"></path></svg>
                "Settings"
            </button>
            // Alerts live under ••• (iOS caps a tab bar at five and this is the
            // fifth), so the badge has to ride here or an alert is invisible
            // until the user happens to open More. Dot, not a count: a tab item
            // is ~65 px wide and a number would collide with its label — the
            // count itself is one tap away on the Alerts row.
            <button class="tabitem" class:tabitem-active=move || mode.get().under_more() on:click=go("/more")>
                <span style="position:relative; display:inline-flex;">
                    <svg width="23" height="23" viewBox="0 0 24 24" fill="currentColor"><circle cx="5" cy="12" r="1.7"></circle><circle cx="12" cy="12" r="1.7"></circle><circle cx="19" cy="12" r="1.7"></circle></svg>
                    // Braces required: bare `badge.get() > 0` lets the `view!`
                    // macro read the `>` as the tag close.
                    <Show when=move || { badge.get() > 0 }>
                        <span
                            aria-label="Unread alerts"
                            style="position:absolute; top:-1px; right:-3px; width:8px; height:8px; border-radius:9999px; background:var(--color-danger); box-shadow:0 0 0 2px var(--color-surface-overlay);"
                        ></span>
                    </Show>
                </span>
                "More"
            </button>
        </div>
    }
}

/// Full-screen iOS shell: gradient bg, glass top bar (optional `‹ Settings`
/// back + title), scroll body, shared bottom tab bar. `back=None` = landing
/// (left-aligned title, no back); `back=Some(route)` = detail (centered title +
/// back button). Root uses `h-dvh` so the tab bar clears the mobile browser bar.
///
/// `title` is `impl Into<String>` (not `&'static str`) so a caller can hand it a
/// runtime-resolved label — the settings drill-down titles its screens from
/// `SettingsTab::i18n_label`, which is a `String`. Literals still work verbatim.
/// `back` / `back_label` stay `&'static str`: they are route constants.
///
/// `wrapped` marks the case where `children` is a **desktop page body** rather
/// than hand-built iOS content (the settings drill-down's 17 pages without a
/// native screen, and every `PhoneDashboard` leaf). Those pages bring their own
/// `p-6` gutters and their own inner scroll, so the shell's 16 px padding and
/// 20 px gap were stacked on top of desktop padding — 40 px of gutter on a
/// 390 px viewport. It also scopes the `.phone-wrapped` shim (`styles/ios.css`),
/// which stacks the desktop two-column layouts, collapses multi-column grids and
/// drops the macOS traffic-light inset that is meaningless inside this shell.
#[component]
#[must_use]
pub fn PhoneShell(
    #[prop(into)] title: String,
    #[prop(optional)] back: Option<&'static str>,
    #[prop(optional)] back_label: Option<&'static str>,
    #[prop(optional)] wrapped: bool,
    children: Children,
) -> impl IntoView {
    let navigate = use_navigate();
    let back_btn = back.map(|to| {
        let navigate = navigate.clone();
        let label = back_label.unwrap_or("Settings");
        view! {
            <button
                style="position:absolute; left:10px; top:50%; transform:translateY(-10%); display:flex; align-items:center; gap:2px; background:none; border:0; cursor:pointer; color:var(--color-primary); font:inherit; font-size:16px; padding:4px 6px 4px 0;"
                on:click=move |_| navigate(to, NavigateOptions::default())
            >
                <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 6 9 12 15 18"></polyline></svg>
                {label}
            </button>
        }
    });
    // Title: left-aligned on the landing; centered on detail screens (iOS nav).
    let title_style = if back.is_some() {
        "width:100%; text-align:center; font-size:17px; font-weight:600; letter-spacing:-0.01em; color:var(--color-text-primary);"
    } else {
        "flex:1; font-size:20px; font-weight:700; letter-spacing:-0.02em; color:var(--color-text-primary);"
    };
    view! {
        <div
            class="fixed inset-x-0 top-0 h-dvh z-[70] flex flex-col"
            style="background:radial-gradient(120% 55% at 50% 0%, oklch(0.62 0.10 310 / 0.14), transparent 62%),radial-gradient(120% 45% at 50% 100%, oklch(0.60 0.09 250 / 0.10), transparent 60%),var(--color-surface);"
        >
            // Layout is `.phone-topbar` in `styles/ios.css`, NOT an inline
            // `style=`: an inline declaration outranks any non-`!important`
            // stylesheet rule, so the landscape height rule would be present,
            // matched, and silently overridden. Keep this attribute list free of
            // `style` — a guard below asserts it.
            <div class="glass phone-topbar">
                {back_btn}
                <span style=title_style>{title}</span>
            </div>
            <div
                class=if wrapped {
                    "cc-hide-scroll phone-wrapped"
                } else {
                    "cc-hide-scroll"
                }
                style=if wrapped {
                    "flex:1; min-height:0; overflow-y:auto; display:flex; flex-direction:column;"
                } else {
                    "flex:1; min-height:0; overflow-y:auto; display:flex; flex-direction:column; gap:20px; padding:16px 16px 18px;"
                }
            >
                {children()}
            </div>
            <PhoneTabBar/>
        </div>
    }
}

#[cfg(test)]
mod tests {
    const IOS_CSS: &str = include_str!("../../../styles/ios.css");
    const SRC: &str = include_str!("shell.rs");

    fn production_half(src: &str) -> &str {
        src.split("#[cfg").next().unwrap_or(src)
    }

    /// The top bar's layout must be a class, and the class must exist.
    ///
    /// Both halves, because either one alone is silently useless: a class with
    /// no rule renders an unstyled bar, and a rule with no class renders the
    /// old inline one. CSS has no "unknown selector" error and Leptos has no
    /// "unknown class" error, so neither half fails loudly.
    #[test]
    fn the_top_bar_layout_is_a_class_on_both_ends() {
        assert!(
            production_half(SRC).contains("class=\"glass phone-topbar\""),
            "the phone shell top bar no longer carries `phone-topbar`"
        );
        assert!(
            IOS_CSS.contains(".phone-topbar {"),
            "styles/ios.css has no base rule for the phone shell top bar"
        );
    }

    /// …and it must not carry an inline `style`.
    ///
    /// This is the specific regression, not a style preference: an inline
    /// declaration outranks every non-`!important` stylesheet rule, so the
    /// landscape height rule would be in the build, match the viewport, and be
    /// overridden — with `getComputedStyle` reporting the inline value and
    /// nothing anywhere reporting a fault. The composer-tools fold shipped
    /// exactly this way once.
    #[test]
    fn the_top_bar_has_no_inline_style_to_outrank_the_media_query() {
        let src = production_half(SRC);
        let Some((_, after)) = src.split_once("class=\"glass phone-topbar\"") else {
            panic!("top bar not found — this guard is no longer looking at it");
        };
        let attrs = after.split('>').next().unwrap_or("");
        assert!(
            !attrs.contains("style="),
            "the phone shell top bar grew an inline `style`, which silently \
             outranks the landscape rule in styles/ios.css: {attrs}"
        );
    }

    /// Landscape compression is three rules in one block; any one of them
    /// missing leaves height on the floor of a ~390 px-tall viewport.
    #[test]
    fn landscape_compresses_the_shell_chrome() {
        let (_, landscape) = IOS_CSS
            .split_once("@media (orientation: landscape) and (max-height: 500px) {")
            .expect("the landscape block is gone");
        let block = landscape.split("\n}").next().unwrap_or("");
        for sel in [".phone-composer-tools", ".phone-topbar", ".tabitem"] {
            assert!(
                block.contains(sel),
                "landscape block no longer compresses `{sel}`"
            );
        }
    }

    /// …and the base rule must sit ABOVE that block.
    ///
    /// Both are a single class, so neither outranks the other and source order
    /// decides. With the base rule below, its `min-height: 50px` beat the
    /// landscape `44px` — measured live at 50 px while the media query itself
    /// reported `matches: true`. Nothing about that is visible in the CSS, in
    /// the build output, or in a media-query check; only `getComputedStyle`
    /// shows it. Cheap to pin, invisible to lose.
    #[test]
    fn the_base_top_bar_rule_precedes_the_landscape_override() {
        let base = IOS_CSS
            .find(".phone-topbar {\n  position: relative;")
            .expect("the base .phone-topbar rule is gone");
        let media = IOS_CSS
            .find("@media (orientation: landscape) and (max-height: 500px) {")
            .expect("the landscape block is gone");
        assert!(
            base < media,
            "the base `.phone-topbar` rule moved below the landscape block — \
             equal specificity means the later one wins, so the landscape \
             height is silently ignored"
        );
    }
}
