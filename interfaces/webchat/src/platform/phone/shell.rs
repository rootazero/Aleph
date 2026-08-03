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
            <button class="tabitem" class:tabitem-active=move || mode.get().under_more() on:click=go("/more")>
                <svg width="23" height="23" viewBox="0 0 24 24" fill="currentColor"><circle cx="5" cy="12" r="1.7"></circle><circle cx="12" cy="12" r="1.7"></circle><circle cx="19" cy="12" r="1.7"></circle></svg>
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
#[component]
#[must_use]
pub fn PhoneShell(
    #[prop(into)] title: String,
    #[prop(optional)] back: Option<&'static str>,
    #[prop(optional)] back_label: Option<&'static str>,
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
            <div
                class="glass"
                style="position:relative; flex:none; display:flex; align-items:center; gap:8px; min-height:50px; padding:calc(4px + env(safe-area-inset-top)) 14px 8px; z-index:4; background-color:color-mix(in oklch, var(--color-surface-overlay) 78%, transparent);"
            >
                {back_btn}
                <span style=title_style>{title}</span>
            </div>
            <div
                class="cc-hide-scroll"
                style="flex:1; min-height:0; overflow-y:auto; display:flex; flex-direction:column; gap:20px; padding:16px 16px 18px;"
            >
                {children()}
            </div>
            <PhoneTabBar/>
        </div>
    }
}
