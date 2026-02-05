//! Step definitions for memory facts features

use cucumber::{given, when, then, gherkin::Step};
use crate::world::{AlephWorld, MemoryContext};
use alephcore::memory::database::VectorDatabase;
use alephcore::memory::{FactType, EMBEDDING_DIM};
use tempfile::tempdir;
use std::sync::Arc;

// ═══ Given Steps ═══

#[given("a temporary vector database")]
async fn given_temp_vector_db(w: &mut AlephWorld) {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    let db = VectorDatabase::new(db_path).expect("Failed to create VectorDatabase");

    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.temp_dir = Some(temp_dir);
    ctx.db = Some(Arc::new(db));
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
    let db = ctx.db.as_ref().expect("Database not initialized");
    for fact in &ctx.facts {
        db.insert_fact(fact.clone()).await.expect("Failed to insert fact");
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
    let db = ctx.db.as_ref().expect("Database not initialized");
    for fact in &ctx.facts {
        db.insert_fact(fact.clone()).await.expect("Failed to insert fact");
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

            let fact = MemoryContext::create_fact(
                id,
                content,
                FactType::Preference,
                embedding,
                true,
            );
            ctx.facts.push(fact);
        }
    }

    // Insert all facts
    let db = ctx.db.as_ref().expect("Database not initialized");
    for fact in &ctx.facts {
        db.insert_fact(fact.clone()).await.expect("Failed to insert fact");
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

// ═══ When Steps ═══

#[when("I insert the fact into the database")]
async fn when_insert_fact(w: &mut AlephWorld) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.db.as_ref().expect("Database not initialized");

    if let Some(fact) = ctx.facts.last() {
        db.insert_fact(fact.clone()).await.expect("Failed to insert fact");
    }
}

#[when("I insert all facts into the database")]
async fn when_insert_all_facts(w: &mut AlephWorld) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.db.as_ref().expect("Database not initialized");

    for fact in &ctx.facts {
        db.insert_fact(fact.clone()).await.expect("Failed to insert fact");
    }
}

#[when(expr = "I search with a zero embedding and limit {int}")]
async fn when_search_zero_embedding(w: &mut AlephWorld, limit: i32) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.db.as_ref().expect("Database not initialized");

    let query = vec![0.0f32; EMBEDDING_DIM];
    let results = db.search_facts(&query, limit as u32, false).await.expect("Search failed");
    ctx.search_results = results;
}

#[when(expr = "I prepare FTS query for {string}")]
async fn when_prepare_fts_query(w: &mut AlephWorld, input: String) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    ctx.fts_query = Some(VectorDatabase::prepare_fts_query(&input));
}

#[when("I prepare FTS query for input with quotes")]
async fn when_prepare_fts_query_with_quotes(w: &mut AlephWorld) {
    let ctx = w.memory.get_or_insert_with(MemoryContext::default);
    // Input: he said "hello"
    ctx.fts_query = Some(VectorDatabase::prepare_fts_query("he said \"hello\""));
}

#[when("I hybrid search with the same embedding and empty text")]
async fn when_hybrid_search_same_embedding(w: &mut AlephWorld) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.db.as_ref().expect("Database not initialized");

    // Get the embedding from the last inserted fact
    let embedding = ctx.facts.last()
        .and_then(|f| f.embedding.clone())
        .unwrap_or_else(|| vec![0.5f32; EMBEDDING_DIM]);

    let results = db
        .hybrid_search_facts(&embedding, "", 0.7, 0.3, 0.0, 10, 5)
        .await
        .expect("Hybrid search failed");

    ctx.search_results = results;
}

#[when(expr = "I hybrid search for {string} with embedding value {float}")]
async fn when_hybrid_search_text(w: &mut AlephWorld, text: String, value: f32) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.db.as_ref().expect("Database not initialized");

    let embedding = vec![value; EMBEDDING_DIM];
    let results = db
        .hybrid_search_facts(&embedding, &text, 0.7, 0.3, 0.0, 10, 5)
        .await
        .expect("Hybrid search failed");

    ctx.search_results = results;
}

#[when(expr = "I hybrid search with opposite embedding and min_score {float}")]
async fn when_hybrid_search_opposite_min_score(w: &mut AlephWorld, min_score: f32) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.db.as_ref().expect("Database not initialized");

    // Opposite embedding
    let embedding = vec![-0.5f32; EMBEDDING_DIM];
    let results = db
        .hybrid_search_facts(&embedding, "", 0.7, 0.3, min_score, 10, 5)
        .await
        .expect("Hybrid search failed");

    ctx.search_results = results;
}

#[when(expr = "I hybrid search with empty text and limit {int}")]
async fn when_hybrid_search_limit(w: &mut AlephWorld, limit: i32) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.db.as_ref().expect("Database not initialized");

    let embedding = vec![0.0f32; EMBEDDING_DIM];
    let results = db
        .hybrid_search_facts(&embedding, "", 0.7, 0.3, 0.0, 20, limit as usize)
        .await
        .expect("Hybrid search failed");

    ctx.search_results = results;
}

#[when("I hybrid search with the shared embedding")]
async fn when_hybrid_search_shared_embedding(w: &mut AlephWorld) {
    let ctx = w.memory.as_mut().expect("Memory context not initialized");
    let db = ctx.db.as_ref().expect("Database not initialized");

    // Use the common embedding (0.5)
    let embedding = vec![0.5f32; EMBEDDING_DIM];
    let results = db
        .hybrid_search_facts(&embedding, "", 0.7, 0.3, 0.0, 10, 10)
        .await
        .expect("Hybrid search failed");

    ctx.search_results = results;
}

// ═══ Then Steps ═══

#[then("I should be able to search and find the fact")]
async fn then_can_search_fact(w: &mut AlephWorld) {
    let ctx = w.memory.as_ref().expect("Memory context not initialized");
    let db = ctx.db.as_ref().expect("Database not initialized");

    // Get the embedding from the last inserted fact
    let embedding = ctx.facts.last()
        .and_then(|f| f.embedding.clone())
        .expect("No fact with embedding");

    // Search should find the fact (this uses facts_vec internally)
    let results = db.search_facts(&embedding, 10, false).await.expect("Search failed");
    assert!(!results.is_empty(), "Should find the inserted fact via vector search");
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
    assert!(!ctx.search_results.is_empty(), "Expected at least one result");
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
    assert!(fts_query.is_empty(), "Expected empty FTS query, got: {}", fts_query);
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
    assert_eq!(
        ctx.search_results[0].id, expected_id,
        "Result ID mismatch"
    );
}
