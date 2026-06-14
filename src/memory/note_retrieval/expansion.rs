//! Associative graph expansion of the retrieval candidate pool.
//!
//! Query-independent: 4-signal relatedness measures note<->note, so a peer
//! surfaces purely because it is tied to a *query-relevant* seed. Conservative
//! by construction — a peer's score is scaled strictly below its seed. Never
//! fails retrieval: store errors are swallowed (logged) and treated as "no
//! expansion", matching `retrieve()`'s embedding-fallback philosophy. A cold
//! cache (`related_peers` empty) yields zero expansion -> legacy behavior.

use std::collections::{HashMap, HashSet};

use crate::config::types::memory::ExpansionConfig;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::NoteSearchResult;

/// Expand `hits` with the strongest 4-signal related peers of the top seeds.
/// Returns hydrated `NoteSearchResult`s (content carried) stamped with a
/// propagated score, sorted by score desc then path asc. Empty when expansion
/// is inactive, hits are empty, the cache is cold, or every peer fails to
/// hydrate.
pub async fn graph_expand<S: NoteStore + Send + Sync>(
    store: &S,
    agent_id: &str,
    hits: &[NoteSearchResult],
    cfg: &ExpansionConfig,
) -> Vec<NoteSearchResult> {
    if !cfg.is_active() || hits.is_empty() {
        return Vec::new();
    }

    // Dedup target: never re-surface a path already among the direct hits.
    let mut seen: HashSet<String> = hits.iter().map(|h| h.path.clone()).collect();
    // (peer_path, propagated_score) in discovery order. Seeds iterate in hit
    // (RRF-desc) order, so a peer tied to multiple seeds is captured via its
    // strongest seed first; the `seen` insert blocks weaker re-captures.
    let mut collected: Vec<(String, f32)> = Vec::new();

    'outer: for seed in hits.iter().take(cfg.max_seeds) {
        let peers = match store
            .related_peers(agent_id, &seed.path, cfg.peers_per_seed)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(error = %e, seed = %seed.path,
                    "graph expansion: related_peers failed (non-fatal)");
                continue;
            }
        };
        // Normalize by the seed's strongest edge so unbounded 4-signal
        // magnitudes can't dominate; the top peer of a seed maxes at
        // `weight * seed.score`.
        let seed_top_edge = peers.iter().map(|(_, s)| *s).fold(0.0_f32, f32::max);
        if seed_top_edge <= 0.0 {
            continue;
        }
        for (peer, edge) in peers {
            if collected.len() >= cfg.max_expanded {
                break 'outer;
            }
            if seen.insert(peer.clone()) {
                let propagated = seed.score * cfg.weight * (edge / seed_top_edge);
                collected.push((peer, propagated));
            }
        }
    }

    if collected.is_empty() {
        return Vec::new();
    }

    // Hydrate content for the collected peers (they need full content for the
    // agent and the optional reranker). Missing/deleted paths are dropped.
    let paths: Vec<String> = collected.iter().map(|(p, _)| p.clone()).collect();
    let hydrated = match store.get_notes_with_content(agent_id, &paths).await {
        Ok(h) => h,
        Err(e) => {
            tracing::debug!(error = %e,
                "graph expansion: content hydration failed (non-fatal)");
            return Vec::new();
        }
    };

    let score_by_path: HashMap<&str, f32> =
        collected.iter().map(|(p, s)| (p.as_str(), *s)).collect();
    let mut out: Vec<NoteSearchResult> = hydrated
        .into_iter()
        .filter_map(|mut r| {
            let s = *score_by_path.get(r.path.as_str())?;
            r.score = s;
            Some(r)
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::SqliteMemoryBackend;

    fn hit(path: &str, score: f32) -> NoteSearchResult {
        NoteSearchResult {
            path: path.to_string(),
            filename: path.rsplit('/').next().unwrap_or(path).to_string(),
            category: path.split('/').next().unwrap_or("general").to_string(),
            tags: vec![],
            content: String::new(),
            score,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn backend() -> SqliteMemoryBackend {
        SqliteMemoryBackend::in_memory().unwrap()
    }

    /// Index a note so `get_note_index`/`get_notes_with_content` resolve `path`.
    /// Returns the path the index assigned (category/filename).
    async fn index(b: &SqliteMemoryBackend, title: &str) -> String {
        let note = KnowledgeNote {
            title: title.to_string(),
            category: "general".to_string(),
            facts: vec![format!("{title} fact")],
            content_hash: format!("h_{title}"),
            ..Default::default()
        };
        b.index_note(&note, "default", "general").await.unwrap();
        format!("general/{title}")
    }

    #[tokio::test]
    async fn empty_hits_yields_no_expansion() {
        let b = backend();
        let out = graph_expand(&b, "default", &[], &ExpansionConfig::default()).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn disabled_config_yields_no_expansion() {
        let b = backend();
        let cfg = ExpansionConfig { enabled: false, ..Default::default() };
        let out = graph_expand(&b, "default", &[hit("general/a", 0.9)], &cfg).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn cold_cache_yields_no_expansion() {
        // related_peers returns empty when nothing materialized -> legacy.
        let b = backend();
        let _ = index(&b, "a").await;
        let out = graph_expand(&b, "default", &[hit("general/a", 0.9)],
            &ExpansionConfig::default()).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn propagation_scales_peer_below_seed() {
        let b = backend();
        let a = index(&b, "a").await;
        let bp = index(&b, "b").await;
        // Single peer: seed_top_edge == edge -> factor 1.0 -> 0.8 * 0.5 = 0.4.
        b.replace_graph_related("default", &[(a.clone(), bp.clone(), 4.0)])
            .await
            .unwrap();
        let out = graph_expand(&b, "default", &[hit(&a, 0.8)],
            &ExpansionConfig::default()).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, bp);
        assert!((out[0].score - 0.4).abs() < 1e-6, "got {}", out[0].score);
        assert!(out[0].score < 0.8, "peer must not outrank its seed");
    }

    #[tokio::test]
    async fn two_peers_normalized_by_top_edge() {
        let b = backend();
        let a = index(&b, "a").await;
        let p1 = index(&b, "p1").await;
        let p2 = index(&b, "p2").await;
        b.replace_graph_related("default", &[
            (a.clone(), p1.clone(), 4.0),
            (a.clone(), p2.clone(), 2.0),
        ]).await.unwrap();
        let out = graph_expand(&b, "default", &[hit(&a, 0.8)],
            &ExpansionConfig::default()).await;
        let by: std::collections::HashMap<&str, f32> =
            out.iter().map(|r| (r.path.as_str(), r.score)).collect();
        // p1: 0.8*0.5*(4/4)=0.4 ; p2: 0.8*0.5*(2/4)=0.2
        assert!((by[p1.as_str()] - 0.4).abs() < 1e-6);
        assert!((by[p2.as_str()] - 0.2).abs() < 1e-6);
        // Sorted desc -> p1 first.
        assert_eq!(out[0].path, p1);
    }

    #[tokio::test]
    async fn peer_already_in_hits_is_not_re_added() {
        let b = backend();
        let a = index(&b, "a").await;
        let bp = index(&b, "b").await;
        b.replace_graph_related("default", &[(a.clone(), bp.clone(), 4.0)])
            .await
            .unwrap();
        // b is already a direct hit -> expansion must not duplicate it.
        let hits = vec![hit(&a, 0.9), hit(&bp, 0.7)];
        let out = graph_expand(&b, "default", &hits, &ExpansionConfig::default()).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn global_max_expanded_cap_respected() {
        let b = backend();
        let a = index(&b, "a").await;
        let mut rows = Vec::new();
        for i in 0..6 {
            let p = index(&b, &format!("p{i}")).await;
            rows.push((a.clone(), p, (10 - i) as f32));
        }
        b.replace_graph_related("default", &rows).await.unwrap();
        let cfg = ExpansionConfig { peers_per_seed: 10, max_expanded: 3, ..Default::default() };
        let out = graph_expand(&b, "default", &[hit(&a, 0.9)], &cfg).await;
        assert_eq!(out.len(), 3, "global cap must bound total expansion");
    }

    #[tokio::test]
    async fn hydration_miss_is_dropped() {
        let b = backend();
        let a = index(&b, "a").await;
        // Peer path is materialized but never indexed -> get_notes_with_content
        // skips it -> dropped from output.
        b.replace_graph_related("default", &[(a.clone(), "general/ghost".to_string(), 4.0)])
            .await
            .unwrap();
        let out = graph_expand(&b, "default", &[hit(&a, 0.9)],
            &ExpansionConfig::default()).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn peer_shared_by_two_seeds_scored_via_stronger_seed() {
        let b = backend();
        let s1 = index(&b, "s1").await;
        let s2 = index(&b, "s2").await;
        let shared = index(&b, "shared").await;
        // `shared` is a peer of BOTH seeds; expansion must score it via s1
        // (the stronger, earlier hit) and not re-add it for s2.
        b.replace_graph_related("default", &[
            (s1.clone(), shared.clone(), 4.0),
            (s2.clone(), shared.clone(), 4.0),
        ]).await.unwrap();
        // s1 stronger (0.9) than s2 (0.5); single edge each -> factor 1.0.
        let hits = vec![hit(&s1, 0.9), hit(&s2, 0.5)];
        let out = graph_expand(&b, "default", &hits, &ExpansionConfig::default()).await;
        assert_eq!(out.len(), 1, "shared peer captured once, not per-seed");
        // 0.9 * 0.5 * 1.0 = 0.45 (via s1), NOT 0.25 (via s2)
        assert!((out[0].score - 0.45).abs() < 1e-6, "got {}", out[0].score);
    }
}
