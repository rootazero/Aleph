//! Localized copy for the model-route modes and load-balancing strategies.
//!
//! Same contract as [`super::mode_labels`] / [`super::exec_tier_labels`]: core
//! ships the ids, the words are this surface's job because they follow the
//! reader's locale, and `t!` / `t_string!` take compile-time key paths so a
//! runtime id resolves through an explicit match.
//!
//! # Why the key lists live here too
//!
//! They were declared twice, and the phone copy said so in a comment —
//! "copied verbatim from route.rs" on both `MODE_KEYS` and `LB_KEYS`. That
//! duplication then grew a third head: the wide screen resolved its labels
//! through `settings.route.mode_*` / `lb_*`, and the phone screen answered the
//! same question again in hard-coded English. Both locales had carried those
//! nine strings since the wide screen was written, so nothing needed
//! translating — a key with no reader and a hard-coded literal are the two
//! halves of one defect, and this module is the one reader.

use leptos_i18n::I18nContext;

use crate::i18n::{t_string, Locale};

/// The three selectable route modes, in the order both screens list them.
pub const MODE_KEYS: &[&str] = &["auto", "always_local", "always_cloud"];

/// Load-balancing strategy ids, in the order both screens list them.
pub const LB_KEYS: &[&str] = &[
    "ordered",
    "round_robin",
    "least_busy",
    "latency_aware",
    "usage_based",
    "cost_aware",
];

/// Short mode name for a selector row.
///
/// An id this client does not know degrades to the raw id rather than to the
/// last arm of the match: both screens used `_ => "Always Cloud"`, which turns
/// a mode a newer core added into a confident, wrong label.
pub fn mode_label(i18n: I18nContext<Locale>, id: &str) -> String {
    match id {
        "auto" => t_string!(i18n, settings.route.mode_auto).to_string(),
        "always_local" => t_string!(i18n, settings.route.mode_local).to_string(),
        "always_cloud" => t_string!(i18n, settings.route.mode_cloud).to_string(),
        other => other.to_string(),
    }
}

/// What the mode does, in one sentence. Empty for an unknown id — there is
/// nothing truthful to say about a mode this client has never heard of.
pub fn mode_desc(i18n: I18nContext<Locale>, id: &str) -> String {
    match id {
        "auto" => t_string!(i18n, settings.route.mode_auto_desc).to_string(),
        "always_local" => t_string!(i18n, settings.route.mode_local_desc).to_string(),
        "always_cloud" => t_string!(i18n, settings.route.mode_cloud_desc).to_string(),
        _ => String::new(),
    }
}

/// Short strategy name for a selector row.
pub fn lb_label(i18n: I18nContext<Locale>, id: &str) -> String {
    match id {
        "ordered" => t_string!(i18n, settings.route.lb_ordered).to_string(),
        "round_robin" => t_string!(i18n, settings.route.lb_round_robin).to_string(),
        "least_busy" => t_string!(i18n, settings.route.lb_least_busy).to_string(),
        "latency_aware" => t_string!(i18n, settings.route.lb_latency_aware).to_string(),
        "usage_based" => t_string!(i18n, settings.route.lb_usage_based).to_string(),
        "cost_aware" => t_string!(i18n, settings.route.lb_cost_aware).to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{LB_KEYS, MODE_KEYS};

    /// Every id a screen offers must be an id the label functions have an arm
    /// for, or the selector renders a raw snake_case token in a list of proper
    /// names — the "degrade to the id" arm is for a *newer core*, not for this
    /// module's own list.
    ///
    /// Source-level because the match arms are the fact; a runtime call would
    /// need an `I18nContext`, which only exists inside a reactive owner.
    #[test]
    fn every_offered_id_has_a_label_arm() {
        // `production_lines`, not `split("#[cfg(test)]")`: the blind cut stops
        // at the first marker rather than walking gated items, and it
        // under-scans silently — `no_guard_in_this_crate_hand_rolls_the_cfg_test_cut`
        // holds this crate to one implementation of that question.
        let src = include_str!("route_labels.rs");
        let arms: String = crate::i18n_census::production_lines(src)
            .into_iter()
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n");
        let mut checked = 0usize;
        for id in MODE_KEYS.iter().chain(LB_KEYS.iter()) {
            checked += 1;
            assert!(
                arms.contains(&format!("\"{id}\" =>")),
                "`{id}` is offered by a selector but has no label arm"
            );
        }
        assert_eq!(
            checked,
            MODE_KEYS.len() + LB_KEYS.len(),
            "the scan must cover every offered id"
        );
    }
}
