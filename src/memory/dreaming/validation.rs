//! Validation Layer — verification after Dream Pipeline execution.
//!
//! - **L1 Format** (produced): YAML frontmatter, categories, non-empty content,
//!   over a bounded newest-first sample of the corpus.
//! - **L2 Consistency** (produced): duplicate content hashes across the index.
//! - **L3 Semantic** (never produced): reserved for an LLM coherence check.
//! - **L4 Retrospective** (never produced): reserved for a recall hit-rate check
//!   against the previous cycle.
//!
//! L3/L4 are `None` on **every** cycle in this repo — the base agent's and every
//! project namespace's alike. That is parity, not a per-namespace gap: no
//! producer exists anywhere, and [`DreamValidationReport::overall_ok`] gates on
//! L1+L2 by design, so nothing downstream is waiting on them. This header used
//! to say the two tiers were "run externally", which described an intended
//! design as if it were a running one; there is no external runner.
//!
//! Deciding to build them is a real decision with real cost (an LLM call per
//! cycle *per corpus*, and a semantic-coherence verdict overlaps what the
//! `note_drift` stage already produces — R7), not a wiring gap to close.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::memory::notes::indexer::CATEGORY_DIRS;

/// A single validation issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub tier: String,
    pub note_path: String,
    pub message: String,
}

/// Result of a single validation tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationTier {
    pub passed: bool,
    pub checks_run: u32,
    pub checks_passed: u32,
    pub issues: Vec<ValidationIssue>,
}

/// Full validation report across all tiers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamValidationReport {
    pub l1_format: ValidationTier,
    pub l2_consistency: ValidationTier,
    /// Always `None` — no producer exists. See the module header.
    pub l3_semantic: Option<ValidationTier>,
    /// Always `None` — no producer exists. See the module header.
    pub l4_retrospective: Option<ValidationTier>,
}

impl DreamValidationReport {
    /// Overall OK if L1 and L2 both passed. L3/L4 failures are warnings.
    #[must_use]
    pub const fn overall_ok(&self) -> bool {
        self.l1_format.passed && self.l2_consistency.passed
    }
}

// ---------------------------------------------------------------------------
// L1: Format validation helpers
// ---------------------------------------------------------------------------

/// Validate frontmatter and content of a single note's markdown.
#[must_use]
pub fn validate_frontmatter(content: &str, note_path: &str) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let tier = "L1".to_string();

    // Check for frontmatter delimiters
    let parts: Vec<&str> = content.splitn(3, "---").collect();
    if parts.len() < 3 {
        issues.push(ValidationIssue {
            // rust-doctor-disable-next-line excessive-clone
            tier: tier.clone(),
            note_path: note_path.to_string(),
            message: "missing YAML frontmatter delimiters".into(),
        });
        return issues;
    }

    let frontmatter = parts[1].trim();
    let body = parts[2].trim();

    // Check category field exists
    let has_category = frontmatter
        .lines()
        .any(|line| line.trim_start().starts_with("category:"));
    if !has_category {
        issues.push(ValidationIssue {
            // rust-doctor-disable-next-line excessive-clone
            tier: tier.clone(),
            note_path: note_path.to_string(),
            message: "missing category field in frontmatter".into(),
        });
    } else if let Some(cat) = extract_yaml_value(frontmatter, "category") {
        // CATEGORY_DIRS is the single source of truth and now includes
        // `entity`/`synthesis`/`query`, so no ad-hoc exceptions are needed —
        // the prior hand-maintained `synthesis`/`query` allowances (and the
        // missing `entity`, which made L1 flag every ingest-written entity note
        // invalid) are subsumed here.
        let valid_categories: HashSet<&str> = CATEGORY_DIRS.iter().copied().collect();
        if !valid_categories.contains(cat.as_str()) {
            issues.push(ValidationIssue {
                // rust-doctor-disable-next-line excessive-clone
                tier: tier.clone(),
                note_path: note_path.to_string(),
                message: format!("invalid category '{cat}' not in CATEGORY_DIRS"),
            });
        }
    }

    // Check for empty content (body after frontmatter)
    if body.is_empty() {
        issues.push(ValidationIssue {
            tier,
            note_path: note_path.to_string(),
            message: "empty content body after frontmatter".into(),
        });
    }

    issues
}

/// Simple YAML value extractor for `key: value` lines.
fn extract_yaml_value(yaml: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    yaml.lines()
        .find(|line| line.trim_start().starts_with(&prefix))
        .map(|line| {
            line.trim_start()
                .strip_prefix(&prefix)
                .unwrap_or("")
                .trim()
                .to_string()
        })
}

// ---------------------------------------------------------------------------
// L2: Consistency validation helpers
// ---------------------------------------------------------------------------

/// Check for duplicate content hashes across notes.
#[must_use]
pub fn check_duplicate_hashes(notes: &[(String, String)]) -> Vec<ValidationIssue> {
    let mut seen: HashMap<&str, Vec<&str>> = HashMap::new();
    for (path, hash) in notes {
        if !hash.is_empty() {
            seen.entry(hash.as_str()).or_default().push(path.as_str());
        }
    }

    let mut issues = Vec::new();
    for (hash, paths) in &seen {
        if paths.len() > 1 {
            issues.push(ValidationIssue {
                tier: "L2".into(),
                note_path: paths.join(", "),
                message: format!(
                    "duplicate content_hash '{}' across {} notes",
                    hash.get(..hash.len().min(16)).unwrap_or(hash),
                    paths.len()
                ),
            });
        }
    }
    issues
}

/// Run L1 format validation on a batch of notes.
#[must_use]
pub fn run_l1_validation(note_contents: &HashMap<String, String>) -> ValidationTier {
    let mut issues = Vec::new();
    let mut checks_run = 0u32;
    let mut checks_passed = 0u32;

    for (path, content) in note_contents {
        checks_run += 1;
        let note_issues = validate_frontmatter(content, path);
        if note_issues.is_empty() {
            checks_passed += 1;
        } else {
            issues.extend(note_issues);
        }
    }

    ValidationTier {
        passed: issues.is_empty(),
        checks_run,
        checks_passed,
        issues,
    }
}

/// Run L2 consistency validation on note hashes.
#[must_use]
pub fn run_l2_validation(note_hashes: &[(String, String)]) -> ValidationTier {
    let dup_issues = check_duplicate_hashes(note_hashes);
    let checks_run = 1u32; // duplicate hash check
    let checks_passed = if dup_issues.is_empty() { 1 } else { 0 };

    ValidationTier {
        passed: dup_issues.is_empty(),
        checks_run,
        checks_passed,
        issues: dup_issues,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_frontmatter_passes_l1() {
        let content = "---\ncategory: learning\ntags: [rust]\ncreated: 2026-04-17\nupdated: 2026-04-17\n---\n\n- Some fact\n";
        let issues = validate_frontmatter(content, "learning/test");
        assert!(issues.is_empty(), "got issues: {:?}", issues);
    }

    #[test]
    fn missing_category_fails_l1() {
        let content = "---\ntags: [rust]\n---\n\n- Some fact\n";
        let issues = validate_frontmatter(content, "learning/test");
        assert!(!issues.is_empty());
        assert!(issues[0].message.contains("category"));
    }

    #[test]
    fn empty_content_fails_l1() {
        let content =
            "---\ncategory: learning\ntags: []\ncreated: 2026-04-17\nupdated: 2026-04-17\n---\n";
        let issues = validate_frontmatter(content, "learning/test");
        assert!(issues.iter().any(|i| i.message.contains("empty")));
    }

    #[test]
    fn invalid_category_fails_l1() {
        let content = "---\ncategory: nonexistent\ntags: []\ncreated: 2026-04-17\nupdated: 2026-04-17\n---\n\n- fact\n";
        let issues = validate_frontmatter(content, "learning/test");
        assert!(issues.iter().any(|i| i.message.contains("category")));
    }

    #[test]
    fn duplicate_hashes_fail_l2() {
        let notes = vec![
            ("a/note1".to_string(), "hash_abc".to_string()),
            ("b/note2".to_string(), "hash_abc".to_string()),
            ("c/note3".to_string(), "hash_xyz".to_string()),
        ];
        let issues = check_duplicate_hashes(&notes);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("duplicate"));
    }

    #[test]
    fn no_duplicate_hashes_passes_l2() {
        let notes = vec![
            ("a/note1".to_string(), "hash_abc".to_string()),
            ("b/note2".to_string(), "hash_def".to_string()),
        ];
        let issues = check_duplicate_hashes(&notes);
        assert!(issues.is_empty());
    }

    #[test]
    fn validation_report_overall_ok_when_l1_l2_pass() {
        let report = DreamValidationReport {
            l1_format: ValidationTier {
                passed: true,
                checks_run: 5,
                checks_passed: 5,
                issues: vec![],
            },
            l2_consistency: ValidationTier {
                passed: true,
                checks_run: 3,
                checks_passed: 3,
                issues: vec![],
            },
            l3_semantic: None,
            l4_retrospective: None,
        };
        assert!(report.overall_ok());
    }

    #[test]
    fn validation_report_not_ok_when_l1_fails() {
        let report = DreamValidationReport {
            l1_format: ValidationTier {
                passed: false,
                checks_run: 5,
                checks_passed: 3,
                issues: vec![],
            },
            l2_consistency: ValidationTier {
                passed: true,
                checks_run: 3,
                checks_passed: 3,
                issues: vec![],
            },
            l3_semantic: None,
            l4_retrospective: None,
        };
        assert!(!report.overall_ok());
    }

    #[test]
    fn l3_failure_still_overall_ok() {
        let report = DreamValidationReport {
            l1_format: ValidationTier {
                passed: true,
                checks_run: 5,
                checks_passed: 5,
                issues: vec![],
            },
            l2_consistency: ValidationTier {
                passed: true,
                checks_run: 3,
                checks_passed: 3,
                issues: vec![],
            },
            l3_semantic: Some(ValidationTier {
                passed: false,
                checks_run: 1,
                checks_passed: 0,
                issues: vec![],
            }),
            l4_retrospective: None,
        };
        // L3 failure is warning, not blocking
        assert!(report.overall_ok());
    }

    #[test]
    fn serde_roundtrip_report() {
        let report = DreamValidationReport {
            l1_format: ValidationTier {
                passed: true,
                checks_run: 1,
                checks_passed: 1,
                issues: vec![],
            },
            l2_consistency: ValidationTier {
                passed: true,
                checks_run: 1,
                checks_passed: 1,
                issues: vec![],
            },
            l3_semantic: None,
            l4_retrospective: None,
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: DreamValidationReport = serde_json::from_str(&json).unwrap();
        assert!(back.overall_ok());
    }
}
