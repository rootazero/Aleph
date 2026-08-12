//! Localized copy for the two session dials this Panel added last: reasoning
//! depth and memory mode.
//!
//! Core ships the ids and their order (`builtin_think_levels()` /
//! `builtin_memory_modes()`); the words are this surface's job, because they
//! have to follow the reader's locale. Same contract as `exec_tier_labels` and
//! `mode_labels`.
//!
//! `t!` / `t_string!` take compile-time key paths, so a runtime id resolves
//! through an explicit match. An id this client does not know degrades to the
//! raw id rather than rendering blank — a core can ship a rung this build has
//! never heard of.

use leptos_i18n::I18nContext;

use crate::i18n::{t_string, Locale};

/// Short name for a reasoning-depth rung.
pub fn think_label(i18n: I18nContext<Locale>, id: &str) -> String {
    match id {
        "off" => t_string!(i18n, settings.policies.think_off_label).to_string(),
        "minimal" => t_string!(i18n, settings.policies.think_minimal_label).to_string(),
        "low" => t_string!(i18n, settings.policies.think_low_label).to_string(),
        "medium" => t_string!(i18n, settings.policies.think_medium_label).to_string(),
        "high" => t_string!(i18n, settings.policies.think_high_label).to_string(),
        "xhigh" => t_string!(i18n, settings.policies.think_xhigh_label).to_string(),
        other => other.to_string(),
    }
}

/// What a rung costs and buys, in one sentence. Empty for an unknown id —
/// there is nothing truthful to say about a level this client never heard of.
pub fn think_desc(i18n: I18nContext<Locale>, id: &str) -> String {
    match id {
        "off" => t_string!(i18n, settings.policies.think_off_desc).to_string(),
        "minimal" => t_string!(i18n, settings.policies.think_minimal_desc).to_string(),
        "low" => t_string!(i18n, settings.policies.think_low_desc).to_string(),
        "medium" => t_string!(i18n, settings.policies.think_medium_desc).to_string(),
        "high" => t_string!(i18n, settings.policies.think_high_desc).to_string(),
        "xhigh" => t_string!(i18n, settings.policies.think_xhigh_desc).to_string(),
        _ => String::new(),
    }
}

/// Short name for a memory mode.
pub fn memory_label(i18n: I18nContext<Locale>, id: &str) -> String {
    match id {
        "on" => t_string!(i18n, settings.policies.memory_on_label).to_string(),
        "off" => t_string!(i18n, settings.policies.memory_off_label).to_string(),
        other => other.to_string(),
    }
}

/// What a memory mode does — and, for `off`, what it deliberately does NOT do.
pub fn memory_desc(i18n: I18nContext<Locale>, id: &str) -> String {
    match id {
        "on" => t_string!(i18n, settings.policies.memory_on_desc).to_string(),
        "off" => t_string!(i18n, settings.policies.memory_off_desc).to_string(),
        _ => String::new(),
    }
}
