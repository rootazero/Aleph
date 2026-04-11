//! DriftDetectStage: detects contradictions and knowledge drift.
//!
//! For each new fact produced by earlier pipeline stages, this stage
//! performs a vector search against existing LTM to find semantically
//! similar facts.  High-similarity pairs are collected as `DriftCandidate`s.
//!
//! A prompt builder (`build_arbitration_prompt`) prepares an LLM request
//! for future arbitration.  For now, all candidates default to
//! `DriftAction::Coexist` as a safe fallback — actual LLM arbitration
//! will be wired in a later task.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::context::MemoryFact;
use crate::memory::store::types::SearchFilter;
use crate::memory::store::MemoryStore;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;

/// Number of similar facts to retrieve per vector search.
const DRIFT_SEARCH_K: usize = 5;

/// Resolution action for a detected knowledge drift.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "UPPERCASE")]
pub enum DriftAction {
    Supersede {
        old_id: String,
        new_id: String,
    },
    Merge {
        old_id: String,
        new_id: String,
        merged_content: String,
    },
    Coexist {
        old_id: String,
        new_id: String,
    },
    Ignore,
}

/// A candidate pair where a new fact is semantically similar to an
/// existing fact, indicating potential knowledge drift or contradiction.
#[derive(Debug, Clone)]
pub struct DriftCandidate {
    /// The newly extracted fact.
    pub new_fact: MemoryFact,
    /// The existing fact found via vector search.
    pub existing_fact: MemoryFact,
    /// Cosine similarity between the two embeddings.
    pub similarity: f32,
}

/// Build an LLM arbitration prompt for a batch of drift candidate pairs.
///
/// Each tuple contains:
/// `(old_content, old_type, old_ts, new_content, new_type, new_ts)`
///
/// The returned prompt asks the LLM to return a JSON array of actions.
pub fn build_arbitration_prompt(pairs: &[(&str, &str, i64, &str, &str, i64)]) -> String {
    let mut prompt = String::from(
        "You are a memory curator. Given pairs of OLD and NEW memory facts, \
         decide for each pair what action to take.\n\n\
         Actions:\n\
         - SUPERSEDE: The NEW fact replaces the OLD fact (OLD is outdated).\n\
         - MERGE: Combine both facts into one unified statement.\n\
         - COEXIST: Both facts are valid and can coexist.\n\
         - IGNORE: The similarity is coincidental; no action needed.\n\n\
         Respond with a JSON array. Each element must have an \"action\" field \
         (SUPERSEDE, MERGE, COEXIST, or IGNORE). For SUPERSEDE and COEXIST, \
         include \"old_id\" and \"new_id\". For MERGE, also include \"merged_content\".\n\n",
    );

    for (i, (old_content, old_type, old_ts, new_content, new_type, new_ts)) in
        pairs.iter().enumerate()
    {
        prompt.push_str(&format!(
            "### Pair {}\n\
             OLD (type={}, ts={}): {}\n\
             NEW (type={}, ts={}): {}\n\n",
            i + 1,
            old_type,
            old_ts,
            old_content,
            new_type,
            new_ts,
            new_content,
        ));
    }

    prompt.push_str("Respond ONLY with a JSON array, no other text.");
    prompt
}

/// Detects contradictions between new facts and existing knowledge.
pub struct DriftDetectStage;

#[async_trait]
impl DreamStage for DriftDetectStage {
    fn name(&self) -> &'static str {
        "drift_detect"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        !ctx.new_facts.is_empty()
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let mut candidates: Vec<DriftCandidate> = Vec::new();
        let max_candidates = ctx.config.drift_max_pairs_per_run();
        let similarity_threshold = ctx.config.drift_similarity_threshold();
        let filter = SearchFilter::new().with_valid_only();

        for fact in &ctx.new_facts {
            if candidates.len() >= max_candidates {
                break;
            }

            let embedding = match &fact.embedding {
                Some(emb) if !emb.is_empty() => emb,
                _ => {
                    debug!(fact_id = %fact.id, "Skipping drift check: no embedding");
                    continue;
                }
            };

            let dim_hint = embedding.len() as u32;
            let results = ctx
                .database
                .vector_search(embedding, dim_hint, &filter, DRIFT_SEARCH_K)
                .await?;

            for scored in results {
                if candidates.len() >= max_candidates {
                    break;
                }

                // Skip self-matches (same fact ID)
                if scored.fact.id == fact.id {
                    continue;
                }

                if scored.score >= similarity_threshold {
                    candidates.push(DriftCandidate {
                        new_fact: fact.clone(),
                        existing_fact: scored.fact,
                        similarity: scored.score,
                    });
                }
            }
        }

        info!(
            candidate_count = candidates.len(),
            "Drift detection found {} candidate pairs",
            candidates.len()
        );

        // Resolve drift candidates via LLM arbitration when a provider is available,
        // otherwise fall back to Coexist (safe default).
        let resolutions = if let Some(ref provider) = ctx.provider {
            let type_strings: Vec<_> = candidates
                .iter()
                .map(|c| {
                    (
                        c.existing_fact.note_type.to_string(),
                        c.new_fact.note_type.to_string(),
                    )
                })
                .collect();

            let pairs: Vec<_> = candidates
                .iter()
                .zip(type_strings.iter())
                .map(|(c, (old_type, new_type))| {
                    (
                        c.existing_fact.content.as_str(),
                        old_type.as_str(),
                        c.existing_fact.created_at,
                        c.new_fact.content.as_str(),
                        new_type.as_str(),
                        c.new_fact.created_at,
                    )
                })
                .collect();

            let candidate_ids: Vec<_> = candidates
                .iter()
                .map(|c| (c.existing_fact.id.clone(), c.new_fact.id.clone()))
                .collect();

            let prompt = build_arbitration_prompt(&pairs);

            match call_llm_arbitration(provider.as_ref(), &prompt, &candidate_ids).await {
                Ok(actions) => actions,
                Err(e) => {
                    warn!(error = %e, "DriftDetectStage: LLM arbitration failed, defaulting to Coexist");
                    default_coexist(&candidates)
                }
            }
        } else {
            info!("DriftDetectStage: no provider available, defaulting to Coexist");
            default_coexist(&candidates)
        };

        // Apply resolutions
        for action in &resolutions {
            match action {
                DriftAction::Supersede { old_id, new_id } => {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    ctx.database.close_fact_validity(old_id, now).await?;
                    ctx.database.set_fact_valid_from(new_id, now).await?;
                }
                DriftAction::Merge {
                    old_id,
                    new_id,
                    merged_content,
                } => {
                    // Close old fact's validity window (preserve as historical)
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    ctx.database.close_fact_validity(old_id, now).await?;
                    ctx.database.set_fact_valid_from(new_id, now).await?;
                    ctx.database
                        .update_fact_content(new_id, merged_content)
                        .await?;
                }
                DriftAction::Coexist { .. } | DriftAction::Ignore => {
                    // No-op: both facts remain valid
                }
            }
        }

        ctx.drift_resolutions = resolutions;
        Ok(ctx)
    }
}

/// Call the LLM to arbitrate drift candidates.
async fn call_llm_arbitration(
    provider: &dyn AiProvider,
    prompt: &str,
    candidate_ids: &[(String, String)],
) -> Result<Vec<DriftAction>, AlephError> {
    let msgs = [UnifiedMessage::user(prompt)];
    let system = "You are a memory curator. Respond ONLY with a JSON array. No explanation.";
    let payload = RequestPayload::new(&msgs).with_system(Some(system));
    let response = provider
        .process(payload)
        .await
        .map_err(|e| AlephError::other(format!("drift arbitration LLM call failed: {}", e)))?;

    let text = response.text_content();
    parse_arbitration_response(&text, candidate_ids)
}

/// Parse the LLM response into a list of `DriftAction`s.
///
/// Extracts a JSON array from the response text (tolerating preamble/postamble)
/// and maps each element to the corresponding action.
fn parse_arbitration_response(
    text: &str,
    candidate_ids: &[(String, String)],
) -> Result<Vec<DriftAction>, AlephError> {
    let json_start = text
        .find('[')
        .ok_or_else(|| AlephError::other("no JSON array in response"))?;
    let json_end = text
        .rfind(']')
        .ok_or_else(|| AlephError::other("no closing bracket"))?;
    let json_str = &text[json_start..=json_end];

    let raw: Vec<serde_json::Value> = serde_json::from_str(json_str)
        .map_err(|e| AlephError::other(format!("failed to parse arbitration JSON: {}", e)))?;

    let mut actions = Vec::new();
    for (i, val) in raw.iter().enumerate() {
        let action_str = val
            .get("action")
            .and_then(|a| a.as_str())
            .unwrap_or("COEXIST");
        let (old_id, new_id) = candidate_ids
            .get(i)
            .cloned()
            .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));

        let action = match action_str.to_uppercase().as_str() {
            "SUPERSEDE" => DriftAction::Supersede { old_id, new_id },
            "MERGE" => {
                let merged = val
                    .get("merged_content")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string();
                DriftAction::Merge {
                    old_id,
                    new_id,
                    merged_content: merged,
                }
            }
            "IGNORE" => DriftAction::Ignore,
            _ => DriftAction::Coexist { old_id, new_id },
        };
        actions.push(action);
    }

    Ok(actions)
}

/// Default all candidates to Coexist when LLM arbitration is unavailable.
fn default_coexist(candidates: &[DriftCandidate]) -> Vec<DriftAction> {
    candidates
        .iter()
        .map(|c| DriftAction::Coexist {
            old_id: c.existing_fact.id.clone(),
            new_id: c.new_fact.id.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Serde parse tests for DriftAction
    // -----------------------------------------------------------------------

    #[test]
    fn serde_parse_supersede() {
        let json = r#"{"action":"SUPERSEDE","old_id":"a","new_id":"b"}"#;
        let action: DriftAction = serde_json::from_str(json).unwrap();
        match action {
            DriftAction::Supersede { old_id, new_id } => {
                assert_eq!(old_id, "a");
                assert_eq!(new_id, "b");
            }
            _ => panic!("Expected Supersede"),
        }
    }

    #[test]
    fn serde_parse_merge() {
        let json = r#"{"action":"MERGE","old_id":"a","new_id":"b","merged_content":"combined"}"#;
        let action: DriftAction = serde_json::from_str(json).unwrap();
        match action {
            DriftAction::Merge {
                old_id,
                new_id,
                merged_content,
            } => {
                assert_eq!(old_id, "a");
                assert_eq!(new_id, "b");
                assert_eq!(merged_content, "combined");
            }
            _ => panic!("Expected Merge"),
        }
    }

    #[test]
    fn serde_parse_coexist() {
        let json = r#"{"action":"COEXIST","old_id":"x","new_id":"y"}"#;
        let action: DriftAction = serde_json::from_str(json).unwrap();
        match action {
            DriftAction::Coexist { old_id, new_id } => {
                assert_eq!(old_id, "x");
                assert_eq!(new_id, "y");
            }
            _ => panic!("Expected Coexist"),
        }
    }

    #[test]
    fn serde_parse_ignore() {
        let json = r#"{"action":"IGNORE"}"#;
        let action: DriftAction = serde_json::from_str(json).unwrap();
        assert!(matches!(action, DriftAction::Ignore));
    }

    #[test]
    fn serde_parse_array_of_actions() {
        let json = r#"[
            {"action":"SUPERSEDE","old_id":"a","new_id":"b"},
            {"action":"COEXIST","old_id":"c","new_id":"d"},
            {"action":"IGNORE"}
        ]"#;
        let actions: Vec<DriftAction> = serde_json::from_str(json).unwrap();
        assert_eq!(actions.len(), 3);
        assert!(matches!(actions[0], DriftAction::Supersede { .. }));
        assert!(matches!(actions[1], DriftAction::Coexist { .. }));
        assert!(matches!(actions[2], DriftAction::Ignore));
    }

    // -----------------------------------------------------------------------
    // build_arbitration_prompt tests
    // -----------------------------------------------------------------------

    #[test]
    fn arbitration_prompt_contains_expected_text() {
        let pairs = vec![(
            "The user likes Python",
            "preference",
            1000i64,
            "The user prefers Rust",
            "preference",
            2000i64,
        )];
        let prompt = build_arbitration_prompt(&pairs);

        assert!(prompt.contains("You are a memory curator"));
        assert!(prompt.contains("### Pair 1"));
        assert!(prompt.contains("OLD (type=preference, ts=1000): The user likes Python"));
        assert!(prompt.contains("NEW (type=preference, ts=2000): The user prefers Rust"));
        assert!(prompt.contains("SUPERSEDE"));
        assert!(prompt.contains("MERGE"));
        assert!(prompt.contains("COEXIST"));
        assert!(prompt.contains("IGNORE"));
        assert!(prompt.contains("JSON array"));
    }

    #[test]
    fn arbitration_prompt_multiple_pairs() {
        let pairs = vec![
            (
                "fact A old",
                "learning",
                100i64,
                "fact A new",
                "learning",
                200i64,
            ),
            (
                "fact B old",
                "personal",
                300i64,
                "fact B new",
                "personal",
                400i64,
            ),
        ];
        let prompt = build_arbitration_prompt(&pairs);
        assert!(prompt.contains("### Pair 1"));
        assert!(prompt.contains("### Pair 2"));
        assert!(prompt.contains("fact A old"));
        assert!(prompt.contains("fact B new"));
    }

    #[test]
    fn arbitration_prompt_empty_pairs() {
        let pairs: Vec<(&str, &str, i64, &str, &str, i64)> = vec![];
        let prompt = build_arbitration_prompt(&pairs);
        assert!(prompt.contains("You are a memory curator"));
        assert!(!prompt.contains("### Pair"));
    }

    // -----------------------------------------------------------------------
    // DriftDetectStage::execute with empty new_facts
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn execute_with_empty_new_facts_returns_empty_resolutions() {
        use crate::memory::decay::DecayConfig;
        use crate::memory::dreaming::report::{DreamRunMetadata, DreamRunType};
        use crate::memory::dreaming::GraphDecayConfig;
        use crate::sync_primitives::Arc;

        let tmp = tempfile::tempdir().expect("tempdir");
        let database = crate::memory::store::SqliteMemoryBackend::new(tmp.path())
                .expect("sqlite backend");
        let database = Arc::new(database);

        let ctx = DreamContext {
            memories: Vec::new(),
            clusters: Vec::new(),
            new_facts: Vec::new(),
            drift_resolutions: Vec::new(),
            config: crate::config::DreamingConfig::default(),
            run_metadata: DreamRunMetadata {
                run_type: DreamRunType::Daily,
                run_date: "2026-04-03".to_string(),
                run_start_ts: 0,
            },
            activity_checker: Arc::new(|| false),
            synthesis_insights_count: 0,
            database: database.clone(),
            graph_decay_config: GraphDecayConfig::default(),
            memory_decay_config: DecayConfig::default(),
            command_handler: None,
            provider: None,
            graph_decay_report: None,
            memory_decay_report: None,
            wiki_lint_report: None,
        };

        // should_run returns false when new_facts is empty
        let stage = DriftDetectStage;
        assert!(!stage.should_run(&ctx).await);

        // If execute were called anyway, it should return empty resolutions
        let result = stage.execute(ctx).await.unwrap();
        assert!(result.drift_resolutions.is_empty());
    }

    // -----------------------------------------------------------------------
    // parse_arbitration_response tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_arbitration_valid_json() {
        let text =
            r#"[{"action": "SUPERSEDE"}, {"action": "MERGE", "merged_content": "combined fact"}]"#;
        let ids = vec![
            ("old1".to_string(), "new1".to_string()),
            ("old2".to_string(), "new2".to_string()),
        ];
        let actions = parse_arbitration_response(text, &ids).unwrap();
        assert_eq!(actions.len(), 2);
        assert!(matches!(&actions[0], DriftAction::Supersede { old_id, .. } if old_id == "old1"));
        assert!(
            matches!(&actions[1], DriftAction::Merge { merged_content, .. } if merged_content == "combined fact")
        );
    }

    #[test]
    fn parse_arbitration_with_preamble() {
        let text = "Here's my analysis:\n[{\"action\": \"COEXIST\"}]\nDone.";
        let ids = vec![("o".to_string(), "n".to_string())];
        let actions = parse_arbitration_response(text, &ids).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], DriftAction::Coexist { .. }));
    }

    #[test]
    fn parse_arbitration_invalid_json_errors() {
        let text = "not json at all";
        let ids: Vec<(String, String)> = vec![];
        assert!(parse_arbitration_response(text, &ids).is_err());
    }

    #[test]
    fn parse_arbitration_unknown_action_defaults_coexist() {
        let text = r#"[{"action": "UNKNOWN_ACTION"}]"#;
        let ids = vec![("o".to_string(), "n".to_string())];
        let actions = parse_arbitration_response(text, &ids).unwrap();
        assert!(matches!(&actions[0], DriftAction::Coexist { .. }));
    }

    // -----------------------------------------------------------------------
    // execute with new_facts containing embeddings but no similar results
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn execute_with_new_facts_no_similar_existing_returns_empty_resolutions() {
        use crate::memory::decay::DecayConfig;
        use crate::memory::dreaming::report::{DreamRunMetadata, DreamRunType};
        use crate::memory::dreaming::GraphDecayConfig;
        use crate::sync_primitives::Arc;

        let tmp = tempfile::tempdir().expect("tempdir");
        let database = crate::memory::store::SqliteMemoryBackend::new(tmp.path())
                .expect("sqlite backend");
        let database = Arc::new(database);

        // Create a fact with an embedding so that the vector_search path is exercised.
        let mut fact = MemoryFact::new(
            "The user prefers dark mode".to_string(),
            crate::memory::context::NoteType::Preference,
            vec![],
        );
        fact.embedding = Some(vec![0.1; 768]);

        let ctx = DreamContext {
            memories: Vec::new(),
            clusters: Vec::new(),
            new_facts: vec![fact],
            drift_resolutions: Vec::new(),
            config: crate::config::DreamingConfig::default(),
            run_metadata: DreamRunMetadata {
                run_type: DreamRunType::Daily,
                run_date: "2026-04-03".to_string(),
                run_start_ts: 0,
            },
            activity_checker: Arc::new(|| false),
            synthesis_insights_count: 0,
            database: database.clone(),
            graph_decay_config: GraphDecayConfig::default(),
            memory_decay_config: DecayConfig::default(),
            command_handler: None,
            provider: None,
            graph_decay_report: None,
            memory_decay_report: None,
            wiki_lint_report: None,
        };

        let stage = DriftDetectStage;
        // should_run returns true because new_facts is non-empty
        assert!(stage.should_run(&ctx).await);

        // Execute: the empty database returns no vector_search results,
        // so no candidates are found and resolutions should be empty.
        let result = stage.execute(ctx).await.unwrap();
        assert!(
            result.drift_resolutions.is_empty(),
            "Expected empty resolutions when no similar facts exist in DB"
        );
    }

    // -----------------------------------------------------------------------
    // default_coexist produces correct actions
    // -----------------------------------------------------------------------

    #[test]
    fn default_coexist_produces_correct_actions() {
        let candidates = vec![
            DriftCandidate {
                new_fact: MemoryFact::new(
                    "new A".to_string(),
                    crate::memory::context::NoteType::Preference,
                    vec![],
                ),
                existing_fact: {
                    let mut f = MemoryFact::new(
                        "old A".to_string(),
                        crate::memory::context::NoteType::Preference,
                        vec![],
                    );
                    f.id = "old-a".to_string();
                    f
                },
                similarity: 0.9,
            },
            DriftCandidate {
                new_fact: {
                    let mut f = MemoryFact::new(
                        "new B".to_string(),
                        crate::memory::context::NoteType::Learning,
                        vec![],
                    );
                    f.id = "new-b".to_string();
                    f
                },
                existing_fact: {
                    let mut f = MemoryFact::new(
                        "old B".to_string(),
                        crate::memory::context::NoteType::Learning,
                        vec![],
                    );
                    f.id = "old-b".to_string();
                    f
                },
                similarity: 0.88,
            },
            DriftCandidate {
                new_fact: {
                    let mut f = MemoryFact::new(
                        "new C".to_string(),
                        crate::memory::context::NoteType::Other,
                        vec![],
                    );
                    f.id = "new-c".to_string();
                    f
                },
                existing_fact: {
                    let mut f = MemoryFact::new(
                        "old C".to_string(),
                        crate::memory::context::NoteType::Other,
                        vec![],
                    );
                    f.id = "old-c".to_string();
                    f
                },
                similarity: 0.92,
            },
        ];

        let actions = default_coexist(&candidates);

        assert_eq!(actions.len(), 3, "Should produce one action per candidate");

        // Verify each action is Coexist with correct ID mapping
        for (i, action) in actions.iter().enumerate() {
            match action {
                DriftAction::Coexist { old_id, new_id } => {
                    assert_eq!(old_id, &candidates[i].existing_fact.id);
                    assert_eq!(new_id, &candidates[i].new_fact.id);
                }
                other => panic!("Expected Coexist for candidate {}, got {:?}", i, other),
            }
        }
    }
}
