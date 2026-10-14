use extism_pdk::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// Summary of a single fact, provided by the host.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FactSummary {
    pub id: String,
    pub category: String,
    pub fact_type: String,
    pub confidence: f64,
    pub decay_score: f64,
    pub created_at: String, // ISO 8601
    pub workspace: Option<String>,
}

// ---------------------------------------------------------------------------
// memory_stats
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StatsInput {
    #[serde(default)]
    pub facts: Vec<FactSummary>,
    pub workspace: Option<String>,
    #[serde(default)]
    pub include_decay: bool,
}

#[derive(Debug, Serialize)]
pub struct CategoryStats {
    pub count: usize,
    pub avg_confidence: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_decay: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct StatsOutput {
    pub total_facts: usize,
    pub by_category: BTreeMap<String, CategoryStats>,
    pub by_type: BTreeMap<String, usize>,
}

#[plugin_fn]
pub fn memory_stats(Json(input): Json<StatsInput>) -> FnResult<Json<StatsOutput>> {
    let facts: Vec<&FactSummary> = match &input.workspace {
        Some(ws) => input
            .facts
            .iter()
            .filter(|f| f.workspace.as_deref() == Some(ws.as_str()))
            .collect(),
        None => input.facts.iter().collect(),
    };

    let total_facts = facts.len();

    // Group by category
    let mut cat_groups: BTreeMap<String, Vec<&FactSummary>> = BTreeMap::new();
    for fact in &facts {
        cat_groups
            .entry(fact.category.clone())
            .or_default()
            .push(fact);
    }

    let by_category = cat_groups
        .into_iter()
        .map(|(cat, group)| {
            let count = group.len();
            let avg_confidence = if count > 0 {
                group.iter().map(|f| f.confidence).sum::<f64>() / count as f64
            } else {
                0.0
            };
            let avg_decay = if input.include_decay && count > 0 {
                Some(group.iter().map(|f| f.decay_score).sum::<f64>() / count as f64)
            } else {
                None
            };
            (
                cat,
                CategoryStats {
                    count,
                    avg_confidence,
                    avg_decay,
                },
            )
        })
        .collect();

    // Group by type
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    for fact in &facts {
        *by_type.entry(fact.fact_type.clone()).or_default() += 1;
    }

    Ok(Json(StatsOutput {
        total_facts,
        by_category,
        by_type,
    }))
}

// ---------------------------------------------------------------------------
// memory_timeline
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TimelineInput {
    #[serde(default)]
    pub facts: Vec<FactSummary>,
    #[serde(default = "default_days")]
    pub days: u32,
    #[serde(default = "default_granularity")]
    pub granularity: String,
}

fn default_days() -> u32 {
    30
}
fn default_granularity() -> String {
    "day".to_string()
}

#[derive(Debug, Serialize)]
pub struct TimelineBucket {
    pub period: String,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct TimelineOutput {
    pub granularity: String,
    pub buckets: Vec<TimelineBucket>,
}

/// Extract the date portion (YYYY-MM-DD) from an ISO 8601 string.
fn extract_date(iso: &str) -> &str {
    iso.get(..10).unwrap_or(iso)
}

/// Convert a YYYY-MM-DD date to an ISO week string (YYYY-Www).
/// Uses a simple Monday-based week calculation.
fn date_to_week(date: &str) -> String {
    // Parse year, month, day
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() < 3 {
        return date.to_string();
    }
    let year: i32 = parts[0].parse().unwrap_or(0);
    let month: u32 = parts[1].parse().unwrap_or(1);
    let day: u32 = parts[2].parse().unwrap_or(1);

    // Day of year (approximate, handles common months)
    let days_in_months = [0u32, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let mut doy: u32 = day;
    for m in 1..month {
        doy += days_in_months.get(m as usize).copied().unwrap_or(30);
        if m == 2 && is_leap {
            doy += 1;
        }
    }

    let week = ((doy - 1) / 7) + 1;
    format!("{year}-W{week:02}")
}

#[plugin_fn]
pub fn memory_timeline(Json(input): Json<TimelineInput>) -> FnResult<Json<TimelineOutput>> {
    let use_weeks = input.granularity == "week";

    let mut buckets: BTreeMap<String, usize> = BTreeMap::new();
    for fact in &input.facts {
        let date = extract_date(&fact.created_at);
        let key = if use_weeks {
            date_to_week(date)
        } else {
            date.to_string()
        };
        *buckets.entry(key).or_default() += 1;
    }

    let buckets = buckets
        .into_iter()
        .map(|(period, count)| TimelineBucket { period, count })
        .collect();

    Ok(Json(TimelineOutput {
        granularity: input.granularity,
        buckets,
    }))
}

// ---------------------------------------------------------------------------
// memory_health
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct HealthInput {
    #[serde(default)]
    pub facts: Vec<FactSummary>,
}

#[derive(Debug, Serialize)]
pub struct HealthOutput {
    pub total_facts: usize,
    pub stale_count: usize,
    pub stale_ratio: f64,
    pub avg_confidence: f64,
    pub avg_decay_score: f64,
    pub health_grade: String,
    pub recommendations: Vec<String>,
}

/// A fact is considered stale if its decay score drops below this threshold.
const STALE_THRESHOLD: f64 = 0.3;

#[plugin_fn]
pub fn memory_health(Json(input): Json<HealthInput>) -> FnResult<Json<HealthOutput>> {
    let total = input.facts.len();

    if total == 0 {
        return Ok(Json(HealthOutput {
            total_facts: 0,
            stale_count: 0,
            stale_ratio: 0.0,
            avg_confidence: 0.0,
            avg_decay_score: 0.0,
            health_grade: "N/A".to_string(),
            recommendations: vec!["No facts found — start building memory.".to_string()],
        }));
    }

    let stale_count = input
        .facts
        .iter()
        .filter(|f| f.decay_score < STALE_THRESHOLD)
        .count();
    let stale_ratio = stale_count as f64 / total as f64;

    let avg_confidence =
        input.facts.iter().map(|f| f.confidence).sum::<f64>() / total as f64;
    let avg_decay =
        input.facts.iter().map(|f| f.decay_score).sum::<f64>() / total as f64;

    // Grade: A (healthy) → D (needs attention)
    let health_grade = if stale_ratio < 0.1 && avg_confidence > 0.8 {
        "A"
    } else if stale_ratio < 0.25 && avg_confidence > 0.6 {
        "B"
    } else if stale_ratio < 0.5 {
        "C"
    } else {
        "D"
    }
    .to_string();

    let mut recommendations = Vec::new();
    if stale_ratio > 0.3 {
        recommendations.push(format!(
            "{:.0}% of facts are stale (decay < {STALE_THRESHOLD}) — consider pruning.",
            stale_ratio * 100.0
        ));
    }
    if avg_confidence < 0.5 {
        recommendations.push(
            "Average confidence is low — review fact sources and validation.".to_string(),
        );
    }
    if total > 10000 {
        recommendations.push(
            "Large fact store — consider compression or archiving old facts.".to_string(),
        );
    }
    if recommendations.is_empty() {
        recommendations.push("Memory health looks good.".to_string());
    }

    Ok(Json(HealthOutput {
        total_facts: total,
        stale_count,
        stale_ratio,
        avg_confidence,
        avg_decay_score: avg_decay,
        health_grade,
        recommendations,
    }))
}

// ---------------------------------------------------------------------------
// memory_report
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ReportInput {
    #[serde(default)]
    pub facts: Vec<FactSummary>,
    #[serde(default = "default_format")]
    pub format: String,
}

fn default_format() -> String {
    "markdown".to_string()
}

#[derive(Debug, Serialize)]
pub struct ReportOutput {
    pub format: String,
    pub content: String,
}

#[plugin_fn]
pub fn memory_report(Json(input): Json<ReportInput>) -> FnResult<Json<ReportOutput>> {
    let total = input.facts.len();

    // Compute category distribution
    let mut by_category: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    for fact in &input.facts {
        *by_category.entry(fact.category.clone()).or_default() += 1;
        *by_type.entry(fact.fact_type.clone()).or_default() += 1;
    }

    // Health metrics
    let stale_count = input
        .facts
        .iter()
        .filter(|f| f.decay_score < STALE_THRESHOLD)
        .count();
    let stale_ratio = if total > 0 {
        stale_count as f64 / total as f64
    } else {
        0.0
    };
    let avg_confidence = if total > 0 {
        input.facts.iter().map(|f| f.confidence).sum::<f64>() / total as f64
    } else {
        0.0
    };
    let avg_decay = if total > 0 {
        input.facts.iter().map(|f| f.decay_score).sum::<f64>() / total as f64
    } else {
        0.0
    };

    // Timeline (last 7 days, daily)
    let mut daily: BTreeMap<String, usize> = BTreeMap::new();
    for fact in &input.facts {
        let date = extract_date(&fact.created_at).to_string();
        *daily.entry(date).or_default() += 1;
    }

    let content = if input.format == "json" {
        let report = serde_json::json!({
            "total_facts": total,
            "by_category": by_category,
            "by_type": by_type,
            "stale_count": stale_count,
            "stale_ratio": stale_ratio,
            "avg_confidence": avg_confidence,
            "avg_decay_score": avg_decay,
            "daily_timeline": daily,
        });
        serde_json::to_string_pretty(&report).unwrap_or_default()
    } else {
        let mut md = String::new();
        md.push_str("# Memory Analytics Report\n\n");
        md.push_str(&format!("**Total facts:** {total}\n\n"));

        // Category table
        md.push_str("## By Category\n\n");
        md.push_str("| Category | Count |\n|----------|-------|\n");
        for (cat, count) in &by_category {
            md.push_str(&format!("| {cat} | {count} |\n"));
        }

        // Type table
        md.push_str("\n## By Type\n\n");
        md.push_str("| Type | Count |\n|------|-------|\n");
        for (t, count) in &by_type {
            md.push_str(&format!("| {t} | {count} |\n"));
        }

        // Health
        md.push_str("\n## Health\n\n");
        md.push_str(&format!("- **Stale facts:** {stale_count} ({:.1}%)\n", stale_ratio * 100.0));
        md.push_str(&format!("- **Avg confidence:** {avg_confidence:.2}\n"));
        md.push_str(&format!("- **Avg decay score:** {avg_decay:.2}\n"));

        // Timeline
        if !daily.is_empty() {
            md.push_str("\n## Timeline (daily)\n\n");
            md.push_str("| Date | Facts |\n|------|-------|\n");
            for (date, count) in &daily {
                md.push_str(&format!("| {date} | {count} |\n"));
            }
        }

        md
    };

    Ok(Json(ReportOutput {
        format: input.format,
        content,
    }))
}
