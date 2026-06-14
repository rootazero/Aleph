//! Markdown parsing helpers for knowledge notes.

use chrono::NaiveDate;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::AlephError;

use super::types::{FactProvenance, ProvenanceOrigin};

/// Inline-comment provenance marker, e.g.
/// `\<!-- src: raw/abc, origin: raw_source, inferred: false -->`. The `src:`
/// segment is optional (e.g. inferred facts have no source).
pub static PROVENANCE_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(
        r"<!--\s*(?:src:\s*([^,]+?),\s*)?origin:\s*(raw_source|prior_note|inferred|legacy)\s*,\s*inferred:\s*(true|false)\s*-->",
    ).unwrap()
});

/// Parse a fact-line list and return one `FactProvenance` per fact, defaulting
/// to `Legacy` when no marker is present on that line.
pub fn extract_provenance_markers(body: &str, facts: &[String]) -> Vec<FactProvenance> {
    let mut out: Vec<FactProvenance> = Vec::with_capacity(facts.len());
    let mut idx = 0;
    for raw_line in body.lines() {
        if idx >= facts.len() {
            break;
        }
        let trimmed = raw_line.trim_start();
        // Only top-level bullets start a fact — mirrors `extract_facts`, which
        // attaches indented `- ` lines to the parent fact. Counting indented
        // bullets here shifted every subsequent provenance assignment by one.
        let indent = raw_line.len() - trimmed.len();
        if indent == 0 && trimmed.starts_with("- ") {
            let prov = PROVENANCE_RE
                .captures(raw_line)
                .map(|c| FactProvenance {
                    origin: match &c[2] {
                        "raw_source" => ProvenanceOrigin::RawSource,
                        "prior_note" => ProvenanceOrigin::PriorNote,
                        "inferred" => ProvenanceOrigin::Inferred,
                        _ => ProvenanceOrigin::Legacy,
                    },
                    source_id: c.get(1).map(|m| m.as_str().trim().to_string()),
                    inferred: &c[3] == "true",
                })
                .unwrap_or_default();
            out.push(prov);
            idx += 1;
        }
    }
    while out.len() < facts.len() {
        out.push(FactProvenance::default());
    }
    out
}

/// YAML frontmatter parsed from the top of a markdown note.
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Frontmatter {
    #[serde(default)]
    pub(super) category: String,
    #[serde(default)]
    pub(super) tags: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_optional_date_string")]
    pub(super) created: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_date_string")]
    pub(super) updated: Option<String>,
    #[serde(default = "default_confidence")]
    pub(super) confidence: f32,
    #[serde(default)]
    pub(super) severity: super::types::Severity,
    #[serde(default, alias = "source_facts")]
    pub(super) source_notes: Vec<String>,
    #[serde(default)]
    pub(super) status: super::types::NoteStatus,
    #[serde(default)]
    pub(super) supersedes: Vec<String>,
    #[serde(default)]
    pub(super) superseded_by: Vec<String>,
    /// When `true`, the note is exempt from time decay and archival (the
    /// "permanent" core-knowledge tier). Absent in legacy notes → `false`.
    #[serde(default)]
    pub(super) permanent: bool,
    /// Typed relation edges (Gap A). Absent in legacy notes → empty.
    #[serde(default)]
    pub(super) relations: Vec<super::relation::Relation>,
    /// Obsidian / llm_wiki page-type (mirrors category). Absent in legacy notes → None.
    #[serde(default, rename = "type")]
    pub(super) note_type: Option<String>,
    /// Obsidian title frontmatter. Not mapped into KnowledgeNote.title (filename is SSOT).
    /// Round-trip only: kept so serde does not error on the field.
    #[allow(dead_code)]
    #[serde(default)]
    pub(super) title: Option<String>,
    /// Obsidian aliases from frontmatter `aliases:`. Absent in legacy notes → empty.
    #[serde(default)]
    pub(super) aliases: Vec<String>,
}

/// Accept a YAML date field as either a quoted string, a native YAML date
/// (which `serde_yaml` may surface as a Tagged value or other scalar depending
/// on version), or null. Re-serialize non-string variants and strip
/// surrounding quotes/whitespace so downstream callers always receive a clean
/// `YYYY-MM-DD`-shaped string (or `None` for empty/null).
fn deserialize_optional_date_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let v = serde_yaml::Value::deserialize(d)?;
    Ok(match v {
        serde_yaml::Value::Null => None,
        serde_yaml::Value::String(s) => Some(s),
        other => {
            let s = serde_yaml::to_string(&other)
                .map_err(serde::de::Error::custom)?
                .trim()
                .trim_matches(|c: char| c == '\'' || c == '"' || c.is_whitespace())
                .to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        }
    })
}

pub(super) const fn default_confidence() -> f32 {
    1.0
}

/// Split markdown content into parsed frontmatter and body text.
pub fn split_frontmatter(content: &str) -> Result<(Frontmatter, String), AlephError> {
    let trimmed = content.trim();

    if !trimmed.starts_with("---") {
        return Err(AlephError::ConfigError {
            message: "Note missing YAML frontmatter (must start with ---)".to_string(),
            suggestion: None,
        });
    }

    // Find the closing `---`
    let after_open = &trimmed[3..];
    let close_pos = after_open
        .find("---")
        .ok_or_else(|| AlephError::ConfigError {
            message: "Note missing closing --- for YAML frontmatter".to_string(),
            suggestion: None,
        })?;

    let yaml_str = &after_open[..close_pos];
    let body = after_open[close_pos + 3..].trim().to_string();

    let fm: Frontmatter = serde_yaml::from_str(yaml_str).map_err(|e| AlephError::ConfigError {
        message: format!("Failed to parse YAML frontmatter: {e}"),
        suggestion: None,
    })?;

    Ok((fm, body))
}

/// Parse an optional date string (YYYY-MM-DD) to a unix timestamp (midnight UTC).
/// Returns 0 if the date is `None` or empty.
pub fn parse_date_to_unix(date: &Option<String>) -> Result<i64, AlephError> {
    let Some(s) = date.as_deref() else {
        return Ok(0);
    };
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }

    let nd = NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| AlephError::ConfigError {
        message: format!("Invalid date '{s}': {e}"),
        suggestion: Some("Use YYYY-MM-DD format".to_string()),
    })?;

    let dt = nd.and_hms_opt(0, 0, 0).expect("midnight is always valid");
    Ok(dt.and_utc().timestamp())
}

/// Extract bullet-point facts from the body.
///
/// Each top-level bullet (`- `) starts a new fact. Indented lines (indent >= 2)
/// that follow a bullet are attached to the current fact. A blank line terminates
/// the current fact. Non-bullet lines at indent 0 also terminate the current fact
/// and are ignored.
pub fn extract_facts(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for raw_line in body.lines() {
        let trimmed_start = raw_line.trim_start();
        let indent = raw_line.len() - trimmed_start.len();
        let is_top_bullet = indent == 0 && trimmed_start.starts_with("- ");
        let is_blank = raw_line.trim().is_empty();

        if is_top_bullet {
            if let Some(c) = current.take() {
                out.push(c);
            }
            current = Some(trimmed_start[2..].to_string());
        } else if is_blank {
            if let Some(c) = current.take() {
                out.push(c);
            }
        } else if let Some(acc) = current.as_mut() {
            if indent >= 2 {
                // attach indented line to current fact
                acc.push('\n');
                acc.push_str(raw_line);
            } else {
                // non-bullet line at indent 0 ends any current fact and is ignored
                if let Some(c) = current.take() {
                    out.push(c);
                }
            }
        } else {
            // non-bullet line at indent 0 ends any current fact and is ignored
            if let Some(c) = current.take() {
                out.push(c);
            }
        }
    }
    if let Some(c) = current.take() {
        out.push(c);
    }
    out
}
