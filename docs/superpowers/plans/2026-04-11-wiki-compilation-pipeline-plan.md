# Wiki Compilation Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fill the WikiIngestStage stub with graph-driven wiki compilation so the Dream Pipeline automatically synthesizes discrete facts into structured wiki pages.

**Architecture:** WikiIngestStage scans the knowledge graph for high-value entity nodes, collects associated facts via `memory_entities`, calls LLM to synthesize structured markdown wiki pages, stores them as `FactType::Wiki` facts, and syncs wikilinks back to the graph. Stale wiki pages (where source facts have decayed) are recompiled.

**Tech Stack:** Rust, async_trait, SQLite (graph_nodes + memory_entities tables), LLM via AiProvider trait, existing wiki_sync module.

---

### Task 1: Add `get_high_score_nodes` to GraphStore trait and SQLite implementation

**Files:**
- Modify: `src/memory/store/mod.rs` (GraphStore trait, ~line 345)
- Modify: `src/memory/store/sqlite/graph.rs` (SqliteMemoryBackend impl)

- [ ] **Step 1: Write the failing test in `src/memory/store/sqlite/graph.rs`**

Add at the end of the `#[cfg(test)] mod tests` block:

```rust
#[tokio::test]
async fn test_get_high_score_nodes() {
    let (_tmp, backend) = create_test_backend();

    // Insert nodes with different decay scores
    let mut node1 = make_test_node("gn-high", "Rust", "language");
    node1.decay_score = 0.9;
    backend.upsert_node(&node1, "default").await.unwrap();

    let mut node2 = make_test_node("gn-low", "Python", "language");
    node2.decay_score = 0.2;
    backend.upsert_node(&node2, "default").await.unwrap();

    let mut node3 = make_test_node("gn-mid", "Go", "language");
    node3.decay_score = 0.6;
    backend.upsert_node(&node3, "default").await.unwrap();

    // Query with threshold 0.5 — should return Rust (0.9) and Go (0.6), sorted desc
    let nodes = backend.get_high_score_nodes(0.5, 10, "default").await.unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].name, "Rust");
    assert_eq!(nodes[1].name, "Go");

    // Query with limit 1 — should return only Rust
    let nodes = backend.get_high_score_nodes(0.5, 1, "default").await.unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].name, "Rust");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib store::sqlite::graph::tests::test_get_high_score_nodes 2>&1 | tail -20`

Expected: FAIL — method `get_high_score_nodes` not found

- [ ] **Step 3: Add trait method to `src/memory/store/mod.rs`**

Add after the `delete_memory_entities_by_source` method (around line 441), before the closing `}` of `trait GraphStore`:

```rust
    /// Get graph nodes with decay_score above a threshold, sorted by score descending.
    async fn get_high_score_nodes(
        &self,
        min_score: f32,
        limit: usize,
        workspace: &str,
    ) -> Result<Vec<GraphNode>, AlephError>;
```

- [ ] **Step 4: Add SQLite implementation in `src/memory/store/sqlite/graph.rs`**

Add inside the `impl GraphStore for SqliteMemoryBackend` block, before the closing `}`:

```rust
    async fn get_high_score_nodes(
        &self,
        min_score: f32,
        limit: usize,
        workspace: &str,
    ) -> Result<Vec<GraphNode>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, kind, aliases, metadata, decay_score, created_at, updated_at \
                 FROM graph_nodes \
                 WHERE decay_score > ?1 AND agent = ?2 \
                 ORDER BY decay_score DESC \
                 LIMIT ?3",
            )
            .map_err(|e| AlephError::config(format!("get_high_score_nodes prepare: {e}")))?;

        let rows = stmt
            .query_map(
                params![min_score as f64, workspace, limit as i64],
                |row| {
                    let aliases_json: String = row.get(3)?;
                    let aliases: Vec<String> =
                        serde_json::from_str(&aliases_json).unwrap_or_default();
                    Ok(GraphNode {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        kind: row.get(2)?,
                        aliases,
                        metadata_json: row.get(4)?,
                        decay_score: {
                            let v: f64 = row.get(5)?;
                            v as f32
                        },
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                        agent: workspace.to_string(),
                    })
                },
            )
            .map_err(|e| AlephError::config(format!("get_high_score_nodes query: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("get_high_score_nodes collect: {e}")))?;

        Ok(rows)
    }
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore --lib store::sqlite::graph::tests::test_get_high_score_nodes 2>&1 | tail -20`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/memory/store/mod.rs src/memory/store/sqlite/graph.rs
git commit -m "memory: add get_high_score_nodes to GraphStore trait"
```

---

### Task 2: Extend WikiIngestConfig with compilation parameters

**Files:**
- Modify: `src/memory/dreaming/stages/wiki_ingest.rs`

- [ ] **Step 1: Write the failing test**

Add at the end of the `#[cfg(test)] mod tests` block in `wiki_ingest.rs`:

```rust
#[test]
fn wiki_ingest_config_compile_defaults() {
    let config = WikiIngestConfig::default();
    assert!((config.min_node_score - 0.5).abs() < f32::EPSILON);
    assert_eq!(config.min_facts_for_compile, 3);
    assert!((config.stale_threshold - 0.5).abs() < f32::EPSILON);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dreaming::stages::wiki_ingest::tests::wiki_ingest_config_compile_defaults 2>&1 | tail -20`

Expected: FAIL — fields not found

- [ ] **Step 3: Extend `WikiIngestConfig` struct**

Replace the existing `WikiIngestConfig` struct and `Default` impl in `wiki_ingest.rs`:

```rust
/// Configuration for wiki ingestion during dreams.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiIngestConfig {
    pub enabled: bool,
    pub max_pages_per_run: usize,
    pub cooldown_days: u32,
    /// Minimum entity node decay_score to consider for compilation.
    pub min_node_score: f32,
    /// Minimum number of valid facts needed to trigger wiki compilation.
    pub min_facts_for_compile: usize,
    /// Source-fact validity ratio below which a wiki page is recompiled.
    pub stale_threshold: f32,
}

impl Default for WikiIngestConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_pages_per_run: 10,
            cooldown_days: 1,
            min_node_score: 0.5,
            min_facts_for_compile: 3,
            stale_threshold: 0.5,
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib dreaming::stages::wiki_ingest::tests::wiki_ingest_config_compile_defaults 2>&1 | tail -20`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/stages/wiki_ingest.rs
git commit -m "memory: extend WikiIngestConfig with compilation parameters"
```

---

### Task 3: Implement wiki compilation core logic in WikiIngestStage

**Files:**
- Modify: `src/memory/dreaming/stages/wiki_ingest.rs`

- [ ] **Step 1: Add required imports**

Replace the imports at the top of `wiki_ingest.rs`:

```rust
//! WikiIngestStage: compiles discrete facts into structured wiki pages.
//!
//! Scans the knowledge graph for high-value entity nodes, collects associated
//! facts, calls LLM to synthesize structured markdown wiki pages, stores them
//! as `FactType::Wiki` facts, and syncs wikilinks back to the graph.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::context::{
    FactSource, FactSpecificity, FactType, MemoryFact, MemoryLayer, MemoryScope, MemoryTier,
};
use crate::memory::store::{GraphStore as StoreGraphStore, MemoryStore};
use crate::memory::wiki_sync;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
```

- [ ] **Step 2: Add the prompt builder function**

Add after the `WikiIngestConfig` struct and before the `WikiIngestStage` struct:

```rust
/// Build a prompt for LLM wiki page compilation.
fn build_wiki_compile_prompt(
    entity_name: &str,
    facts: &[&MemoryFact],
    existing_content: Option<&str>,
) -> String {
    let mut prompt = String::new();

    if let Some(existing) = existing_content {
        prompt.push_str(&format!(
            "Update the following wiki page for \"{}\" with new information.\n\n\
             Existing page:\n```markdown\n{}\n```\n\n\
             New facts to integrate:\n",
            entity_name, existing
        ));
    } else {
        prompt.push_str(&format!(
            "Create a wiki page for \"{}\" by synthesizing these facts:\n\n",
            entity_name
        ));
    }

    for (i, fact) in facts.iter().enumerate() {
        prompt.push_str(&format!(
            "{}. [type={}, confidence={:.2}] {}\n",
            i + 1,
            fact.fact_type.as_str(),
            fact.confidence,
            fact.content,
        ));
    }

    prompt.push_str(
        "\nWrite a concise structured markdown page (under 500 words):\n\
         - Start with `# Entity Name`\n\
         - Include a `## Summary` section synthesizing key themes\n\
         - Include a `## Key Facts` section with bullet points\n\
         - Include a `## Related` section with [[wikilinks]] to related entities\n\
         - Note contradictions between facts explicitly\n\
         - Do NOT wrap the output in a code fence\n\
         - Respond ONLY with the markdown content, no preamble\n",
    );

    prompt
}

/// System prompt for wiki compilation.
const WIKI_COMPILE_SYSTEM: &str =
    "You are a knowledge base compiler. Synthesize facts into structured wiki pages. \
     Use [[wikilinks]] to reference related entities. Be concise and factual.";
```

- [ ] **Step 3: Rewrite the `WikiIngestStage` execute method**

Replace the entire `WikiIngestStage` struct and its `DreamStage` impl:

```rust
/// Compiles discrete facts into structured wiki pages via LLM synthesis.
pub struct WikiIngestStage;

#[async_trait]
impl DreamStage for WikiIngestStage {
    fn name(&self) -> &'static str {
        "wiki_ingest"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.provider.is_some()
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let config = WikiIngestConfig::default();
        if !config.enabled {
            return Ok(ctx);
        }

        let provider = match ctx.provider.as_ref() {
            Some(p) => p,
            None => return Ok(ctx),
        };

        // 1. Get high-score entity nodes
        let candidate_nodes = StoreGraphStore::get_high_score_nodes(
            ctx.database.as_ref(),
            config.min_node_score,
            config.max_pages_per_run * 2, // fetch extra to allow filtering
            "default",
        )
        .await?;

        if candidate_nodes.is_empty() {
            debug!("WikiIngestStage: no high-score entity nodes found");
            return Ok(ctx);
        }

        // 2. Collect existing wiki facts for staleness checks
        let all_facts = ctx.database.get_all_facts(false, None).await?;
        let wiki_facts: Vec<&MemoryFact> = all_facts
            .iter()
            .filter(|f| f.fact_type == FactType::Wiki && f.fact_source == FactSource::Synthesis)
            .collect();

        let mut compiled_count: usize = 0;
        let mut recompiled_count: usize = 0;

        for node in &candidate_nodes {
            if compiled_count + recompiled_count >= config.max_pages_per_run {
                break;
            }

            // 3. Get fact IDs linked to this node
            let fact_links = StoreGraphStore::get_facts_for_node(
                ctx.database.as_ref(),
                &node.id,
                "default",
            )
            .await?;

            // 4. Load and filter valid facts
            let mut source_facts: Vec<&MemoryFact> = Vec::new();
            for (fact_id, _weight) in &fact_links {
                if let Some(fact) = all_facts.iter().find(|f| f.id == *fact_id) {
                    if fact.is_valid && fact.fact_type != FactType::Wiki {
                        source_facts.push(fact);
                    }
                }
            }

            if source_facts.len() < config.min_facts_for_compile {
                continue;
            }

            // 5. Check if wiki page already exists for this node
            let existing_wiki = wiki_facts.iter().find(|wf| {
                let slug = wf
                    .path
                    .split('/')
                    .next_back()
                    .unwrap_or("")
                    .trim_end_matches(".md");
                slug.eq_ignore_ascii_case(&slugify(&node.name))
            });

            if let Some(existing) = existing_wiki {
                // Check staleness: what ratio of source_memory_ids are still valid?
                let total_sources = existing.source_memory_ids.len();
                if total_sources > 0 {
                    let valid_sources = existing
                        .source_memory_ids
                        .iter()
                        .filter(|sid| all_facts.iter().any(|f| f.id == **sid && f.is_valid))
                        .count();
                    let valid_ratio = valid_sources as f32 / total_sources as f32;
                    if valid_ratio >= config.stale_threshold {
                        // Page is fresh enough, skip
                        continue;
                    }
                    // Page is stale, recompile
                    debug!(
                        entity = %node.name,
                        valid_ratio = valid_ratio,
                        "WikiIngestStage: recompiling stale wiki page"
                    );
                } else {
                    continue; // No source tracking, skip
                }
            }

            // 6. Call LLM to compile wiki page
            let existing_content = existing_wiki.map(|wf| wf.content.as_str());
            let prompt = build_wiki_compile_prompt(&node.name, &source_facts, existing_content);
            let msgs = [UnifiedMessage::user(&prompt)];
            let payload = RequestPayload::new(&msgs).with_system(Some(WIKI_COMPILE_SYSTEM));

            let content = match provider.process(payload).await {
                Ok(response) => {
                    let text = response.text_content();
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        warn!(entity = %node.name, "WikiIngestStage: LLM returned empty content");
                        continue;
                    }
                    trimmed.to_string()
                }
                Err(e) => {
                    warn!(entity = %node.name, error = %e, "WikiIngestStage: LLM compilation failed");
                    continue;
                }
            };

            let slug = slugify(&node.name);
            let path = format!("aleph://wiki/{}.md", slug);
            let source_ids: Vec<String> = source_facts.iter().map(|f| f.id.clone()).collect();

            // 7. Create or update the wiki fact
            if let Some(existing) = existing_wiki {
                // Update existing fact
                let mut updated = (*existing).clone();
                updated.content = content;
                updated.source_memory_ids = source_ids;
                updated.updated_at = crate::memory::dreaming::now_timestamp();
                ctx.database.update_fact(&updated).await?;

                // Re-sync wikilinks
                wiki_sync::sync_wikilinks_to_graph(&updated, &ctx.graph_store).await?;

                recompiled_count += 1;
            } else {
                // Create new wiki fact
                let wiki_fact = MemoryFact::new(content, FactType::Wiki, source_ids)
                    .with_fact_source(FactSource::Synthesis)
                    .with_tier(MemoryTier::Core)
                    .with_layer(MemoryLayer::L0Abstract)
                    .with_scope(MemoryScope::Global)
                    .with_specificity(FactSpecificity::Abstract)
                    .with_path(&path)
                    .with_confidence(0.8);

                ctx.database.insert_fact(&wiki_fact).await?;

                // Sync wikilinks to graph
                wiki_sync::sync_wikilinks_to_graph(&wiki_fact, &ctx.graph_store).await?;

                // Link wiki fact to entity node via memory_entities
                ctx.graph_store
                    .link_memory_entity(&wiki_fact.id, &node.id, 1.0, "wiki_compile")
                    .await?;

                compiled_count += 1;
            }
        }

        info!(
            compiled = compiled_count,
            recompiled = recompiled_count,
            "WikiIngestStage: wiki compilation complete"
        );

        Ok(ctx)
    }
}

/// Convert an entity name to a URL-safe slug.
fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
```

- [ ] **Step 4: Update tests**

Replace the `#[cfg(test)] mod tests` block with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_ingest_stage_name() {
        assert_eq!(WikiIngestStage.name(), "wiki_ingest");
    }

    #[test]
    fn wiki_ingest_config_defaults() {
        let config = WikiIngestConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_pages_per_run, 10);
        assert_eq!(config.cooldown_days, 1);
    }

    #[test]
    fn wiki_ingest_config_compile_defaults() {
        let config = WikiIngestConfig::default();
        assert!((config.min_node_score - 0.5).abs() < f32::EPSILON);
        assert_eq!(config.min_facts_for_compile, 3);
        assert!((config.stale_threshold - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Rust Ownership"), "rust-ownership");
        assert_eq!(slugify("LLM/GPT-4"), "llm-gpt-4");
        assert_eq!(slugify("hello"), "hello");
        assert_eq!(slugify("  spaces  "), "spaces");
    }

    #[test]
    fn build_prompt_new_page() {
        let fact = MemoryFact::new(
            "Rust uses ownership for memory safety".into(),
            FactType::Learning,
            vec![],
        );
        let facts = vec![&fact];
        let prompt = build_wiki_compile_prompt("Rust", &facts, None);
        assert!(prompt.contains("Create a wiki page"));
        assert!(prompt.contains("Rust"));
        assert!(prompt.contains("ownership for memory safety"));
        assert!(prompt.contains("[[wikilinks]]"));
    }

    #[test]
    fn build_prompt_update_page() {
        let fact = MemoryFact::new("New fact".into(), FactType::Learning, vec![]);
        let facts = vec![&fact];
        let prompt = build_wiki_compile_prompt("Rust", &facts, Some("# Rust\nOld content"));
        assert!(prompt.contains("Update the following wiki page"));
        assert!(prompt.contains("Old content"));
        assert!(prompt.contains("New fact"));
    }
}
```

- [ ] **Step 5: Run tests to verify**

Run: `cargo test -p alephcore --lib dreaming::stages::wiki_ingest 2>&1 | tail -20`

Expected: PASS for all tests

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/stages/wiki_ingest.rs
git commit -m "memory: implement wiki compilation logic in WikiIngestStage"
```

---

### Task 4: Register WikiIngestStage in the Dream Pipeline

**Files:**
- Modify: `src/memory/dreaming/mod.rs` (DreamPipeline::daily, ~line 95)
- Modify: `src/memory/dreaming/stages/mod.rs` (re-exports)

- [ ] **Step 1: Write the failing test**

Update the existing test in `src/memory/dreaming/mod.rs`:

Find the test `test_pipeline_builder`:
```rust
#[test]
fn test_pipeline_builder() {
    let pipeline = DreamPipeline::daily();
    assert_eq!(pipeline.stages.len(), 5);
}
```

Change to:
```rust
#[test]
fn test_pipeline_builder() {
    let pipeline = DreamPipeline::daily();
    assert_eq!(pipeline.stages.len(), 6);
}
```

Also find `test_pipeline_weekly_has_six_stages`:
```rust
#[test]
fn test_pipeline_weekly_has_six_stages() {
    let pipeline = DreamPipeline::weekly();
    assert_eq!(pipeline.stages.len(), 6);
}
```

Change to:
```rust
#[test]
fn test_pipeline_weekly_has_seven_stages() {
    let pipeline = DreamPipeline::weekly();
    assert_eq!(pipeline.stages.len(), 7);
}
```

Also find the async tests `daily_pipeline_has_five_stages` and `weekly_pipeline_has_six_stages` and update:

```rust
#[tokio::test]
async fn daily_pipeline_has_six_stages() {
    let pipeline = DreamPipeline::daily();
    assert_eq!(pipeline.stages.len(), 6);
}

#[tokio::test]
async fn weekly_pipeline_has_seven_stages() {
    let pipeline = DreamPipeline::weekly();
    assert_eq!(pipeline.stages.len(), 7);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib dreaming::tests::test_pipeline_builder 2>&1 | tail -20`

Expected: FAIL — `6 != 5`

- [ ] **Step 3: Add WikiIngestStage re-export to `src/memory/dreaming/stages/mod.rs`**

Add to the re-export block at the bottom:

```rust
pub use wiki_ingest::WikiIngestStage;
```

- [ ] **Step 4: Update DreamPipeline::daily() in `src/memory/dreaming/mod.rs`**

Change `DreamPipeline::daily()` from:

```rust
pub fn daily() -> Self {
    Self::new()
        .stage(SummarizeStage)
        .stage(DriftDetectStage)
        .stage(ConsolidateStage)
        .stage(WikiLintStage)
        .stage(DecayStage)
}
```

To:

```rust
pub fn daily() -> Self {
    Self::new()
        .stage(SummarizeStage)
        .stage(DriftDetectStage)
        .stage(ConsolidateStage)
        .stage(WikiIngestStage)
        .stage(WikiLintStage)
        .stage(DecayStage)
}
```

- [ ] **Step 5: Add WikiIngestStage to the re-exports in `src/memory/dreaming/mod.rs`**

Find the line:

```rust
pub use stages::{
    ConsolidateStage, DecayStage, DeepSynthesisStage, DriftDetectStage, SummarizeStage,
    WikiLintStage,
};
```

Change to:

```rust
pub use stages::{
    ConsolidateStage, DecayStage, DeepSynthesisStage, DriftDetectStage, SummarizeStage,
    WikiIngestStage, WikiLintStage,
};
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib dreaming::tests 2>&1 | tail -30`

Expected: PASS for all pipeline tests

- [ ] **Step 7: Commit**

```bash
git add src/memory/dreaming/mod.rs src/memory/dreaming/stages/mod.rs
git commit -m "memory: register WikiIngestStage in daily Dream Pipeline"
```

---

### Task 5: Enhance WikiLintStage with source-fact staleness detection

**Files:**
- Modify: `src/memory/dreaming/stages/wiki_lint.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `wiki_lint.rs`:

```rust
#[test]
fn wiki_lint_report_has_stale_threshold_field() {
    let report = WikiLintReport::default();
    assert!(report.stale_pages.is_empty());
    // stale_pages should be populated by source-fact validity checks
    let report = WikiLintReport {
        stale_pages: vec!["old-page".to_string()],
        ..Default::default()
    };
    assert_eq!(report.stale_pages.len(), 1);
}
```

- [ ] **Step 2: Run test to verify it passes (structural sanity check)**

Run: `cargo test -p alephcore --lib dreaming::stages::wiki_lint::tests::wiki_lint_report_has_stale_threshold_field 2>&1 | tail -20`

Expected: PASS (the field already exists)

- [ ] **Step 3: Add source-fact staleness detection to `WikiLintStage::execute`**

In `wiki_lint.rs`, find the section after orphan page detection (after the `for slug in &known_slugs` loop) and before the graph-based link suggestions. Add staleness detection:

```rust
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
```

Also add the import for `FactSource` at the top of the file:

```rust
use crate::memory::context::{FactSource, FactType};
```

- [ ] **Step 4: Update the log line to include stale count**

The existing info! log already includes `stale_pages = report.stale_pages.len()` — verify this is the case. No change needed if so.

- [ ] **Step 5: Run all wiki_lint tests**

Run: `cargo test -p alephcore --lib dreaming::stages::wiki_lint 2>&1 | tail -20`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/stages/wiki_lint.rs
git commit -m "memory: add source-fact staleness detection to WikiLintStage"
```

---

### Task 6: Full build verification and integration test

**Files:**
- No new files — verification only

- [ ] **Step 1: Run cargo check**

Run: `cargo check -p alephcore 2>&1 | tail -20`

Expected: no errors

- [ ] **Step 2: Run cargo clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -30`

Expected: no warnings

- [ ] **Step 3: Run all dreaming tests**

Run: `cargo test -p alephcore --lib dreaming 2>&1 | tail -30`

Expected: all tests pass

- [ ] **Step 4: Run all memory tests**

Run: `cargo test -p alephcore --lib memory 2>&1 | tail -40`

Expected: all tests pass

- [ ] **Step 5: Run wiki_sync tests**

Run: `cargo test -p alephcore --lib wiki 2>&1 | tail -20`

Expected: all tests pass

- [ ] **Step 6: Commit (if any clippy fixes were needed)**

```bash
git add -A
git commit -m "memory: fix clippy warnings in wiki compilation pipeline"
```
