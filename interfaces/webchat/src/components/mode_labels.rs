//! Localized copy for the session usage modes (chat / work / code).
//!
//! Core ships the mode IDS and their order (`builtin_modes()`); the words are
//! this surface's job, because they have to follow the reader's locale. Same
//! contract as `exec_tier_labels`.
//!
//! `t!` / `t_string!` take compile-time key paths, so a runtime id resolves
//! through an explicit match.

use leptos_i18n::I18nContext;

use crate::i18n::{t_string, Locale};

/// Short mode name for the pill and the option row.
///
/// An id this client does not know (core could ship a fourth mode to an older
/// Panel) degrades to the raw id rather than rendering blank.
pub fn mode_label(i18n: I18nContext<Locale>, id: &str) -> String {
    match id {
        "chat" => t_string!(i18n, settings.policies.mode_chat_label).to_string(),
        "work" => t_string!(i18n, settings.policies.mode_work_label).to_string(),
        "code" => t_string!(i18n, settings.policies.mode_code_label).to_string(),
        other => other.to_string(),
    }
}

/// What the mode does, in one sentence. Empty for an unknown id — there is
/// nothing truthful to say about a mode this client has never heard of.
pub fn mode_desc(i18n: I18nContext<Locale>, id: &str) -> String {
    match id {
        "chat" => t_string!(i18n, settings.policies.mode_chat_desc).to_string(),
        "work" => t_string!(i18n, settings.policies.mode_work_desc).to_string(),
        "code" => t_string!(i18n, settings.policies.mode_code_desc).to_string(),
        _ => String::new(),
    }
}
