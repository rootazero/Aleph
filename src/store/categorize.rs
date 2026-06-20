//! Deterministic functional-category assignment for catalog entries.
//!
//! No LLM (v1 decision): a keyword map over name+description+tags, with an
//! optional upstream hint (e.g. the Docker MCP catalog's `category`) taking
//! precedence. Runs as a post-sync enrichment pass so the panel's category
//! browse (P3) is populated instead of every entry collapsing to `Other`.
use crate::store::types::ExtensionCategory;

/// Map a raw upstream category string to our enum. `None` when unrecognized.
#[must_use]
pub fn category_from_hint(hint: &str) -> Option<ExtensionCategory> {
    use ExtensionCategory::{
        Automation, Communication, Data, Design, Developer, Files, Finance, Knowledge,
        Productivity, Search, Utilities, Writing,
    };
    Some(match hint.trim().to_ascii_lowercase().as_str() {
        "search" | "web-search" | "web_search" => Search,
        "developer" | "dev" | "development" | "devops" | "ci-cd" | "ci/cd" => Developer,
        "data" | "database" | "databases" | "analytics" => Data,
        "productivity" => Productivity,
        "writing" => Writing,
        "communication" | "messaging" | "chat" | "email" => Communication,
        "knowledge" | "docs" | "documentation" | "reference" => Knowledge,
        "files" | "storage" | "filesystem" => Files,
        "design" => Design,
        "automation" | "workflow" => Automation,
        "finance" | "payments" | "crypto" => Finance,
        "utilities" | "utility" | "tools" => Utilities,
        _ => return None,
    })
}

/// Keyword groups, most specific first. First group with any keyword present
/// in the haystack wins; otherwise `Other`.
const GROUPS: &[(&[&str], ExtensionCategory)] = &[
    (
        &["postgres", "mysql", "sqlite", "mongodb", "database", " sql", "bigquery", "snowflake", "redis", "duckdb"],
        ExtensionCategory::Data,
    ),
    (
        &["web search", "brave search", "google search", "serp", "duckduckgo", "perplexity", "websearch"],
        ExtensionCategory::Search,
    ),
    (
        &["github", "gitlab", "kubernetes", "docker", "terraform", "jira", "compiler", "debugger", "lint", "devops"],
        ExtensionCategory::Developer,
    ),
    (
        &["slack", "discord", "telegram", "gmail", "sendgrid", "twilio", " sms", "mailgun"],
        ExtensionCategory::Communication,
    ),
    (
        &["notion", "obsidian", "confluence", "wiki", "knowledge base"],
        ExtensionCategory::Knowledge,
    ),
    (
        &["filesystem", "file system", " s3", "dropbox", "google drive", "ftp", "object storage"],
        ExtensionCategory::Files,
    ),
    (&["figma", "canva", "image generation", "design"], ExtensionCategory::Design),
    (
        &["stripe", "paypal", "payment", "invoice", "accounting", "ethereum", "finance"],
        ExtensionCategory::Finance,
    ),
    (&["calendar", "todo", "reminder", "productivity"], ExtensionCategory::Productivity),
    (&["grammar", "copywriting", "blog post", "writing assistant"], ExtensionCategory::Writing),
    (&["zapier", "automation", "cron", "scheduler", "workflow"], ExtensionCategory::Automation),
];

/// Deterministic category from free text. Hint (if recognized) wins.
#[must_use]
pub fn categorize(
    name: &str,
    description: &str,
    tags: &[String],
    hint: Option<&str>,
) -> ExtensionCategory {
    if let Some(c) = hint.and_then(category_from_hint) {
        return c;
    }
    let hay = format!("{name} {description} {}", tags.join(" ")).to_ascii_lowercase();
    for (keys, cat) in GROUPS {
        if keys.iter().any(|k| hay.contains(k)) {
            return *cat;
        }
    }
    ExtensionCategory::Other
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::types::ExtensionCategory as C;

    #[test]
    fn hint_maps_known_upstream_categories() {
        assert_eq!(category_from_hint("developer"), Some(C::Developer));
        assert_eq!(category_from_hint("Database"), Some(C::Data));
        assert_eq!(category_from_hint("search"), Some(C::Search));
        assert_eq!(category_from_hint("nonsense"), None);
    }

    #[test]
    fn hint_wins_over_text() {
        // text says "github" (Developer) but explicit hint says data
        let c = categorize("gh thing", "github helper", &[], Some("data"));
        assert_eq!(c, C::Data);
    }

    #[test]
    fn text_keywords_route_to_category() {
        assert_eq!(categorize("pg", "a postgres database client", &[], None), C::Data);
        assert_eq!(categorize("ghx", "github pull request tool", &[], None), C::Developer);
        assert_eq!(categorize("brave", "web search via brave", &[], None), C::Search);
        assert_eq!(categorize("slackbot", "post to slack channels", &[], None), C::Communication);
    }

    #[test]
    fn unknown_text_is_other() {
        assert_eq!(categorize("zzz", "an inscrutable widget", &[], None), C::Other);
    }

    #[test]
    fn tags_are_considered() {
        assert_eq!(categorize("x", "no hints in name", &["database".into()], None), C::Data);
    }
}
