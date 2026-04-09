//! Progressive search scope narrowing.
//!
//! Implements three-level scope: TopicLocal → DomainWide → Global.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchScope {
    TopicLocal { domain: String, topic: String },
    DomainWide { domain: String },
    Global,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressiveSearchConfig {
    pub enabled: bool,
    pub min_results: usize,
    pub topic_boost: f32,
    pub domain_boost: f32,
}

impl Default for ProgressiveSearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_results: 3,
            topic_boost: 0.1,
            domain_boost: 0.05,
        }
    }
}

pub fn build_scope_sequence(domain: &str, topic: &str) -> Vec<SearchScope> {
    let mut scopes = Vec::new();
    if !domain.is_empty() && !topic.is_empty() {
        scopes.push(SearchScope::TopicLocal {
            domain: domain.to_string(),
            topic: topic.to_string(),
        });
    }
    if !domain.is_empty() {
        scopes.push(SearchScope::DomainWide {
            domain: domain.to_string(),
        });
    }
    scopes
}

pub fn infer_scope_from_facts(paths: &[&str]) -> (String, String) {
    use std::collections::HashMap;

    use crate::memory::context::parse_domain_topic;

    let mut counts: HashMap<(&str, &str), usize> = HashMap::new();
    for path in paths {
        let (domain, topic): (&str, &str) = parse_domain_topic(path);
        if !domain.is_empty() {
            *counts.entry((domain, topic)).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|((d, t), _)| (d.to_string(), t.to_string()))
        .unwrap_or_else(|| (String::new(), String::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_scope_sequence_full_path() {
        let scopes = build_scope_sequence("user", "preferences");
        assert_eq!(scopes.len(), 2);
        assert_eq!(
            scopes[0],
            SearchScope::TopicLocal {
                domain: "user".into(),
                topic: "preferences".into()
            }
        );
        assert_eq!(
            scopes[1],
            SearchScope::DomainWide {
                domain: "user".into()
            }
        );
    }

    #[test]
    fn build_scope_sequence_domain_only() {
        let scopes = build_scope_sequence("knowledge", "");
        assert_eq!(scopes.len(), 1);
        assert_eq!(
            scopes[0],
            SearchScope::DomainWide {
                domain: "knowledge".into()
            }
        );
    }

    #[test]
    fn build_scope_sequence_empty() {
        assert!(build_scope_sequence("", "").is_empty());
    }

    #[test]
    fn infer_scope_picks_most_frequent() {
        let paths = [
            "aleph://user/preferences/coding",
            "aleph://user/preferences/editor",
            "aleph://knowledge/projects/aleph",
        ];
        let (domain, topic) = infer_scope_from_facts(&paths);
        assert_eq!(domain, "user");
        assert_eq!(topic, "preferences");
    }

    #[test]
    fn infer_scope_empty_input() {
        let paths: [&str; 0] = [];
        let (d, t) = infer_scope_from_facts(&paths);
        assert_eq!(d, "");
        assert_eq!(t, "");
    }

    #[test]
    fn infer_scope_invalid_paths_ignored() {
        let paths = ["not-a-vfs-path", "random/thing"];
        let (d, t) = infer_scope_from_facts(&paths);
        assert_eq!(d, "");
        assert_eq!(t, "");
    }
}
