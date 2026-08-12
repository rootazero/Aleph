//! Localized copy for the execution-permission tiers.
//!
//! Core ships the tier IDS and their order (`builtin_tiers()`); the words are
//! this surface's job, because they have to follow the reader's locale. Shared
//! by the composer's `ExecTierPicker` and Settings → Policies so the two never
//! describe the same tier differently.
//!
//! `t!` / `t_string!` take compile-time key paths, so a runtime id resolves
//! through an explicit match — the same pattern as `components/extensions/labels.rs`.

use leptos_i18n::I18nContext;

use crate::i18n::{t_string, Locale};

/// The tier whose blast radius warrants a second, explicit click. Shared by the
/// composer pill and Settings → Policies: one tier id, one confirmation rule.
pub const FULL_TIER: &str = "full";

/// Short tier name for the pill and the option row.
///
/// An id this client does not know (core could ship a fourth tier to an older
/// Panel) degrades to the raw id rather than rendering blank.
pub fn tier_label(i18n: I18nContext<Locale>, id: &str) -> String {
    match id {
        "plan" => t_string!(i18n, settings.policies.exec_tier_plan_label).to_string(),
        "ask" => t_string!(i18n, settings.policies.exec_tier_ask_label).to_string(),
        "auto" => t_string!(i18n, settings.policies.exec_tier_auto_label).to_string(),
        "full" => t_string!(i18n, settings.policies.exec_tier_full_label).to_string(),
        other => other.to_string(),
    }
}

/// What the tier does, in one sentence. Empty for an unknown id — there is
/// nothing truthful to say about a tier this client has never heard of.
pub fn tier_desc(i18n: I18nContext<Locale>, id: &str) -> String {
    match id {
        "plan" => t_string!(i18n, settings.policies.exec_tier_plan_desc).to_string(),
        "ask" => t_string!(i18n, settings.policies.exec_tier_ask_desc).to_string(),
        "auto" => t_string!(i18n, settings.policies.exec_tier_auto_desc).to_string(),
        "full" => t_string!(i18n, settings.policies.exec_tier_full_desc).to_string(),
        _ => String::new(),
    }
}
