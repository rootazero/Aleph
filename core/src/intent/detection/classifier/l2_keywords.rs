//! L2 Keyword matching for intent classification (<20ms).

use super::keywords::{EXCLUSION_VERBS, KEYWORD_SETS};
use super::l1_regex::extract_path;
use super::types::ExecutableTask;
use crate::intent::detection::keyword::{KeywordIndex, KeywordMatchMode, KeywordRule};
use crate::intent::types::TaskCategory;
use crate::config::{KeywordPolicy, PolicyKeywordRule};

/// L2: Keyword + rule matching (<20ms)
pub fn match_keywords(input: &str) -> Option<ExecutableTask> {
    let input_lower = input.to_lowercase();

    // Check exclusion patterns first - if input contains analysis/understanding verbs,
    // it should NOT trigger agent mode (e.g., "分析图片" is analysis, not file operation)
    if contains_exclusion_verb(&input_lower) {
        return None;
    }

    for set in KEYWORD_SETS {
        let has_verb = set.verbs.iter().any(|v| input_lower.contains(v));
        let has_noun = set.nouns.iter().any(|n| input_lower.contains(n));

        if has_verb && has_noun {
            let target = extract_path(input);
            return Some(ExecutableTask {
                category: set.category,
                action: input.to_string(),
                target,
                confidence: 0.85, // Keyword match = good confidence
            });
        }
    }
    None
}

/// Check if input contains exclusion verbs (analysis/understanding actions)
fn contains_exclusion_verb(input: &str) -> bool {
    EXCLUSION_VERBS.iter().any(|v| input.contains(v))
}

/// L2 Enhanced: Use KeywordIndex for weighted matching
pub fn match_keywords_enhanced(
    input: &str,
    keyword_index: &KeywordIndex,
) -> Option<ExecutableTask> {
    // Check exclusion patterns first
    if contains_exclusion_verb(&input.to_lowercase()) {
        return None;
    }

    // Try keyword index
    if let Some(km) = keyword_index.best_match(input, 0.5) {
        if let Some(category) = intent_type_to_category(&km.intent_type) {
            let target = extract_path(input);
            return Some(ExecutableTask {
                category,
                action: input.to_string(),
                target,
                confidence: km.score,
            });
        }
    }
    None
}

/// Convert intent type string to TaskCategory
pub fn intent_type_to_category(intent_type: &str) -> Option<TaskCategory> {
    match intent_type {
        "FileOrganize" => Some(TaskCategory::FileOrganize),
        "FileTransfer" => Some(TaskCategory::FileTransfer),
        "FileCleanup" => Some(TaskCategory::FileCleanup),
        "CodeExecution" => Some(TaskCategory::CodeExecution),
        "DocumentGenerate" => Some(TaskCategory::DocumentGenerate),
        _ => None,
    }
}

/// Load keyword rules from config into a KeywordIndex
pub fn load_keyword_rules(rules: &[PolicyKeywordRule]) -> KeywordIndex {
    let mut keyword_index = KeywordIndex::new();

    for rule_config in rules {
        let mode = match rule_config.match_mode.as_str() {
            "all" => KeywordMatchMode::All,
            "weighted" => KeywordMatchMode::Weighted,
            _ => KeywordMatchMode::Any,
        };

        let mut rule = KeywordRule::new(&rule_config.id, &rule_config.intent_type);
        for kw in &rule_config.keywords {
            rule = rule.with_keyword(&kw.word, kw.weight);
        }
        rule = rule
            .with_match_mode(mode)
            .with_min_score(rule_config.min_score);

        keyword_index.add_rule(rule);
    }

    keyword_index
}

/// Create classifier keyword index from policy
pub fn create_keyword_index_from_policy(policy: &KeywordPolicy) -> KeywordIndex {
    if policy.enabled {
        load_keyword_rules(&policy.rules)
    } else {
        KeywordIndex::new()
    }
}
