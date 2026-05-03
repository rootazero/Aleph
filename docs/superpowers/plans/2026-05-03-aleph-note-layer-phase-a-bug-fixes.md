# Aleph Note Layer — Phase A: Bug Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix six correctness bugs in Aleph's note layer (wikilink pipe-alias, link resolution storage, YAML quoting, date round-trip, fact parser sub-bullets, sanitize_title empty-result) without breaking the markdown-first contract.

**Architecture:** Single-pass surgical fixes touching `src/memory/notes/{note.rs, wikilink.rs, indexer.rs}`, `src/memory/store/sqlite/{schema.rs, notes.rs}`, and `src/memory/dreaming/stages/note_lint.rs`. Schema migration adds one column (`notes_links.to_raw`) and one composite index (`idx_notes_filename_agent`). All changes are independently shippable; A2's schema change is required by Phase B (B1 diff-upsert).

**Tech Stack:** Rust 2021, rusqlite + sqlite-vec, tokio, regex, serde_yaml, sha2.

**Spec:** `docs/superpowers/specs/2026-05-03-aleph-note-layer-llm-wiki-optimization-design.md` §2 (Phase A). All A1–A7 sub-section IDs map 1:1 with task headings below.

**Verification gate:** All `cargo test -p alephcore --lib memory::notes` plus the new integration tests below must be green; manual `full_rebuild` against the author's `~/.aleph/memory/note/` produces zero new errors.

---

## Task 1 (A1): Wikilink pipe-alias regex

**Files:**
- Modify: `src/memory/notes/wikilink.rs:10` (regex), `:18-23` (extract), `:26-36` (rewrite), `:44-54` (remove)
- Test: same file `#[cfg(test)] mod tests`

- [ ] **Step 1: Write failing tests for pipe-alias forms**

Add to `src/memory/notes/wikilink.rs` `mod tests`:

```rust
#[test]
fn extract_pipe_alias_returns_target_only() {
    let text = "see [[rust|Rust 学习]] and [[plain]]";
    assert_eq!(extract_wikilinks(text), vec!["rust", "plain"]);
}

#[test]
fn extract_with_alias_returns_pairs() {
    let text = "see [[rust|Rust 学习]] and [[plain]]";
    assert_eq!(
        extract_wikilinks_with_alias(text),
        vec![
            ("rust".to_string(), Some("Rust 学习".to_string())),
            ("plain".to_string(), None),
        ]
    );
}

#[test]
fn rewrite_preserves_alias_when_pipe_form() {
    let text = "before [[old|Old Display]] after";
    let result = rewrite_wikilinks(text, "old", "new");
    assert_eq!(result, "before [[new|Old Display]] after");
}

#[test]
fn remove_drops_full_pipe_form() {
    let text = "x [[stale|Stale]] y";
    assert_eq!(remove_wikilink(text, "stale"), "x  y");
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alephcore --lib memory::notes::wikilink::tests::extract_pipe_alias_returns_target_only memory::notes::wikilink::tests::extract_with_alias_returns_pairs memory::notes::wikilink::tests::rewrite_preserves_alias_when_pipe_form memory::notes::wikilink::tests::remove_drops_full_pipe_form
```
Expected: 4 failures (`extract_with_alias_returns_pairs` fails to compile because the function does not exist; the others fail on assertion).

- [ ] **Step 3: Update regex and add `extract_wikilinks_with_alias`**

Replace the regex literal at `src/memory/notes/wikilink.rs:10`:

```rust
static WIKILINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]\|]+)(?:\|([^\]]*))?\]\]").unwrap());
```

Update `extract_wikilinks` to capture only group 1:

```rust
pub fn extract_wikilinks(text: &str) -> Vec<String> {
    WIKILINK_RE
        .captures_iter(text)
        .map(|cap| cap[1].to_string())
        .collect()
}
```

Add the new `extract_wikilinks_with_alias` below it:

```rust
/// Extract wikilink targets and their optional aliases from `text`.
///
/// `[[target]]` yields `(target, None)`; `[[target|alias]]` yields `(target, Some(alias))`.
pub fn extract_wikilinks_with_alias(text: &str) -> Vec<(String, Option<String>)> {
    WIKILINK_RE
        .captures_iter(text)
        .map(|cap| {
            (
                cap[1].to_string(),
                cap.get(2).map(|m| m.as_str().to_string()),
            )
        })
        .collect()
}
```

Update `rewrite_wikilinks` to preserve alias:

```rust
pub fn rewrite_wikilinks(text: &str, old_name: &str, new_name: &str) -> String {
    WIKILINK_RE
        .replace_all(text, |caps: &regex::Captures| {
            if &caps[1] == old_name {
                match caps.get(2) {
                    Some(alias) => format!("[[{new_name}|{}]]", alias.as_str()),
                    None => format!("[[{new_name}]]"),
                }
            } else {
                caps[0].to_string()
            }
        })
        .into_owned()
}
```

`remove_wikilink` already removes the entire matched `[[...]]` token via `caps[0]`; verify by re-reading lines 44-54 — no edit needed because `caps[0]` already covers the alias form once the regex is updated.

- [ ] **Step 4: Run all wikilink tests**

```bash
cargo test -p alephcore --lib memory::notes::wikilink
```
Expected: all green (existing 7 tests + 4 new tests).

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/wikilink.rs
git commit -m "fix(notes): wikilink regex now handles [[target|alias]] form"
```

---

## Task 2 (A2.1): Add `to_raw` column to `notes_links` schema

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs:217-225`
- Modify: `src/memory/store/sqlite/schema.rs` (migration block — add `migrate_notes_links_to_raw`)

- [ ] **Step 1: Write a migration test**

Add to `src/memory/store/sqlite/schema.rs` `mod tests`:

```rust
#[test]
fn migrate_notes_links_adds_to_raw_and_backfills() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // Old schema without to_raw
    conn.execute_batch(
        "CREATE TABLE notes_links (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id TEXT NOT NULL DEFAULT 'default',
            from_note TEXT NOT NULL,
            to_note TEXT NOT NULL,
            UNIQUE(agent_id, from_note, to_note)
        );
        INSERT INTO notes_links (agent_id, from_note, to_note)
            VALUES ('a', 'cat/x', 'rust');",
    )
    .unwrap();

    migrate_notes_links_to_raw(&conn).unwrap();

    let to_raw: String = conn
        .query_row(
            "SELECT to_raw FROM notes_links WHERE from_note='cat/x'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(to_raw, "rust");
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib memory::store::sqlite::schema::tests::migrate_notes_links_adds_to_raw_and_backfills
```
Expected: fails to compile because `migrate_notes_links_to_raw` does not exist.

- [ ] **Step 3: Update DDL and add migration helper**

Replace the `notes_links` block at `src/memory/store/sqlite/schema.rs:217-225`:

```rust
const NOTES_LINKS_DDL: &str = "
CREATE TABLE IF NOT EXISTS notes_links (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    from_note   TEXT NOT NULL,
    to_note     TEXT NOT NULL,
    to_raw      TEXT NOT NULL,
    UNIQUE(agent_id, from_note, to_note)
);
CREATE INDEX IF NOT EXISTS idx_notes_links_from ON notes_links(agent_id, from_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to   ON notes_links(agent_id, to_note);
";
```

Add the migration helper near other `migrate_*` helpers in the same file:

```rust
/// Add `to_raw` column to existing `notes_links` rows; backfill with `to_note` value.
///
/// Idempotent: re-running on a migrated table is a no-op.
pub fn migrate_notes_links_to_raw(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let has_col: bool = conn
        .prepare("PRAGMA table_info(notes_links)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == "to_raw");
    if has_col {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE notes_links ADD COLUMN to_raw TEXT NOT NULL DEFAULT '';
         UPDATE notes_links SET to_raw = to_note WHERE to_raw = '';",
    )?;
    Ok(())
}
```

Wire `migrate_notes_links_to_raw` into the `init_schema` block — locate the `init_schema` function and add the call right after the `notes_links` table creation. Example diff:

```rust
conn.execute_batch(NOTES_LINKS_DDL)
    .map_err(|e| AlephError::config(format!("Failed to create notes_links table: {e}")))?;
migrate_notes_links_to_raw(conn)
    .map_err(|e| AlephError::config(format!("Failed to migrate notes_links: {e}")))?;
```

- [ ] **Step 4: Run schema tests**

```bash
cargo test -p alephcore --lib memory::store::sqlite::schema
```
Expected: all green including new migration test.

- [ ] **Step 5: Commit**

```bash
git add src/memory/store/sqlite/schema.rs
git commit -m "feat(notes): add to_raw column to notes_links with backfill migration"
```

---

## Task 3 (A2.2): Persist resolved `to_note` in indexer write path

**Files:**
- Modify: `src/memory/store/sqlite/notes.rs:90-105` (link insert block)
- Modify: `src/memory/notes/indexer.rs` (extract resolved targets before calling `index_note`)

- [ ] **Step 1: Write integration test for cross-form incoming links**

Add to `src/memory/store/sqlite/notes.rs` `mod tests` (or extend an existing tests module):

```rust
#[tokio::test]
async fn incoming_links_resolve_mixed_link_forms() {
    use crate::memory::notes::KnowledgeNote;
    let temp = std::env::temp_dir().join(format!("aleph_test_{}", uuid::Uuid::new_v4()));
    let db = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());

    // Target note exists at reference/rust
    db.index_note(
        &KnowledgeNote {
            title: "rust".into(),
            category: "reference".into(),
            facts: vec!["body".into()],
            content_hash: "h0".into(),
            ..Default::default()
        },
        "default",
        "reference",
    )
    .await
    .unwrap();

    // Note A links via short form [[rust]]; B links via full path [[reference/rust]]
    db.index_note(
        &KnowledgeNote {
            title: "a".into(),
            category: "preference".into(),
            facts: vec!["see [[rust]]".into()],
            links: vec!["rust".into()],
            content_hash: "h1".into(),
            ..Default::default()
        },
        "default",
        "preference",
    )
    .await
    .unwrap();
    db.index_note(
        &KnowledgeNote {
            title: "b".into(),
            category: "preference".into(),
            facts: vec!["see [[reference/rust]]".into()],
            links: vec!["reference/rust".into()],
            content_hash: "h2".into(),
            ..Default::default()
        },
        "default",
        "preference",
    )
    .await
    .unwrap();

    let incoming = db.get_incoming_links("reference/rust", "default").await.unwrap();
    assert_eq!(incoming.len(), 2, "both A and B should link to reference/rust");
}
```

- [ ] **Step 2: Run test — it should fail**

```bash
cargo test -p alephcore --lib memory::store::sqlite::notes::tests::incoming_links_resolve_mixed_link_forms
```
Expected: fails — `incoming` returns only 1 (the full-path linker).

- [ ] **Step 3: Update `index_note` to write resolved `to_note`**

In `src/memory/store/sqlite/notes.rs`, replace the link insert block (around lines 92-103):

```rust
// Replace links: delete old, insert new with resolved to_note.
conn.execute(
    "DELETE FROM notes_links WHERE from_note = ?1 AND agent_id = ?2",
    params![path, agent_id],
)
.map_err(|e| AlephError::config(format!("index_note delete links: {e}")))?;

for raw_target in &note.links {
    // Resolve raw_target inline: if it contains '/', treat as exact path; else
    // run a SELECT to find a unique filename match.
    let resolved = if raw_target.contains('/') {
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM notes_index WHERE agent_id = ?1 AND path = ?2",
                params![agent_id, raw_target],
                |_| Ok(true),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("resolve link path: {e}")))?
            .unwrap_or(false);
        if exists { raw_target.clone() } else { raw_target.clone() }
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT path FROM notes_index WHERE agent_id = ?1 AND filename = ?2 LIMIT 2",
            )
            .map_err(|e| AlephError::config(format!("resolve filename prep: {e}")))?;
        let paths: Vec<String> = stmt
            .query_map(params![agent_id, raw_target], |r| r.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("resolve filename query: {e}")))?
            .filter_map(|r| r.ok())
            .collect();
        if paths.len() == 1 { paths[0].clone() } else { raw_target.clone() }
    };

    conn.execute(
        "INSERT OR IGNORE INTO notes_links (agent_id, from_note, to_note, to_raw)
            VALUES (?1, ?2, ?3, ?4)",
        params![agent_id, path, resolved, raw_target],
    )
    .map_err(|e| AlephError::config(format!("index_note insert link: {e}")))?;
}
```

Add the import for `OptionalExtension` at the top of the file if not already present:

```rust
use rusqlite::OptionalExtension;
```

- [ ] **Step 4: Run the test — it should pass**

```bash
cargo test -p alephcore --lib memory::store::sqlite::notes::tests::incoming_links_resolve_mixed_link_forms
```
Expected: pass.

- [ ] **Step 5: Run the broader notes test set to catch regressions**

```bash
cargo test -p alephcore --lib memory::notes memory::store::sqlite::notes
```
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add src/memory/store/sqlite/notes.rs
git commit -m "fix(notes): resolve wikilink targets at write time so cross-form graph queries hit"
```

---

## Task 4 (A2.3): Late-resolution lint rule

**Files:**
- Modify: `src/memory/dreaming/stages/note_lint.rs` (add a new rule that retries unresolved links)

- [ ] **Step 1: Write failing test**

Add to `src/memory/dreaming/stages/note_lint.rs` `#[cfg(test)] mod tests`:

```rust
#[tokio::test]
async fn lint_resolves_pending_links_after_target_appears() {
    use crate::memory::notes::KnowledgeNote;
    let (ctx, store) = build_test_dream_ctx().await;

    // Note A links to [[rust]] before any rust note exists → stored as to_raw="rust", to_note="rust".
    store.index_note(
        &KnowledgeNote {
            title: "a".into(),
            category: "preference".into(),
            facts: vec!["see [[rust]]".into()],
            links: vec!["rust".into()],
            content_hash: "h1".into(),
            ..Default::default()
        },
        &ctx.agent_id,
        "preference",
    ).await.unwrap();

    // Now rust exists at reference/rust.
    store.index_note(
        &KnowledgeNote {
            title: "rust".into(),
            category: "reference".into(),
            facts: vec!["body".into()],
            content_hash: "h2".into(),
            ..Default::default()
        },
        &ctx.agent_id,
        "reference",
    ).await.unwrap();

    let stage = NoteLintStage::default();
    stage.execute(ctx.clone()).await.unwrap();

    let incoming = store.get_incoming_links("reference/rust", &ctx.agent_id).await.unwrap();
    assert_eq!(incoming.len(), 1, "lint should have resolved [[rust]] -> reference/rust");
}
```

You will need a `build_test_dream_ctx` helper that spins up an in-memory SqliteMemoryBackend and a `DreamContext`. If a similar helper already exists in this file or `dreaming/mod.rs` tests, reuse it; otherwise add a small one in this same `mod tests`:

```rust
async fn build_test_dream_ctx() -> (
    crate::memory::dreaming::DreamContext,
    std::sync::Arc<crate::memory::store::SqliteMemoryBackend>,
) {
    let temp = std::env::temp_dir().join(format!("aleph_lint_{}", uuid::Uuid::new_v4()));
    let store = std::sync::Arc::new(
        crate::memory::store::SqliteMemoryBackend::new(&temp).unwrap(),
    );
    let ctx = crate::memory::dreaming::DreamContext {
        agent_id: "default".into(),
        store: store.clone(),
        notes: vec![],
        ..Default::default()
    };
    (ctx, store)
}
```

- [ ] **Step 2: Run test — it should fail**

```bash
cargo test -p alephcore --lib memory::dreaming::stages::note_lint::tests::lint_resolves_pending_links_after_target_appears
```
Expected: fail (lint stage does not yet retry resolution).

- [ ] **Step 3: Add a public `relink_unresolved` method on `NoteStore`**

Edit `src/memory/notes/store.rs` and add to the trait (with default impl returning `Ok(0)`):

```rust
/// Retry resolution for any links where `to_note == to_raw` and `to_raw`
/// has no '/'. Updates `to_note` to the resolved path when filename is unique.
///
/// Returns the number of rows updated.
async fn relink_unresolved(&self, agent_id: &str) -> Result<usize, AlephError> {
    let _ = agent_id;
    Ok(0)
}
```

Implement it for `SqliteMemoryBackend` in `src/memory/store/sqlite/notes.rs`:

```rust
async fn relink_unresolved(&self, agent_id: &str) -> Result<usize, AlephError> {
    let conn = lock_conn!(self)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, to_raw FROM notes_links
             WHERE agent_id = ?1 AND to_note = to_raw AND instr(to_raw, '/') = 0",
        )
        .map_err(|e| AlephError::config(format!("relink prep: {e}")))?;

    let rows: Vec<(i64, String)> = stmt
        .query_map(params![agent_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| AlephError::config(format!("relink scan: {e}")))?
        .filter_map(|r| r.ok())
        .collect();

    let mut updated = 0usize;
    for (id, raw) in rows {
        let mut find = conn
            .prepare("SELECT path FROM notes_index WHERE agent_id = ?1 AND filename = ?2 LIMIT 2")
            .map_err(|e| AlephError::config(format!("relink find: {e}")))?;
        let paths: Vec<String> = find
            .query_map(params![agent_id, raw], |r| r.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("relink find query: {e}")))?
            .filter_map(|r| r.ok())
            .collect();
        if paths.len() == 1 {
            conn.execute(
                "UPDATE notes_links SET to_note = ?1 WHERE id = ?2",
                params![paths[0], id],
            )
            .map_err(|e| AlephError::config(format!("relink update: {e}")))?;
            updated += 1;
        }
    }
    Ok(updated)
}
```

- [ ] **Step 4: Wire into `note_lint` stage**

In `src/memory/dreaming/stages/note_lint.rs::execute`, add at the end of the stage's body (before returning `Ok(ctx)`):

```rust
let _ = ctx.store.relink_unresolved(&ctx.agent_id).await?;
```

- [ ] **Step 5: Run test — it should pass**

```bash
cargo test -p alephcore --lib memory::dreaming::stages::note_lint
```
Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/store.rs src/memory/store/sqlite/notes.rs src/memory/dreaming/stages/note_lint.rs
git commit -m "feat(notes): note_lint retries wikilink resolution as targets appear"
```

---

## Task 5 (A3): YAML inline-array quoting

**Files:**
- Modify: `src/memory/notes/note.rs:133-180` (writer: `to_markdown`)

- [ ] **Step 1: Write failing tests**

Add to `mod tests` in `src/memory/notes/note.rs`:

```rust
#[test]
fn yaml_inline_array_quotes_special_chars() {
    let items = vec![
        "plain".to_string(),
        "has, comma".to_string(),
        "has: colon".to_string(),
        "has '' quote".to_string(),
    ];
    let s = yaml_inline_array(&items);
    assert_eq!(s, "[plain, 'has, comma', 'has: colon', 'has '''' quote']");
}

#[test]
fn yaml_inline_array_empty() {
    assert_eq!(yaml_inline_array(&[]), "[]");
}

#[test]
fn tags_with_special_chars_round_trip() {
    let n = KnowledgeNote {
        title: "t".into(),
        category: "preference".into(),
        tags: vec!["has, comma".into(), "has: colon".into()],
        facts: vec!["x".into()],
        content_hash: String::new(),
        ..Default::default()
    };
    let md = n.to_markdown();
    let parsed = KnowledgeNote::from_markdown("t", &md).expect("must round-trip");
    assert_eq!(parsed.tags, vec!["has, comma".to_string(), "has: colon".to_string()]);
}
```

- [ ] **Step 2: Run tests — should fail**

```bash
cargo test -p alephcore --lib memory::notes::note::tests::yaml_inline_array_quotes_special_chars memory::notes::note::tests::yaml_inline_array_empty memory::notes::note::tests::tags_with_special_chars_round_trip
```
Expected: first two fail to compile (function missing); third fails on assertion.

- [ ] **Step 3: Add `yaml_inline_array` helper and use it in writer**

Add at the bottom of `src/memory/notes/note.rs`, near `sha256_hex`:

```rust
/// Emit a YAML flow-style array, quoting any element that contains a YAML
/// reserved character so the round-trip survives `serde_yaml::from_str`.
pub(crate) fn yaml_inline_array(items: &[String]) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let parts: Vec<String> = items
        .iter()
        .map(|s| {
            let needs_quote = s.chars().any(|c| matches!(
                c,
                '\'' | '"' | ',' | ':' | '[' | ']' | '{' | '}'
                | '#' | '&' | '*' | '!' | '|' | '>' | '%' | '@' | '`'
            )) || s.starts_with(' ') || s.ends_with(' ') || s.is_empty();
            if needs_quote {
                let escaped = s.replace('\'', "''");
                format!("'{escaped}'")
            } else {
                s.clone()
            }
        })
        .collect();
    format!("[{}]", parts.join(", "))
}
```

In `to_markdown` (around lines 143-167), replace the manual array emission. Locate the line:

```rust
out.push_str(&format!("tags: [{}]\n", tags_yaml.join(", ")));
```

and replace with:

```rust
out.push_str(&format!("tags: {}\n", yaml_inline_array(&self.tags)));
```

Replace the `source_facts` branch (around line 159-166) to also use `yaml_inline_array`:

```rust
out.push_str(&format!("source_facts: {}\n", yaml_inline_array(&self.source_facts)));
```

(The `source_facts` → `source_notes` rename is in Phase R2; A3 keeps the existing field name.)

- [ ] **Step 4: Run tests — should pass**

```bash
cargo test -p alephcore --lib memory::notes::note
```
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/note.rs
git commit -m "fix(notes): YAML flow-style arrays quote elements with reserved chars"
```

---

## Task 6 (A4): Frontmatter date round-trip

**Files:**
- Modify: `src/memory/notes/note.rs:36-38` (`Frontmatter`), `:133-180` (`to_markdown`), `:189-217` (`split_frontmatter` / parsing)

- [ ] **Step 1: Write failing tests**

Add to `mod tests`:

```rust
#[test]
fn date_writer_quotes_iso_string() {
    let n = KnowledgeNote {
        title: "t".into(),
        category: "preference".into(),
        facts: vec!["x".into()],
        created_at: 1714377600,
        updated_at: 1714377600,
        ..Default::default()
    };
    let md = n.to_markdown();
    assert!(md.contains("created: \"2026-04-29\""), "expected quoted date, got:\n{md}");
}

#[test]
fn date_reader_accepts_native_yaml_date() {
    let md = "---
category: skill
tags: []
created: 2026-04-01
updated: 2026-04-01
---

- fact
";
    let n = KnowledgeNote::from_markdown("t", md).expect("must parse native date");
    assert!(n.created_at > 0);
}

#[test]
fn date_reader_accepts_quoted_iso_date() {
    let md = "---
category: skill
tags: []
created: \"2026-04-01\"
updated: \"2026-04-01\"
---

- fact
";
    let n = KnowledgeNote::from_markdown("t", md).expect("must parse quoted date");
    assert!(n.created_at > 0);
}
```

- [ ] **Step 2: Run tests — at least the writer test should fail**

```bash
cargo test -p alephcore --lib memory::notes::note::tests::date_writer_quotes_iso_string memory::notes::note::tests::date_reader_accepts_native_yaml_date memory::notes::note::tests::date_reader_accepts_quoted_iso_date
```
Expected: writer test fails (current writer emits unquoted); reader tests may pass on the current serde_yaml version but are pinned for regression safety.

- [ ] **Step 3: Quote dates in writer; harden reader**

In `to_markdown`, replace:

```rust
out.push_str(&format!("created: {created}\n"));
out.push_str(&format!("updated: {updated}\n"));
```

with:

```rust
out.push_str(&format!("created: \"{created}\"\n"));
out.push_str(&format!("updated: \"{updated}\"\n"));
```

In the `Frontmatter` struct, add a custom deserializer that accepts string, native YAML date, or null:

```rust
fn deserialize_optional_date_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where D: serde::Deserializer<'de>
{
    use serde::Deserialize;
    let v = serde_yaml::Value::deserialize(d)?;
    Ok(match v {
        serde_yaml::Value::Null => None,
        serde_yaml::Value::String(s) => Some(s),
        // serde_yaml represents YYYY-MM-DD natively as a Tagged value or a
        // BoundedDateTime depending on version — re-serialize then strip
        // surrounding quotes.
        other => {
            let s = serde_yaml::to_string(&other)
                .map_err(serde::de::Error::custom)?
                .trim()
                .trim_matches(|c: char| c == '\'' || c == '"' || c.is_whitespace())
                .to_string();
            if s.is_empty() { None } else { Some(s) }
        }
    })
}
```

Apply it to the two date fields:

```rust
#[derive(Debug, Deserialize, Serialize)]
struct Frontmatter {
    #[serde(default)]
    category: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_optional_date_string")]
    created: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_date_string")]
    updated: Option<String>,
    #[serde(default = "default_confidence")]
    confidence: f32,
    #[serde(default)]
    severity: Severity,
    #[serde(default)]
    source_facts: Vec<String>,
}
```

- [ ] **Step 4: Run tests — should pass**

```bash
cargo test -p alephcore --lib memory::notes::note
```
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/note.rs
git commit -m "fix(notes): quote frontmatter dates on write; accept multiple shapes on read"
```

---

## Task 7 (A5): `extract_facts` sub-bullet support

**Files:**
- Modify: `src/memory/notes/note.rs:240-247` (`extract_facts`)

- [ ] **Step 1: Write failing tests**

Add to `mod tests`:

```rust
#[test]
fn extract_facts_keeps_subbullets() {
    let body = "- top fact
  - sub fact
- second top
";
    let facts = extract_facts(body);
    assert_eq!(facts.len(), 2);
    assert!(facts[0].contains("top fact"));
    assert!(facts[0].contains("sub fact"), "sub-bullet must attach to parent: {:?}", facts[0]);
    assert_eq!(facts[1].trim(), "second top");
}

#[test]
fn extract_facts_keeps_continuation_lines() {
    let body = "- claim line one
  continuation line two
  continuation line three
- next claim
";
    let facts = extract_facts(body);
    assert_eq!(facts.len(), 2);
    assert!(facts[0].contains("continuation line two"));
    assert!(facts[0].contains("continuation line three"));
}

#[test]
fn extract_facts_empty_line_ends_fact() {
    let body = "- one

  this should NOT belong to one
- two
";
    let facts = extract_facts(body);
    assert_eq!(facts.len(), 2);
    assert!(!facts[0].contains("should NOT"));
}
```

- [ ] **Step 2: Run tests — should fail**

```bash
cargo test -p alephcore --lib memory::notes::note::tests::extract_facts_keeps_subbullets memory::notes::note::tests::extract_facts_keeps_continuation_lines memory::notes::note::tests::extract_facts_empty_line_ends_fact
```
Expected: all three fail.

- [ ] **Step 3: Replace `extract_facts` with state-machine parser**

Replace the function body at `src/memory/notes/note.rs:240-247`:

```rust
fn extract_facts(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for raw_line in body.lines() {
        let trimmed_start = raw_line.trim_start();
        let indent = raw_line.len() - trimmed_start.len();
        let is_top_bullet = indent == 0 && trimmed_start.starts_with("- ");
        let is_blank = raw_line.trim().is_empty();

        if is_top_bullet {
            if let Some(c) = current.take() {
                out.push(c);
            }
            current = Some(trimmed_start[2..].to_string());
        } else if is_blank {
            if let Some(c) = current.take() {
                out.push(c);
            }
        } else if current.is_some() && indent >= 2 {
            // attach to current fact preserving original indent below 4 cols visual
            let acc = current.as_mut().unwrap();
            acc.push('\n');
            acc.push_str(raw_line);
        } else {
            // a non-bullet line at indent 0 ends any current fact and is ignored
            if let Some(c) = current.take() {
                out.push(c);
            }
        }
    }
    if let Some(c) = current.take() {
        out.push(c);
    }
    out
}
```

- [ ] **Step 4: Run tests — should pass**

```bash
cargo test -p alephcore --lib memory::notes::note
```
Expected: all green (existing tests must still pass — check `parses_note_from_markdown` particularly).

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/note.rs
git commit -m "fix(notes): extract_facts preserves sub-bullets and continuation lines"
```

---

## Task 8 (A6): `sanitize_title` empty-result error

**Files:**
- Modify: `src/memory/notes/note.rs:253-259` (signature change)
- Modify: every caller — `src/memory/notes/indexer.rs`, `src/builtin_tools/note_manage.rs`

- [ ] **Step 1: Write failing tests**

Add to `mod tests`:

```rust
#[test]
fn sanitize_title_rejects_empty_result() {
    assert!(sanitize_title("").is_err());
    assert!(sanitize_title("..").is_err());
    assert!(sanitize_title("///").is_err());
    assert!(sanitize_title("   ").is_err());
}

#[test]
fn sanitize_title_returns_ok_for_normal_input() {
    assert_eq!(sanitize_title("rust learning").unwrap(), "rust learning");
    assert_eq!(sanitize_title("../etc/passwd").unwrap(), "etcpasswd");
}
```

Update existing `sanitize_title_strips_path_traversal` (around line 457) to unwrap each call: change `assert_eq!(sanitize_title("../../etc/passwd"), "etcpasswd");` to `assert_eq!(sanitize_title("../../etc/passwd").unwrap(), "etcpasswd");` for every assertion in that test.

- [ ] **Step 2: Run tests — should fail to compile**

```bash
cargo test -p alephcore --lib memory::notes::note::tests::sanitize_title_rejects_empty_result
```
Expected: compile error — `sanitize_title` still returns `String`.

- [ ] **Step 3: Change signature and update body**

Replace `sanitize_title` at `src/memory/notes/note.rs:253-259`:

```rust
/// Sanitize a note title for safe use as a filename.
///
/// Strips path separators, null bytes, and filesystem-unsafe characters
/// to prevent path traversal attacks from LLM-generated titles.
///
/// Returns `Err(AlephError::Validation)` if the result is empty / all-dots /
/// all-whitespace — callers should reject the operation rather than write a
/// note with an unstable filename.
pub fn sanitize_title(title: &str) -> Result<String, AlephError> {
    let cleaned: String = title
        .replace(['/', '\\', '\0', ':', '*', '?', '"', '<', '>', '|'], "")
        .replace("..", "")
        .trim()
        .to_string();
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.' || c.is_whitespace()) {
        return Err(AlephError::Validation {
            message: format!("note title sanitizes to empty: {title:?}"),
        });
    }
    Ok(cleaned)
}
```

If `AlephError::Validation` does not yet have a `message`-only constructor variant, check `src/error.rs` for an existing equivalent (`ConfigError`, `Validation`, etc.) and use it. If none exists with this exact shape, add the variant to the error enum:

```rust
// in src/error.rs (only if no equivalent exists)
Validation { message: String },
```

- [ ] **Step 4: Update all callers to propagate the Result**

Run:

```bash
rg -n "sanitize_title\(" --no-heading src/
```

For every call site, add `?`. Common patterns to fix:

`src/memory/notes/indexer.rs` — locate every `sanitize_title(...)` and append `?`. Example:

```rust
// before
let safe = sanitize_title(&note.title);
// after
let safe = sanitize_title(&note.title)?;
```

`src/builtin_tools/note_manage.rs` — similarly. Each affected function must already return `Result<_, AlephError>` (or be made to).

- [ ] **Step 5: Run full lib test set**

```bash
cargo test -p alephcore --lib
```
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/note.rs src/error.rs src/memory/notes/indexer.rs src/builtin_tools/note_manage.rs
git commit -m "fix(notes): sanitize_title returns Err on empty/all-dot result"
```

---

## Task 9 (Phase A verification gate)

**Files:** none (verification only)

- [ ] **Step 1: Run the regression matrix**

```bash
cargo test -p alephcore --lib memory::notes::wikilink memory::notes::note memory::store::sqlite::notes memory::store::sqlite::schema memory::dreaming::stages::note_lint
```
Expected: all green; new tests from Tasks 1, 3, 4, 5, 6, 7, 8 all present and passing.

- [ ] **Step 2: Manual smoke against author's note corpus**

```bash
cargo run --release --bin aleph-server -- admin notes full-rebuild
```
Expected: zero new errors in `IndexStats.errors`; the command exits 0.

If the `admin notes full-rebuild` subcommand does not exist, run instead:

```bash
cargo test -p alephcore --lib memory::notes::indexer::tests::full_rebuild_indexes_all_notes -- --nocapture
```
and inspect the printed `IndexStats`.

- [ ] **Step 3: Confirm the spec's Phase A gate items**

Cross-check Section 2.A7 of `docs/superpowers/specs/2026-05-03-aleph-note-layer-llm-wiki-optimization-design.md`:
1. wikilink + note tests pass (Tasks 1, 5, 6, 7, 8)
2. `incoming_links_resolves_mixed_link_forms` passes (Task 3)
3. `lint_resolves_pending_links` passes (Task 4)
4. `full_rebuild` smoke OK (Step 2)

- [ ] **Step 4: Tag the phase-complete commit**

```bash
git log --oneline -10
git tag note-layer-phase-a-complete
```

Phase A done. Phase B (perf & cadence) depends on Task 2's `to_raw` column landing on main.
