//! Phase 1 — gather related pages for a batch of raw memories.

use crate::error::AlephError;
use crate::memory::embedding_provider::EmbeddingProvider;
use crate::memory::notes::store::NoteStore;
use crate::memory::store::raw_memory::RawMemory;
use crate::sync_primitives::Arc;
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct RelatedBudget {
    pub max_related_pages: usize,
    pub preview_char_cap: usize,
    pub total_byte_cap: usize,
}

impl Default for RelatedBudget {
    fn default() -> Self {
        Self {
            max_related_pages: 15,
            preview_char_cap: 800,
            total_byte_cap: 12 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RelatedPage {
    pub path: String,
    pub title: String,
    pub summary: String,
    pub content_preview: String,
    pub tags: Vec<String>,
    pub content_hash: String,
    pub score: f32,
}

/// Embed the aggregated raw batch text, hybrid-search for N seed pages,
/// expand 1-hop via outgoing links, truncate to budget.
pub async fn gather_related<S: NoteStore + Send + Sync + 'static>(
    store: Arc<S>,
    embedder: Arc<dyn EmbeddingProvider>,
    raws: &[RawMemory],
    agent_id: &str,
    budget: &RelatedBudget,
) -> Result<Vec<RelatedPage>, AlephError> {
    if raws.is_empty() {
        return Ok(vec![]);
    }

    let mut aggregated = String::new();
    for r in raws {
        aggregated.push_str(&r.content);
        aggregated.push('\n');
        if let Some(att) = &r.attachment_text {
            aggregated.push_str(att);
            aggregated.push('\n');
        }
    }

    let embedding = embedder.embed(&aggregated).await?;
    let dim_hint = embedding.len() as u32;

    let seed_limit = budget.max_related_pages.saturating_mul(2).max(6);
    let seeds = store
        .hybrid_search_notes(&embedding, &aggregated, agent_id, dim_hint, seed_limit)
        .await?;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut ranked: Vec<(String, f32)> = Vec::new();
    for s in &seeds {
        if seen.insert(s.path.clone()) {
            ranked.push((s.path.clone(), s.score));
        }
    }

    // 1-hop expand: outgoing links of the top-ranked seeds (up to 6).
    let expand_roots: Vec<String> = ranked.iter().take(6).map(|(p, _)| p.clone()).collect();
    for root in expand_roots {
        let outgoing = store.get_outgoing_links(&root, agent_id).await?;
        for link in outgoing {
            if seen.contains(&link) {
                continue;
            }
            if store.get_note_index(&link, agent_id).await?.is_some() {
                if seen.insert(link.clone()) {
                    ranked.push((link, 0.0));
                }
            }
        }
    }

    let mut out: Vec<RelatedPage> = Vec::new();
    let mut running_bytes: usize = 0;
    for (path, score) in ranked.into_iter().take(budget.max_related_pages) {
        let Some(entry) = store.get_note_index(&path, agent_id).await? else {
            continue;
        };
        let full = seeds
            .iter()
            .find(|s| s.path == path)
            .map(|s| s.content.clone())
            .unwrap_or_default();
        let preview: String = if full.is_empty() {
            String::new()
        } else {
            full.chars().take(budget.preview_char_cap).collect()
        };
        let summary_first_bullet = first_body_bullet(&full).unwrap_or_default();

        let rp_bytes = preview.len() + entry.path.len() + summary_first_bullet.len() + 64;
        if running_bytes + rp_bytes > budget.total_byte_cap {
            break;
        }
        running_bytes += rp_bytes;

        out.push(RelatedPage {
            path: entry.path.clone(),
            title: entry.filename.clone(),
            summary: summary_first_bullet,
            content_preview: preview,
            tags: entry.tags.clone(),
            content_hash: entry.content_hash.clone(),
            score,
        });
    }

    Ok(out)
}

fn first_body_bullet(raw: &str) -> Option<String> {
    let body = if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            &rest[end + 4..]
        } else {
            rest
        }
    } else {
        raw
    };
    for line in body.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("- ") {
            return Some(rest.chars().take(200).collect());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_budget_default() {
        let b = RelatedBudget::default();
        assert_eq!(b.max_related_pages, 15);
        assert_eq!(b.preview_char_cap, 800);
    }

    #[test]
    fn first_body_bullet_picks_first_dash() {
        let md = "---\nk: v\n---\n# Heading\n\n- alpha\n- beta\n";
        assert_eq!(first_body_bullet(md).unwrap(), "alpha");
    }

    #[test]
    fn first_body_bullet_none_when_no_bullet() {
        assert!(first_body_bullet("plain prose").is_none());
    }

    // Behaviour-level tests that require NoteStore are covered in the
    // Task 15 integration test.
}
