use leptos_i18n::I18nContext;

use crate::i18n::{t_string, Locale};

/// Localize a `kind` string using literal i18n key paths.
/// The `t!` macro requires compile-time literal keys, so we match on the runtime string.
pub fn kind_label(i18n: I18nContext<Locale>, kind: &str) -> String {
    match kind {
        "skill" => t_string!(i18n, extensions.kind.skill).to_string(),
        "plugin" => t_string!(i18n, extensions.kind.plugin).to_string(),
        "mcp" => t_string!(i18n, extensions.kind.mcp).to_string(),
        _ => t_string!(i18n, extensions.kind.other).to_string(),
    }
}

/// Localize a `trust_tier` string using literal i18n key paths.
pub fn trust_label(i18n: I18nContext<Locale>, tier: &str) -> String {
    match tier {
        "official" => t_string!(i18n, extensions.trust.official).to_string(),
        "verified" => t_string!(i18n, extensions.trust.verified).to_string(),
        "community" => t_string!(i18n, extensions.trust.community).to_string(),
        _ => t_string!(i18n, extensions.trust.unverified).to_string(),
    }
}

/// Localize a category value string using literal i18n key paths.
/// Covers "featured", "all", and all 13 CATEGORIES values.
/// Used for BOTH the left-column CategoryNav labels and shelf titles (browse.rs).
pub fn category_label(i18n: I18nContext<Locale>, value: &str) -> String {
    match value {
        "featured" => t_string!(i18n, extensions.cat.featured).to_string(),
        "all" => t_string!(i18n, extensions.cat.all).to_string(),
        "search" => t_string!(i18n, extensions.cat.search).to_string(),
        "developer" => t_string!(i18n, extensions.cat.developer).to_string(),
        "data" => t_string!(i18n, extensions.cat.data).to_string(),
        "productivity" => t_string!(i18n, extensions.cat.productivity).to_string(),
        "writing" => t_string!(i18n, extensions.cat.writing).to_string(),
        "communication" => t_string!(i18n, extensions.cat.communication).to_string(),
        "knowledge" => t_string!(i18n, extensions.cat.knowledge).to_string(),
        "files" => t_string!(i18n, extensions.cat.files).to_string(),
        "design" => t_string!(i18n, extensions.cat.design).to_string(),
        "automation" => t_string!(i18n, extensions.cat.automation).to_string(),
        "finance" => t_string!(i18n, extensions.cat.finance).to_string(),
        "utilities" => t_string!(i18n, extensions.cat.utilities).to_string(),
        "other" => t_string!(i18n, extensions.cat.other).to_string(),
        _ => value.to_string(),
    }
}
