//! WikiLintStage: health checks for the wiki knowledge base.

use async_trait::async_trait;
use serde::Serialize;
use tracing::info;

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::context::FactType;
use crate::memory::store::MemoryStore;
use crate::wiki::wikilink::extract_wikilinks;

/// Report from wiki lint stage.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WikiLintReport {
    pub broken_links: Vec<(String, String)>,
    pub orphan_pages: Vec<String>,
    pub stale_pages: Vec<String>,
    pub suggested_pages: Vec<String>,
    pub auto_fixed: usize,
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
            .filter(|f| f.fact_type == FactType::Wiki && f.is_valid)
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
                    .last()
                    .map(|s| s.trim_end_matches(".md").to_string())
            })
            .collect();

        let mut inbound_links: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for fact in &wiki_facts {
            let slug = fact
                .path
                .split('/')
                .last()
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

        info!(
            broken_links = report.broken_links.len(),
            orphan_pages = report.orphan_pages.len(),
            stale_pages = report.stale_pages.len(),
            auto_fixed = report.auto_fixed,
            "WikiLintStage complete"
        );

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
