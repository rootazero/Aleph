//! WikiLintStage: health checks for the wiki knowledge base.

use async_trait::async_trait;
use serde::Serialize;
use tracing::info;

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::context::{FactSource, NoteType};
use crate::memory::store::MemoryStore;
use crate::wiki::wikilink::extract_wikilinks;

/// Report from wiki lint stage.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WikiLintReport {
    pub broken_links: Vec<(String, String)>,
    pub orphan_pages: Vec<String>,
    pub stale_pages: Vec<String>,
    pub suggested_pages: Vec<String>,
    pub suggested_links: Vec<SuggestedLink>,
    pub auto_fixed: usize,
}

/// A suggested wikilink based on graph topology.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SuggestedLink {
    pub from_page: String,
    pub to_page: String,
    pub reason: String,
    pub confidence: f32,
}

/// Health checks for wiki pages.
pub struct WikiLintStage;

#[async_trait]
impl DreamStage for WikiLintStage {
    fn name(&self) -> &'static str {
        "wiki_lint"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let all_facts = ctx.database.get_all_facts(false, None).await?;
        let wiki_facts: Vec<_> = all_facts
            .iter()
            .filter(|f| f.note_type == NoteType::Wiki && f.is_valid)
            .collect();

        if wiki_facts.is_empty() {
            info!("WikiLintStage: no wiki pages to lint");
            return Ok(ctx);
        }

        let mut report = WikiLintReport::default();

        let known_slugs: std::collections::HashSet<String> = wiki_facts
            .iter()
            .filter_map(|f| {
                f.path
                    .split('/')
                    .next_back()
                    .map(|s| s.trim_end_matches(".md").to_string())
            })
            .collect();

        let mut inbound_links: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for fact in &wiki_facts {
            let slug = fact
                .path
                .split('/')
                .next_back()
                .unwrap_or("")
                .trim_end_matches(".md")
                .to_string();

            let wikilinks = extract_wikilinks(&fact.content);
            for target in &wikilinks {
                inbound_links.insert(target.clone());
                if !known_slugs.contains(target) {
                    report.broken_links.push((slug.clone(), target.clone()));
                }
            }
        }

        for slug in &known_slugs {
            if !inbound_links.contains(slug) {
                report.orphan_pages.push(slug.clone());
            }
        }

        // Source-fact staleness detection for synthesis wiki pages
        let stale_threshold: f32 = 0.5;
        for fact in &wiki_facts {
            if fact.fact_source != FactSource::Synthesis {
                continue;
            }
            let total = fact.source_memory_ids.len();
            if total == 0 {
                continue;
            }
            let valid = fact
                .source_memory_ids
                .iter()
                .filter(|sid| all_facts.iter().any(|f| f.id == **sid && f.is_valid))
                .count();
            let ratio = valid as f32 / total as f32;
            if ratio < stale_threshold {
                let slug = fact
                    .path
                    .split('/')
                    .next_back()
                    .unwrap_or("")
                    .trim_end_matches(".md")
                    .to_string();
                if !report.stale_pages.contains(&slug) {
                    report.stale_pages.push(slug);
                }
            }
        }

        // NOTE: Graph-based link suggestions have been removed.
        // Knowledge Notes use native wikilink resolution via notes_links table.

        info!(
            broken_links = report.broken_links.len(),
            orphan_pages = report.orphan_pages.len(),
            stale_pages = report.stale_pages.len(),
            suggested_links = report.suggested_links.len(),
            auto_fixed = report.auto_fixed,
            "WikiLintStage complete"
        );

        let mut ctx = ctx;
        if !report.broken_links.is_empty() || !report.orphan_pages.is_empty() {
            ctx.wiki_lint_report = serde_json::to_string(&report).ok();
        }
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_lint_stage_name() {
        assert_eq!(WikiLintStage.name(), "wiki_lint");
    }

    #[test]
    fn wiki_lint_report_default() {
        let report = WikiLintReport::default();
        assert!(report.broken_links.is_empty());
        assert!(report.orphan_pages.is_empty());
        assert_eq!(report.auto_fixed, 0);
    }
}
