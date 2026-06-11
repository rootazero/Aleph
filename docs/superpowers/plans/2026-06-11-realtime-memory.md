# Real-time Memory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Aleph's memory consolidation real-time — notes link to related notes via shared keywords, sessions flush+link on conclude, and errors persist a lesson immediately — instead of waiting for embedding, turn-thresholds, or the nightly dream.

**Architecture:** A single deterministic `KeywordLinker` (LLM extracts an entity/keyword set per note → code pairs notes by set overlap → links written with the connecting keyword as the edge `relation`) is wired into two entrypoints (the ingest creation path and the NoteWeave dream stage). A per-agent flush-state registry drives an async session-end flush with a bounded readiness gate. A prompt-layer nudge empowers the model to write `feedback/lessons` notes the moment it errs. No `src/harness/` changes (R10).

**Tech Stack:** Rust (alephcore), SQLite (`notes_index`/`notes_links`/`notes_fts`), `async_trait`, `RecordingMockProvider` + `MockEmbeddingProvider` for tests, Leptos panel canvas (read-only verification), Playwright for the visual check.

**Spec:** `docs/superpowers/specs/2026-06-11-realtime-memory-design.md`

**Build/test reminder (from CLAUDE.md):** shared target-dir serializes compiles — parallel `cargo` calls queue, that's expected. Run `cargo test -p alephcore --lib <filter>` for unit tests. After backend logic changes that must hit the live daemon: `cargo build --release -p alephcore --bin aleph-server`, then swap the `.app` binary and let the supervisor relaunch.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `src/memory/notes/keyword_linker/mod.rs` | `KeywordLinker` — orchestrates extract → pair → emit triples | Create |
| `src/memory/notes/keyword_linker/overlap.rs` | Pure deterministic pairing (`pair_by_overlap`) | Create |
| `src/memory/notes/keyword_linker/extract.rs` | LLM keyword extraction + prompt | Create |
| `src/memory/notes/keyword_frontmatter.rs` | Parse/serialize the `keywords:` frontmatter field | Create |
| `src/memory/notes/store.rs` | Add `add_link_with_relation` to `NoteStore` | Modify |
| `src/memory/notes/ingest/ingestor.rs` | `enforce_link_contract` → keyword-first + FTS fallback | Modify |
| `src/memory/dreaming/stages/note_weave.rs` | Rework to keyword-overlap relinking | Modify |
| `src/memory/flush/registry.rs` | Per-agent flush-state registry | Create |
| `src/memory/flush/mod.rs` | `session_end_flush` orchestration + readiness gate | Create |
| `src/gateway/session_manager/ops/emit.rs` | Fire `session_end_flush` on conclude | Modify |
| `src/thinker/layers/memory_protocol.rs` | Add lesson-capture nudge | Modify |

---

## PHASE 1 — Keyword Linking

### Task 1: Keyword frontmatter field

**Files:**
- Create: `src/memory/notes/keyword_frontmatter.rs`
- Modify: `src/memory/notes/mod.rs` (add `pub mod keyword_frontmatter;`)

- [ ] **Step 1: Write the failing test**

```rust
// at the bottom of src/memory/notes/keyword_frontmatter.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keywords_line() {
        let fm = "category: entity\nkeywords: [us-iran-conflict, monitoring, ceasefire]\ntags: []\n";
        assert_eq!(
            parse_keywords(fm),
            vec!["us-iran-conflict", "monitoring", "ceasefire"]
        );
    }

    #[test]
    fn missing_keywords_is_empty() {
        assert!(parse_keywords("category: entity\ntags: []\n").is_empty());
    }

    #[test]
    fn serialize_round_trips() {
        let kw = vec!["a".to_string(), "b-c".to_string()];
        let line = serialize_keywords(&kw);
        assert_eq!(line, "keywords: [a, b-c]");
        assert_eq!(parse_keywords(&format!("{line}\n")), kw);
    }

    #[test]
    fn serialize_empty_is_empty_brackets() {
        assert_eq!(serialize_keywords(&[]), "keywords: []");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib keyword_frontmatter`
Expected: FAIL — `cannot find function parse_keywords`.

- [ ] **Step 3: Write minimal implementation**

```rust
//! Parse/serialize the note frontmatter `keywords:` field.
//!
//! Format mirrors the existing `tags:` line: `keywords: [a, b, c]`. Values are
//! lowercase kebab/plain tokens; commas separate, surrounding whitespace and
//! brackets are stripped. Empty list serializes as `keywords: []`.

/// Extract the keyword list from a frontmatter string. Returns empty when the
/// `keywords:` line is absent or empty.
pub fn parse_keywords(frontmatter: &str) -> Vec<String> {
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("keywords:") {
            let inner = rest.trim().trim_start_matches('[').trim_end_matches(']');
            return inner
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
    }
    Vec::new()
}

/// Render a `keywords: [a, b]` frontmatter line (no trailing newline).
pub fn serialize_keywords(keywords: &[String]) -> String {
    format!("keywords: [{}]", keywords.join(", "))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib keyword_frontmatter`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/keyword_frontmatter.rs src/memory/notes/mod.rs
git commit -m "memory: add note keywords frontmatter parse/serialize"
```

---

### Task 2: Deterministic overlap pairing

**Files:**
- Create: `src/memory/notes/keyword_linker/overlap.rs`
- Create: `src/memory/notes/keyword_linker/mod.rs` (module wiring only this task)
- Modify: `src/memory/notes/mod.rs` (add `pub mod keyword_linker;`)

The pairing rule (spec 1.3): two notes link when their keyword sets share **≥1
specific entity** OR **≥2 generic keywords**. A "specific entity" is a
multi-token keyword (contains `-` or a space → e.g. `us-iran-conflict`); a
single bare token (e.g. `news`) is generic. The connecting keyword (the most
specific shared one) becomes the edge `relation`.

- [ ] **Step 1: Write the failing test**

```rust
// bottom of src/memory/notes/keyword_linker/overlap.rs
#[cfg(test)]
mod tests {
    use super::*;

    fn kw(path: &str, words: &[&str]) -> NoteKeywords {
        NoteKeywords {
            path: path.to_string(),
            keywords: words.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn links_on_one_shared_specific_entity() {
        let notes = vec![
            kw("entity/us-iran-conflict-2026", &["us-iran-conflict", "ceasefire"]),
            kw("personal/news-monitoring", &["us-iran-conflict", "cron"]),
        ];
        let links = pair_by_overlap(&notes);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].from, "entity/us-iran-conflict-2026");
        assert_eq!(links[0].to, "personal/news-monitoring");
        assert_eq!(links[0].relation, "us-iran-conflict");
    }

    #[test]
    fn no_link_on_single_generic_keyword() {
        let notes = vec![
            kw("a/x", &["news", "alpha"]),
            kw("a/y", &["news", "beta"]),
        ];
        assert!(pair_by_overlap(&notes).is_empty());
    }

    #[test]
    fn links_on_two_shared_generic_keywords() {
        let notes = vec![
            kw("a/x", &["news", "finance", "alpha"]),
            kw("a/y", &["news", "finance", "beta"]),
        ];
        let links = pair_by_overlap(&notes);
        assert_eq!(links.len(), 1);
        // relation is the lexicographically-first shared keyword when none is specific
        assert_eq!(links[0].relation, "finance");
    }

    #[test]
    fn no_self_link_and_pairs_are_unordered_unique() {
        let notes = vec![
            kw("a/x", &["topic-one"]),
            kw("a/y", &["topic-one"]),
            kw("a/z", &["topic-one"]),
        ];
        let links = pair_by_overlap(&notes);
        // 3 nodes all sharing one specific entity → 3 undirected pairs
        assert_eq!(links.len(), 3);
        assert!(links.iter().all(|l| l.from != l.to));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib keyword_linker::overlap`
Expected: FAIL — `cannot find type NoteKeywords`.

- [ ] **Step 3: Write minimal implementation**

```rust
//! Deterministic note pairing by keyword-set overlap. No LLM, no embedding.

use std::collections::BTreeSet;

/// A note's path plus its extracted keyword set.
#[derive(Debug, Clone)]
pub struct NoteKeywords {
    pub path: String,
    pub keywords: Vec<String>,
}

/// An undirected link candidate with the connecting keyword as `relation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTriple {
    pub from: String,
    pub to: String,
    pub relation: String,
}

/// A keyword is "specific" when it names an entity/multi-token concept —
/// heuristically, it contains a `-` or whitespace (e.g. `us-iran-conflict`).
fn is_specific(keyword: &str) -> bool {
    keyword.contains('-') || keyword.contains(char::is_whitespace)
}

/// Pair every note against every other; emit a link when their keyword sets
/// share ≥1 specific entity OR ≥2 generic keywords. The connecting keyword
/// (most specific shared one, else lexicographically-first) is the relation.
/// Pairs are undirected and unique (i<j), no self-links.
pub fn pair_by_overlap(notes: &[NoteKeywords]) -> Vec<LinkTriple> {
    let sets: Vec<BTreeSet<&str>> = notes
        .iter()
        .map(|n| n.keywords.iter().map(String::as_str).collect())
        .collect();
    let mut out = Vec::new();
    for i in 0..notes.len() {
        for j in (i + 1)..notes.len() {
            let shared: Vec<&str> = sets[i].intersection(&sets[j]).copied().collect();
            if shared.is_empty() {
                continue;
            }
            let specific: Vec<&str> = shared.iter().copied().filter(|s| is_specific(s)).collect();
            let connects = if !specific.is_empty() {
                // most specific = longest token
                specific.iter().copied().max_by_key(|s| s.len())
            } else if shared.len() >= 2 {
                shared.iter().copied().min() // lexicographically first
            } else {
                None
            };
            if let Some(relation) = connects {
                out.push(LinkTriple {
                    from: notes[i].path.clone(),
                    to: notes[j].path.clone(),
                    relation: relation.to_string(),
                });
            }
        }
    }
    out
}
```

Then `src/memory/notes/keyword_linker/mod.rs`:

```rust
//! Keyword-based note linking: LLM extracts a keyword set per note, code pairs
//! notes by set overlap (see `overlap`), links carry the connecting keyword.

pub mod overlap;

pub use overlap::{pair_by_overlap, LinkTriple, NoteKeywords};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib keyword_linker::overlap`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/keyword_linker/ src/memory/notes/mod.rs
git commit -m "memory: add deterministic keyword-overlap note pairing"
```

---

### Task 3: LLM keyword extraction

**Files:**
- Create: `src/memory/notes/keyword_linker/extract.rs`
- Modify: `src/memory/notes/keyword_linker/mod.rs` (add `pub mod extract;`)

Extraction asks the LLM for a 3–6 keyword set per note in one batched call.
Reuses `extract_json_robust` and `RecordingMockProvider` (see `ingestor.rs`
tests for the pattern).

- [ ] **Step 1: Write the failing test**

```rust
// bottom of src/memory/notes/keyword_linker/extract.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::recording_mock::RecordingMockProvider;
    use crate::sync_primitives::Arc;

    #[tokio::test]
    async fn extracts_keyword_sets_per_note() {
        let provider: Arc<dyn crate::providers::AiProvider> =
            Arc::new(RecordingMockProvider::new(
                r#"{"notes":[
                    {"path":"entity/us-iran-conflict-2026","keywords":["us-iran-conflict","ceasefire","monitoring"]},
                    {"path":"personal/news-monitoring","keywords":["us-iran-conflict","cron","news"]}
                ]}"#
                .into(),
            ));
        let inputs = vec![
            NoteForExtraction { path: "entity/us-iran-conflict-2026".into(), title: "US-Iran".into(), summary: "tensions".into(), facts: vec![] },
            NoteForExtraction { path: "personal/news-monitoring".into(), title: "News".into(), summary: "cron".into(), facts: vec![] },
        ];
        let out = extract_keywords(&*provider, &inputs).await.unwrap();
        assert_eq!(out.len(), 2);
        assert!(out[0].keywords.contains(&"us-iran-conflict".to_string()));
    }

    #[tokio::test]
    async fn malformed_json_yields_empty() {
        let provider: Arc<dyn crate::providers::AiProvider> =
            Arc::new(RecordingMockProvider::new("not json".into()));
        let inputs = vec![NoteForExtraction { path: "a/x".into(), title: "X".into(), summary: String::new(), facts: vec![] }];
        let out = extract_keywords(&*provider, &inputs).await.unwrap();
        assert!(out.is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib keyword_linker::extract`
Expected: FAIL — `cannot find type NoteForExtraction`.

- [ ] **Step 3: Write minimal implementation**

```rust
//! LLM keyword/entity extraction for note linking. One batched call.

use crate::error::AlephError;
use crate::memory::notes::keyword_linker::overlap::NoteKeywords;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::utils::json_extract::extract_json_robust;
use tracing::warn;

/// One note's salient text for keyword extraction.
pub struct NoteForExtraction {
    pub path: String,
    pub title: String,
    pub summary: String,
    pub facts: Vec<String>,
}

const SYSTEM: &str = "You extract a compact keyword/entity set for each note so \
related notes can be linked. For every note return 3-6 keywords: prefer specific \
named entities (people, orgs, projects, events — e.g. \"us-iran-conflict\") as \
lowercase kebab-case; include a few generic topic words too. Output JSON only: \
{\"notes\":[{\"path\":\"<path>\",\"keywords\":[\"...\"]}]}. Use the exact path given.";

/// Extract keyword sets for a batch of notes. Returns one `NoteKeywords` per
/// note the LLM returned; degrades to empty on malformed output (P7 — linking
/// is an enhancement, never block).
pub async fn extract_keywords(
    provider: &dyn AiProvider,
    notes: &[NoteForExtraction],
) -> Result<Vec<NoteKeywords>, AlephError> {
    if notes.is_empty() {
        return Ok(vec![]);
    }
    let mut user = String::from("## Notes\n\n");
    for n in notes {
        user.push_str(&format!("### path={}\ntitle: {}\nsummary: {}\n", n.path, n.title, n.summary));
        for f in n.facts.iter().take(6) {
            user.push_str(&format!("- {f}\n"));
        }
        user.push('\n');
    }
    let msgs = [UnifiedMessage::user(&user)];
    let resp = provider
        .process(RequestPayload::new(&msgs).with_system(Some(SYSTEM)))
        .await
        .map_err(|e| AlephError::other(format!("keyword extract LLM: {e}")))?;
    let Some(json) = extract_json_robust(&resp.text_content()) else {
        warn!("keyword extract: no JSON in response; returning empty");
        return Ok(vec![]);
    };
    let out = json
        .get("notes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|n| {
                    let path = n.get("path")?.as_str()?.to_string();
                    let keywords = n
                        .get("keywords")?
                        .as_array()?
                        .iter()
                        .filter_map(|k| k.as_str().map(str::to_string))
                        .collect();
                    Some(NoteKeywords { path, keywords })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(out)
}
```

Add to `keyword_linker/mod.rs`:

```rust
pub mod extract;
pub use extract::{extract_keywords, NoteForExtraction};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib keyword_linker::extract`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/keyword_linker/
git commit -m "memory: add LLM keyword extraction for note linking"
```

---

### Task 4: `add_link_with_relation` store method

**Files:**
- Modify: `src/memory/notes/store.rs` (trait `NoteStore` + the SQLite impl)
- Test: `src/memory/notes/store.rs` (`#[cfg(test)]`)

The plain-link path leaves `notes_links.relation` NULL. Add a method that sets
it via an UPSERT so the keyword link carries its connecting keyword. Call it
AFTER the body `[[ ]]` link is written (so the row already exists from indexing).

- [ ] **Step 1: Write the failing test**

```rust
// inside the existing #[cfg(test)] mod in store.rs
#[tokio::test]
async fn add_link_with_relation_sets_relation_column() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap();
    // index two notes so paths exist
    store.index_note(&sample_note("A", "cat", vec![]), "default", "cat").await.unwrap();
    store.index_note(&sample_note("B", "cat", vec![]), "default", "cat").await.unwrap();
    store.add_link_with_relation("default", "cat/a", "cat/b", "shared-topic").await.unwrap();
    let links = store.get_outgoing_links("cat/a", "default").await.unwrap();
    assert!(links.iter().any(|l| l.relation.as_deref() == Some("shared-topic")));
}
```

(Confirm the link DTO field name via `get_outgoing_links`' return type at
`store.rs:89`; if it is not `relation: Option<String>`, adapt the assertion.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib store::tests::add_link_with_relation_sets_relation_column`
Expected: FAIL — `no method named add_link_with_relation`.

- [ ] **Step 3: Write minimal implementation**

Add to the `NoteStore` trait (near `get_outgoing_links`, `store.rs:89`):

```rust
/// Upsert a directed link `from -> to` and set its `relation` label.
/// Used by keyword linking after the body `[[ ]]` link is written.
async fn add_link_with_relation(
    &self,
    agent_id: &str,
    from_note: &str,
    to_note: &str,
    relation: &str,
) -> Result<(), AlephError>;
```

In the SQLite impl (mirror an existing `INSERT ... ON CONFLICT` in this file):

```rust
async fn add_link_with_relation(
    &self,
    agent_id: &str,
    from_note: &str,
    to_note: &str,
    relation: &str,
) -> Result<(), AlephError> {
    let conn = self.conn()?; // mirror however other methods acquire the connection
    conn.execute(
        "INSERT INTO notes_links (agent_id, from_note, to_note, to_raw, relation)
         VALUES (?1, ?2, ?3, ?3, ?4)
         ON CONFLICT(agent_id, from_note, to_note)
         DO UPDATE SET relation = excluded.relation",
        rusqlite::params![agent_id, from_note, to_note, relation],
    )?;
    Ok(())
}
```

(If the test backend uses a connection pool/`spawn_blocking`, follow the exact
pattern of the neighbouring write method — do not invent a `conn()` accessor.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib store::tests::add_link_with_relation_sets_relation_column`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/store.rs
git commit -m "memory: NoteStore::add_link_with_relation for keyword edges"
```

---

### Task 5: Keyword-first creation path (with FTS fallback)

**Files:**
- Modify: `src/memory/notes/ingest/ingestor.rs` (`enforce_link_contract` + a new `keyword_candidates` helper)
- Test: `src/memory/notes/ingest/ingestor.rs` (`#[cfg(test)]`)

Rework `enforce_link_contract` (ingestor.rs:674) so candidate gathering does
NOT depend on a non-empty embedding-derived `related`. When `related` is empty,
pull candidates from `notes_fts` by the new note's title/summary tokens, run
`KeywordLinker` extraction over (new notes + candidates), pair, and merge links
into the `Create.links` + emit relations.

- [ ] **Step 1: Write the failing test (embedding-down still links)**

```rust
// in the plan_tests / a new mod in ingestor.rs
#[tokio::test]
async fn enforce_link_contract_links_via_keywords_when_related_empty() {
    let (dir, backend, indexer) = mk().await;
    // Seed an existing note so FTS has a candidate.
    backend
        .index_note(&sample_note_with_facts("news-monitoring", "personal",
            vec!["Daily US-Iran conflict news summaries"]), "default", "personal")
        .await.unwrap();
    // Planner LLM call #1 returns a linkless create; extraction call #2 returns
    // overlapping keywords. RecordingMock returns queued responses in order.
    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::with_queue(vec![
        r#"{"notes":[
            {"path":"entity/us-iran-conflict-2026","keywords":["us-iran-conflict","monitoring"]},
            {"path":"personal/news-monitoring","keywords":["us-iran-conflict","cron"]}
        ]}"#.into(),
    ]));
    let ing = ingestor_with(&dir, backend.clone(), indexer, provider);
    let ops = vec![PageOp::Create {
        note_path: "entity/us-iran-conflict-2026".into(),
        title: "US-Iran Conflict".into(),
        summary: "tensions monitored".into(),
        facts: vec!["US-Iran conflict monitoring".into()],
        links: vec![],
        tags: vec![],
        relations: vec![],
    }];
    let out = ing.enforce_link_contract(ops, &[]).await; // related is EMPTY
    match &out[0] {
        PageOp::Create { links, .. } => {
            assert!(links.iter().any(|l| l == "personal/news-monitoring"),
                "keyword overlap must link the create even with empty related");
        }
        _ => panic!(),
    }
}
```

(`RecordingMockProvider::with_queue` / `ingestor_with` / `sample_note_with_facts`
are small test helpers — add them next to the existing `mk()` helper, mirroring
the existing `DefaultCompoundIngestor { … }` struct literal used throughout the
test module. If `RecordingMockProvider` lacks a queue constructor, add one that
pops responses FIFO.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib enforce_link_contract_links_via_keywords_when_related_empty`
Expected: FAIL — the create stays linkless (current code early-returns when `related` is empty at ingestor.rs:679).

- [ ] **Step 3: Write minimal implementation**

Replace the early-return-on-empty-related guard in `enforce_link_contract` with
a keyword path. Add a helper that gathers FTS candidates and runs the linker:

```rust
use crate::memory::notes::keyword_linker::{extract_keywords, pair_by_overlap, NoteForExtraction, NoteKeywords};

impl<S: NoteStore + Send + Sync + 'static> DefaultCompoundIngestor<S> {
    /// Keyword-overlap linking for `Create` ops that the embedding-based
    /// `related` set left unlinked. Pulls candidates from FTS, extracts
    /// keyword sets for (new creates + candidates), pairs, and merges links.
    async fn keyword_link_creates(&self, mut ops: Vec<PageOp>) -> Vec<PageOp> {
        // Collect linkless creates.
        let targets: Vec<(usize, NoteForExtraction)> = ops.iter().enumerate().filter_map(|(i, op)| {
            if let PageOp::Create { note_path, title, summary, facts, links, relations, .. } = op {
                if links.is_empty() && relations.is_empty() {
                    return Some((i, NoteForExtraction {
                        path: note_path.clone(), title: title.clone(),
                        summary: summary.clone(), facts: facts.clone(),
                    }));
                }
            }
            None
        }).collect();
        if targets.is_empty() {
            return ops;
        }
        // FTS candidates: query each create's title/summary, dedup by path,
        // skip paths that are themselves in this batch.
        let batch_paths: std::collections::HashSet<&str> =
            targets.iter().map(|(_, n)| n.path.as_str()).collect();
        let mut candidates: Vec<NoteForExtraction> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (_, n) in &targets {
            let q = format!("{} {}", n.title, n.summary);
            // search_notes_fts already splits the query into keywords internally
            // (see note_manage create round). Cap a few per create.
            if let Ok(hits) = self.store.search_notes_fts(&q, "", 5).await {
                for h in hits {
                    if batch_paths.contains(h.path.as_str()) || !seen.insert(h.path.clone()) {
                        continue;
                    }
                    candidates.push(NoteForExtraction {
                        path: h.path, title: h.title, summary: h.summary, facts: vec![],
                    });
                }
            }
        }
        // Extract keyword sets for the union, pair, merge links onto creates.
        let mut all: Vec<NoteForExtraction> = targets.iter().map(|(_, n)| NoteForExtraction {
            path: n.path.clone(), title: n.title.clone(), summary: n.summary.clone(), facts: n.facts.clone(),
        }).collect();
        all.extend(candidates);
        let kw: Vec<NoteKeywords> = match extract_keywords(&*self.provider, &all).await {
            Ok(k) if !k.is_empty() => k,
            _ => return ops, // degrade: no keywords → leave creates as-is
        };
        let links = pair_by_overlap(&kw);
        for (idx, n) in &targets {
            for l in &links {
                let other = if l.from == n.path { Some(&l.to) }
                            else if l.to == n.path { Some(&l.from) } else { None };
                if let (Some(other), PageOp::Create { links, .. }) = (other, &mut ops[*idx]) {
                    if !links.contains(other) {
                        links.push(other.clone());
                    }
                }
            }
        }
        ops
    }
}
```

In `enforce_link_contract`, change the head from:

```rust
if related.is_empty() {
    return ops;
}
```

to:

```rust
if related.is_empty() {
    // Embedding-derived related set is empty (sparse wiki or embedding down):
    // fall back to keyword-overlap linking via FTS candidates instead of
    // leaving every create an orphan.
    return self.keyword_link_creates(ops).await;
}
```

(The existing non-empty-`related` body keeps the `[P<n>]` repair path. Confirm
`search_notes_fts` signature at the call site in `note_manage`/`retrieval` and
adapt args; the third arg is a limit.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib enforce_link_contract_links_via_keywords_when_related_empty`
Expected: PASS.

- [ ] **Step 5: Run the whole ingestor test module (no regressions)**

Run: `cargo test -p alephcore --lib notes::ingest::ingestor`
Expected: PASS (all existing tests still green).

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/ingest/ingestor.rs
git commit -m "memory: keyword-first link contract with FTS fallback when embedding is empty"
```

---

### Task 6: Rework NoteWeave dream stage to keyword overlap

**Files:**
- Modify: `src/memory/dreaming/stages/note_weave.rs`
- Test: `src/memory/dreaming/stages/note_weave.rs` (`#[cfg(test)]`)

Reuse `KeywordLinker`: enumerate all notes for the agent, build
`NoteForExtraction` from the index + bodies, `extract_keywords`,
`pair_by_overlap`, then for each pair write the body `[[ ]]` link both
directions (existing `indexer.append_to_note`) and set the relation via
`store.add_link_with_relation`. Keep the existing orphan-first ordering (link
orphans before well-connected notes) and the per-run cap from the current stage.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn note_weave_links_orphans_by_keyword_overlap() {
    // Build a store with two orphan notes that share a specific entity.
    // Drive the stage with a RecordingMockProvider returning overlapping keywords.
    // Assert notes_links is non-empty and relation is set after run.
    // (Mirror the existing note_weave test harness in this file.)
}
```

Fill the body using this file's existing test scaffolding (it already
constructs a store + `DreamContext`). Assert
`store.get_outgoing_links(...)` returns a link with `relation = Some(_)`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib note_weave_links_orphans_by_keyword_overlap`
Expected: FAIL (stage still does embedding-orphan logic).

- [ ] **Step 3: Replace the stage body**

Swap the embedding candidate-gathering for the keyword path. Keep the
`DreamStage` trait impl signature and the cap constant. Pseudocode of the new
`run` body:

```rust
// 1. list all notes for agent (existing store.list / get_graph_data)
// 2. read each note body → NoteForExtraction (title/summary/facts from index)
// 3. let kw = extract_keywords(provider, &inputs).await?;  (skip if empty)
// 4. let links = pair_by_overlap(&kw); cap at MAX_WEAVE_LINKS
// 5. for each LinkTriple: indexer.append_to_note(agent, from, &[], &[to]);
//    indexer.append_to_note(agent, to, &[], &[from]);  // bidirectional
//    store.add_link_with_relation(agent, &from, &to, &relation).await.ok();
//    store.add_link_with_relation(agent, &to, &from, &relation).await.ok();
// 6. record woven count in the DreamReport field the stage already populates
```

Preserve the existing per-direction failure tolerance (commit `5bf273539`): each
`add_link_with_relation` / `append_to_note` is independently `.ok()`-guarded so
one failing direction doesn't abort the rest.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib note_weave`
Expected: PASS (new test + existing stage tests; update any test asserting the old embedding behaviour).

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/stages/note_weave.rs
git commit -m "memory: NoteWeave relinks orphans by keyword overlap (reuses KeywordLinker)"
```

---

### Task 7: Backfill the live 17 notes (manual ops)

**Files:** none (operational).

- [ ] **Step 1: Rebuild the daemon binary**

Run: `cargo build --release -p alephcore --bin aleph-server`
Expected: clean build.

- [ ] **Step 2: Swap the running `.app` binary and relaunch**

```bash
mv /Applications/Aleph.app/Contents/MacOS/aleph-server{,.bak}
cp target/release/aleph-server /Applications/Aleph.app/Contents/MacOS/
kill $(pgrep -f 'Aleph.app/Contents/MacOS/aleph-server')   # supervisor relaunches
```

- [ ] **Step 3: Force one dream cycle (runs reworked NoteWeave)**

Trigger the gateway `dreaming` RPC that calls `try_run_now()` (the handler at
`src/gateway/handlers/dreaming.rs:29`). From the panel command palette or:

```bash
# via the same JSON-RPC the panel uses; method per dreaming handler registration
aleph dream run   # if a CLI subcommand exists; else invoke the RPC method directly
```

- [ ] **Step 4: Verify links landed**

```bash
sqlite3 ~/.aleph/data/memory.db \
  "SELECT from_note, to_note, relation FROM notes_links WHERE agent_id='main' ORDER BY from_note;"
```
Expected: rows connecting the geopolitics cluster (us-iran-conflict ↔
news-monitoring ↔ geopolitical-monitoring ↔ news-summary-cron ↔ us-stock-crash)
and the Dreame cluster (dreame-report ↔ entity/dreame), each with a non-NULL
`relation`.

- [ ] **Step 5: (no commit — operational step)**

---

### Task 8: Canvas verification via Playwright (manual)

**Files:** none (verification). Uses the `webapp-testing` / Playwright skill.

- [ ] **Step 1: Get an authenticated panel URL**

Run: `/Applications/Aleph.app/Contents/MacOS/aleph-server bootstrap-url`
Expected: a `http://127.0.0.1:PORT/auth/bootstrap?nonce=…` URL.

- [ ] **Step 2: Open the panel memory/graph mode and screenshot**

Drive Playwright to the bootstrap URL, switch to the memory graph canvas
(`graph.query` view), screenshot.
Expected: the 17 nodes render with edges connecting the two clusters (the
canvas reads edges from `notes_links` — confirmed in `gateway/handlers/graph.rs`).

- [ ] **Step 3: (no commit — verification)**

---

## PHASE 2 — Real-time Session-end Flush

### Task 9: Per-agent flush-state registry

**Files:**
- Create: `src/memory/flush/registry.rs`
- Create: `src/memory/flush/mod.rs` (`pub mod registry;`)
- Modify: `src/memory/mod.rs` (`pub mod flush;`)

Mirrors the process-global session-keyed registry pattern
(`scratchpad_registry`). Tracks per-agent flush state: `InProgress(Notify)` or
`Idle`. A waiter awaits the `Notify` with a bounded timeout.

- [ ] **Step 1: Write the failing test**

```rust
// bottom of src/memory/flush/registry.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn await_ready_returns_immediately_when_idle() {
        let reg = FlushRegistry::new();
        // No flush started → ready right away.
        let waited = reg.await_ready("main", Duration::from_millis(200)).await;
        assert!(waited, "idle agent is immediately ready");
    }

    #[tokio::test]
    async fn await_ready_blocks_until_flush_done() {
        let reg = FlushRegistry::new();
        let guard = reg.begin("main");
        let reg2 = reg.clone();
        let h = tokio::spawn(async move {
            reg2.await_ready("main", Duration::from_secs(2)).await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        guard.finish(); // flush completed
        assert!(h.await.unwrap(), "waiter unblocks once flush finishes");
    }

    #[tokio::test]
    async fn await_ready_times_out_if_flush_hangs() {
        let reg = FlushRegistry::new();
        let _guard = reg.begin("main"); // never finishes
        let waited = reg.await_ready("main", Duration::from_millis(100)).await;
        assert!(!waited, "bounded wait gives up");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib flush::registry`
Expected: FAIL — `cannot find type FlushRegistry`.

- [ ] **Step 3: Write minimal implementation**

```rust
//! Per-agent flush-state registry. A session-end flush registers itself here;
//! a follow-on session's `await_ready` blocks (bounded) until it finishes, so a
//! fast back-to-back session sees consolidated memory while a normal session
//! never waits.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::Notify;

use crate::sync_primitives::Arc;

#[derive(Clone, Default)]
pub struct FlushRegistry {
    inner: Arc<Mutex<HashMap<String, Arc<Notify>>>>,
}

/// Dropped/`finish()`ed when a flush completes — wakes all waiters.
pub struct FlushGuard {
    notify: Arc<Notify>,
    reg: FlushRegistry,
    agent: String,
}

impl FlushGuard {
    pub fn finish(self) { /* Drop does the work */ }
}

impl Drop for FlushGuard {
    fn drop(&mut self) {
        self.reg
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.agent);
        self.notify.notify_waiters();
    }
}

impl FlushRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a flush in progress for `agent`. Hold the guard for the flush
    /// duration; drop/`finish()` when done.
    pub fn begin(&self, agent: &str) -> FlushGuard {
        let notify = Arc::new(Notify::new());
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(agent.to_string(), notify.clone());
        FlushGuard { notify, reg: self.clone(), agent: agent.to_string() }
    }

    /// Wait until `agent` has no in-progress flush, or `timeout` elapses.
    /// Returns `true` if ready within the window, `false` on timeout.
    pub async fn await_ready(&self, agent: &str, timeout: Duration) -> bool {
        let notify = {
            let map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            match map.get(agent) {
                Some(n) => n.clone(),
                None => return true, // idle → ready
            }
        };
        tokio::time::timeout(timeout, notify.notified()).await.is_ok()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib flush::registry`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/memory/flush/ src/memory/mod.rs
git commit -m "memory: add per-agent flush-state registry with bounded readiness gate"
```

---

### Task 10: Session-end flush orchestration + wiring

**Files:**
- Modify: `src/memory/flush/mod.rs` (add `session_end_flush`)
- Modify: `src/gateway/session_manager/ops/emit.rs` (fire it on conclude, ~line 68)
- Test: `src/memory/flush/mod.rs` (`#[cfg(test)]`)

`session_end_flush(agent)` registers a `FlushGuard`, runs
`CompressionService::compress_to_notes(agent)` (drains pending raws → notes,
which now keyword-link via Task 5), and drops the guard. Spawned, not awaited.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn session_end_flush_compresses_pending_raw_into_a_note() {
    // Build a CompressionService over a temp store with one pending raw memory.
    // Run session_end_flush(agent) and await the spawned handle in the test.
    // Assert notes_index gained a row for the agent (compression ran without a
    // 20-turn threshold or dream cycle).
    // (Mirror compress_to_notes tests in src/memory/compression/service.rs.)
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib session_end_flush_compresses_pending_raw_into_a_note`
Expected: FAIL — `cannot find function session_end_flush`.

- [ ] **Step 3: Write minimal implementation**

```rust
// src/memory/flush/mod.rs
pub mod registry;
pub use registry::{FlushGuard, FlushRegistry};

use crate::sync_primitives::Arc;
use crate::memory::compression::CompressionService;
use tracing::warn;

/// Run an immediate compress→link flush for `agent`, guarded in `reg` so a
/// follow-on session can await readiness. Intended to be `tokio::spawn`ed at
/// session conclude (async, non-blocking — Pillar 2 "async with readiness gate").
pub async fn session_end_flush(
    reg: FlushRegistry,
    agent: String,
    compression: Arc<CompressionService>,
) {
    let _guard = reg.begin(&agent);
    if let Err(e) = compression.compress_to_notes(&agent).await {
        warn!(agent = %agent, error = %e, "session_end_flush: compress_to_notes failed");
    }
    // _guard drops here → wakes any waiter.
}
```

Wire it at `emit.rs` where the session-end MCP currently fires (~line 68). After
the existing `session_end_mcp()` call, spawn the flush using the process-global
`FlushRegistry` and the engine's `CompressionService` (thread a single shared
`FlushRegistry` through `AppContext`, constructed once at startup — mirror how
`scratchpad_registry` is reached). Guard on `compression_service.is_some()`.

```rust
if let Some(cs) = compression_service_handle() {
    let reg = crate::memory::flush::global_registry();
    let agent_id = agent_id.to_string();
    tokio::spawn(crate::memory::flush::session_end_flush(reg, agent_id, cs));
}
```

(Add a `global_registry()` process-global accessor in `flush/mod.rs` mirroring
`goal::global()` / `scratchpad_registry`. If the gateway already exposes the
`CompressionService` via context here, use that instead of a new accessor —
follow the existing reach pattern at this call site.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib session_end_flush_compresses_pending_raw_into_a_note`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/memory/flush/mod.rs src/gateway/session_manager/ops/emit.rs
git commit -m "memory: async session-end flush (compress+link) on session conclude"
```

---

### Task 11: Readiness gate on session start

**Files:**
- Modify: the session/context assembly entrypoint (`src/memory/assembler/gather.rs` or the first-turn context build) — locate the agent's context-gather start.
- Test: same file (`#[cfg(test)]`)

Before assembling memory context for a new session, call
`FlushRegistry::await_ready(agent, BOUND)` (e.g. `Duration::from_secs(2)`) so a
fast follow-on session sees the prior session's consolidated notes. A normal
session (no in-progress flush) returns immediately.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn context_gather_awaits_in_progress_flush() {
    let reg = FlushRegistry::new();
    let guard = reg.begin("main");
    // Start gather in a task; it must not complete its readiness wait until the
    // flush guard drops.
    let reg2 = reg.clone();
    let started = std::time::Instant::now();
    let h = tokio::spawn(async move {
        reg2.await_ready("main", std::time::Duration::from_secs(2)).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    drop(guard);
    assert!(h.await.unwrap());
    assert!(started.elapsed() >= std::time::Duration::from_millis(80));
}
```

(This validates the gate primitive at the gather boundary. The wiring step
inserts the `await_ready` call into the real gather path.)

- [ ] **Step 2: Run test to verify it fails (then wire)**

Run: `cargo test -p alephcore --lib context_gather_awaits_in_progress_flush`
Expected: PASS for the primitive; the wiring is verified by reading the gather
path and inserting the call before context assembly. Add the call:

```rust
// at the top of the new-session context gather, before reading notes/facts:
crate::memory::flush::global_registry()
    .await_ready(agent_id, std::time::Duration::from_secs(2))
    .await;
```

- [ ] **Step 3: Verify no regression in gather tests**

Run: `cargo test -p alephcore --lib memory::assembler`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/memory/assembler/gather.rs
git commit -m "memory: gate new-session context gather on in-progress flush readiness"
```

---

## PHASE 3 — Error → Immediate Lesson Capture

### Task 12: Lesson-capture nudge in MemoryProtocolLayer

**Files:**
- Modify: `src/thinker/layers/memory_protocol.rs` (extend `inject`)
- Test: `src/thinker/layers/memory_protocol.rs` (`#[cfg(test)]`)

Add a soft nudge (R8 — nudge not rule): on recognizing an error (its own or a
user correction), the model immediately writes a `feedback/lessons` note via
`note_manage` with the cause and how to avoid it.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod lesson_nudge_tests {
    use super::*;
    use crate::thinker::prompt_layer::{LayerInput, PromptLayer};

    #[test]
    fn injects_lesson_capture_nudge() {
        let mut out = String::new();
        MemoryProtocolLayer.inject(&mut out, &LayerInput::default());
        assert!(out.contains("feedback/lessons"),
            "must teach immediate lesson capture on error");
        assert!(out.contains("note_manage"));
    }
}
```

(If `LayerInput::default()` is not available, construct the minimal `LayerInput`
the existing tests in `src/thinker/layers/` use.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib memory_protocol`
Expected: FAIL — current text has no `feedback/lessons`.

- [ ] **Step 3: Append the nudge in `inject`**

After the existing three-tool block, append:

```rust
output.push_str(
    "\nWhen you recognize a mistake — your own or one the user corrected — \
     record the lesson immediately with `note_manage` (create a \
     `feedback/lessons` note): state the cause (why it happened) and how to \
     avoid it next time. Don't wait for the session to end; a durable lesson \
     written now is recalled (and linked) for the next session.\n",
);
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib memory_protocol`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/thinker/layers/memory_protocol.rs
git commit -m "prompt: nudge immediate feedback/lessons capture on error (MemoryProtocolLayer)"
```

---

## Final verification

- [ ] **Full lib test sweep:** `cargo test -p alephcore --lib` → all green.
- [ ] **Clippy:** `just clippy` → zero warnings.
- [ ] **Live backfill confirmed:** Task 7 Step 4 shows `notes_links` populated for `main`.
- [ ] **Canvas confirmed:** Task 8 screenshot shows the two clusters connected.
- [ ] **Spec coverage:** Pillar 1 (Tasks 1-8), Pillar 2 (Tasks 9-11), Pillar 3 (Task 12).

---

## Self-Review Notes (author)

- **Spec coverage:** P1 engine (T1-3), relation persistence (T4), creation entrypoint (T5), NoteWeave entrypoint (T6), backfill (T7), canvas (T8). P2 registry (T9), flush+wiring (T10), readiness gate (T11). P3 prompt nudge (T12). All spec sections mapped.
- **Known adaptation points (read the real signature before coding):**
  `search_notes_fts` arg order/limit; the link DTO field returned by
  `get_outgoing_links` (assumed `relation: Option<String>`); how the SQLite
  backend acquires a connection in `store.rs` (mirror the neighbour write, do
  NOT invent `conn()`); the exact `LayerInput` constructor for the prompt test;
  the `CompressionService` reach at `emit.rs` (use the existing context handle
  if present rather than a new global). These are existing APIs — confirm shapes,
  don't fabricate.
- **Ordering invariant (T6):** write the body `[[ ]]` link via `append_to_note`
  BEFORE `add_link_with_relation`, so the UPSERT updates an existing row's
  relation rather than racing the indexer.
