//! SummarizeStage: generates daily insight summaries from clusters.

use async_trait::async_trait;
use tracing::{info, warn};

use super::types::MemoryCluster;
use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::store::DreamStore;

/// Maximum number of non-noise clusters to include in the summary.
const MAX_CLUSTERS_IN_SUMMARY: usize = 10;

/// Maximum snippet length (in characters) before truncation.
const MAX_SNIPPET_LEN: usize = 80;

/// Maximum number of example snippets per cluster.
const MAX_SNIPPETS_PER_CLUSTER: usize = 3;

/// Produces a daily insight summary from the clustered memories.
pub struct SummarizeStage;

#[async_trait]
impl DreamStage for SummarizeStage {
    fn name(&self) -> &'static str {
        "summarize"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        if ctx.clusters.is_empty() {
            return Ok(ctx);
        }

        let run_date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let summary = build_cluster_summary(&ctx.clusters, &run_date);

        let insight = crate::memory::dreaming::DailyInsight::new(
            run_date,
            summary,
            ctx.memories.len() as u32,
        );
        if let Err(e) = ctx.database.upsert_daily_insight(insight).await {
            warn!(error = %e, "SummarizeStage: failed to persist daily insight");
        }

        info!(
            clusters = ctx.clusters.len(),
            "SummarizeStage: built summary"
        );
        Ok(ctx)
    }
}

/// Build a human-readable summary from memory clusters.
///
/// Includes up to `MAX_CLUSTERS_IN_SUMMARY` non-noise clusters, each with
/// member count and example snippets from the first few members.
fn build_cluster_summary(clusters: &[MemoryCluster], date: &str) -> String {
    let mut out = format!("Daily Insight ({date})\n");

    let meaningful: Vec<&MemoryCluster> = clusters.iter().filter(|c| !c.is_noise).collect();

    if meaningful.is_empty() {
        out.push_str("No meaningful clusters found today.\n");
        return out;
    }

    for (i, cluster) in meaningful.iter().take(MAX_CLUSTERS_IN_SUMMARY).enumerate() {
        out.push_str(&format!(
            "\n{}. {} ({} memories)\n",
            i + 1,
            cluster.label,
            cluster.members.len()
        ));

        for member in cluster.members.iter().take(MAX_SNIPPETS_PER_CLUSTER) {
            let snippet = truncate_text(&member.user_input, MAX_SNIPPET_LEN);
            out.push_str(&format!("   - {snippet}\n"));
        }
    }

    let noise_count = clusters.iter().filter(|c| c.is_noise).count();
    if noise_count > 0 {
        out.push_str(&format!("\n({noise_count} noise cluster(s) omitted)\n"));
    }

    out
}

/// UTF-8 safe truncation with "..." suffix.
///
/// If the text is shorter than or equal to `max_len`, returns it unchanged.
/// Otherwise truncates at the last valid char boundary within `max_len - 3`
/// and appends "...".
fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }

    // Need at least 4 chars to show one char + "..."
    if max_len < 4 {
        return "...".to_string();
    }

    let target = max_len - 3;
    // Find the last valid char boundary at or before `target`
    let boundary = text
        .char_indices()
        .take_while(|(i, _)| *i <= target)
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0);

    format!("{}...", &text[..boundary])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{ContextAnchor, MemoryEntry};
    use crate::memory::dreaming::stages::types::MetadataGroupKey;

    fn make_cluster(label: &str, inputs: &[&str], is_noise: bool) -> MemoryCluster {
        let members: Vec<MemoryEntry> = inputs
            .iter()
            .enumerate()
            .map(|(i, input)| {
                MemoryEntry::new(
                    format!("mem-{i}"),
                    ContextAnchor {
                        window_title: String::new(),
                        timestamp: 0,
                        session_id: "none".to_string(),
                    },
                    input.to_string(),
                    String::new(),
                )
            })
            .collect();

        MemoryCluster {
            id: format!("cluster-{label}"),
            label: label.to_string(),
            members,
            centroid: None,
            metadata_key: MetadataGroupKey::None,
            is_noise,
        }
    }

    #[test]
    fn build_cluster_summary_empty_clusters() {
        let clusters: Vec<MemoryCluster> = vec![];
        let summary = build_cluster_summary(&clusters, "2026-04-03");
        assert!(summary.starts_with("Daily Insight (2026-04-03)"));
        // With empty input the function returns early, so no cluster lines
        assert!(!summary.contains("1."));
    }

    #[test]
    fn build_cluster_summary_only_noise() {
        let clusters = vec![make_cluster("noise-group", &["hello"], true)];
        let summary = build_cluster_summary(&clusters, "2026-04-03");
        assert!(summary.contains("No meaningful clusters found"));
    }

    #[test]
    fn build_cluster_summary_with_clusters() {
        let clusters = vec![
            make_cluster("Work Tasks", &["fix the build", "deploy to prod"], false),
            make_cluster("noise", &["random"], true),
            make_cluster("Research", &["read paper on LLMs"], false),
        ];
        let summary = build_cluster_summary(&clusters, "2026-04-03");
        assert!(summary.contains("1. Work Tasks (2 memories)"));
        assert!(summary.contains("2. Research (1 memories)"));
        assert!(summary.contains("fix the build"));
        assert!(summary.contains("(1 noise cluster(s) omitted)"));
        // Noise cluster label should not appear as a numbered item
        assert!(!summary.contains("noise (1 memories)"));
    }

    #[test]
    fn truncate_text_short_string() {
        assert_eq!(truncate_text("hello", 10), "hello");
    }

    #[test]
    fn truncate_text_exact_length() {
        assert_eq!(truncate_text("hello", 5), "hello");
    }

    #[test]
    fn truncate_text_long_string() {
        let result = truncate_text("hello world, this is a test", 10);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 10);
    }

    #[test]
    fn truncate_text_utf8_safe() {
        // Chinese characters are multi-byte; ensure we don't split mid-char
        let input = "你好世界测试数据";
        let result = truncate_text(input, 10);
        assert!(result.ends_with("..."));
        // Verify the string is valid UTF-8 (would panic on invalid)
        let _ = result.as_bytes();
    }

    #[test]
    fn truncate_text_very_small_max() {
        assert_eq!(truncate_text("hello world", 3), "...");
        assert_eq!(truncate_text("hello world", 2), "...");
    }
}
