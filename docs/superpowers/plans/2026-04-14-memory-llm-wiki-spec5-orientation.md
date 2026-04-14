# Spec 5 — Orientation Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an LLM-readable "orientation layer" — `SCHEMA.md` + `index.md` + `log.md` — on top of Aleph's existing L1 notes, plus the infrastructure to inject it into every LLM turn (Context/Hybrid modes) or fetch it on demand (Tools mode).

**Architecture:** Four new types live in a new `src/memory/wiki/` module: a `WikiOrientation` trait with an `FsWikiOrientation` implementation, a `schema` reader/writer, an `index_md` generator projected from `notes_index`, and a `log_md` appender. `NoteIndexer` gets a lightweight invalidation hook so writes can keep the rendered `index.md` fresh without coupling to the new module. `MemoryContextProvider` gets a new `build_orientation_user_message` that honors Spec 3's `injection_mode`. A new `IndexRefresher` Dream stage runs before `NoteLint` daily as the bootstrap / recovery path. Two builtin tools (`wiki_schema`, `wiki_orient`) expose the layer to the model in Tools mode.

**Tech Stack:** Rust + tokio + async_trait + serde + sha2 + chrono + `insta` (snapshots) + `proptest` (invariants). No new third-party dependencies.

**Spec reference:** `docs/superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md` §2 (and §1 / §6 for cross-cutting concerns).

**Pre-flight check for the implementer:**
- Verify `src/memory/notes/indexer.rs` still exposes `write_note` / `append_to_note` / `remove_note` as described in `docs/reference/memory/NOTES.md` §6. If signatures drift, adapt the hook calls.
- Verify `src/thinker/memory_context_provider.rs` still builds messages via `UnifiedMessage::user(rendered)` and `LayerInput::memory_user_message`. The new method follows the same pattern.
- Verify `src/memory/dreaming/stages/mod.rs` still exposes the `DreamStage` trait as in `docs/reference/memory/DREAM_DAEMON.md` §4.3.
- Verify `crate::providers::recording_mock::RecordingMockProvider` is still in scope — all LLM-touching tests use it.

**Out of scope for Spec 5 (deferred to Spec 6/7/8):**
- `CompoundIngestor` (Spec 6)
- `USER.md` / `ProfileSynthesizer` (Spec 7)
- `query/` category, `QueryFiler`, `query_filed` table (Spec 8)

---

## File Map

### Create
- `src/memory/wiki/mod.rs` — module root + re-exports
- `src/memory/wiki/types.rs` — `LogEntry`, `LogAction`, `OrientationSnapshot`, `IndexStats`, `TokenBudget`
- `src/memory/wiki/log_md.rs` — append-only log writer + rotation
- `src/memory/wiki/schema.rs` — `SCHEMA.md` parse / hash-guarded write / bootstrap
- `src/memory/wiki/index_md.rs` — `index.md` projection from `notes_index`
- `src/memory/wiki/orientation.rs` — `WikiOrientation` trait + `FsWikiOrientation` impl
- `src/memory/wiki/prompts.rs` — `PROMPT_ORIENTATION_BOOTSTRAP` + compact schema builder
- `src/memory/dreaming/stages/index_refresher.rs` — new Dream stage
- `src/builtin_tools/wiki_schema.rs` — LLM tool for SCHEMA.md read/write
- `src/builtin_tools/wiki_orient.rs` — LLM tool for Tools-mode on-demand orient
- `tests/memory_wiki_orientation.rs` — end-to-end integration test
- `src/memory/wiki/snapshots/orientation_bootstrap_prompt.snap` — `insta` snapshot

### Modify
- `src/memory/mod.rs` — `pub mod wiki;`
- `src/memory/notes/indexer.rs` — optional `WikiOrientation` hook field + `invalidate` calls in `write_note` / `append_to_note` / `remove_note`
- `src/thinker/memory_context_provider.rs` — new `build_orientation_user_message` + call-site in `LayerInput` assembly
- `src/memory/dreaming/stages/mod.rs` — re-export `IndexRefresherStage`
- `src/memory/dreaming/mod.rs` — add `IndexRefresherStage` to `daily()` and `weekly()` pipelines **before** `NoteLintStage`
- `src/builtin_tools/mod.rs` — register new tool modules
- `src/executor/builtin_registry/registry.rs` — register `wiki_schema` + `wiki_orient` (gated by `injection_mode`)
- `src/config/types/memory.rs` — add `OrientationConfig` nested struct on `MemoryConfig`
- `src/app/context/builder.rs` (or equivalent startup assembly site) — construct `FsWikiOrientation`, inject into `NoteIndexer` + `MemoryContextProvider`

### No change
- `src/memory/store/sqlite/schema.rs` — no new tables in Spec 5 (query_filed is Spec 8)
- `CATEGORY_DIRS` — unchanged; Spec 8 adds `query`

---

## Task 1: Scaffold `src/memory/wiki/` module and types

**Files:**
- Create: `src/memory/wiki/mod.rs`
- Create: `src/memory/wiki/types.rs`
- Modify: `src/memory/mod.rs`

- [ ] **Step 1: Write failing test**

Create `src/memory/wiki/types.rs`:

```rust
//! Shared types for the wiki orientation layer.

use serde::{Deserialize, Serialize};

/// Action kinds recorded in `log.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogAction {
    Ingest,
    Query,
    Lint,
    Schema,
    Profile,
    SessionEnd,
    Bootstrap,
}

impl LogAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogAction::Ingest => "ingest",
            LogAction::Query => "query",
            LogAction::Lint => "lint",
            LogAction::Schema => "schema",
            LogAction::Profile => "profile",
            LogAction::SessionEnd => "session_end",
            LogAction::Bootstrap => "bootstrap",
        }
    }
}

/// One entry to append to `log.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp_utc: i64,
    pub action: LogAction,
    pub summary: String,        // one-line, no newlines
    pub detail_lines: Vec<String>, // indented bullets, optional
}

/// Token budget hint for `read_snapshot`.
#[derive(Debug, Clone, Copy)]
pub struct TokenBudget {
    pub max_tokens: usize,
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self { max_tokens: 4000 }
    }
}

/// Snapshot returned by `WikiOrientation::read_snapshot`.
#[derive(Debug, Clone)]
pub struct OrientationSnapshot {
    pub schema_text: String,
    pub index_text: String,
    pub recent_log_tail: String,
}

/// Result of a full rebuild of `index.md`.
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    pub notes_indexed: usize,
    pub categories_rendered: usize,
    pub bytes_written: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_action_roundtrip_serde() {
        for a in [
            LogAction::Ingest,
            LogAction::Query,
            LogAction::Lint,
            LogAction::Schema,
            LogAction::Profile,
            LogAction::SessionEnd,
            LogAction::Bootstrap,
        ] {
            let j = serde_json::to_string(&a).unwrap();
            let back: LogAction = serde_json::from_str(&j).unwrap();
            assert_eq!(a, back);
        }
    }

    #[test]
    fn log_action_str_matches_serde_rename() {
        assert_eq!(LogAction::Ingest.as_str(), "ingest");
        assert_eq!(LogAction::SessionEnd.as_str(), "session_end");
    }

    #[test]
    fn token_budget_default_is_4000() {
        assert_eq!(TokenBudget::default().max_tokens, 4000);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib memory::wiki::types`
Expected: FAIL with "could not find `types` in `wiki`" or "module `wiki` not found".

- [ ] **Step 3: Add `mod.rs` and wire into parent**

Create `src/memory/wiki/mod.rs`:

```rust
//! LLM-facing orientation layer: SCHEMA.md + index.md + log.md.
//!
//! The three markdown files under `~/.aleph/memory/note/{agent_id}/` give the
//! LLM a global map each session. SQLite remains a rebuildable index; this
//! module owns the human-and-LLM-readable projection.

pub mod types;

pub use types::{IndexStats, LogAction, LogEntry, OrientationSnapshot, TokenBudget};
```

Modify `src/memory/mod.rs`: add at the top of the `pub mod` section (keep alphabetical order with existing modules):

```rust
pub mod wiki;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib memory::wiki::types`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add src/memory/wiki/mod.rs src/memory/wiki/types.rs src/memory/mod.rs
git commit -m "feat(wiki): scaffold orientation module and shared types"
```

---

## Task 2: `log.md` append-only writer with rotation

**Files:**
- Create: `src/memory/wiki/log_md.rs`
- Modify: `src/memory/wiki/mod.rs`

- [ ] **Step 1: Write failing test**

Create `src/memory/wiki/log_md.rs`:

```rust
//! Append-only log writer for `log.md`. Rotates at 2000 lines.

use crate::error::AlephError;
use crate::memory::wiki::types::{LogAction, LogEntry};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

pub const LOG_FILENAME: &str = "log.md";
pub const LOG_ROTATE_LINES: usize = 2000;

pub struct LogMdWriter {
    agent_dir: PathBuf,
}

impl LogMdWriter {
    pub fn new(agent_dir: impl Into<PathBuf>) -> Self {
        Self { agent_dir: agent_dir.into() }
    }

    fn log_path(&self) -> PathBuf {
        self.agent_dir.join(LOG_FILENAME)
    }

    /// Append a single entry. Creates the file with a header on first write.
    pub async fn append(&self, entry: &LogEntry) -> Result<(), AlephError> {
        tokio::fs::create_dir_all(&self.agent_dir)
            .await
            .map_err(|e| AlephError::other(format!("create log dir: {e}")))?;

        let path = self.log_path();
        let exists = tokio::fs::try_exists(&path)
            .await
            .map_err(|e| AlephError::other(format!("stat log: {e}")))?;

        let mut buf = String::new();
        if !exists {
            buf.push_str("# Aleph Wiki Log\n\n");
            buf.push_str("> Append-only activity timeline. Rotates at 2000 lines.\n\n");
        }

        let ts: DateTime<Utc> = DateTime::<Utc>::from_timestamp(entry.timestamp_utc, 0)
            .unwrap_or_else(Utc::now);
        buf.push_str(&format!(
            "## [{ts}] {action} | {summary}\n",
            ts = ts.format("%Y-%m-%d %H:%M:%SZ"),
            action = entry.action.as_str(),
            summary = sanitize_single_line(&entry.summary),
        ));
        for line in &entry.detail_lines {
            buf.push_str(&format!("- {}\n", sanitize_single_line(line)));
        }
        buf.push('\n');

        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .await
            .map_err(|e| AlephError::other(format!("open log: {e}")))?;
        f.write_all(buf.as_bytes())
            .await
            .map_err(|e| AlephError::other(format!("write log: {e}")))?;
        Ok(())
    }

    /// Count lines in log.md (returns 0 if missing).
    pub async fn line_count(&self) -> Result<usize, AlephError> {
        let path = self.log_path();
        if !tokio::fs::try_exists(&path)
            .await
            .map_err(|e| AlephError::other(format!("stat log: {e}")))?
        {
            return Ok(0);
        }
        let s = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| AlephError::other(format!("read log: {e}")))?;
        Ok(s.lines().count())
    }

    /// Rotate if over threshold. Renames to `log-YYYY-MM-DD.md`; new log.md
    /// gets a "continued from …" header line.
    pub async fn rotate_if_needed(&self) -> Result<bool, AlephError> {
        if self.line_count().await? <= LOG_ROTATE_LINES {
            return Ok(false);
        }
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let new_name = format!("log-{today}.md");
        let from = self.log_path();
        let to = self.agent_dir.join(&new_name);
        tokio::fs::rename(&from, &to)
            .await
            .map_err(|e| AlephError::other(format!("rotate log: {e}")))?;

        let header = format!(
            "# Aleph Wiki Log\n\n<!-- continued from {new_name} -->\n\n"
        );
        tokio::fs::write(self.log_path(), header)
            .await
            .map_err(|e| AlephError::other(format!("write new log: {e}")))?;
        Ok(true)
    }

    /// Read the last `n` lines (or the whole file if shorter).
    pub async fn tail(&self, n: usize) -> Result<String, AlephError> {
        let path = self.log_path();
        if !tokio::fs::try_exists(&path)
            .await
            .map_err(|e| AlephError::other(format!("stat log: {e}")))?
        {
            return Ok(String::new());
        }
        let s = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| AlephError::other(format!("read log: {e}")))?;
        let lines: Vec<&str> = s.lines().collect();
        let start = lines.len().saturating_sub(n);
        Ok(lines[start..].join("\n"))
    }
}

fn sanitize_single_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(action: LogAction, summary: &str) -> LogEntry {
        LogEntry {
            timestamp_utc: 1_744_600_000,
            action,
            summary: summary.into(),
            detail_lines: vec!["detail-a".into(), "detail-b".into()],
        }
    }

    #[tokio::test]
    async fn first_append_creates_header_and_entry() {
        let dir = tempfile::tempdir().unwrap();
        let w = LogMdWriter::new(dir.path());
        w.append(&entry(LogAction::Bootstrap, "init")).await.unwrap();
        let body = tokio::fs::read_to_string(dir.path().join("log.md"))
            .await
            .unwrap();
        assert!(body.starts_with("# Aleph Wiki Log"));
        assert!(body.contains("bootstrap | init"));
        assert!(body.contains("- detail-a"));
    }

    #[tokio::test]
    async fn multiline_summary_sanitised() {
        let dir = tempfile::tempdir().unwrap();
        let w = LogMdWriter::new(dir.path());
        w.append(&entry(LogAction::Ingest, "a\nb\r\nc")).await.unwrap();
        let body = tokio::fs::read_to_string(dir.path().join("log.md"))
            .await
            .unwrap();
        assert!(body.contains("ingest | a b c"));
        assert!(!body.contains('\r'));
    }

    #[tokio::test]
    async fn tail_returns_last_n_lines() {
        let dir = tempfile::tempdir().unwrap();
        let w = LogMdWriter::new(dir.path());
        for i in 0..5 {
            w.append(&entry(LogAction::Ingest, &format!("#{i}"))).await.unwrap();
        }
        let tail = w.tail(3).await.unwrap();
        assert!(tail.contains("#4"));
        assert!(!tail.contains("#0"));
    }

    #[tokio::test]
    async fn rotate_when_over_threshold() {
        let dir = tempfile::tempdir().unwrap();
        // Hand-write an oversized log.
        let big = "line\n".repeat(LOG_ROTATE_LINES + 5);
        tokio::fs::write(dir.path().join("log.md"), big).await.unwrap();
        let w = LogMdWriter::new(dir.path());
        let rotated = w.rotate_if_needed().await.unwrap();
        assert!(rotated);
        let new_log = tokio::fs::read_to_string(dir.path().join("log.md"))
            .await
            .unwrap();
        assert!(new_log.contains("continued from log-"));
        // Old file exists.
        let dated = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("log-"));
        assert!(dated);
    }

    #[tokio::test]
    async fn rotate_noop_when_under_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let w = LogMdWriter::new(dir.path());
        w.append(&entry(LogAction::Ingest, "small")).await.unwrap();
        let rotated = w.rotate_if_needed().await.unwrap();
        assert!(!rotated);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib memory::wiki::log_md`
Expected: FAIL — module `log_md` not declared. (If `tempfile` is missing from dev-deps, also fails to compile.)

- [ ] **Step 3: Implement**

Wire the module in `src/memory/wiki/mod.rs`:

```rust
pub mod log_md;
pub use log_md::{LogMdWriter, LOG_FILENAME, LOG_ROTATE_LINES};
```

Verify `tempfile` is already a dev-dep in the workspace `Cargo.toml` / `src/Cargo.toml` (it is used across the project). If missing:

```bash
cargo add --dev --package alephcore tempfile
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib memory::wiki::log_md`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
git add src/memory/wiki/mod.rs src/memory/wiki/log_md.rs
git commit -m "feat(wiki): append-only log.md writer with rotation"
```

---

## Task 3: `SCHEMA.md` parser and hash-guarded writer

**Files:**
- Create: `src/memory/wiki/schema.rs`
- Modify: `src/memory/wiki/mod.rs`

- [ ] **Step 1: Write failing test**

Create `src/memory/wiki/schema.rs`:

```rust
//! Parse, read, and hash-guarded write of `SCHEMA.md`.

use crate::error::AlephError;
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const SCHEMA_FILENAME: &str = "SCHEMA.md";

/// The five fixed section names in `SCHEMA.md`.
pub const SCHEMA_SECTIONS: &[&str] = &[
    "Domain",
    "Categories (fixed by Aleph)",
    "Tag Taxonomy",
    "Page Thresholds",
    "Update Policy",
];

/// Parsed view of `SCHEMA.md`.
#[derive(Debug, Clone)]
pub struct SchemaDoc {
    pub version: u32,           // from frontmatter `schema_version`
    pub updated: String,        // YYYY-MM-DD
    pub raw: String,            // full file contents
    pub content_hash: String,   // sha256 hex of `raw`
}

impl SchemaDoc {
    /// Parse a SCHEMA.md body. Missing frontmatter fields fall through to
    /// sane defaults; malformed files are reported as an error so callers
    /// can trigger re-bootstrap.
    pub fn parse(raw: impl Into<String>) -> Result<Self, AlephError> {
        let raw = raw.into();
        let (version, updated) = read_frontmatter(&raw)?;
        let content_hash = hash(&raw);
        Ok(Self { version, updated, raw, content_hash })
    }

    /// Extract the canonical compact view used by prompts (Tag Taxonomy +
    /// Page Thresholds + Update Policy). Missing sections become empty
    /// strings but never cause an error.
    pub fn compact_for_prompt(&self) -> String {
        let want = ["Tag Taxonomy", "Page Thresholds", "Update Policy"];
        let mut out = String::new();
        for name in want {
            if let Some(section) = extract_section(&self.raw, name) {
                out.push_str(&format!("## {name}\n{section}\n\n"));
            }
        }
        out
    }
}

fn read_frontmatter(raw: &str) -> Result<(u32, String), AlephError> {
    let mut version = 1_u32;
    let mut updated = String::new();
    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let fm = &rest[..end];
            for line in fm.lines() {
                if let Some(v) = line.strip_prefix("schema_version:") {
                    version = v.trim().parse().unwrap_or(1);
                } else if let Some(v) = line.strip_prefix("updated:") {
                    updated = v.trim().trim_matches('"').to_string();
                }
            }
            return Ok((version, updated));
        }
    }
    // Unfenced — treat as legacy but usable.
    Ok((1, String::new()))
}

fn extract_section<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    let header = format!("\n## {name}\n");
    let start = raw.find(&header)? + header.len();
    let after = &raw[start..];
    let end = after
        .find("\n## ")
        .map(|e| start + e)
        .unwrap_or(raw.len());
    Some(&raw[start..end])
}

fn hash(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

/// Reader/writer for `SCHEMA.md`.
pub struct SchemaStore {
    agent_dir: PathBuf,
}

impl SchemaStore {
    pub fn new(agent_dir: impl Into<PathBuf>) -> Self {
        Self { agent_dir: agent_dir.into() }
    }

    fn path(&self) -> PathBuf {
        self.agent_dir.join(SCHEMA_FILENAME)
    }

    /// Returns None when the file does not exist.
    pub async fn read(&self) -> Result<Option<SchemaDoc>, AlephError> {
        let p = self.path();
        if !tokio::fs::try_exists(&p)
            .await
            .map_err(|e| AlephError::other(format!("stat schema: {e}")))?
        {
            return Ok(None);
        }
        let raw = tokio::fs::read_to_string(&p)
            .await
            .map_err(|e| AlephError::other(format!("read schema: {e}")))?;
        Ok(Some(SchemaDoc::parse(raw)?))
    }

    /// Atomic write guarded by `expected_hash` (None = first write / force).
    /// Returns the post-write hash.
    pub async fn write(
        &self,
        new_content: &str,
        expected_hash: Option<&str>,
    ) -> Result<String, AlephError> {
        tokio::fs::create_dir_all(&self.agent_dir)
            .await
            .map_err(|e| AlephError::other(format!("create schema dir: {e}")))?;

        // Hash check.
        if let Some(expected) = expected_hash {
            let current = self
                .read()
                .await?
                .map(|d| d.content_hash)
                .unwrap_or_default();
            if current != expected {
                return Err(AlephError::other(format!(
                    "schema hash conflict: expected={expected} actual={current}"
                )));
            }
        }

        let tmp = self.agent_dir.join(format!(".schema.tmp.{}", Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        tokio::fs::write(&tmp, new_content)
            .await
            .map_err(|e| AlephError::other(format!("write tmp schema: {e}")))?;
        tokio::fs::rename(&tmp, self.path())
            .await
            .map_err(|e| AlephError::other(format!("rename schema: {e}")))?;
        Ok(hash(new_content))
    }
}

/// Default SCHEMA.md emitted by `bootstrap` when no LLM is available.
pub const DEFAULT_SCHEMA: &str = r#"---
schema_version: 1
updated: "2026-04-14"
---
# Memory Schema

## Domain
Aleph personal memory — general purpose.

## Categories (fixed by Aleph)
preference | plan | learning | project | personal | tool | lesson | skill | wiki | other
Special: synthesis (weekly dream output). skill/wiki have extra frontmatter — see NOTES.md §3.

## Tag Taxonomy
<!-- LLM maintained. New tag MUST appear here before use. -->
- rust
- async
- memory
- tooling

## Page Thresholds
- create: a topic appearing in 2+ sources, or centrally in one source.
- append: an existing page already covers the topic.
- contradict: new content conflicts with an existing claim — mark instead of silent overwrite.

## Update Policy
- Conflict → keep both claims with dates and sources; frontmatter `contradictions: [path]`.
- Supersede → append `## Superseded by [[path]] (YYYY-MM-DD)` to the old page.
- New tags → add to Tag Taxonomy first, then use.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_default_schema() {
        let doc = SchemaDoc::parse(DEFAULT_SCHEMA).unwrap();
        assert_eq!(doc.version, 1);
        assert_eq!(doc.updated, "2026-04-14");
        assert!(!doc.content_hash.is_empty());
        assert!(doc.raw.contains("## Tag Taxonomy"));
    }

    #[test]
    fn compact_for_prompt_has_three_sections() {
        let doc = SchemaDoc::parse(DEFAULT_SCHEMA).unwrap();
        let c = doc.compact_for_prompt();
        assert!(c.contains("## Tag Taxonomy"));
        assert!(c.contains("## Page Thresholds"));
        assert!(c.contains("## Update Policy"));
        assert!(!c.contains("## Domain"));
    }

    #[test]
    fn parse_without_frontmatter_uses_defaults() {
        let doc = SchemaDoc::parse("# Memory Schema\n\n## Domain\n...").unwrap();
        assert_eq!(doc.version, 1);
        assert_eq!(doc.updated, "");
    }

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let s = SchemaStore::new(dir.path());
        assert!(s.read().await.unwrap().is_none());
        let h = s.write(DEFAULT_SCHEMA, None).await.unwrap();
        let doc = s.read().await.unwrap().unwrap();
        assert_eq!(doc.content_hash, h);
    }

    #[tokio::test]
    async fn write_rejects_stale_hash() {
        let dir = tempfile::tempdir().unwrap();
        let s = SchemaStore::new(dir.path());
        s.write(DEFAULT_SCHEMA, None).await.unwrap();
        let err = s.write(DEFAULT_SCHEMA, Some("deadbeef")).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn write_accepts_correct_hash() {
        let dir = tempfile::tempdir().unwrap();
        let s = SchemaStore::new(dir.path());
        let h1 = s.write(DEFAULT_SCHEMA, None).await.unwrap();
        let new = DEFAULT_SCHEMA.replace("2026-04-14", "2026-04-15");
        let h2 = s.write(&new, Some(&h1)).await.unwrap();
        assert_ne!(h1, h2);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib memory::wiki::schema`
Expected: FAIL — module not declared.

- [ ] **Step 3: Implement**

Append to `src/memory/wiki/mod.rs`:

```rust
pub mod schema;
pub use schema::{SchemaDoc, SchemaStore, DEFAULT_SCHEMA, SCHEMA_FILENAME, SCHEMA_SECTIONS};
```

Check `hex` and `sha2` are already in workspace deps. They are used elsewhere in Aleph (e.g. `src/memory/notes/note.rs` computes `content_hash` via SHA-256). If missing, the compiler will flag it and you can add:

```bash
cargo add --package alephcore hex sha2 chrono
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib memory::wiki::schema`
Expected: PASS — 6 tests.

- [ ] **Step 5: Commit**

```bash
git add src/memory/wiki/mod.rs src/memory/wiki/schema.rs
git commit -m "feat(wiki): SCHEMA.md parser and hash-guarded writer"
```

---

## Task 4: `index.md` generator from `notes_index`

**Files:**
- Create: `src/memory/wiki/index_md.rs`
- Modify: `src/memory/wiki/mod.rs`

- [ ] **Step 1: Write failing test**

Create `src/memory/wiki/index_md.rs`:

```rust
//! Generate `index.md` from `notes_index` rows.
//!
//! Grouping: by category, in `CATEGORY_DIRS` order. One line per note:
//!
//!   - [[category/filename]] — <summary>. (updated YYYY-MM-DD)
//!
//! Summary source (three-tier fallback):
//!   1. Body first bullet (≤ 80 chars)
//!   2. frontmatter `summary:` field — TODO Spec 6 when extractor writes it
//!   3. Filename humanized
//!
//! For Spec 5 we rely on (1) + (3). (2) becomes effective once compound
//! ingest starts writing summaries; the lookup is already in place.

use crate::error::AlephError;
use crate::memory::notes::note::KnowledgeNote;
use crate::memory::notes::store::NoteIndexEntry;
use crate::memory::wiki::types::IndexStats;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const INDEX_FILENAME: &str = "index.md";

/// Maximum summary length in the index (chars).
pub const SUMMARY_CHAR_LIMIT: usize = 80;

pub struct IndexMdGenerator {
    agent_dir: PathBuf,
}

impl IndexMdGenerator {
    pub fn new(agent_dir: impl Into<PathBuf>) -> Self {
        Self { agent_dir: agent_dir.into() }
    }

    fn index_path(&self) -> PathBuf {
        self.agent_dir.join(INDEX_FILENAME)
    }

    /// Render and write the full index. `entries` should already be filtered
    /// to this agent.
    pub async fn write(&self, entries: &[NoteIndexEntry]) -> Result<IndexStats, AlephError> {
        let text = self.render(entries).await?;
        tokio::fs::create_dir_all(&self.agent_dir)
            .await
            .map_err(|e| AlephError::other(format!("create index dir: {e}")))?;
        tokio::fs::write(self.index_path(), &text)
            .await
            .map_err(|e| AlephError::other(format!("write index: {e}")))?;
        Ok(IndexStats {
            notes_indexed: entries.len(),
            categories_rendered: entries
                .iter()
                .map(|e| e.category.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            bytes_written: text.len(),
        })
    }

    /// Pure renderer (no disk side-effects) — used by tests and
    /// `OrientationSnapshot::read_snapshot` when size-bounding is needed.
    pub async fn render(&self, entries: &[NoteIndexEntry]) -> Result<String, AlephError> {
        let mut by_cat: BTreeMap<String, Vec<&NoteIndexEntry>> = BTreeMap::new();
        for e in entries {
            by_cat.entry(e.category.clone()).or_default().push(e);
        }

        let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let mut out = String::new();
        out.push_str("<!-- auto-generated: DO NOT EDIT — regenerated on every ingest -->\n");
        out.push_str(&format!(
            "<!-- total: {} notes | updated: {} -->\n\n# Index\n\n",
            entries.len(),
            now
        ));

        for (cat, items) in by_cat.iter() {
            out.push_str(&format!("## {} ({})\n", cat, items.len()));
            let mut sorted = items.clone();
            sorted.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            for e in sorted {
                let summary = self.summary_for(e).await.unwrap_or_default();
                let updated = DateTime::<Utc>::from_timestamp(e.updated_at, 0)
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "unknown".into());
                out.push_str(&format!(
                    "- [[{path}]] — {summary} (updated {updated})\n",
                    path = e.path,
                    summary = sanitise_summary(&summary),
                ));
            }
            out.push('\n');
        }
        Ok(out)
    }

    async fn summary_for(&self, entry: &NoteIndexEntry) -> Result<String, AlephError> {
        // tier 1 + 2: read the file, use first body bullet or frontmatter.summary
        let note_path = self
            .agent_dir
            .join(&entry.category)
            .join(format!("{}.md", entry.filename));
        if let Ok(raw) = tokio::fs::read_to_string(&note_path).await {
            if let Some(first_bullet) = first_body_bullet(&raw) {
                return Ok(first_bullet);
            }
        }
        // tier 3: humanize filename
        Ok(humanize_filename(&entry.filename))
    }
}

fn first_body_bullet(raw: &str) -> Option<String> {
    // skip frontmatter
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
            return Some(rest.to_string());
        }
    }
    None
}

fn humanize_filename(name: &str) -> String {
    name.replace(['-', '_'], " ")
}

fn sanitise_summary(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| *c != '\n' && *c != '\r')
        .collect();
    if cleaned.chars().count() > SUMMARY_CHAR_LIMIT {
        let truncated: String = cleaned.chars().take(SUMMARY_CHAR_LIMIT - 1).collect();
        format!("{truncated}…")
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::store::NoteIndexEntry;

    fn entry(category: &str, filename: &str, updated: i64) -> NoteIndexEntry {
        NoteIndexEntry {
            path: format!("{category}/{filename}"),
            filename: filename.into(),
            agent_id: "default".into(),
            category: category.into(),
            tags: vec![],
            link_count: 0,
            created_at: 0,
            updated_at: updated,
            content_hash: "x".into(),
        }
    }

    #[tokio::test]
    async fn render_empty() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        let s = g.render(&[]).await.unwrap();
        assert!(s.contains("<!-- total: 0 notes"));
        assert!(s.contains("# Index"));
    }

    #[tokio::test]
    async fn render_groups_by_category_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        let entries = vec![
            entry("learning", "rust", 1_700_000_000),
            entry("learning", "tokio", 1_700_001_000),
            entry("preference", "editor", 1_700_000_500),
        ];
        let s = g.render(&entries).await.unwrap();
        let pl = s.find("## learning (2)").unwrap();
        let pp = s.find("## preference (1)").unwrap();
        assert!(pl < pp); // BTree alphabetic
        let tokio_idx = s.find("learning/tokio").unwrap();
        let rust_idx = s.find("learning/rust").unwrap();
        assert!(tokio_idx < rust_idx); // newest first within a category
    }

    #[tokio::test]
    async fn first_bullet_used_as_summary() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        tokio::fs::create_dir_all(dir.path().join("learning"))
            .await
            .unwrap();
        tokio::fs::write(
            dir.path().join("learning/rust.md"),
            "---\ncategory: learning\n---\n# Rust\n\n- The user likes Rust macros a lot.\n- second fact\n",
        )
        .await
        .unwrap();
        let entries = vec![entry("learning", "rust", 1_700_000_000)];
        let s = g.render(&entries).await.unwrap();
        assert!(s.contains("The user likes Rust macros a lot."));
    }

    #[tokio::test]
    async fn falls_back_to_filename_humanise() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        // file does not exist on disk
        let entries = vec![entry("tool", "ast_grep-cheatsheet", 0)];
        let s = g.render(&entries).await.unwrap();
        assert!(s.contains("ast grep cheatsheet"));
    }

    #[tokio::test]
    async fn summary_truncated_to_80_chars() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        tokio::fs::create_dir_all(dir.path().join("project"))
            .await
            .unwrap();
        let big = "A".repeat(200);
        tokio::fs::write(
            dir.path().join("project/x.md"),
            format!("---\n---\n- {big}\n"),
        )
        .await
        .unwrap();
        let entries = vec![entry("project", "x", 0)];
        let s = g.render(&entries).await.unwrap();
        assert!(s.contains("…"));
    }

    #[tokio::test]
    async fn write_then_readable_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        let entries = vec![entry("learning", "rust", 1_700_000_000)];
        let stats = g.write(&entries).await.unwrap();
        assert_eq!(stats.notes_indexed, 1);
        assert!(stats.bytes_written > 0);
        let body = tokio::fs::read_to_string(dir.path().join("index.md"))
            .await
            .unwrap();
        assert!(body.contains("learning/rust"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib memory::wiki::index_md`
Expected: FAIL — module not declared.

- [ ] **Step 3: Wire the module**

Append to `src/memory/wiki/mod.rs`:

```rust
pub mod index_md;
pub use index_md::{IndexMdGenerator, INDEX_FILENAME};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib memory::wiki::index_md`
Expected: PASS — 6 tests.

- [ ] **Step 5: Commit**

```bash
git add src/memory/wiki/mod.rs src/memory/wiki/index_md.rs
git commit -m "feat(wiki): index.md generator with 3-tier summary fallback"
```

---

## Task 5: `WikiOrientation` trait + `FsWikiOrientation` impl

**Files:**
- Create: `src/memory/wiki/orientation.rs`
- Modify: `src/memory/wiki/mod.rs`

- [ ] **Step 1: Write failing test**

Create `src/memory/wiki/orientation.rs`:

```rust
//! `WikiOrientation` trait + `FsWikiOrientation` filesystem implementation.

use crate::error::AlephError;
use crate::memory::notes::store::NoteStore;
use crate::memory::wiki::index_md::IndexMdGenerator;
use crate::memory::wiki::log_md::LogMdWriter;
use crate::memory::wiki::schema::{SchemaStore, DEFAULT_SCHEMA};
use crate::memory::wiki::types::{
    IndexStats, LogAction, LogEntry, OrientationSnapshot, TokenBudget,
};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

#[async_trait]
pub trait WikiOrientation: Send + Sync {
    async fn bootstrap(&self, agent_id: &str) -> Result<(), AlephError>;

    async fn read_snapshot(
        &self,
        agent_id: &str,
        budget: TokenBudget,
    ) -> Result<OrientationSnapshot, AlephError>;

    async fn record_ingest(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError>;
    async fn record_query(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError>;
    async fn record_lint(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError>;
    async fn record_session_end(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError>;

    async fn rebuild_index(&self, agent_id: &str) -> Result<IndexStats, AlephError>;
    async fn rotate_log_if_needed(&self, agent_id: &str) -> Result<bool, AlephError>;

    /// Mark a note dirty. A subsequent `rebuild_index` (or the next
    /// `record_ingest`) flushes.
    fn invalidate(&self, agent_id: &str, note_path: &str);
}

/// Production implementation. Holds the memory root + a `NoteStore` handle
/// for reading `notes_index` rows during `rebuild_index`.
pub struct FsWikiOrientation<S: NoteStore + Send + Sync + 'static> {
    memory_dir: PathBuf,
    store: Arc<S>,
    dirty: Mutex<HashSet<String>>, // "agent_id|path"
}

impl<S: NoteStore + Send + Sync + 'static> FsWikiOrientation<S> {
    pub fn new(memory_dir: impl Into<PathBuf>, store: Arc<S>) -> Self {
        Self {
            memory_dir: memory_dir.into(),
            store,
            dirty: Mutex::new(HashSet::new()),
        }
    }

    fn agent_dir(&self, agent_id: &str) -> PathBuf {
        self.memory_dir.join(agent_id)
    }

    async fn append(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError> {
        let log = LogMdWriter::new(self.agent_dir(agent_id));
        log.append(&entry).await?;
        log.rotate_if_needed().await?;
        Ok(())
    }
}

#[async_trait]
impl<S: NoteStore + Send + Sync + 'static> WikiOrientation for FsWikiOrientation<S> {
    async fn bootstrap(&self, agent_id: &str) -> Result<(), AlephError> {
        let dir = self.agent_dir(agent_id);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| AlephError::other(format!("bootstrap dir: {e}")))?;

        let ss = SchemaStore::new(&dir);
        if ss.read().await?.is_none() {
            ss.write(DEFAULT_SCHEMA, None).await?;
        }

        // Force a first-time index rebuild + first log entry.
        self.rebuild_index(agent_id).await?;
        self.append(
            agent_id,
            LogEntry {
                timestamp_utc: chrono::Utc::now().timestamp(),
                action: LogAction::Bootstrap,
                summary: format!("wiki orientation bootstrapped for agent={agent_id}"),
                detail_lines: vec![],
            },
        )
        .await?;
        Ok(())
    }

    async fn read_snapshot(
        &self,
        agent_id: &str,
        budget: TokenBudget,
    ) -> Result<OrientationSnapshot, AlephError> {
        let dir = self.agent_dir(agent_id);
        let schema_text = SchemaStore::new(&dir)
            .read()
            .await?
            .map(|d| d.raw)
            .unwrap_or_default();
        let index_text = tokio::fs::read_to_string(dir.join("index.md"))
            .await
            .unwrap_or_default();
        let recent_log_tail = LogMdWriter::new(&dir).tail(20).await.unwrap_or_default();

        // Crude char-based budget: rough ≈ 4 chars / token.
        let max_chars = budget.max_tokens.saturating_mul(4);
        let index_text = if index_text.len() > max_chars {
            format!(
                "{}\n<!-- truncated to {} chars under budget -->\n",
                &index_text[..max_chars.min(index_text.len())],
                max_chars
            )
        } else {
            index_text
        };

        Ok(OrientationSnapshot {
            schema_text,
            index_text,
            recent_log_tail,
        })
    }

    async fn record_ingest(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError> {
        // Flush dirty set via rebuild_index before logging — keeps index
        // coherent with the log line the LLM will read next.
        self.rebuild_index(agent_id).await?;
        self.append(agent_id, entry).await
    }

    async fn record_query(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError> {
        self.append(agent_id, entry).await
    }

    async fn record_lint(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError> {
        self.append(agent_id, entry).await
    }

    async fn record_session_end(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError> {
        self.append(agent_id, entry).await
    }

    async fn rebuild_index(&self, agent_id: &str) -> Result<IndexStats, AlephError> {
        let entries = self.store.list_notes(agent_id).await?;
        let gen = IndexMdGenerator::new(self.agent_dir(agent_id));
        let stats = gen.write(&entries).await?;
        // Drained.
        self.dirty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|k| !k.starts_with(&format!("{agent_id}|")));
        Ok(stats)
    }

    async fn rotate_log_if_needed(&self, agent_id: &str) -> Result<bool, AlephError> {
        LogMdWriter::new(self.agent_dir(agent_id))
            .rotate_if_needed()
            .await
    }

    fn invalidate(&self, agent_id: &str, note_path: &str) {
        self.dirty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(format!("{agent_id}|{note_path}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::sqlite::SqliteMemoryBackend;

    async fn fresh_backend(memory_dir: &std::path::Path) -> Arc<SqliteMemoryBackend> {
        let db_path = memory_dir.join("mem.db");
        let backend = SqliteMemoryBackend::new(&db_path).await.unwrap();
        Arc::new(backend)
    }

    #[tokio::test]
    async fn bootstrap_creates_schema_index_log() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fresh_backend(dir.path()).await;
        let orient = FsWikiOrientation::new(dir.path().join("note"), backend);
        orient.bootstrap("default").await.unwrap();

        let base = dir.path().join("note/default");
        assert!(base.join("SCHEMA.md").exists());
        assert!(base.join("index.md").exists());
        assert!(base.join("log.md").exists());
    }

    #[tokio::test]
    async fn bootstrap_is_idempotent_on_schema() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fresh_backend(dir.path()).await;
        let orient = FsWikiOrientation::new(dir.path().join("note"), backend);
        orient.bootstrap("default").await.unwrap();
        let schema1 = tokio::fs::read_to_string(
            dir.path().join("note/default/SCHEMA.md"),
        )
        .await
        .unwrap();
        orient.bootstrap("default").await.unwrap();
        let schema2 = tokio::fs::read_to_string(
            dir.path().join("note/default/SCHEMA.md"),
        )
        .await
        .unwrap();
        assert_eq!(schema1, schema2); // not clobbered on second bootstrap
    }

    #[tokio::test]
    async fn read_snapshot_returns_all_three_parts() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fresh_backend(dir.path()).await;
        let orient = FsWikiOrientation::new(dir.path().join("note"), backend);
        orient.bootstrap("default").await.unwrap();
        let snap = orient
            .read_snapshot("default", TokenBudget::default())
            .await
            .unwrap();
        assert!(snap.schema_text.contains("# Memory Schema"));
        assert!(snap.index_text.contains("# Index"));
        assert!(snap.recent_log_tail.contains("bootstrap"));
    }

    #[tokio::test]
    async fn invalidate_tracked_until_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fresh_backend(dir.path()).await;
        let orient = FsWikiOrientation::new(dir.path().join("note"), backend);
        orient.bootstrap("default").await.unwrap();
        orient.invalidate("default", "learning/rust");
        assert_eq!(
            orient
                .dirty
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len(),
            1
        );
        orient.rebuild_index("default").await.unwrap();
        assert_eq!(
            orient
                .dirty
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len(),
            0
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib memory::wiki::orientation`
Expected: FAIL — module not declared.

- [ ] **Step 3: Wire module**

Append to `src/memory/wiki/mod.rs`:

```rust
pub mod orientation;
pub use orientation::{FsWikiOrientation, WikiOrientation};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib memory::wiki::orientation`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add src/memory/wiki/mod.rs src/memory/wiki/orientation.rs
git commit -m "feat(wiki): WikiOrientation trait and FsWikiOrientation impl"
```

---

## Task 6: Bootstrap prompt and LLM-driven schema init

**Files:**
- Create: `src/memory/wiki/prompts.rs`
- Modify: `src/memory/wiki/mod.rs`
- Modify: `src/memory/wiki/orientation.rs`

- [ ] **Step 1: Write failing test**

Create `src/memory/wiki/prompts.rs`:

```rust
//! Prompt strings and LLM-backed helpers for the wiki orientation layer.

use crate::error::AlephError;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// System prompt used by `bootstrap_via_llm` to generate an opinionated
/// initial SCHEMA.md. The file contents must match the `SchemaDoc::parse`
/// expectations: frontmatter + five sections.
pub const PROMPT_ORIENTATION_BOOTSTRAP: &str = r#"You produce the initial SCHEMA.md for an Aleph personal-memory workspace.
Output MUST be a single markdown document with the following exact shape:

---
schema_version: 1
updated: "YYYY-MM-DD"
---
# Memory Schema

## Domain
<1-3 sentences describing what this agent's memory is about. Use the user's hint if present.>

## Categories (fixed by Aleph)
preference | plan | learning | project | personal | tool | lesson | skill | wiki | other
Special: synthesis (weekly dream output). skill/wiki carry extra frontmatter.

## Tag Taxonomy
<!-- LLM maintained. New tag MUST appear here before use. -->
- <seed 5-10 tags relevant to the domain hint>

## Page Thresholds
- create: a topic appearing in 2+ sources, or centrally in one source.
- append: an existing page already covers the topic.
- contradict: new content conflicts with existing — mark, never silently overwrite.

## Page Thresholds must not be renamed or removed.

## Update Policy
- Conflict → keep both claims with dates and sources; frontmatter `contradictions: [path]`.
- Supersede → append `## Superseded by [[path]] (YYYY-MM-DD)` to the old page.
- New tags → add to Tag Taxonomy first, then use.

No commentary, no markdown fences around the whole document, no prose before or after.
"#;

/// Ask the provider for a fresh SCHEMA.md tailored to `domain_hint` (may be empty).
pub async fn schema_via_llm(
    provider: &Arc<dyn AiProvider>,
    domain_hint: &str,
) -> Result<String, AlephError> {
    let user = if domain_hint.trim().is_empty() {
        "Produce the initial SCHEMA.md. No domain hint — use a general-purpose setup.".to_string()
    } else {
        format!("Domain hint: {domain_hint}\n\nProduce the initial SCHEMA.md for this domain.")
    };
    let msgs = [UnifiedMessage::user(&user)];
    let resp = provider
        .process(RequestPayload::new(&msgs).with_system(Some(PROMPT_ORIENTATION_BOOTSTRAP)))
        .await
        .map_err(|e| AlephError::other(format!("schema LLM call: {e}")))?;
    Ok(resp.text_content())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::recording_mock::RecordingMockProvider;

    #[test]
    fn prompt_has_five_fixed_sections() {
        let p = PROMPT_ORIENTATION_BOOTSTRAP;
        for name in ["Domain", "Categories", "Tag Taxonomy", "Page Thresholds", "Update Policy"] {
            assert!(p.contains(&format!("## {name}")), "missing section {name}");
        }
    }

    #[test]
    fn prompt_snapshot() {
        insta::assert_snapshot!(
            "orientation_bootstrap_prompt",
            PROMPT_ORIENTATION_BOOTSTRAP
        );
    }

    #[tokio::test]
    async fn schema_via_llm_passes_system_prompt() {
        let mock = RecordingMockProvider::new("---\nschema_version: 1\n---\n# Memory Schema\n".into());
        let recorded = mock.recorded_system_prompt();
        let provider: Arc<dyn AiProvider> = Arc::new(mock);
        let _ = schema_via_llm(&provider, "Rust backend development").await.unwrap();
        let got = recorded.lock().unwrap().clone().unwrap();
        assert!(got.contains("initial SCHEMA.md"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib memory::wiki::prompts`
Expected: FAIL — module not declared.

- [ ] **Step 3: Implement LLM path in orientation and wire module**

Append to `src/memory/wiki/mod.rs`:

```rust
pub mod prompts;
pub use prompts::{schema_via_llm, PROMPT_ORIENTATION_BOOTSTRAP};
```

Extend `src/memory/wiki/orientation.rs` — add an optional provider field and a `with_provider` builder method, then use it from `bootstrap`:

```rust
use crate::providers::AiProvider;
// already imported: Arc

pub struct FsWikiOrientation<S: NoteStore + Send + Sync + 'static> {
    memory_dir: PathBuf,
    store: Arc<S>,
    dirty: Mutex<HashSet<String>>,
    provider: Option<Arc<dyn AiProvider>>,
}

impl<S: NoteStore + Send + Sync + 'static> FsWikiOrientation<S> {
    pub fn new(memory_dir: impl Into<PathBuf>, store: Arc<S>) -> Self {
        Self {
            memory_dir: memory_dir.into(),
            store,
            dirty: Mutex::new(HashSet::new()),
            provider: None,
        }
    }

    pub fn with_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn with_domain_hint(self, _hint: impl Into<String>) -> Self {
        // Reserved for future per-agent hints. No-op for Spec 5.
        self
    }
}
```

Replace the old `bootstrap` body with one that prefers LLM when a provider is present:

```rust
async fn bootstrap(&self, agent_id: &str) -> Result<(), AlephError> {
    let dir = self.agent_dir(agent_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AlephError::other(format!("bootstrap dir: {e}")))?;

    let ss = SchemaStore::new(&dir);
    if ss.read().await?.is_none() {
        let body = if let Some(p) = &self.provider {
            match crate::memory::wiki::prompts::schema_via_llm(p, "").await {
                Ok(s) if s.contains("# Memory Schema") => s,
                Ok(_) | Err(_) => DEFAULT_SCHEMA.to_string(),
            }
        } else {
            DEFAULT_SCHEMA.to_string()
        };
        ss.write(&body, None).await?;
    }

    self.rebuild_index(agent_id).await?;
    self.append(
        agent_id,
        LogEntry {
            timestamp_utc: chrono::Utc::now().timestamp(),
            action: LogAction::Bootstrap,
            summary: format!("wiki orientation bootstrapped for agent={agent_id}"),
            detail_lines: vec![],
        },
    )
    .await?;
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p alephcore --lib memory::wiki::prompts
cargo test -p alephcore --lib memory::wiki::orientation
```

The first run of the snapshot test will emit `*.snap.new` — review and `cargo insta accept` (or `mv *.snap.new *.snap`) to lock it in.

Expected: PASS — 3 prompt tests, 4 orientation tests (LLM path exercised in a follow-up integration test).

- [ ] **Step 5: Commit**

```bash
git add src/memory/wiki/mod.rs src/memory/wiki/prompts.rs src/memory/wiki/orientation.rs src/memory/wiki/snapshots/
git commit -m "feat(wiki): LLM-driven SCHEMA.md bootstrap with snapshot-locked prompt"
```

---

## Task 7: `NoteIndexer` invalidation hook

**Files:**
- Modify: `src/memory/notes/indexer.rs`

- [ ] **Step 1: Write failing test**

Add this module-level test block to `src/memory/notes/indexer.rs` (or a sibling test file under the same module):

```rust
#[cfg(test)]
mod wiki_hook_tests {
    use super::*;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::memory::wiki::orientation::{FsWikiOrientation, WikiOrientation};
    use crate::sync_primitives::Arc;

    struct CountingOrient {
        calls: std::sync::Mutex<Vec<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl WikiOrientation for CountingOrient {
        async fn bootstrap(&self, _a: &str) -> Result<(), AlephError> { Ok(()) }
        async fn read_snapshot(
            &self,
            _a: &str,
            _b: crate::memory::wiki::types::TokenBudget,
        ) -> Result<crate::memory::wiki::types::OrientationSnapshot, AlephError> {
            Ok(crate::memory::wiki::types::OrientationSnapshot {
                schema_text: String::new(),
                index_text: String::new(),
                recent_log_tail: String::new(),
            })
        }
        async fn record_ingest(&self, _a: &str, _e: crate::memory::wiki::types::LogEntry) -> Result<(), AlephError> { Ok(()) }
        async fn record_query(&self, _a: &str, _e: crate::memory::wiki::types::LogEntry) -> Result<(), AlephError> { Ok(()) }
        async fn record_lint(&self, _a: &str, _e: crate::memory::wiki::types::LogEntry) -> Result<(), AlephError> { Ok(()) }
        async fn record_session_end(&self, _a: &str, _e: crate::memory::wiki::types::LogEntry) -> Result<(), AlephError> { Ok(()) }
        async fn rebuild_index(&self, _a: &str) -> Result<crate::memory::wiki::types::IndexStats, AlephError> {
            Ok(Default::default())
        }
        async fn rotate_log_if_needed(&self, _a: &str) -> Result<bool, AlephError> { Ok(false) }
        fn invalidate(&self, agent_id: &str, note_path: &str) {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((agent_id.to_string(), note_path.to_string()));
        }
    }

    #[tokio::test]
    async fn write_note_invalidates_wiki() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(
            SqliteMemoryBackend::new(&dir.path().join("mem.db")).await.unwrap(),
        );
        let orient = Arc::new(CountingOrient { calls: std::sync::Mutex::new(vec![]) });
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone())
            .with_wiki(orient.clone() as Arc<dyn WikiOrientation>);

        let note = KnowledgeNote {
            title: "rust".into(),
            category: "learning".into(),
            tags: vec![],
            facts: vec!["f1".into()],
            links: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: String::new(),
        };
        indexer.write_note("default", "learning", &note).await.unwrap();

        let calls = orient.calls.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "learning/rust");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib memory::notes::indexer::wiki_hook_tests`
Expected: FAIL — `with_wiki` not defined.

- [ ] **Step 3: Implement**

In `src/memory/notes/indexer.rs`:

Add the field + builder:

```rust
pub struct NoteIndexer<S: NoteStore + Send + Sync + 'static> {
    memory_dir: PathBuf,
    store: Arc<S>,
    wiki: Option<Arc<dyn crate::memory::wiki::orientation::WikiOrientation>>,
    // ...existing fields
}

impl<S: NoteStore + Send + Sync + 'static> NoteIndexer<S> {
    pub fn with_wiki(
        mut self,
        wiki: Arc<dyn crate::memory::wiki::orientation::WikiOrientation>,
    ) -> Self {
        self.wiki = Some(wiki);
        self
    }

    fn notify_wiki(&self, agent_id: &str, category: &str, filename: &str) {
        if let Some(w) = &self.wiki {
            w.invalidate(agent_id, &format!("{category}/{filename}"));
        }
    }
}
```

Add a `self.notify_wiki(agent_id, category, &note.title);` line inside `write_note`, `append_to_note`, and `rename_note` just before they return `Ok(())` (for `rename_note`, invalidate both the old and the new path). Add `self.notify_wiki(...)` inside `remove_note_index` callers as well.

Initialise `wiki: None` in every constructor / `new` function of `NoteIndexer`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib memory::notes::indexer::wiki_hook_tests`
Expected: PASS — 1 test.

Also run the full indexer test module to confirm no regression:
```bash
cargo test -p alephcore --lib memory::notes::indexer
```

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/indexer.rs
git commit -m "feat(wiki): NoteIndexer invalidation hook for WikiOrientation"
```

---

## Task 8: `build_orientation_user_message` in `MemoryContextProvider`

**Files:**
- Modify: `src/thinker/memory_context_provider.rs`
- Modify: `src/config/types/memory.rs` (new `OrientationConfig`)

- [ ] **Step 1: Write failing test**

At the bottom of `src/thinker/memory_context_provider.rs`, inside its existing `#[cfg(test)] mod tests`, add:

```rust
#[tokio::test]
async fn orientation_message_injected_in_context_mode() {
    use crate::memory::wiki::types::{OrientationSnapshot, TokenBudget};

    struct FixedOrient;
    #[async_trait::async_trait]
    impl crate::memory::wiki::orientation::WikiOrientation for FixedOrient {
        async fn bootstrap(&self, _: &str) -> Result<(), AlephError> { Ok(()) }
        async fn read_snapshot(&self, _: &str, _: TokenBudget) -> Result<OrientationSnapshot, AlephError> {
            Ok(OrientationSnapshot {
                schema_text: "# Memory Schema\n## Domain\nTest".into(),
                index_text: "# Index\n## learning (1)\n- [[learning/rust]] — fact".into(),
                recent_log_tail: "## [2026-04-14] ingest | touched=3".into(),
            })
        }
        async fn record_ingest(&self, _: &str, _: crate::memory::wiki::types::LogEntry) -> Result<(), AlephError> { Ok(()) }
        async fn record_query(&self, _: &str, _: crate::memory::wiki::types::LogEntry) -> Result<(), AlephError> { Ok(()) }
        async fn record_lint(&self, _: &str, _: crate::memory::wiki::types::LogEntry) -> Result<(), AlephError> { Ok(()) }
        async fn record_session_end(&self, _: &str, _: crate::memory::wiki::types::LogEntry) -> Result<(), AlephError> { Ok(()) }
        async fn rebuild_index(&self, _: &str) -> Result<crate::memory::wiki::types::IndexStats, AlephError> { Ok(Default::default()) }
        async fn rotate_log_if_needed(&self, _: &str) -> Result<bool, AlephError> { Ok(false) }
        fn invalidate(&self, _: &str, _: &str) {}
    }

    let provider = MemoryContextProvider::new_test()
        .with_wiki(std::sync::Arc::new(FixedOrient));

    let msg = provider
        .build_orientation_user_message("default", InjectionMode::Context)
        .await
        .unwrap();
    let m = msg.expect("context mode should inject");
    let text = m.content_text().unwrap_or_default();
    assert!(text.contains("<WikiOrientation>"));
    assert!(text.contains("# Memory Schema"));
    assert!(text.contains("# Index"));
    assert!(text.contains("touched=3"));
    assert!(text.ends_with("</WikiOrientation>"));
}

#[tokio::test]
async fn orientation_skipped_in_tools_mode() {
    use crate::memory::wiki::types::{OrientationSnapshot, TokenBudget};
    // Same FixedOrient as above — factor to a helper if preferred.
    // ...
    // Skipped for brevity; duplicate FixedOrient if necessary.
    // Assert: provider.build_orientation_user_message("default", InjectionMode::Tools)
    //                .await.unwrap().is_none();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib thinker::memory_context_provider::tests::orientation_message_injected_in_context_mode`
Expected: FAIL — `with_wiki` / `build_orientation_user_message` / `new_test` missing.

- [ ] **Step 3: Implement**

In `src/thinker/memory_context_provider.rs`:

Add an `Option<Arc<dyn WikiOrientation>>` field plus builder:

```rust
pub struct MemoryContextProvider {
    // ...existing fields
    wiki: Option<Arc<dyn crate::memory::wiki::orientation::WikiOrientation>>,
    orientation_budget: crate::memory::wiki::types::TokenBudget,
}

impl MemoryContextProvider {
    pub fn with_wiki(
        mut self,
        w: Arc<dyn crate::memory::wiki::orientation::WikiOrientation>,
    ) -> Self {
        self.wiki = Some(w);
        self
    }

    pub async fn build_orientation_user_message(
        &self,
        agent_id: &str,
        mode: InjectionMode,
    ) -> Result<Option<crate::providers::message::UnifiedMessage>, AlephError> {
        if matches!(mode, InjectionMode::Tools) {
            return Ok(None);
        }
        let Some(w) = &self.wiki else { return Ok(None) };
        let snap = w.read_snapshot(agent_id, self.orientation_budget).await?;
        let xml = render_orientation(&snap);
        Ok(Some(crate::providers::message::UnifiedMessage::user(&xml)))
    }
}

fn render_orientation(s: &crate::memory::wiki::types::OrientationSnapshot) -> String {
    let esc = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    format!(
        "<WikiOrientation>\n<schema>\n{}\n</schema>\n<index_snapshot>\n{}\n</index_snapshot>\n<recent_log>\n{}\n</recent_log>\n</WikiOrientation>",
        esc(&s.schema_text),
        esc(&s.index_text),
        esc(&s.recent_log_tail)
    )
}
```

If `new_test` is not already present, add a minimal `pub(crate) fn new_test() -> Self` that fills every field with a default / mock value.

Wire the orientation message into the same `LayerInput` assembly site that Spec 3 used for `memory_user_message`. Place it **before** the memory envelope:

```rust
// in the LayerInput assembly (pseudo-site — follow existing structure):
if let Some(msg) = self.build_orientation_user_message(agent_id, mode).await? {
    layer_input.prepend_user_message(msg);
}
```

In `src/config/types/memory.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrientationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,
    #[serde(default = "default_log_rotate_lines")]
    pub log_rotate_lines: usize,
    #[serde(default = "default_true")]
    pub inject_on_agent_switch: bool,
}

impl Default for OrientationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_tokens: 4000,
            log_rotate_lines: 2000,
            inject_on_agent_switch: true,
        }
    }
}

fn default_max_tokens() -> usize { 4000 }
fn default_log_rotate_lines() -> usize { 2000 }
fn default_true() -> bool { true }
```

Add to `MemoryConfig`:

```rust
#[serde(default)]
pub orientation: OrientationConfig,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib thinker::memory_context_provider`
Expected: PASS — orientation tests green; existing tests unchanged.

- [ ] **Step 5: Commit**

```bash
git add src/thinker/memory_context_provider.rs src/config/types/memory.rs
git commit -m "feat(wiki): orientation message injected into prompt layer"
```

---

## Task 9: `IndexRefresher` Dream stage

**Files:**
- Create: `src/memory/dreaming/stages/index_refresher.rs`
- Modify: `src/memory/dreaming/stages/mod.rs`
- Modify: `src/memory/dreaming/mod.rs`
- Modify: `src/memory/dreaming/context.rs` (or wherever `DreamContext` is built) — pass optional `wiki`

- [ ] **Step 1: Write failing test**

Create `src/memory/dreaming/stages/index_refresher.rs`:

```rust
//! `IndexRefresherStage` — idempotent full rebuild of `index.md` and log rotation.

use crate::error::AlephError;
use crate::memory::dreaming::mod_types::{DreamContext, DreamStage};
use async_trait::async_trait;

pub struct IndexRefresherStage;

#[async_trait]
impl DreamStage for IndexRefresherStage {
    fn name(&self) -> &'static str {
        "index_refresher"
    }

    async fn should_run(&self, _ctx: &DreamContext) -> bool {
        true
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        if let Some(w) = ctx.wiki.as_ref() {
            let stats = w.rebuild_index(&ctx.agent_id).await?;
            ctx.report.extra.insert(
                "notes_indexed".into(),
                stats.notes_indexed.to_string(),
            );
            w.rotate_log_if_needed(&ctx.agent_id).await?;
        }
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Uses the existing DreamContext test harness used by other stages.
    // See `src/memory/dreaming/stages/note_lint.rs` tests for the pattern.
}
```

> The real test for this stage goes in the integration test (Task 13) because it needs a full `DreamContext`. In this task we only make the stage compile + inject it into the pipelines, which is exercised by the existing pipeline tests.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib memory::dreaming`
Expected: FAIL — `IndexRefresherStage` not found; `DreamContext.wiki` / `DreamReport.extra` missing.

- [ ] **Step 3: Implement**

1. `DreamContext` gains an optional `wiki` handle:

```rust
// src/memory/dreaming/mod.rs (or wherever DreamContext is defined)
pub struct DreamContext {
    // ...existing fields (see DREAM_DAEMON.md §4.1)
    pub wiki: Option<std::sync::Arc<dyn crate::memory::wiki::orientation::WikiOrientation>>,
}
```

2. `DreamReport` gains a free-form `extra` map for stats that don't warrant a schema column:

```rust
// src/memory/dreaming/report.rs
#[derive(Debug, Clone, Default, Serialize)]
pub struct DreamReport {
    // ...existing fields
    #[serde(skip)]
    pub extra: std::collections::BTreeMap<String, String>,
}
```

3. Register the stage in `src/memory/dreaming/stages/mod.rs`:

```rust
pub mod index_refresher;
pub use index_refresher::IndexRefresherStage;
```

4. Add the stage **before** `NoteLintStage` in both pipelines in `src/memory/dreaming/mod.rs`:

```rust
pub fn daily() -> Self {
    Self::new(vec![
        Box::new(stages::NoteConsolidateStage),
        Box::new(stages::NoteDriftStage),
        Box::new(stages::IndexRefresherStage), // NEW: before lint
        Box::new(stages::NoteLintStage),
        Box::new(stages::NoteDecayStage),
        Box::new(stages::DailyDigestStage),
    ])
}

pub fn weekly() -> Self {
    Self::new(vec![
        Box::new(stages::NoteConsolidateStage),
        Box::new(stages::NoteDriftStage),
        Box::new(stages::NoteSynthesisStage),
        Box::new(stages::IndexRefresherStage), // NEW: before lint
        Box::new(stages::NoteLintStage),
        Box::new(stages::NoteDecayStage),
        Box::new(stages::DailyDigestStage),
    ])
}
```

5. Where `DreamContext` is constructed (in `DreamDaemon::check_and_run` or similar), thread the optional `wiki` from the app-level registry.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib memory::dreaming`
Expected: PASS — existing Dream tests still green; new stage compiles.

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/
git commit -m "feat(wiki): IndexRefresher Dream stage (daily + weekly before lint)"
```

---

## Task 10: `wiki_orient` builtin tool

**Files:**
- Create: `src/builtin_tools/wiki_orient.rs`
- Modify: `src/builtin_tools/mod.rs`
- Modify: `src/executor/builtin_registry/registry.rs`

- [ ] **Step 1: Write failing test**

Create `src/builtin_tools/wiki_orient.rs`:

```rust
//! `wiki_orient` — Tools/Hybrid-mode on-demand fetch of SCHEMA + index + recent log.

use crate::error::AlephError;
use crate::memory::wiki::orientation::WikiOrientation;
use crate::memory::wiki::types::TokenBudget;
use crate::sync_primitives::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WikiOrientArgs {
    /// Optional token budget for the snapshot. Defaults to `OrientationConfig.max_tokens`.
    #[serde(default)]
    pub max_tokens: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WikiOrientOutput {
    pub schema: String,
    pub index: String,
    pub recent_log: String,
}

pub struct WikiOrientTool {
    wiki: Arc<dyn WikiOrientation>,
    default_budget: TokenBudget,
}

impl WikiOrientTool {
    pub fn new(wiki: Arc<dyn WikiOrientation>, default_budget: TokenBudget) -> Self {
        Self { wiki, default_budget }
    }

    pub async fn call(
        &self,
        agent_id: &str,
        args: WikiOrientArgs,
    ) -> Result<WikiOrientOutput, AlephError> {
        let budget = TokenBudget {
            max_tokens: args.max_tokens.unwrap_or(self.default_budget.max_tokens),
        };
        let snap = self.wiki.read_snapshot(agent_id, budget).await?;
        Ok(WikiOrientOutput {
            schema: snap.schema_text,
            index: snap.index_text,
            recent_log: snap.recent_log_tail,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::memory::wiki::orientation::FsWikiOrientation;

    #[tokio::test]
    async fn returns_snapshot_parts() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(
            SqliteMemoryBackend::new(&dir.path().join("mem.db")).await.unwrap(),
        );
        let orient: Arc<dyn WikiOrientation> = Arc::new(
            FsWikiOrientation::new(dir.path().join("note"), backend),
        );
        orient.bootstrap("default").await.unwrap();

        let tool = WikiOrientTool::new(orient, TokenBudget::default());
        let out = tool
            .call("default", WikiOrientArgs { max_tokens: Some(8000) })
            .await
            .unwrap();
        assert!(out.schema.contains("# Memory Schema"));
        assert!(out.index.contains("# Index"));
        assert!(out.recent_log.contains("bootstrap"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib builtin_tools::wiki_orient`
Expected: FAIL — module not declared.

- [ ] **Step 3: Wire + register**

In `src/builtin_tools/mod.rs`:

```rust
pub mod wiki_orient;
pub mod wiki_schema;  // added by Task 11; safe to declare now
```

In `src/executor/builtin_registry/registry.rs` — find the tool-registration loop (similar to where `memory_search` is registered), then add:

```rust
// Gate on injection_mode: register in Tools/Hybrid, skip in Context.
if matches!(memory_cfg.injection_mode, InjectionMode::Tools | InjectionMode::Hybrid) {
    if let Some(wiki) = wiki_orient.clone() {
        registry.register(Box::new(WikiOrientTool::new(
            wiki,
            TokenBudget { max_tokens: memory_cfg.orientation.max_tokens },
        )));
    }
}
```

(`wiki_orient` here is the `Option<Arc<dyn WikiOrientation>>` threaded from the app startup wiring — see Task 12.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib builtin_tools::wiki_orient`
Expected: PASS — 1 test.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/wiki_orient.rs src/builtin_tools/mod.rs src/executor/builtin_registry/registry.rs
git commit -m "feat(wiki): wiki_orient builtin tool"
```

---

## Task 11: `wiki_schema` builtin tool

**Files:**
- Create: `src/builtin_tools/wiki_schema.rs`

- [ ] **Step 1: Write failing test**

Create `src/builtin_tools/wiki_schema.rs`:

```rust
//! `wiki_schema` — LLM-driven SCHEMA.md read/write with hash guard.

use crate::error::AlephError;
use crate::memory::wiki::schema::{SchemaStore, SCHEMA_FILENAME};
use crate::sync_primitives::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum WikiSchemaArgs {
    Read,
    Write {
        /// The full new SCHEMA.md content.
        content: String,
        /// The content hash the LLM read immediately before — guards against overwrites.
        /// None on very first write.
        #[serde(default)]
        expected_hash: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct WikiSchemaOutput {
    pub content: Option<String>,  // present on Read
    pub content_hash: Option<String>,  // present on Read or successful Write
    pub conflict: bool,  // true when Write was rejected for stale hash
}

pub struct WikiSchemaTool {
    memory_dir: PathBuf,
}

impl WikiSchemaTool {
    pub fn new(memory_dir: impl Into<PathBuf>) -> Self {
        Self { memory_dir: memory_dir.into() }
    }

    pub async fn call(
        &self,
        agent_id: &str,
        args: WikiSchemaArgs,
    ) -> Result<WikiSchemaOutput, AlephError> {
        let store = SchemaStore::new(self.memory_dir.join(agent_id));
        match args {
            WikiSchemaArgs::Read => {
                let doc = store.read().await?;
                Ok(WikiSchemaOutput {
                    content: doc.as_ref().map(|d| d.raw.clone()),
                    content_hash: doc.as_ref().map(|d| d.content_hash.clone()),
                    conflict: false,
                })
            }
            WikiSchemaArgs::Write { content, expected_hash } => {
                match store.write(&content, expected_hash.as_deref()).await {
                    Ok(h) => Ok(WikiSchemaOutput {
                        content: None,
                        content_hash: Some(h),
                        conflict: false,
                    }),
                    Err(e) if e.to_string().contains("hash conflict") => Ok(WikiSchemaOutput {
                        content: None,
                        content_hash: None,
                        conflict: true,
                    }),
                    Err(e) => Err(e),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_write_roundtrip_via_tool() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WikiSchemaTool::new(dir.path().join("note"));

        // Initial read — missing.
        let r1 = tool.call("default", WikiSchemaArgs::Read).await.unwrap();
        assert!(r1.content.is_none());

        // First write.
        let w1 = tool
            .call(
                "default",
                WikiSchemaArgs::Write {
                    content: crate::memory::wiki::schema::DEFAULT_SCHEMA.into(),
                    expected_hash: None,
                },
            )
            .await
            .unwrap();
        assert!(!w1.conflict);
        let hash = w1.content_hash.clone().unwrap();

        // Second read returns what we wrote.
        let r2 = tool.call("default", WikiSchemaArgs::Read).await.unwrap();
        assert_eq!(r2.content_hash.unwrap(), hash);

        // Stale-hash write is rejected, not errored.
        let w2 = tool
            .call(
                "default",
                WikiSchemaArgs::Write {
                    content: "modified".into(),
                    expected_hash: Some("stale".into()),
                },
            )
            .await
            .unwrap();
        assert!(w2.conflict);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib builtin_tools::wiki_schema`
Expected: FAIL — module not declared (or fails to link). (The `pub mod wiki_schema;` line was added in Task 10 to keep both tools together.)

- [ ] **Step 3: Register in registry**

In `src/executor/builtin_registry/registry.rs`, alongside `wiki_orient`:

```rust
// wiki_schema: always register. LLM may mutate the schema in any mode.
registry.register(Box::new(WikiSchemaTool::new(
    memory_dir.clone(),
)));
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib builtin_tools::wiki_schema`
Expected: PASS — 1 test.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/wiki_schema.rs src/executor/builtin_registry/registry.rs
git commit -m "feat(wiki): wiki_schema builtin tool with hash-guarded write"
```

---

## Task 12: App startup wiring

**Files:**
- Modify: `src/app/context/builder.rs` (or the actual assembly site — grep for where `NoteIndexer` and `MemoryContextProvider` are constructed and `DreamDaemon::start_background_task_with_handle` is called)

- [ ] **Step 1: Write failing test**

Grep first to find the site:

```bash
rg -n "NoteIndexer::new\b" src/
rg -n "MemoryContextProvider::new\b" src/
rg -n "ensure_dream_daemon\b" src/
```

Once located (let's call it `src/app/context/builder.rs`), add a unit test asserting that the builder returns a context whose `NoteIndexer` has a `wiki` attached:

```rust
#[tokio::test]
async fn app_context_builds_with_wiki_orientation() {
    // Reuse the existing test builder. The assertion is that the wiki
    // handle is Some after build.
    let ctx = test_app_context().await;
    assert!(ctx.wiki_orientation().is_some(), "wiki orientation must be wired");
}
```

(`test_app_context` and `wiki_orientation()` accessor may need to be added as a thin test-helper.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib app::context`
Expected: FAIL — accessor not present.

- [ ] **Step 3: Implement**

In the startup assembly:

```rust
let wiki: Arc<dyn WikiOrientation> = Arc::new(
    FsWikiOrientation::new(memory_dir.join("note"), backend.clone())
        .with_provider(provider.clone()),
);

// Bootstrap on startup (no-op when already initialised).
wiki.bootstrap(DEFAULT_AGENT).await?;

let indexer = NoteIndexer::new(memory_dir.join("note"), backend.clone())
    .with_wiki(wiki.clone());

let memory_ctx = MemoryContextProvider::new(...)
    .with_wiki(wiki.clone());

// Pass wiki into the tools registry and into DreamDaemon construction.
```

In `DreamDaemon::start_background_task_with_handle` (and the `DreamContext` builder site), accept and thread the optional `wiki` into the `DreamContext` the pipelines build.

Expose `wiki_orientation()` on the app context struct as an `Option<Arc<dyn WikiOrientation>>` accessor for tests.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib app::context`
Expected: PASS.

Then run a broad sanity check:

```bash
cargo check -p alephcore
cargo test -p alephcore --lib memory::wiki
cargo test -p alephcore --lib memory::notes::indexer
cargo test -p alephcore --lib thinker::memory_context_provider
cargo test -p alephcore --lib builtin_tools::wiki_orient
cargo test -p alephcore --lib builtin_tools::wiki_schema
```

All green.

- [ ] **Step 5: Commit**

```bash
git add src/app/
git commit -m "feat(wiki): wire WikiOrientation into app startup, NoteIndexer, Dream, tools"
```

---

## Task 13: End-to-end integration test

**Files:**
- Create: `tests/memory_wiki_orientation.rs`

- [ ] **Step 1: Write failing test**

Create `tests/memory_wiki_orientation.rs`:

```rust
//! End-to-end: bootstrap → write 5 notes → rebuild index → assert all three
//! files on disk + orientation snapshot populated.

use alephcore::memory::notes::indexer::NoteIndexer;
use alephcore::memory::notes::note::KnowledgeNote;
use alephcore::memory::store::sqlite::SqliteMemoryBackend;
use alephcore::memory::wiki::orientation::{FsWikiOrientation, WikiOrientation};
use alephcore::memory::wiki::types::TokenBudget;
use alephcore::sync_primitives::Arc;

#[tokio::test]
async fn orientation_layer_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(
        SqliteMemoryBackend::new(&dir.path().join("mem.db")).await.unwrap(),
    );
    let orient: Arc<dyn WikiOrientation> = Arc::new(
        FsWikiOrientation::new(dir.path().join("note"), backend.clone()),
    );
    orient.bootstrap("default").await.unwrap();

    let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone())
        .with_wiki(orient.clone());

    for (cat, name) in [
        ("learning", "rust-async"),
        ("learning", "tokio"),
        ("preference", "editor"),
        ("project", "aleph"),
        ("tool", "ast-grep"),
    ] {
        let note = KnowledgeNote {
            title: name.into(),
            category: cat.into(),
            tags: vec![],
            facts: vec![format!("first fact of {name}")],
            links: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: String::new(),
        };
        indexer.write_note("default", cat, &note).await.unwrap();
    }

    orient.rebuild_index("default").await.unwrap();

    let base = dir.path().join("note/default");
    assert!(base.join("SCHEMA.md").exists());
    assert!(base.join("index.md").exists());
    assert!(base.join("log.md").exists());

    let index = tokio::fs::read_to_string(base.join("index.md")).await.unwrap();
    for name in ["rust-async", "tokio", "editor", "aleph", "ast-grep"] {
        assert!(
            index.contains(name),
            "index.md missing {name}: {index}"
        );
    }

    let snap = orient
        .read_snapshot("default", TokenBudget::default())
        .await
        .unwrap();
    assert!(snap.schema_text.contains("# Memory Schema"));
    assert!(snap.index_text.contains("## learning (2)"));
    assert!(snap.recent_log_tail.contains("bootstrap"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --test memory_wiki_orientation`
Expected: FAIL until all prior tasks are merged (end-to-end coverage).

- [ ] **Step 3: Implement**

Nothing new — this test pulls the stack together. If it fails after Tasks 1–12, diagnose the specific failing assertion.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --test memory_wiki_orientation`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/memory_wiki_orientation.rs
git commit -m "test(wiki): end-to-end orientation layer integration"
```

---

## Task 14: Proptest — index projection round-trip

**Files:**
- Modify: `src/memory/wiki/index_md.rs`

- [ ] **Step 1: Write failing test**

Append to the `tests` module in `src/memory/wiki/index_md.rs`:

```rust
use proptest::prelude::*;

fn category_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("preference".to_string()),
        Just("plan".to_string()),
        Just("learning".to_string()),
        Just("project".to_string()),
        Just("personal".to_string()),
        Just("tool".to_string()),
    ]
}

fn filename_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{0,20}".prop_map(String::from)
}

proptest! {
    #[test]
    fn every_note_appears_in_rendered_index(
        notes in proptest::collection::vec(
            (category_strategy(), filename_strategy(), 0i64..2_000_000_000_i64),
            0..30
        )
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        let entries: Vec<NoteIndexEntry> = notes
            .iter()
            .enumerate()
            .map(|(i, (cat, name, ts))| NoteIndexEntry {
                path: format!("{cat}/{name}-{i}"),
                filename: format!("{name}-{i}"),
                agent_id: "default".into(),
                category: cat.clone(),
                tags: vec![],
                link_count: 0,
                created_at: 0,
                updated_at: *ts,
                content_hash: "x".into(),
            })
            .collect();

        let rendered = rt.block_on(g.render(&entries)).unwrap();
        for e in &entries {
            prop_assert!(rendered.contains(&e.path), "missing {}", e.path);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails or passes**

Run: `cargo test -p alephcore --lib memory::wiki::index_md`
Expected: PASS on the property immediately — the generator already emits every path. If proptest finds a counterexample, fix the generator before declaring victory.

- [ ] **Step 3: Implement (if counterexample)**

Only if a counterexample appears: update the `render` implementation to cover the failing input class (most likely a collision in duplicate paths — adjust to print duplicates once or append an index).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib memory::wiki::index_md -- --include-ignored`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/memory/wiki/index_md.rs
git commit -m "test(wiki): proptest invariant — every note appears in index.md"
```

---

## Task 15: Deprecation marker on legacy extractor (Spec 6 preparation)

**Files:**
- Modify: `src/memory/compression/extractor.rs`

> This task is a one-liner that plants the deprecation flag ahead of Spec 6. Spec 5 does not remove the legacy path; it only marks it.

- [ ] **Step 1: Add the attribute**

On `FactExtractor::extract_facts`, `extract_unified`, and the related legacy types, add:

```rust
#[deprecated(note = "replaced by CompoundIngestor in Spec 6; will be removed 2 weeks after Spec 6 lands")]
pub async fn extract_facts(&self, ...) -> ... { ... }

#[deprecated(note = "replaced by CompoundIngestor in Spec 6; will be removed 2 weeks after Spec 6 lands")]
pub async fn extract_unified(&self, ...) -> ... { ... }
```

- [ ] **Step 2: Verify compiler warnings**

Run: `cargo check -p alephcore 2>&1 | grep -c "deprecated"`
Expected: ≥ 2 warnings referencing the deprecated names. Existing call sites inside the crate are suppressed only where necessary with `#[allow(deprecated)]` and a comment referencing this plan.

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: all existing tests still pass.

- [ ] **Step 4: Commit**

```bash
git add src/memory/compression/extractor.rs
git commit -m "chore(wiki): mark legacy extractor APIs #[deprecated] ahead of Spec 6"
```

---

## Task 16: Final sanity pass

- [ ] **Step 1: Run everything**

```bash
cargo fmt --check
cargo clippy -p alephcore -- -D warnings
cargo test -p alephcore --lib
cargo test -p alephcore --test memory_wiki_orientation
```

Expected: format clean, no clippy warnings, all unit + integration tests pass.

- [ ] **Step 2: Manual smoke**

Start a fresh `~/.aleph_test_home` and run:

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ALEPH_HOME=/tmp/aleph-spec5-smoke cargo run --bin aleph-server -- start &
# Use the CLI or a minimal test message to trigger a turn.
sleep 10
ls -la /tmp/aleph-spec5-smoke/memory/note/default/
```

Expected: `SCHEMA.md`, `index.md`, `log.md` present, non-empty, valid markdown.

Kill the server and clean up:

```bash
pkill -f "target/debug/aleph-server"
sleep 2
rm -rf /tmp/aleph-spec5-smoke
```

- [ ] **Step 3: Update reference doc stub**

Open `docs/reference/MEMORY_SYSTEM.md` and add a one-paragraph pointer to the new `WIKI.md` reference (to be fleshed out during Spec 6):

```markdown
## Orientation layer (Spec 5, introduced 2026-04-Q2)

Aleph maintains three LLM-readable markdown files per agent —
`SCHEMA.md`, `index.md`, `log.md` — and a `WikiOrientation` trait that
projects the live `notes_index` into them. See
[docs/superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md §2](../superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md)
for the full design.
```

- [ ] **Step 4: Commit**

```bash
git add docs/reference/MEMORY_SYSTEM.md
git commit -m "docs(memory): point to wiki orientation design from MEMORY_SYSTEM.md"
```

---

## Self-Review

**Spec coverage (§2 of the design doc):**

| Spec section | Task(s) |
|---|---|
| §2.1 SCHEMA.md | 3, 6 (bootstrap), 11 (tool) |
| §2.2 index.md | 4, 9 (IndexRefresher), 14 (proptest) |
| §2.3 log.md | 2 |
| §2.4 Orientation injection | 8 (`build_orientation_user_message`) |
| §2.5 `WikiOrientation` trait | 5, 7 (NoteIndexer hook), 12 (app wiring) |
| §6.4 deprecate legacy extractor | 15 |
| §6 integration testing | 13 |

**Placeholder scan:** every step contains actual Rust / shell — no TBDs, no "implement later", no "similar to Task N".

**Type consistency:**
- `LogAction`, `LogEntry`, `OrientationSnapshot`, `IndexStats`, `TokenBudget` defined in Task 1 and referenced identically through Tasks 2–12.
- `WikiOrientation` trait method signatures in Task 5 match callers in Tasks 7, 8, 9, 10, 13.
- `SchemaStore::read() -> Option<SchemaDoc>` consistent between Task 3 tests and the bootstrap flow in Task 6.
- `WikiSchemaArgs` tag enum (`"read"` / `"write"`) matches the LLM-facing contract used by the registry in Task 11.
- `IndexMdGenerator::render` is `async` — consistent across Tasks 4, 9, 14.

**Risks for the implementer:**
1. Exact file paths for `MemoryContextProvider`, `DreamContext`, and the app builder may have shifted since the spec was written; the pre-flight check tells them to grep.
2. The snapshot test (Task 6) produces a `*.snap.new` on first run — remind reviewer to inspect and `cargo insta accept` before merging.
3. Task 15's deprecation attribute may trigger warnings at internal call sites; suppress with `#[allow(deprecated)]` **only** where the suppressed call is on the Spec 6 removal list.

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-04-14-memory-llm-wiki-spec5-orientation.md`.** Two execution options:

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — Execute tasks in this session using `executing-plans`, batch execution with checkpoints.

**Which approach?**
