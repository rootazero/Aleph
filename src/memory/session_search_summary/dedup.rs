//! Spec B — per-session result diversification.
//!
//! HybridAssembler returns top-K candidate facts mixed across sessions
//! (e.g. 3 d0 chunks from session A, 2 from session B, 1 from session C).
//! `top_per_session` collapses these to one best-scoring entry per
//! `session_key`, capped at `max_sessions`.

use std::collections::HashMap;

/// Minimal candidate shape used by Spec B's tool layer.
#[derive(Debug, Clone)]
pub struct ScoredCandidate {
    pub session_key: String,
    pub agent_id: String,
    pub fact_path: String,
    pub summary_text: String,
    pub topic: Option<String>,
    pub timestamp: i64,
    pub score: f32,
}

/// Group by `session_key`, keep top score per group, take the top
/// `max_sessions` groups by group-best-score.
pub fn top_per_session(
    candidates: Vec<ScoredCandidate>,
    max_sessions: usize,
) -> Vec<ScoredCandidate> {
    let mut best_per_session: HashMap<String, ScoredCandidate> = HashMap::new();

    for c in candidates {
        let key = c.session_key.clone();
        match best_per_session.get(&key) {
            Some(prev) if prev.score >= c.score => {}
            _ => {
                best_per_session.insert(key, c);
            }
        }
    }

    let mut survivors: Vec<ScoredCandidate> = best_per_session.into_values().collect();
    // Stable order: by score descending, then session_key ascending for
    // deterministic ties.
    survivors.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.session_key.cmp(&b.session_key))
    });
    survivors.truncate(max_sessions);
    survivors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(sk: &str, score: f32, fact_path: &str) -> ScoredCandidate {
        ScoredCandidate {
            session_key: sk.to_string(),
            agent_id: "agent-1".to_string(),
            fact_path: fact_path.to_string(),
            summary_text: format!("summary-{fact_path}"),
            topic: None,
            timestamp: 0,
            score,
        }
    }

    #[test]
    fn empty_input_returns_empty() {
        let out = top_per_session(vec![], 5);
        assert!(out.is_empty());
    }

    #[test]
    fn single_session_multiple_chunks_collapses_to_one() {
        let input = vec![
            cand("s1", 0.5, "aleph://session/s1/d0/0"),
            cand("s1", 0.9, "aleph://session/s1/d0/1"),
            cand("s1", 0.3, "aleph://session/s1/d1/0"),
        ];
        let out = top_per_session(input, 5);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].score, 0.9);
        assert_eq!(out[0].fact_path, "aleph://session/s1/d0/1");
    }

    #[test]
    fn multiple_sessions_each_keeps_best() {
        let input = vec![
            cand("s1", 0.5, "aleph://session/s1/d0/0"),
            cand("s1", 0.9, "aleph://session/s1/d0/1"),
            cand("s2", 0.7, "aleph://session/s2/d0/0"),
            cand("s2", 0.4, "aleph://session/s2/d1/0"),
            cand("s3", 0.2, "aleph://session/s3/d0/0"),
        ];
        let out = top_per_session(input, 10);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].session_key, "s1");
        assert_eq!(out[0].score, 0.9);
        assert_eq!(out[1].session_key, "s2");
        assert_eq!(out[1].score, 0.7);
        assert_eq!(out[2].session_key, "s3");
        assert_eq!(out[2].score, 0.2);
    }

    #[test]
    fn max_sessions_caps_output_count() {
        let input = vec![
            cand("s1", 0.9, "p1"),
            cand("s2", 0.8, "p2"),
            cand("s3", 0.7, "p3"),
            cand("s4", 0.6, "p4"),
        ];
        let out = top_per_session(input, 2);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].session_key, "s1");
        assert_eq!(out[1].session_key, "s2");
    }

    #[test]
    fn ties_broken_by_session_key_ascending() {
        let input = vec![cand("session-z", 0.5, "p1"), cand("session-a", 0.5, "p2")];
        let out = top_per_session(input, 5);
        assert_eq!(out[0].session_key, "session-a");
        assert_eq!(out[1].session_key, "session-z");
    }
}

#[cfg(test)]
mod proptest_invariants {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn dedup_invariants(
            candidates in prop::collection::vec(
                (
                    "session-[a-z0-9]{1,5}",
                    0.0f32..1.0f32,
                    "aleph://session/[a-z0-9]+/d[0-2]/[0-9]+",
                ),
                0..50,
            ),
            max_sessions in 0usize..20,
        ) {
            let scored: Vec<ScoredCandidate> = candidates
                .into_iter()
                .map(|(sk, score, path)| ScoredCandidate {
                    session_key: sk,
                    agent_id: "a".into(),
                    fact_path: path,
                    summary_text: "s".into(),
                    topic: None,
                    timestamp: 0,
                    score,
                })
                .collect();
            let out = top_per_session(scored, max_sessions);

            // Invariant 1: each session_key appears at most once.
            let mut seen = std::collections::HashSet::new();
            for c in &out {
                prop_assert!(seen.insert(c.session_key.clone()),
                    "duplicate session_key {:?}", c.session_key);
            }
            // Invariant 2: result length ≤ max_sessions.
            prop_assert!(out.len() <= max_sessions);
            // Invariant 3: scores non-increasing.
            for window in out.windows(2) {
                prop_assert!(window[0].score >= window[1].score);
            }
        }
    }
}
