//! Interface-local appearance commands. Never forward these to the chat service.

use crate::appearance::ThemeMode;
use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThemeRequest {
    Open,
    Apply(ThemeMode),
    Invalid,
}

pub(crate) fn parse_theme_request(text: &str) -> Option<ThemeRequest> {
    let mut words = text.split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("/theme") {
        return None;
    }
    let arg = words.next().map(str::to_ascii_lowercase);
    if words.next().is_some() {
        return Some(ThemeRequest::Invalid);
    }
    Some(match arg.as_deref() {
        None | Some("list") => ThemeRequest::Open,
        Some("system") => ThemeRequest::Apply(ThemeMode::System),
        Some("light") => ThemeRequest::Apply(ThemeMode::Light),
        Some("dark") => ThemeRequest::Apply(ThemeMode::Dark),
        _ => ThemeRequest::Invalid,
    })
}

pub(super) fn theme_command_info(description: String) -> super::palette::CommandInfo {
    super::palette::CommandInfo {
        key: "theme".into(),
        description,
        is_namespace: false,
        param_hint: Some("[system|light|dark]".into()),
        children: Vec::new(),
    }
}

#[component]
pub(crate) fn ThemeCommandPicker(
    open: RwSignal<bool>,
    invalid: RwSignal<bool>,
    selected: RwSignal<usize>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <Show when=move || open.get()>
            <div class="absolute bottom-full inset-x-0 mb-2 z-50 rounded-xl border border-border bg-surface-overlay p-3 flex flex-wrap items-center gap-2" role="dialog" aria-label=t_string!(i18n, appearance.theme_picker_aria)>
                <span>{move || t_string!(i18n, appearance.popover_title).to_string()}</span>
                {ThemeMode::ALL.into_iter().enumerate().map(|(index, mode)| view! {
                    <button class=move || if selected.get() == index { "px-3 py-2 rounded-lg bg-primary text-white" } else { "px-3 py-2 rounded-lg hover:bg-surface-sunken" } on:click=move |_| {
                        crate::appearance::apply_mode(mode);
                        open.set(false);
                    }>{move || mode.label(i18n)}</button>
                }).collect::<Vec<_>>()}
                <Show when=move || invalid.get()>
                    <span class="text-text-secondary">{t_string!(i18n, appearance.theme_hint).to_string()}</span>
                </Show>
                <button aria-label=t_string!(i18n, appearance.close_aria) on:click=move |_| open.set(false)>"×"</button>
            </div>
        </Show>
    }
}

/// Consume navigation before the draft's send/queue handlers.
pub(crate) fn handle_picker_key(
    key: &str,
    open: RwSignal<bool>,
    selected: RwSignal<usize>,
) -> bool {
    if !open.get_untracked() {
        return false;
    }
    match key {
        "ArrowDown" | "ArrowRight" => selected.update(|i| *i = (*i + 1) % ThemeMode::ALL.len()),
        "ArrowUp" | "ArrowLeft" => {
            selected.update(|i| *i = (*i + ThemeMode::ALL.len() - 1) % ThemeMode::ALL.len())
        }
        "Enter" => {
            crate::appearance::apply_mode(ThemeMode::ALL[selected.get_untracked()]);
            open.set(false);
        }
        "Escape" => open.set(false),
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_commands_are_local_and_use_panel_names() {
        assert_eq!(
            parse_theme_request(" /theme list "),
            Some(ThemeRequest::Open)
        );
        assert_eq!(
            parse_theme_request("/THEME dark"),
            Some(ThemeRequest::Apply(ThemeMode::Dark))
        );
        assert_eq!(
            parse_theme_request("/theme amber"),
            Some(ThemeRequest::Invalid)
        );
        assert_eq!(
            parse_theme_request("/theme dark extra"),
            Some(ThemeRequest::Invalid)
        );
        assert_eq!(parse_theme_request("/themes"), None);
        assert_eq!(parse_theme_request("hello /theme"), None);
    }
}
