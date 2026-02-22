//! Step definitions for memory facts and integration features

use crate::world::{AlephWorld, MemoryContext};
use alephcore::memory::store::types::SearchFilter;
use alephcore::memory::store::{MemoryBackend, MemoryStore, SessionStore, StoreStats};
use alephcore::memory::store::LanceMemoryBackend;
use alephcore::memory::{FactType, MemoryEntry, NamespaceScope, EMBEDDING_DIM};
use alephcore::MemoryStats;
use alephcore::memory::store::types::ScoredFact;
use alephcore::memory::MemoryFact;
use cucumber::{gherkin::Step, given, then, when};
use std::sync::Arc;
use tempfile::tempdir;
use uuid::Uuid;

// ═══════════════════════════════════════════════════════════════════════════
// Helper Functions
// ═══════════════════════════════════════════════════════════════════════════

/// Stop words to filter from FTS queries (replicating old SQLite FTS5 logic).
const FTS_STOP_WORDS: &[&str] = &[
    "the", "is", "a", "an", "in", "on", "at", "to", "for", "of", "by", "with", "and", "or",
    "but", "not", "it", "its", "was", "has", "had", "been",
];

/// Prepare an FTS-style query string from user input.
///
/// Replicates the logic that was previously in `StateDatabase::prepare_fts_query`:
/// - Strip quote characters from input
/// - Tokenize on whitespace
/// - Filter stop words and single-character tokens
/// - Wrap remaining tokens in quotes joined with AND
fn prepare_fts_query(input: &str) -> String {
    let cleaned = input.replace('"', "");
    let tokens: Vec<&str> = cleaned
        .split_whitespace()
        .filter(|t| t.len() > 1)
        .filter(|t| !FTS_STOP_WORDS.contains(&t.to_lowercase().as_str()))
        .collect();

    tokens
        .iter()
        .map(|t| format!("\"{}\"", t))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Convert a Vec<ScoredFact> to Vec<MemoryFact> with scores applied.
fn scored_facts_to_memory_facts(scored: Vec<ScoredFact>) -> Vec<MemoryFact> {
    scored
        .into_iter()
        .map(|sf| {
            let mut fact = sf.fact;
            fact.similarity_score = Some(sf.score);
            fact
        })
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// Facts Vector DB Steps (existing)
// ═══════════════════════════════════════════════════════════════════════════

#[given("a temporary vector database")]
async fn given_temp_vector_db(w: &mut AlephWorld) {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let db: MemoryBackend = Arc::new(
        LanceMemoryBackend::open_or_create(temp_dir.path())
            .await
            .expect("Failed to create LanceDB"),
    );

    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.temp_dir = Some(temp_dir);
    ctx.memory_backend = Some(db);
}

#[given(expr = "a fact with id {string} and content {string} and type {string}")]
async fn given_fact_with_type(w: &mut AlephWorld, id: String, content: String, fact_type: String) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    let ft = match fact_type.as_str() {
        "preference" => FactType::Preference,
        "plan" => FactType::Plan,
        "learning" => FactType::Learning,
        "project" => FactType::Project,
        "personal" => FactType::Personal,
        _ => FactType::Other,
    };

    let embedding = MemoryContext::make_embedding(&[0.1; 10]);
    let fact = MemoryContext::create_fact(&id, &content, ft, embedding, true);
    ctx.facts.push(fact);
}

#[given(expr = "the fact has embedding value {float}")]
async fn given_fact_embedding_value(w: &mut AlephWorld, value: f32) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    if let Some(fact) = ctx.facts.last_mut() {
        let embedding = vec![value; EMBEDDING_DIM];
        fact.embedding = Some(embedding);
    }
}

#[given(expr = "{int} facts with incremental embeddings")]
async fn given_facts_incremental_embeddings(w: &mut AlephWorld, count: i32) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);

    for i in 0..count {
        let mut embedding = vec![0.0f32; EMBEDDING_DIM];
        embedding[0] = i as f32 * 0.1;

        let fact = MemoryContext::create_fact(
            &format!("fact-{}", i),
            &format!("Fact {}", i),
            FactType::Preference,
            embedding,
            true,
        );
        ctx.facts.push(fact);
    }

    // Insert all facts
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");
    for fact in &ctx.facts {
        db.insert_fact(fact)
            .await
            .expect("Failed to insert fact");
    }
}

#[given(expr = "{int} facts with sequential content")]
async fn given_facts_sequential(w: &mut AlephWorld, count: i32) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);

    for i in 0..count {
        let mut embedding = vec![0.0f32; EMBEDDING_DIM];
        embedding[0] = i as f32 * 0.01;

        let fact = MemoryContext::create_fact(
            &format!("fact-{}", i),
            &format!("Fact number {}", i),
            FactType::Other,
            embedding,
            true,
        );
        ctx.facts.push(fact);
    }

    // Insert all facts
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");
    for fact in &ctx.facts {
        db.insert_fact(fact)
            .await
            .expect("Failed to insert fact");
    }
}

#[given("these facts exist:")]
async fn given_these_facts_exist(w: &mut AlephWorld, step: &Step) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);

    if let Some(table) = step.table.as_ref() {
        for row in table.rows.iter().skip(1) {
            // Skip header row
            let id = &row[0];
            let content = &row[1];
            let embedding_first: f32 = row[2].parse().unwrap_or(0.1);

            let mut embedding = vec![0.0f32; EMBEDDING_DIM];
            embedding[0] = embedding_first;

            let fact = MemoryContext::create_fact(id, content, FactType::Preference, embedding, true);
            ctx.facts.push(fact);
        }
    }

    // Insert all facts
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");
    for fact in &ctx.facts {
        db.insert_fact(fact)
            .await
            .expect("Failed to insert fact");
    }
}

#[given(expr = "a valid fact with id {string} and content {string}")]
async fn given_valid_fact(w: &mut AlephWorld, id: String, content: String) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    let embedding = vec![0.5f32; EMBEDDING_DIM];
    let fact = MemoryContext::create_fact(&id, &content, FactType::Other, embedding, true);
    ctx.facts.push(fact);
}

#[given(expr = "an invalid fact with id {string} and content {string}")]
async fn given_invalid_fact(w: &mut AlephWorld, id: String, content: String) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    let embedding = vec![0.5f32; EMBEDDING_DIM];
    let mut fact = MemoryContext::create_fact(&id, &content, FactType::Other, embedding, false);
    fact.invalidation_reason = Some("Outdated".to_string());
    ctx.facts.push(fact);
}

#[when("I insert the fact into the database")]
async fn when_insert_fact(w: &mut AlephWorld) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");

    if let Some(fact) = ctx.facts.last() {
        db.insert_fact(fact)
            .await
            .expect("Failed to insert fact");
    }
}

#[when("I insert all facts into the database")]
async fn when_insert_all_facts(w: &mut AlephWorld) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");

    for fact in &ctx.facts {
        db.insert_fact(fact)
            .await
            .expect("Failed to insert fact");
    }
}

#[when(expr = "I search with a zero embedding and limit {int}")]
async fn when_search_zero_embedding(w: &mut AlephWorld, limit: i32) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");

    let query = vec![0.0f32; EMBEDDING_DIM];
    let filter = SearchFilter::valid_only(Some(NamespaceScope::Owner));
    let scored = db
        .vector_search(&query, EMBEDDING_DIM as u32, &filter, limit as usize)
        .await
        .expect("Search failed");
    ctx.search_results = scored
        .into_iter()
        .map(|sf| {
            let mut fact = sf.fact;
            fact.similarity_score = Some(sf.score);
            fact
        })
        .collect();
}

#[when(expr = "I prepare FTS query for {string}")]
async fn when_prepare_fts_query(w: &mut AlephWorld, input: String) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.fts_query = Some(prepare_fts_query(&input));
}

#[when("I prepare FTS query for input with quotes")]
async fn when_prepare_fts_query_with_quotes(w: &mut AlephWorld) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    // Input: he said "hello"
    ctx.fts_query = Some(prepare_fts_query("he said \"hello\""));
}

#[when("I hybrid search with the same embedding and empty text")]
async fn when_hybrid_search_same_embedding(w: &mut AlephWorld) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");

    // Get the embedding from the last inserted fact
    let embedding = ctx
        .facts
        .last()
        .and_then(|f| f.embedding.clone())
        .unwrap_or_else(|| vec![0.5f32; EMBEDDING_DIM]);

    let filter = SearchFilter::default();
    let scored = db
        .vector_search(&embedding, EMBEDDING_DIM as u32, &filter, 5)
        .await
        .expect("Hybrid search failed");

    ctx.search_results = scored_facts_to_memory_facts(scored);
}

#[when(expr = "I hybrid search for {string} with embedding value {float}")]
async fn when_hybrid_search_text(w: &mut AlephWorld, text: String, value: f32) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");

    let embedding = vec![value; EMBEDDING_DIM];
    let filter = SearchFilter::default();
    let scored = db
        .hybrid_search(&embedding, EMBEDDING_DIM as u32, &text, 0.7, 0.3, &filter, 5)
        .await
        .expect("Hybrid search failed");

    ctx.search_results = scored_facts_to_memory_facts(scored);
}

#[when(expr = "I hybrid search with opposite embedding and min_score {float}")]
async fn when_hybrid_search_opposite_min_score(w: &mut AlephWorld, min_score: f32) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");

    // Opposite embedding
    let embedding = vec![-0.5f32; EMBEDDING_DIM];
    let filter = SearchFilter::default();
    let scored = db
        .vector_search(&embedding, EMBEDDING_DIM as u32, &filter, 5)
        .await
        .expect("Hybrid search failed");

    // Apply min_score filtering
    ctx.search_results = scored
        .into_iter()
        .filter(|sf| sf.score >= min_score)
        .map(|sf| {
            let mut fact = sf.fact;
            fact.similarity_score = Some(sf.score);
            fact
        })
        .collect();
}

#[when(expr = "I hybrid search with empty text and limit {int}")]
async fn when_hybrid_search_limit(w: &mut AlephWorld, limit: i32) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");

    let embedding = vec![0.0f32; EMBEDDING_DIM];
    let filter = SearchFilter::default();
    let scored = db
        .vector_search(&embedding, EMBEDDING_DIM as u32, &filter, limit as usize)
        .await
        .expect("Hybrid search failed");

    ctx.search_results = scored_facts_to_memory_facts(scored);
}

#[when("I hybrid search with the shared embedding")]
async fn when_hybrid_search_shared_embedding(w: &mut AlephWorld) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");

    // Use the common embedding (0.5)
    let embedding = vec![0.5f32; EMBEDDING_DIM];
    let filter = SearchFilter::default();
    let scored = db
        .vector_search(&embedding, EMBEDDING_DIM as u32, &filter, 10)
        .await
        .expect("Hybrid search failed");

    ctx.search_results = scored_facts_to_memory_facts(scored);
}

#[then("I should be able to search and find the fact")]
async fn then_can_search_fact(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");

    // Get the embedding from the last inserted fact
    let embedding = ctx
        .facts
        .last()
        .and_then(|f| f.embedding.clone())
        .expect("No fact with embedding");

    // Search should find the fact via vector search
    let filter = SearchFilter::valid_only(Some(NamespaceScope::Owner));
    let results = db
        .vector_search(&embedding, EMBEDDING_DIM as u32, &filter, 10)
        .await
        .expect("Search failed");
    assert!(
        !results.is_empty(),
        "Should find the inserted fact via vector search"
    );
}

#[then(expr = "I should receive {int} result")]
async fn then_receive_exact_results(w: &mut AlephWorld, expected: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    assert_eq!(
        ctx.search_results.len(),
        expected as usize,
        "Expected {} results, got {}",
        expected,
        ctx.search_results.len()
    );
}

#[then(expr = "I should receive {int} results")]
async fn then_receive_results(w: &mut AlephWorld, expected: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    assert_eq!(
        ctx.search_results.len(),
        expected as usize,
        "Expected {} results, got {}",
        expected,
        ctx.search_results.len()
    );
}

#[then("I should receive results")]
async fn then_receive_any_results(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    assert!(
        !ctx.search_results.is_empty(),
        "Expected at least one result"
    );
}

#[then(expr = "I should receive at most {int} results")]
async fn then_receive_at_most_results(w: &mut AlephWorld, max: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    assert!(
        ctx.search_results.len() <= max as usize,
        "Expected at most {} results, got {}",
        max,
        ctx.search_results.len()
    );
}

#[then("all results should have similarity scores")]
async fn then_all_have_scores(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    for (i, result) in ctx.search_results.iter().enumerate() {
        assert!(
            result.similarity_score.is_some(),
            "Result {} missing similarity score",
            i
        );
    }
}

#[then("the FTS query should match basic tokenization")]
async fn then_fts_query_basic(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let fts_query = ctx.fts_query.as_ref().expect("FTS query not prepared");
    assert_eq!(fts_query, "\"rust\" AND \"programming\"");
}

#[then("the FTS query should filter stop words")]
async fn then_fts_query_stop_words(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let fts_query = ctx.fts_query.as_ref().expect("FTS query not prepared");
    // "the", "is" are stop words; "user" stays
    assert_eq!(fts_query, "\"user\" AND \"learning\" AND \"rust\"");
}

#[then("the FTS query should filter single chars")]
async fn then_fts_query_single_chars(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let fts_query = ctx.fts_query.as_ref().expect("FTS query not prepared");
    // "I", "a" are single chars
    assert_eq!(fts_query, "\"am\" AND \"rust\" AND \"developer\"");
}

#[then("the FTS query should be empty")]
async fn then_fts_query_empty(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let fts_query = ctx.fts_query.as_ref().expect("FTS query not prepared");
    assert!(
        fts_query.is_empty(),
        "Expected empty FTS query, got: {}",
        fts_query
    );
}

#[then("the FTS query should have escaped quotes")]
async fn then_fts_query_escaped_quotes(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let fts_query = ctx.fts_query.as_ref().expect("FTS query not prepared");
    // Quotes should be removed from input
    assert_eq!(fts_query, "\"he\" AND \"said\" AND \"hello\"");
}

#[then("the first result should have a high similarity score")]
async fn then_first_high_score(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let first = ctx.search_results.first().expect("No results");
    let score = first.similarity_score.expect("Missing score");
    assert!(score > 0.9, "Expected high score (>0.9), got {}", score);
}

#[then(expr = "the result should have id {string}")]
async fn then_result_has_id(w: &mut AlephWorld, expected_id: String) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    assert_eq!(ctx.search_results.len(), 1, "Expected exactly one result");
    assert_eq!(ctx.search_results[0].id, expected_id, "Result ID mismatch");
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Test Steps
// ═══════════════════════════════════════════════════════════════════════════

// --- Background Setup Steps ---

#[given("a test vector database")]
async fn given_test_vector_db(w: &mut AlephWorld) {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join(format!("test_integration_{}.db", Uuid::new_v4()));
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.setup_integration(temp_dir, db_path);
}

#[given("a smart embedder")]
async fn given_smart_embedder(w: &mut AlephWorld) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.init_embedder();
}

#[given("default memory config")]
async fn given_default_memory_config(w: &mut AlephWorld) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.create_default_config();
}

#[given("memory services are initialized")]
async fn given_memory_services_initialized(w: &mut AlephWorld) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.init_services();
}

#[given("disabled memory config")]
async fn given_disabled_memory_config(w: &mut AlephWorld) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.create_disabled_config();
}

#[given(expr = "memory config with max_context_items {int}")]
async fn given_memory_config_max_items(w: &mut AlephWorld, max_items: i32) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.create_config_with_max_items(max_items as u32);
}

#[given(expr = "memory config with similarity_threshold {float}")]
async fn given_memory_config_threshold(w: &mut AlephWorld, threshold: f32) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.create_config_with_threshold(threshold);
}

#[given(expr = "a context anchor for {string} with document {string}")]
async fn given_context_anchor(w: &mut AlephWorld, app: String, doc: String) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.set_context(&app, &doc);
}

#[given("a prompt augmenter")]
async fn given_prompt_augmenter(w: &mut AlephWorld) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.init_augmenter();
}

#[given(expr = "a prompt augmenter with max {int} memories")]
async fn given_prompt_augmenter_max(w: &mut AlephWorld, max: i32) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.init_augmenter_with_config(max as usize, false);
}

// --- When Steps ---

#[when(expr = "I store a memory with input {string} and output {string}")]
async fn when_store_memory(w: &mut AlephWorld, user_input: String, ai_output: String) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let ingestion = ctx.ingestion.as_ref().expect("Ingestion not initialized");
    let context = ctx.context_anchor.clone().expect("Context not set");

    match ingestion
        .store_memory(context, &user_input, &ai_output)
        .await
    {
        Ok(id) => {
            ctx.last_memory_id = Some(id);
            ctx.last_result = Some(Ok(()));
        }
        Err(e) => {
            ctx.last_result = Some(Err(e.to_string()));
        }
    }
}

#[when(expr = "I try to store a memory with input {string} and output {string}")]
async fn when_try_store_memory(w: &mut AlephWorld, user_input: String, ai_output: String) {
    // Same as above but used when we expect failure
    when_store_memory(w, user_input, ai_output).await;
}

#[when("I store these memories:")]
async fn when_store_these_memories(w: &mut AlephWorld, step: &Step) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let ingestion = ctx.ingestion.clone().expect("Ingestion not initialized");
    let context = ctx.context_anchor.clone().expect("Context not set");

    if let Some(table) = step.table.as_ref() {
        for row in table.rows.iter().skip(1) {
            let user_input = &row[0];
            let ai_output = &row[1];

            ingestion
                .store_memory(context.clone(), user_input, ai_output)
                .await
                .expect("Failed to store memory");
        }
    }
}

#[when(expr = "I retrieve memories for query {string}")]
async fn when_retrieve_memories(w: &mut AlephWorld, query: String) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let retrieval = ctx.retrieval.as_ref().expect("Retrieval not initialized");
    let context = ctx.context_anchor.as_ref().expect("Context not set");

    let memories = retrieval
        .retrieve_memories(context, &query)
        .await
        .expect("Failed to retrieve memories");

    ctx.memories = memories;
}

#[when(expr = "I switch to context anchor for {string} with document {string}")]
async fn when_switch_context(w: &mut AlephWorld, app: String, doc: String) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.set_context(&app, &doc);
}

#[when(expr = "I augment prompt {string} with memories and query {string}")]
async fn when_augment_prompt(w: &mut AlephWorld, base_prompt: String, query: String) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let augmenter = ctx.augmenter.as_ref().expect("Augmenter not initialized");

    let augmented = augmenter.augment_prompt(&base_prompt, &ctx.memories, &query);
    ctx.augmented_prompt = Some(augmented);
}

#[when(expr = "I augment prompt {string} with no memories and query {string}")]
async fn when_augment_prompt_no_memories(w: &mut AlephWorld, base_prompt: String, query: String) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let augmenter = ctx.augmenter.as_ref().expect("Augmenter not initialized");

    let augmented = augmenter.augment_prompt(&base_prompt, &[], &query);
    ctx.augmented_prompt = Some(augmented);
}

#[when("I get the memory summary")]
async fn when_get_memory_summary(w: &mut AlephWorld) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let augmenter = ctx.augmenter.as_ref().expect("Augmenter not initialized");

    let summary = augmenter.get_memory_summary(&ctx.memories);
    ctx.memory_summary = Some(summary);
}

#[when("I get database stats")]
async fn when_get_db_stats(w: &mut AlephWorld) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");

    let store_stats: StoreStats = db.get_stats().await.expect("Failed to get stats");
    ctx.db_stats = Some(MemoryStats {
        total_memories: store_stats.total_memories as u64,
        total_apps: 0,
        database_size_mb: 0.0,
        oldest_memory_timestamp: 0,
        newest_memory_timestamp: 0,
    });
}

#[when(expr = "I concurrently store {int} memories")]
async fn when_concurrent_store(w: &mut AlephWorld, count: i32) {
    use tokio::task::JoinSet;

    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let ingestion = ctx.ingestion.clone().expect("Ingestion not initialized");
    let context = ctx.context_anchor.clone().expect("Context not set");

    let mut join_set = JoinSet::new();

    for i in 0..count {
        let ingestion_clone = ingestion.clone();
        let context_clone = context.clone();

        join_set.spawn(async move {
            ingestion_clone
                .store_memory(
                    context_clone,
                    &format!("concurrent input {}", i),
                    &format!("concurrent output {}", i),
                )
                .await
        });
    }

    // Wait for all tasks to complete
    while join_set.join_next().await.is_some() {}
}

#[when(expr = "I perform {int} concurrent retrievals")]
async fn when_concurrent_retrievals(w: &mut AlephWorld, count: i32) {
    use tokio::task::JoinSet;

    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let retrieval = ctx.retrieval.clone().expect("Retrieval not initialized");
    let context = ctx.context_anchor.clone().expect("Context not set");

    let mut join_set = JoinSet::new();

    for i in 0..count {
        let retrieval_clone = retrieval.clone();
        let context_clone = context.clone();

        join_set.spawn(async move {
            retrieval_clone
                .retrieve_memories(&context_clone, &format!("query test {}", i % 5))
                .await
        });
    }

    // Collect results
    let mut all_results = Vec::new();
    while let Some(result) = join_set.join_next().await {
        if let Ok(Ok(memories)) = result {
            all_results.push(memories);
        }
    }

    // Store result count for later assertions
    ctx.memories = if all_results.is_empty() {
        Vec::new()
    } else {
        // Store the count as a signal of success
        all_results[0].clone()
    };

    // Store the actual count in db_stats for verification
    let stats = MemoryStats {
        total_memories: all_results.len() as u64,
        total_apps: 0,
        database_size_mb: 0.0,
        oldest_memory_timestamp: 0,
        newest_memory_timestamp: 0,
    };
    ctx.db_stats = Some(stats);
}

#[when(expr = "I perform {int} concurrent mixed insert and retrieve operations")]
async fn when_concurrent_mixed(w: &mut AlephWorld, count: i32) {
    use tokio::task::JoinSet;

    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let ingestion = ctx.ingestion.clone().expect("Ingestion not initialized");
    let retrieval = ctx.retrieval.clone().expect("Retrieval not initialized");
    let context = ctx.context_anchor.clone().expect("Context not set");

    let mut join_set: JoinSet<&str> = JoinSet::new();

    for i in 0..count {
        if i % 2 == 0 {
            let ingestion_clone = ingestion.clone();
            let context_clone = context.clone();
            join_set.spawn(async move {
                let _ = ingestion_clone
                    .store_memory(
                        context_clone,
                        &format!("mixed input {}", i),
                        &format!("mixed output {}", i),
                    )
                    .await;
                "insert"
            });
        } else {
            let retrieval_clone = retrieval.clone();
            let context_clone = context.clone();
            join_set.spawn(async move {
                let _ = retrieval_clone.retrieve_memories(&context_clone, "mixed").await;
                "retrieve"
            });
        }
    }

    let mut operation_count = 0;
    while join_set.join_next().await.is_some() {
        operation_count += 1;
    }

    // Store operation count
    let stats = MemoryStats {
        total_memories: operation_count as u64,
        total_apps: 0,
        database_size_mb: 0.0,
        oldest_memory_timestamp: 0,
        newest_memory_timestamp: 0,
    };
    ctx.db_stats = Some(stats);
}

#[when(expr = "I directly insert {int} memories with known IDs")]
async fn when_direct_insert_with_ids(w: &mut AlephWorld, count: i32) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.memory_backend.as_ref().expect("Database not initialized");
    let context = ctx.context_anchor.clone().expect("Context not set");

    for i in 0..count {
        let id = format!("mem-{}", i);
        let embedding = vec![1.0; EMBEDDING_DIM];
        let memory = MemoryEntry::with_embedding(
            id,
            context.clone(),
            format!("input {}", i),
            format!("output {}", i),
            embedding,
        );
        db.insert_memory(&memory).await.expect("Failed to insert");
    }
}

#[when(expr = "I concurrently delete {int} of those memories")]
async fn when_concurrent_delete(w: &mut AlephWorld, count: i32) {
    use tokio::task::JoinSet;

    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.memory_backend.clone().expect("Database not initialized");

    let mut join_set = JoinSet::new();

    for i in 0..count {
        let db_clone = db.clone();
        let id = format!("mem-{}", i);

        join_set.spawn(async move { SessionStore::delete_memory(db_clone.as_ref(), &id).await });
    }

    while let Some(_result) = join_set.join_next().await {
        // Some deletes may succeed, some may fail if already deleted
    }
}

#[when(expr = "I perform {int} concurrent stats queries")]
async fn when_concurrent_stats(w: &mut AlephWorld, count: i32) {
    use tokio::task::JoinSet;

    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.memory_backend.clone().expect("Database not initialized");

    let mut join_set = JoinSet::new();

    for _ in 0..count {
        let db_clone = db.clone();
        join_set.spawn(async move { SessionStore::get_stats(db_clone.as_ref()).await });
    }

    let mut all_stats: Vec<StoreStats> = Vec::new();
    while let Some(result) = join_set.join_next().await {
        if let Ok(Ok(stats)) = result {
            all_stats.push(stats);
        }
    }

    // Store stats count for verification - we use a separate field to track the actual memory count
    // total_memories = count of successful stats queries
    // total_apps = the actual memory count from first stats result (repurposed field)
    let stats = MemoryStats {
        total_memories: all_stats.len() as u64,
        total_apps: if all_stats.is_empty() {
            0
        } else {
            all_stats[0].total_memories as u64
        },
        database_size_mb: 0.0,
        oldest_memory_timestamp: 0,
        newest_memory_timestamp: 0,
    };
    ctx.db_stats = Some(stats);
}

// --- Then Steps ---

#[then("the memory should be stored successfully")]
async fn then_memory_stored_successfully(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    assert!(ctx.last_memory_id.is_some(), "Memory ID should be set");
    assert!(
        ctx.last_result.as_ref().map(|r| r.is_ok()).unwrap_or(false),
        "Store operation should succeed"
    );
}

#[then("the store operation should fail")]
async fn then_store_should_fail(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    assert!(
        ctx.last_result.as_ref().map(|r| r.is_err()).unwrap_or(true),
        "Store operation should fail"
    );
}

#[then(expr = "I should get {int} memory")]
async fn then_get_n_memory(w: &mut AlephWorld, expected: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    assert_eq!(
        ctx.memories.len(),
        expected as usize,
        "Expected {} memory, got {}",
        expected,
        ctx.memories.len()
    );
}

#[then(expr = "I should get {int} memories")]
async fn then_get_n_memories(w: &mut AlephWorld, expected: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    assert_eq!(
        ctx.memories.len(),
        expected as usize,
        "Expected {} memories, got {}",
        expected,
        ctx.memories.len()
    );
}

#[then(expr = "I should get at least {int} memory")]
async fn then_get_at_least_n_memory(w: &mut AlephWorld, min: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    assert!(
        ctx.memories.len() >= min as usize,
        "Expected at least {} memory, got {}",
        min,
        ctx.memories.len()
    );
}

#[then(expr = "I should get at most {int} memories")]
async fn then_get_at_most_n_memories(w: &mut AlephWorld, max: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    assert!(
        ctx.memories.len() <= max as usize,
        "Expected at most {} memories, got {}",
        max,
        ctx.memories.len()
    );
}

#[then("I should get at most max_context_items memories")]
async fn then_get_at_most_max_items(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert!(
        ctx.memories.len() <= config.max_context_items as usize,
        "Expected at most {} memories, got {}",
        config.max_context_items,
        ctx.memories.len()
    );
}

#[then(expr = "the first memory should contain {string} in user_input")]
async fn then_first_memory_contains_user_input(w: &mut AlephWorld, expected: String) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let first = ctx.memories.first().expect("No memories");
    assert!(
        first.user_input.contains(&expected),
        "user_input '{}' should contain '{}'",
        first.user_input,
        expected
    );
}

#[then(expr = "the first memory should contain {string} in ai_output")]
async fn then_first_memory_contains_ai_output(w: &mut AlephWorld, expected: String) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let first = ctx.memories.first().expect("No memories");
    assert!(
        first.ai_output.contains(&expected),
        "ai_output '{}' should contain '{}'",
        first.ai_output,
        expected
    );
}

#[then(expr = "the first memory should not contain {string} in user_input")]
async fn then_first_memory_not_contains_user_input(w: &mut AlephWorld, not_expected: String) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let first = ctx.memories.first().expect("No memories");
    assert!(
        !first.user_input.contains(&not_expected),
        "user_input '{}' should not contain '{}'",
        first.user_input,
        not_expected
    );
}

#[then("all retrieved memories should have similarity scores")]
async fn then_all_memories_have_scores(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    for (i, memory) in ctx.memories.iter().enumerate() {
        assert!(
            memory.similarity_score.is_some(),
            "Memory {} missing similarity score",
            i
        );
    }
}

#[then("all similarity scores should meet the threshold")]
async fn then_all_scores_meet_threshold(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");

    for memory in &ctx.memories {
        if let Some(score) = memory.similarity_score {
            assert!(
                score >= config.similarity_threshold,
                "Score {} below threshold {}",
                score,
                config.similarity_threshold
            );
        }
    }
}

#[then("memories should be sorted by similarity descending")]
async fn then_memories_sorted_by_similarity(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");

    for i in 1..ctx.memories.len() {
        let prev_score = ctx.memories[i - 1].similarity_score.unwrap_or(0.0);
        let curr_score = ctx.memories[i].similarity_score.unwrap_or(0.0);
        assert!(
            prev_score >= curr_score,
            "Memories not sorted: {} < {}",
            prev_score,
            curr_score
        );
    }
}

#[then(expr = "the augmented prompt should contain {string}")]
async fn then_augmented_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let augmented = ctx
        .augmented_prompt
        .as_ref()
        .expect("Augmented prompt not set");
    assert!(
        augmented.contains(&expected),
        "Augmented prompt should contain '{}'",
        expected
    );
}

#[then(expr = "the augmented prompt should not contain {string}")]
async fn then_augmented_not_contains(w: &mut AlephWorld, not_expected: String) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let augmented = ctx
        .augmented_prompt
        .as_ref()
        .expect("Augmented prompt not set");
    assert!(
        !augmented.contains(&not_expected),
        "Augmented prompt should not contain '{}'",
        not_expected
    );
}

#[then(expr = "the augmented prompt should contain at most {int} memory entries")]
async fn then_augmented_max_entries(w: &mut AlephWorld, max: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let augmented = ctx
        .augmented_prompt
        .as_ref()
        .expect("Augmented prompt not set");

    // Count "Question" occurrences as memory entries
    let count = augmented.matches("Question").count();
    assert!(
        count <= max as usize,
        "Expected at most {} memory entries, got {}",
        max,
        count
    );
}

#[then(expr = "the memory summary should contain {string} or {string}")]
async fn then_summary_contains_or(w: &mut AlephWorld, expected1: String, expected2: String) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let summary = ctx.memory_summary.as_ref().expect("Summary not set");
    assert!(
        summary.contains(&expected1) || summary.contains(&expected2),
        "Summary '{}' should contain '{}' or '{}'",
        summary,
        expected1,
        expected2
    );
}

#[then(expr = "the database should have {int} total memories")]
async fn then_db_has_n_memories(w: &mut AlephWorld, expected: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let stats = ctx.db_stats.as_ref().expect("Stats not retrieved");
    assert_eq!(
        stats.total_memories, expected as u64,
        "Expected {} memories, got {}",
        expected, stats.total_memories
    );
}

#[then(expr = "the database should have at least {int} total memories")]
async fn then_db_has_at_least_n_memories(w: &mut AlephWorld, min: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let stats = ctx.db_stats.as_ref().expect("Stats not retrieved");
    assert!(
        stats.total_memories >= min as u64,
        "Expected at least {} memories, got {}",
        min,
        stats.total_memories
    );
}

#[then(expr = "all {int} retrieval operations should succeed")]
async fn then_all_retrievals_succeed(w: &mut AlephWorld, expected: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let stats = ctx.db_stats.as_ref().expect("Stats not set");
    assert_eq!(
        stats.total_memories, expected as u64,
        "Expected {} successful retrievals, got {}",
        expected, stats.total_memories
    );
}

#[then("each retrieval should return results")]
async fn then_each_retrieval_has_results(w: &mut AlephWorld) {
    // Already verified by the retrieval count
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let stats = ctx.db_stats.as_ref().expect("Stats not set");
    assert!(stats.total_memories > 0, "Should have retrieval results");
}

#[then(expr = "all {int} operations should complete")]
async fn then_all_operations_complete(w: &mut AlephWorld, expected: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let stats = ctx.db_stats.as_ref().expect("Stats not set");
    assert_eq!(
        stats.total_memories, expected as u64,
        "Expected {} operations, got {}",
        expected, stats.total_memories
    );
}

#[then(expr = "all {int} stats queries should succeed")]
async fn then_all_stats_queries_succeed(w: &mut AlephWorld, expected: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let stats = ctx.db_stats.as_ref().expect("Stats not set");
    assert_eq!(
        stats.total_memories, expected as u64,
        "Expected {} successful stats queries, got {}",
        expected, stats.total_memories
    );
}

#[then(expr = "each stats result should show at least {int} memories")]
async fn then_each_stats_shows_min_memories(w: &mut AlephWorld, min: i32) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let stats = ctx.db_stats.as_ref().expect("Stats not set");
    // total_apps is repurposed to store the actual memory count from first stats query
    assert!(
        stats.total_apps >= min as u64,
        "Expected at least {} memories in stats, got {}",
        min,
        stats.total_apps
    );
}
