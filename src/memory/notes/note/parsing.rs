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
        r"<!--\s*(?:src:\s*([^,]+?),\s*)?origin:\s*(raw_source|prior_note|inferred|legacy|system)\s*,\s*inferred:\s*(true|false)\s*-->",
    // rust-doctor-disable-next-line unwrap-in-production
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
                        "system" => ProvenanceOrigin::System,
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
    pub(super) supersedes: Vec<String>,
    #[serde(default)]
    pub(super) superseded_by: Vec<String>,
    /// When `true`, the note is exempt from time decay and archival (the
    /// "permanent" core-knowledge tier). Absent in legacy notes → `false`.
    #[serde(default)]
    pub(super) permanent: bool,
    /// When `true`, `NoteDrift` has judged this note's information outdated or
    /// contradicted by a newer note. Read by `NoteDecay` to archive it out of
    /// active retrieval. Absent in legacy notes → `false`.
    #[serde(default)]
    pub(super) stale: bool,
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
/// (which `crate::yaml` may surface as a Tagged value or other scalar
/// depending on version), or null. Re-serialize non-string variants and strip
/// surrounding quotes/whitespace so downstream callers always receive a clean
/// `YYYY-MM-DD`-shaped string (or `None` for empty/null).
fn deserialize_optional_date_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let v = crate::yaml::Value::deserialize(d)?;
    Ok(match v {
        crate::yaml::Value::Null => None,
        crate::yaml::Value::String(s) => Some(s),
        other => {
            let s = crate::yaml::to_string(&other)
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

/// Every frontmatter key [`Frontmatter`] models explicitly.
///
/// Anything else in a note's YAML header is **passthrough** — carried on
/// [`super::KnowledgeNote::extra_frontmatter`] and re-emitted verbatim by
/// `to_markdown`, so a key a human (or Obsidian, or an external tool) wrote
/// survives every rewrite instead of being silently destroyed by the first
/// write that passes through this layer.
///
/// `source_facts` is here because it is a serde `alias` for `source_notes`:
/// omitting it would re-emit the same data under two keys.
///
/// Kept in sync with the struct by
/// `tests::known_frontmatter_keys_covers_every_modelled_field`.
pub(super) const KNOWN_FRONTMATTER_KEYS: &[&str] = &[
    "aliases",
    "category",
    "confidence",
    "created",
    "permanent",
    "relations",
    "severity",
    "source_facts",
    "source_notes",
    "stale",
    "supersedes",
    "superseded_by",
    "tags",
    "title",
    "type",
    "updated",
];

/// Frontmatter keys the note layer does not model, preserved for round-trip.
///
/// `BTreeMap` (not `HashMap`): the emission order must be deterministic, or
/// every rewrite would shuffle the header and churn `content_hash`.
pub type ExtraFrontmatter = std::collections::BTreeMap<String, crate::yaml::Value>;

/// Collect the frontmatter keys `Frontmatter` does not model.
///
/// Parsed independently of `Frontmatter` rather than via `#[serde(flatten)]`:
/// flatten routes the whole struct through serde's buffered `Content`
/// representation, which would change how `deserialize_optional_date_string`
/// sees a native YAML date — a silent behaviour change on the parse path this
/// module's own regression tests were written to pin.
fn collect_extra_frontmatter(yaml: &str) -> ExtraFrontmatter {
    let Ok(crate::yaml::Value::Mapping(map)) = crate::yaml::from_str::<crate::yaml::Value>(yaml)
    else {
        return ExtraFrontmatter::new();
    };
    map.into_iter()
        .filter_map(|(k, v)| {
            // `crate::yaml`'s `Mapping` is keyed by `Value`, not by `String`,
            // so a key is a passthrough candidate only when it is a plain
            // string scalar. `ExtraFrontmatter` is keyed by `String` and the
            // re-emitter writes `key: value` lines, so a non-string key
            // (`1: x`, `[a]: x`) has nowhere to go; it is dropped rather than
            // stringified, because stringifying would change the key on the
            // round-trip and a changed key is worse than an absent one.
            let k = k.as_str()?.to_owned();
            if KNOWN_FRONTMATTER_KEYS.contains(&k.as_str()) {
                None
            } else {
                Some((k, v))
            }
        })
        .collect()
}

/// Split markdown content into parsed frontmatter, passthrough keys, and body.
pub fn split_frontmatter(
    content: &str,
) -> Result<(Frontmatter, ExtraFrontmatter, String), AlephError> {
    let trimmed = content.trim();

    if !trimmed.starts_with("---") {
        return Err(AlephError::ConfigError {
            message: "Note missing YAML frontmatter (must start with ---)".to_string(),
            suggestion: None,
        });
    }

    // Find the closing fence: a whole line equal to `---`. A plain substring
    // find would cut the YAML mid-line for values containing `---` (e.g.
    // `title: phase---2`), producing a permanently unparseable note.
    let after_open = &trimmed[3..];
    let mut fence: Option<(usize, usize)> = None; // (yaml_end, body_start)
    let mut pos = 0usize;
    for line in after_open.split_inclusive('\n') {
        if pos > 0 && line.trim_end() == "---" {
            fence = Some((pos, pos + line.len()));
            break;
        }
        pos += line.len();
    }
    let (yaml_end, body_start) = fence.ok_or_else(|| AlephError::ConfigError {
        message: "Note missing closing --- for YAML frontmatter".to_string(),
        suggestion: None,
    })?;

    let yaml_str = &after_open[..yaml_end];
    let body = after_open[body_start..].trim().to_string();

    let fm: Frontmatter = crate::yaml::from_str(yaml_str).map_err(|e| AlephError::ConfigError {
        message: format!("Failed to parse YAML frontmatter: {e}"),
        suggestion: None,
    })?;

    Ok((fm, collect_extra_frontmatter(yaml_str), body))
}

/// Parse an optional date string to a unix timestamp. Accepts RFC3339
/// (`2026-07-02T09:30:00Z`, second precision — what `to_markdown` now emits)
/// and the legacy `YYYY-MM-DD` (midnight UTC). Returns 0 if `None` or empty.
///
/// Day-granular dates made `updated_at` collapse to midnight on every reparse
/// from disk, so `list_notes`' `ORDER BY updated_at DESC` gave arbitrary
/// intra-day ordering after a rebuild — hence the second-precision format.
pub fn parse_date_to_unix(date: &Option<String>) -> Result<i64, AlephError> {
    let Some(s) = date.as_deref() else {
        return Ok(0);
    };
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.timestamp());
    }

    let nd = NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| AlephError::ConfigError {
        message: format!("Invalid date '{s}': {e}"),
        suggestion: Some("Use RFC3339 or YYYY-MM-DD format".to_string()),
    })?;

    // rust-doctor-disable-next-line unwrap-in-production
    let dt = nd.and_hms_opt(0, 0, 0).expect("midnight is always valid");
    Ok(dt.and_utc().timestamp())
}

/// Per-fact provenance parse — single-fact counterpart of
/// `extract_provenance_markers`. Returns `Default` (Legacy) when the fact
/// carries no recognizable marker.
#[must_use]
pub fn fact_provenance_for(fact: &str) -> super::types::FactProvenance {
    use super::types::{FactProvenance, ProvenanceOrigin};
    if let Some(caps) = PROVENANCE_RE.captures(fact) {
        let source_id = caps.get(1).map(|m| m.as_str().trim().to_string());
        let origin = match caps.get(2).map(|m| m.as_str()).unwrap_or("legacy") {
            "raw_source" => ProvenanceOrigin::RawSource,
            "prior_note" => ProvenanceOrigin::PriorNote,
            "inferred" => ProvenanceOrigin::Inferred,
            "system" => ProvenanceOrigin::System,
            _ => ProvenanceOrigin::Legacy,
        };
        let inferred = caps.get(3).map(|m| m.as_str() == "true").unwrap_or(false);
        FactProvenance {
            origin,
            source_id,
            inferred,
        }
    } else {
        FactProvenance::default()
    }
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
