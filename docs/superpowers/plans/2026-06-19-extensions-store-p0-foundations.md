# Extensions Store — P0 Foundations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish the `src/store/` umbrella module (unified types + rusqlite catalog cache + installed-state reconciliation) and a thin `extensions.*` JSON-RPC façade that surfaces the user's already-installed MCP servers, plugins, and skills as unified `ExtensionEntry`s and can toggle/uninstall them by delegating to the existing backends.

**Architecture:** New `src/store/` module with pure, testable types and a rusqlite cache; reconciliation maps the existing `McpServerInfo`/`PluginRecord`/`SkillStatusEntry` into one `ExtensionEntry`; a new `src/gateway/handlers/extensions/` façade delegates to `McpManagerHandle`, `ExtensionManager`/`PluginRegistry`, and `SkillSystem`.

**Tech Stack:** Rust (tokio 1.35, rusqlite 0.37 bundled, serde, serde_json).

## Global Constraints

See `2026-06-19-extensions-store-INDEX.md` → "Global Constraints". Most relevant to P0:
- New `src/store/` module; `ExtensionKind` (store-facing) ≠ runtime `PluginKind`. Do NOT touch `src/extension/`.
- Façade delegates only; returns `JsonRpcResponse`; uses `parse_params`.
- Cache is rusqlite at `~/.aleph/store_catalog.db`; tests use `Connection::open_in_memory()`.
- Test builds narrowly: `cargo test -p alephcore store::` to avoid rustc OOM (per project memory).
- Branch `feat/unified-extensions-store`; `docs/` plan edits are not part of code commits.

**Reference signatures (verified, file:line):**
- RPC: `HandlerRegistry::register(method, handler)` `src/gateway/handlers/mod.rs:853`; `parse_params::<T>(&req) -> Result<T, JsonRpcResponse>` `:187`; `JsonRpcResponse::success(id, Value)` / `::error(id, code, msg)` `src/gateway/protocol.rs:140`; codes `INTERNAL_ERROR`, `INVALID_PARAMS` from `src/gateway/protocol.rs`.
- In-process: `get_extension_manager() -> Result<&'static Arc<ExtensionManager>, JsonRpcResponse>` `src/gateway/handlers/plugins/handlers.rs:36`; `McpManagerHandle` captured at registration (`…/builder/handlers/mcp.rs`); `SkillSystem` handle similar.
- MCP: `McpManagerHandle::list_servers() -> Result<Vec<McpServerInfo>>` `src/mcp/manager/handle.rs:220`; `remove_server(id)` `:84`; `start_server(id)` `:162`; `stop_server(id)` `:181`. `McpServerInfo { id, name, transport: McpTransportType, tool_count, resource_count, prompt_count, health: HealthStatus }` `src/mcp/manager/types.rs:187`. `McpTransportType { Stdio, Http, Sse }`; `HealthStatus { Healthy, Degraded, Unhealthy, Restarting, Dead, Stopped }`.
- Plugins: `PluginRegistry::list_plugins() -> Vec<&PluginRecord>` `src/extension/registry/plugin_registry/mod.rs:95`; `PluginRecord { id, name, version: Option<String>, description: Option<String>, kind: PluginKind, origin, status: PluginStatus, .. }` `src/extension/types/plugins.rs:205`; `PluginStatus::is_active()` `:113`.
- Skills: `SkillSystem::full_status() -> Vec<SkillStatusEntry>` `src/skill/mod.rs:295`; `SkillSystem::remove_skill(&SkillId) -> Result<bool, io::Error>` `:483`; `update_config(&SkillId, SkillConfigUpdate::SetEnabled(bool))` `:410`.

---

### Task 1: Store module scaffold + core enums

**Files:**
- Create: `src/store/mod.rs`
- Create: `src/store/types.rs`
- Modify: `src/lib.rs` (add `pub mod store;` near the other top-level `pub mod` lines)
- Test: inline `#[cfg(test)]` in `src/store/types.rs`

**Interfaces:**
- Produces: `ExtensionKind { Skill, Plugin, Mcp }`, `ExtensionCategory { Search, Developer, Data, Productivity, Writing, Communication, Knowledge, Files, Design, Automation, Finance, Utilities, Other }`, `TrustTier { Official, Verified, Community, Unverified }`, `McpTransport { Stdio, StreamableHttp, Sse }`. All `#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]`, `#[serde(rename_all = "snake_case")]`, with `as_str()` on `ExtensionKind`/`ExtensionCategory`/`TrustTier`.

- [ ] **Step 1: Add the module declaration**

In `src/lib.rs`, add alongside the existing `pub mod extension;` / `pub mod mcp;` lines:
```rust
pub mod store;
```

- [ ] **Step 2: Create `src/store/mod.rs`**

```rust
//! Unified Extensions Store: one user-facing `Extension` concept over the
//! existing plugin / MCP / skill backends. See
//! docs/superpowers/specs/2026-06-19-unified-extensions-store-design.md
pub mod types;

pub use types::{
    EnvDecl, ExtensionCategory, ExtensionEntry, ExtensionKind, HeaderDecl, InstallSpec,
    McpTransport, TrustTier,
};
```
> Note: `ExtensionEntry`, `InstallSpec`, `EnvDecl`, `HeaderDecl` are added in Tasks 2–3; the re-export compiles only after those land. If executing strictly task-by-task, temporarily narrow this `pub use` to the enums from this task and widen it in Task 3.

- [ ] **Step 3: Write the failing test** in `src/store/types.rs`

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionKind { Skill, Plugin, Mcp }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionCategory {
    Search, Developer, Data, Productivity, Writing, Communication,
    Knowledge, Files, Design, Automation, Finance, Utilities, Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier { Official, Verified, Community, Unverified }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport { Stdio, StreamableHttp, Sse }

impl ExtensionKind {
    pub fn as_str(self) -> &'static str {
        match self { Self::Skill => "skill", Self::Plugin => "plugin", Self::Mcp => "mcp" }
    }
}

impl TrustTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Official => "official", Self::Verified => "verified",
            Self::Community => "community", Self::Unverified => "unverified",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&ExtensionKind::Mcp).unwrap(), "\"mcp\"");
        assert_eq!(ExtensionKind::Plugin.as_str(), "plugin");
    }

    #[test]
    fn category_roundtrips() {
        let c = ExtensionCategory::Developer;
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(s, "\"developer\"");
        assert_eq!(serde_json::from_str::<ExtensionCategory>(&s).unwrap(), c);
    }

    #[test]
    fn trust_tier_as_str() {
        assert_eq!(TrustTier::Unverified.as_str(), "unverified");
    }
}
```

- [ ] **Step 4: Run the test to verify it fails (module not wired yet)**

Run: `cargo test -p alephcore store::types::tests::kind_serializes_snake_case`
Expected: FAIL to compile until Step 1's `pub mod store;` is present, then PASS. (If it already passes, the module is wired.)

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p alephcore store::types::tests`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add src/lib.rs src/store/mod.rs src/store/types.rs
git commit -m "feat(store): scaffold store module + core extension enums"
```

---

### Task 2: InstallSpec + env/header declarations

**Files:**
- Modify: `src/store/types.rs`
- Test: inline `#[cfg(test)]` in `src/store/types.rs`

**Interfaces:**
- Consumes: `McpTransport` (Task 1).
- Produces: `EnvDecl { name, description: Option<String>, required: bool, secret: bool, default: Option<String>, placeholder: Option<String> }`, `HeaderDecl { name, secret: bool }`, and `InstallSpec` enum (`#[serde(tag = "type", rename_all = "snake_case")]`) with variants `McpStdio { command: String, args: Vec<String>, env: Vec<EnvDecl> }`, `McpRemote { url: String, transport: McpTransport, headers: Vec<HeaderDecl> }`, `OciImage { image: String }`, `GitDir { git_url: String, subdir: Option<String>, git_ref: Option<String>, sha256: Option<String> }`. `InstallSpec::requires_config(&self) -> bool`.

- [ ] **Step 1: Write the failing test** (append to `src/store/types.rs`, above the existing `#[cfg(test)] mod tests`)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EnvDecl {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderDecl { pub name: String, #[serde(default)] pub secret: bool }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InstallSpec {
    McpStdio { command: String, #[serde(default)] args: Vec<String>, #[serde(default)] env: Vec<EnvDecl> },
    McpRemote { url: String, transport: McpTransport, #[serde(default)] headers: Vec<HeaderDecl> },
    OciImage { image: String },
    GitDir { git_url: String, subdir: Option<String>, git_ref: Option<String>, sha256: Option<String> },
}

impl InstallSpec {
    /// True iff installing requires collecting user-supplied config/secrets.
    pub fn requires_config(&self) -> bool {
        match self {
            Self::McpStdio { env, .. } => env.iter().any(|e| e.required),
            Self::McpRemote { headers, .. } => headers.iter().any(|h| h.secret),
            Self::OciImage { .. } | Self::GitDir { .. } => false,
        }
    }
}
```

Add tests inside the existing `mod tests`:
```rust
    #[test]
    fn install_spec_tagged_json() {
        let spec = InstallSpec::McpStdio {
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
            env: vec![EnvDecl { name: "GITHUB_TOKEN".into(), required: true, secret: true, ..Default::default() }],
        };
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v["type"], "mcp_stdio");
        assert_eq!(v["command"], "npx");
        assert!(spec.requires_config());
    }

    #[test]
    fn oci_image_needs_no_config() {
        let spec = InstallSpec::OciImage { image: "mcp/foo@sha256:abc".into() };
        assert!(!spec.requires_config());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore store::types::tests::install_spec_tagged_json`
Expected: FAIL — `InstallSpec` not defined (before Step 1 code is added). After adding, PASS.

- [ ] **Step 3: Run to verify it passes**

Run: `cargo test -p alephcore store::types::tests`
Expected: PASS (5 tests).

- [ ] **Step 4: Commit**

```bash
git add src/store/types.rs
git commit -m "feat(store): InstallSpec + env/header declarations"
```

---

### Task 3: ExtensionEntry

**Files:**
- Modify: `src/store/types.rs`, `src/store/mod.rs` (widen re-exports)
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: all Task 1–2 types.
- Produces: `ExtensionEntry` struct (fields below) + `ExtensionEntry::from_install_spec(...)` not required; entries are built by reconcile (Task 5) and providers (P1).

```rust
pub struct ExtensionEntry {
    pub id: String,                       // provider-prefixed, e.g. "mcp-official:io.github.user/foo"
    pub kind: ExtensionKind,
    pub category: ExtensionCategory,      // PRIMARY browse axis
    pub name: String,
    pub description: String,
    pub author: Option<String>,
    pub icon: Option<String>,
    pub tags: Vec<String>,
    pub version: Option<String>,
    pub source_id: String,                // provider id; also a de-dup key
    pub repo_url: Option<String>,
    pub trust_tier: TrustTier,
    pub requires_config: bool,
    pub config_schema: Option<serde_json::Value>,
    pub installed: bool,
    pub enabled: bool,
    pub update_available: bool,
}
```

- [ ] **Step 1: Write the failing test** (append to `src/store/types.rs`)

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionEntry {
    pub id: String,
    pub kind: ExtensionKind,
    pub category: ExtensionCategory,
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub source_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    pub trust_tier: TrustTier,
    #[serde(default)]
    pub requires_config: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub update_available: bool,
}
```

Test (inside `mod tests`):
```rust
    fn sample_entry() -> ExtensionEntry {
        ExtensionEntry {
            id: "mcp-official:io.github.acme/foo".into(),
            kind: ExtensionKind::Mcp,
            category: ExtensionCategory::Developer,
            name: "Foo".into(),
            description: "Does foo.".into(),
            author: Some("acme".into()),
            icon: None,
            tags: vec!["mcp".into(), "developer".into()],
            version: Some("1.0.0".into()),
            source_id: "mcp-official".into(),
            repo_url: Some("https://github.com/acme/foo".into()),
            trust_tier: TrustTier::Community,
            requires_config: true,
            config_schema: Some(serde_json::json!({"type":"object"})),
            installed: false,
            enabled: false,
            update_available: false,
        }
    }

    #[test]
    fn entry_roundtrips_through_json() {
        let e = sample_entry();
        let json = serde_json::to_string(&e).unwrap();
        let back: ExtensionEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
        assert_eq!(back.category, ExtensionCategory::Developer);
    }
```

- [ ] **Step 2: Widen re-exports in `src/store/mod.rs`** to the full set in the Task-1 listing (it already lists them).

- [ ] **Step 3: Run to verify it passes**

Run: `cargo test -p alephcore store::types::tests`
Expected: PASS (6 tests).

- [ ] **Step 4: Commit**

```bash
git add src/store/types.rs src/store/mod.rs
git commit -m "feat(store): ExtensionEntry unified catalog type"
```

---

### Task 4: rusqlite catalog cache

**Files:**
- Create: `src/store/cache.rs`
- Modify: `src/store/mod.rs` (add `pub mod cache;`)
- Test: inline `#[cfg(test)]` in `src/store/cache.rs`

**Interfaces:**
- Consumes: `ExtensionEntry`, `ExtensionKind`, `ExtensionCategory`.
- Produces:
  - `pub struct CatalogFilter { pub kind: Option<ExtensionKind>, pub category: Option<ExtensionCategory>, pub source_id: Option<String>, pub query: Option<String> }` (`Default`).
  - free fns over `&rusqlite::Connection`: `init_schema(conn) -> rusqlite::Result<()>`, `upsert_entry(conn, &ExtensionEntry) -> rusqlite::Result<()>`, `query_entries(conn, &CatalogFilter) -> rusqlite::Result<Vec<ExtensionEntry>>`, `clear_source(conn, source_id) -> rusqlite::Result<usize>`.
  - `pub struct CatalogCache { conn: Arc<tokio::sync::Mutex<rusqlite::Connection>> }` with `open(path) -> rusqlite::Result<Self>`, `open_in_memory()`, async `upsert_many(&[ExtensionEntry])`, async `query(&CatalogFilter) -> Vec<ExtensionEntry>`, async `replace_source(source_id, &[ExtensionEntry])`.

- [ ] **Step 1: Write the failing test** in `src/store/cache.rs`

```rust
use std::sync::Arc;
use rusqlite::{params, Connection};
use tokio::sync::Mutex;
use crate::store::types::{ExtensionCategory, ExtensionEntry, ExtensionKind};

#[derive(Debug, Clone, Default)]
pub struct CatalogFilter {
    pub kind: Option<ExtensionKind>,
    pub category: Option<ExtensionCategory>,
    pub source_id: Option<String>,
    pub query: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::types::TrustTier;

    fn entry(id: &str, cat: ExtensionCategory, name: &str) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(), kind: ExtensionKind::Mcp, category: cat,
            name: name.into(), description: "d".into(), author: None, icon: None,
            tags: vec![], version: None, source_id: "mcp-official".into(), repo_url: None,
            trust_tier: TrustTier::Community, requires_config: false, config_schema: None,
            installed: false, enabled: false, update_available: false,
        }
    }

    #[test]
    fn upsert_then_query_by_category() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        upsert_entry(&conn, &entry("a", ExtensionCategory::Developer, "Alpha")).unwrap();
        upsert_entry(&conn, &entry("b", ExtensionCategory::Data, "Beta")).unwrap();

        let dev = query_entries(&conn, &CatalogFilter { category: Some(ExtensionCategory::Developer), ..Default::default() }).unwrap();
        assert_eq!(dev.len(), 1);
        assert_eq!(dev[0].name, "Alpha");

        let all = query_entries(&conn, &CatalogFilter::default()).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn upsert_is_idempotent_by_id() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        upsert_entry(&conn, &entry("a", ExtensionCategory::Developer, "Alpha")).unwrap();
        upsert_entry(&conn, &entry("a", ExtensionCategory::Developer, "Alpha v2")).unwrap();
        let all = query_entries(&conn, &CatalogFilter::default()).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Alpha v2");
    }

    #[test]
    fn query_substring_matches_name() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        upsert_entry(&conn, &entry("a", ExtensionCategory::Developer, "GitHub")).unwrap();
        upsert_entry(&conn, &entry("b", ExtensionCategory::Data, "Postgres")).unwrap();
        let hits = query_entries(&conn, &CatalogFilter { query: Some("git".into()), ..Default::default() }).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "a");
    }

    #[test]
    fn clear_source_removes_only_that_source() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let mut e = entry("a", ExtensionCategory::Developer, "Alpha");
        e.source_id = "docker-mcp".into();
        upsert_entry(&conn, &e).unwrap();
        upsert_entry(&conn, &entry("b", ExtensionCategory::Data, "Beta")).unwrap(); // mcp-official
        assert_eq!(clear_source(&conn, "mcp-official").unwrap(), 1);
        assert_eq!(query_entries(&conn, &CatalogFilter::default()).unwrap().len(), 1);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore store::cache::tests::upsert_then_query_by_category`
Expected: FAIL — `init_schema`/`upsert_entry`/`query_entries`/`clear_source` not defined.

- [ ] **Step 3: Implement the free functions + cache** (in `src/store/cache.rs`, above the test module)

```rust
/// Schema: one row per extension; `data` holds the full JSON, indexed columns
/// drive filtering. `name_lc` enables case-insensitive substring search.
pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS catalog (
            id        TEXT PRIMARY KEY,
            kind      TEXT NOT NULL,
            category  TEXT NOT NULL,
            name_lc   TEXT NOT NULL,
            source_id TEXT NOT NULL,
            installed INTEGER NOT NULL DEFAULT 0,
            data      TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_catalog_category ON catalog(category);
        CREATE INDEX IF NOT EXISTS idx_catalog_kind ON catalog(kind);
        CREATE INDEX IF NOT EXISTS idx_catalog_source ON catalog(source_id);",
    )
}

pub fn upsert_entry(conn: &Connection, e: &ExtensionEntry) -> rusqlite::Result<()> {
    let data = serde_json::to_string(e).map_err(|err| {
        rusqlite::Error::ToSqlConversionFailure(Box::new(err))
    })?;
    conn.execute(
        "INSERT INTO catalog (id, kind, category, name_lc, source_id, installed, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            kind=excluded.kind, category=excluded.category, name_lc=excluded.name_lc,
            source_id=excluded.source_id, installed=excluded.installed, data=excluded.data",
        params![
            e.id,
            serde_json::to_value(e.kind).unwrap().as_str().unwrap(),
            serde_json::to_value(e.category).unwrap().as_str().unwrap(),
            e.name.to_lowercase(),
            e.source_id,
            e.installed as i64,
            data,
        ],
    )?;
    Ok(())
}

pub fn query_entries(conn: &Connection, f: &CatalogFilter) -> rusqlite::Result<Vec<ExtensionEntry>> {
    let mut sql = String::from("SELECT data FROM catalog WHERE 1=1");
    let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(k) = f.kind {
        sql.push_str(" AND kind = ?");
        args.push(Box::new(serde_json::to_value(k).unwrap().as_str().unwrap().to_string()));
    }
    if let Some(c) = f.category {
        sql.push_str(" AND category = ?");
        args.push(Box::new(serde_json::to_value(c).unwrap().as_str().unwrap().to_string()));
    }
    if let Some(s) = &f.source_id {
        sql.push_str(" AND source_id = ?");
        args.push(Box::new(s.clone()));
    }
    if let Some(q) = &f.query {
        sql.push_str(" AND name_lc LIKE ?");
        args.push(Box::new(format!("%{}%", q.to_lowercase())));
    }
    sql.push_str(" ORDER BY name_lc");
    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(refs.as_slice(), |row| {
        let data: String = row.get(0)?;
        serde_json::from_str::<ExtensionEntry>(&data)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))
    })?;
    rows.collect()
}

pub fn clear_source(conn: &Connection, source_id: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM catalog WHERE source_id = ?1", params![source_id])
}

pub struct CatalogCache {
    conn: Arc<Mutex<Connection>>,
}

impl CatalogCache {
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        init_schema(&conn)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        init_schema(&conn)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }
    pub async fn upsert_many(&self, entries: &[ExtensionEntry]) -> rusqlite::Result<()> {
        let guard = self.conn.lock().await;
        for e in entries { upsert_entry(&guard, e)?; }
        Ok(())
    }
    pub async fn query(&self, f: &CatalogFilter) -> rusqlite::Result<Vec<ExtensionEntry>> {
        let guard = self.conn.lock().await;
        query_entries(&guard, f)
    }
    /// Atomic per-source refresh: clear the source's rows then insert fresh.
    pub async fn replace_source(&self, source_id: &str, entries: &[ExtensionEntry]) -> rusqlite::Result<()> {
        let guard = self.conn.lock().await;
        clear_source(&guard, source_id)?;
        for e in entries { upsert_entry(&guard, e)?; }
        Ok(())
    }
}
```

Add `pub mod cache;` to `src/store/mod.rs`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p alephcore store::cache::tests`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/store/cache.rs src/store/mod.rs
git commit -m "feat(store): rusqlite catalog cache with filtered query"
```

---

### Task 5: Installed-state reconciliation

**Files:**
- Create: `src/store/reconcile.rs`
- Modify: `src/store/mod.rs` (add `pub mod reconcile;`)
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `ExtensionEntry`, `ExtensionKind`, `ExtensionCategory`, `TrustTier`; backend types `McpServerInfo`/`McpTransportType`/`HealthStatus` (`src/mcp/manager/types.rs`), `PluginRecord`/`PluginStatus` (`src/extension/types/plugins.rs`), `SkillStatusEntry` (`src/skill/status.rs`).
- Produces: `mcp_to_entry(&McpServerInfo) -> ExtensionEntry`, `plugin_to_entry(&PluginRecord) -> ExtensionEntry`, `skill_to_entry(&SkillStatusEntry) -> ExtensionEntry`. All set `installed: true`, `category: ExtensionCategory::Other` (categorization is the Store Agent's job in P4), `trust_tier: TrustTier::Unverified`, `source_id: "local"`.

> Rationale: locally-installed items (incl. pre-store/manual config) are surfaced verbatim; the Store Agent later enriches category/trust where it can match a catalog entry.

- [ ] **Step 1: Write the failing test** in `src/store/reconcile.rs`

```rust
use crate::store::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, TrustTier};
use crate::mcp::manager::types::{HealthStatus, McpServerInfo, McpTransportType};
use crate::extension::types::plugins::{PluginRecord, PluginStatus};
use crate::skill::status::SkillStatusEntry;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_server_becomes_installed_entry() {
        let info = McpServerInfo {
            id: "github".into(),
            name: "GitHub".into(),
            transport: McpTransportType::Stdio,
            tool_count: 12,
            resource_count: 0,
            prompt_count: 0,
            health: HealthStatus::Healthy,
        };
        let e = mcp_to_entry(&info);
        assert_eq!(e.kind, ExtensionKind::Mcp);
        assert!(e.installed);
        assert!(e.enabled);                 // Healthy => enabled
        assert_eq!(e.id, "local:mcp:github");
        assert_eq!(e.source_id, "local");
        assert_eq!(e.trust_tier, TrustTier::Unverified);
    }

    #[test]
    fn stopped_mcp_is_disabled() {
        let info = McpServerInfo {
            id: "x".into(), name: "X".into(), transport: McpTransportType::Stdio,
            tool_count: 0, resource_count: 0, prompt_count: 0, health: HealthStatus::Stopped,
        };
        assert!(!mcp_to_entry(&info).enabled);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore store::reconcile::tests::mcp_server_becomes_installed_entry`
Expected: FAIL — `mcp_to_entry` not defined.

- [ ] **Step 3: Implement the mappers** (above the test module)

```rust
fn base_entry(kind: ExtensionKind, local_id: &str, name: String) -> ExtensionEntry {
    ExtensionEntry {
        id: format!("local:{}:{}", kind.as_str(), local_id),
        kind,
        category: ExtensionCategory::Other,
        name,
        description: String::new(),
        author: None,
        icon: None,
        tags: vec![kind.as_str().to_string()],
        version: None,
        source_id: "local".into(),
        repo_url: None,
        trust_tier: TrustTier::Unverified,
        requires_config: false,
        config_schema: None,
        installed: true,
        enabled: true,
        update_available: false,
    }
}

pub fn mcp_to_entry(info: &McpServerInfo) -> ExtensionEntry {
    let mut e = base_entry(ExtensionKind::Mcp, &info.id, info.name.clone());
    e.enabled = !matches!(info.health, HealthStatus::Stopped | HealthStatus::Dead);
    e
}

pub fn plugin_to_entry(p: &PluginRecord) -> ExtensionEntry {
    let mut e = base_entry(ExtensionKind::Plugin, &p.id, p.name.clone());
    e.description = p.description.clone().unwrap_or_default();
    e.version = p.version.clone();
    e.enabled = matches!(p.status, PluginStatus::Loaded);
    e
}

pub fn skill_to_entry(s: &SkillStatusEntry) -> ExtensionEntry {
    // SkillStatusEntry fields: id, name, disabled (verify exact names at src/skill/status.rs)
    let mut e = base_entry(ExtensionKind::Skill, &s.id, s.name.clone());
    e.enabled = !s.disabled;
    e
}
```
> Implementer note: open `src/skill/status.rs` and confirm `SkillStatusEntry`'s `id`/`name`/`disabled` field names; adjust `skill_to_entry` if they differ (e.g. `enabled` instead of `disabled`). The two MCP/plugin mappers use fields verified in this plan's reference list.

Add `pub mod reconcile;` to `src/store/mod.rs`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p alephcore store::reconcile::tests`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/store/reconcile.rs src/store/mod.rs
git commit -m "feat(store): reconcile installed mcp/plugin/skill into ExtensionEntry"
```

---

### Task 6: `extensions.*` façade — types + handlers (installed / catalog / toggle / uninstall)

**Files:**
- Create: `src/gateway/handlers/extensions/mod.rs`
- Create: `src/gateway/handlers/extensions/catalog.rs`
- Create: `src/gateway/handlers/extensions/lifecycle.rs`
- Modify: `src/gateway/handlers/mod.rs` (add `pub mod extensions;`)
- Test: inline `#[cfg(test)]` for param/response shaping (handlers that call live managers are smoke-tested in Task 7)

**Interfaces:**
- Consumes: `CatalogCache`/`CatalogFilter` (Task 4), reconcile mappers (Task 5), `parse_params`, `JsonRpcResponse`.
- Produces (registered in Task 7): `extensions.catalog` (filtered read from cache → `{ extensions: [ExtensionEntry] }`), `extensions.installed` (reconciled live list), `extensions.toggle` (`{ id, enabled }`), `extensions.uninstall` (`{ id }`). `id` for local items is `local:{kind}:{backend_id}`; the handler parses kind + backend id from it.

- [ ] **Step 1: Write the failing test** in `src/gateway/handlers/extensions/lifecycle.rs`

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ToggleParams { pub id: String, pub enabled: bool }

#[derive(Debug, Deserialize)]
pub struct UninstallParams { pub id: String }

/// Parse a façade id of the form `local:{kind}:{backend_id}` (or `{provider}:{native_id}`).
/// Returns (kind, backend_id) for `local:` ids; None for catalog ids that aren't installed.
pub fn parse_local_id(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix("local:")?;
    let (kind, backend) = rest.split_once(':')?;
    Some((kind, backend))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_mcp_id() {
        assert_eq!(parse_local_id("local:mcp:github"), Some(("mcp", "github")));
    }
    #[test]
    fn rejects_non_local_id() {
        assert_eq!(parse_local_id("mcp-official:io.x/y"), None);
    }
    #[test]
    fn handles_backend_ids_with_colons() {
        // split_once stops at the first ':', so a backend id may itself contain ':'.
        assert_eq!(parse_local_id("local:skill:my:skill"), Some(("skill", "my:skill")));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore gateway::handlers::extensions::lifecycle::tests::parses_local_mcp_id`
Expected: FAIL — module not present until created.

- [ ] **Step 3: Implement `lifecycle.rs` handlers** (append below the test target code)

```rust
use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::handlers::plugins::handlers::get_extension_manager;
use crate::mcp::manager::handle::McpManagerHandle;
use serde_json::json;

/// extensions.toggle — enable/disable an installed extension, routed by kind.
pub async fn handle_toggle(req: JsonRpcRequest, mcp: McpManagerHandle) -> JsonRpcResponse {
    let p: ToggleParams = match parse_params(&req) { Ok(p) => p, Err(e) => return e };
    let Some((kind, backend)) = parse_local_id(&p.id) else {
        return JsonRpcResponse::error(req.id, INVALID_PARAMS, "toggle requires an installed (local:) id");
    };
    let result: Result<(), String> = match kind {
        "mcp" => {
            if p.enabled { mcp.start_server(backend).await } else { mcp.stop_server(backend).await }
                .map_err(|e| e.to_string())
        }
        "plugin" => {
            // Reuse the CLI-level enable/disable marker toggles.
            if p.enabled { crate::cli_plugins_enable(backend) } else { crate::cli_plugins_disable(backend) }
        }
        "skill" => crate::store::lifecycle_glue::set_skill_enabled(backend, p.enabled).await,
        other => Err(format!("unknown kind: {other}")),
    };
    match result {
        Ok(()) => JsonRpcResponse::success(req.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, e),
    }
}

/// extensions.uninstall — remove an installed extension, routed by kind.
pub async fn handle_uninstall(req: JsonRpcRequest, mcp: McpManagerHandle) -> JsonRpcResponse {
    let p: UninstallParams = match parse_params(&req) { Ok(p) => p, Err(e) => return e };
    let Some((kind, backend)) = parse_local_id(&p.id) else {
        return JsonRpcResponse::error(req.id, INVALID_PARAMS, "uninstall requires an installed (local:) id");
    };
    let result: Result<(), String> = match kind {
        "mcp" => mcp.remove_server(backend).await.map_err(|e| e.to_string()),
        "plugin" => crate::cli_plugins_uninstall(backend),
        "skill" => crate::store::lifecycle_glue::remove_skill(backend).await,
        other => Err(format!("unknown kind: {other}")),
    };
    match result {
        Ok(()) => JsonRpcResponse::success(req.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, e),
    }
}
```

> The `crate::cli_plugins_*` and `crate::store::lifecycle_glue::*` referenced above are thin in-process wrappers added in the next step so handlers stay readable. `get_extension_manager` import is kept for `catalog.rs`.

- [ ] **Step 4: Add the lifecycle glue** — create `src/store/lifecycle_glue.rs`

```rust
//! Thin in-process adapters so the gateway façade can drive skill/plugin
//! lifecycle without re-implementing it. Plugin enable/disable/uninstall reuse
//! the existing CLI handlers; skills go through the live SkillSystem.
use crate::skill::SkillSystem;

pub async fn set_skill_enabled(skill_id: &str, enabled: bool) -> Result<(), String> {
    let system = SkillSystem::current().ok_or_else(|| "skill system not initialized".to_string())?;
    let id = crate::skill::SkillId::from(skill_id.to_string());
    system
        .update_config(&id, crate::skill::config::SkillConfigUpdate::SetEnabled(enabled))
        .await
        .map_err(|e| e.to_string())
}

pub async fn remove_skill(skill_id: &str) -> Result<(), String> {
    let system = SkillSystem::current().ok_or_else(|| "skill system not initialized".to_string())?;
    let id = crate::skill::SkillId::from(skill_id.to_string());
    system.remove_skill(&id).await.map(|_| ()).map_err(|e| e.to_string())
}
```
> Implementer note: confirm the global SkillSystem accessor name (`SkillSystem::current()` is assumed; if the codebase exposes it differently — e.g. a `try_skill_system()` like `try_extension_manager()` — use that). Confirm `SkillId::from`. Re-export `cli_plugins_enable/disable/uninstall` as `pub use crate::bin...` is not possible across the bin boundary, so instead move the marker-file logic: in this step, inline the `.disabled` marker create/remove and `remove_dir_all` against `aleph_plugins_dir()/{name}` (mirrors `src/bin/aleph-server/commands/plugins.rs:184/208/154`). Add these as `pub fn` in `src/store/lifecycle_glue.rs`:

```rust
use crate::discovery::aleph_plugins_dir;

pub fn cli_plugins_enable(name: &str) -> Result<(), String> {
    let marker = aleph_plugins_dir().map_err(|e| e.to_string())?.join(name).join(".disabled");
    if marker.exists() { std::fs::remove_file(&marker).map_err(|e| e.to_string())?; }
    Ok(())
}
pub fn cli_plugins_disable(name: &str) -> Result<(), String> {
    let dir = aleph_plugins_dir().map_err(|e| e.to_string())?.join(name);
    if !dir.exists() { return Err(format!("plugin not found: {name}")); }
    std::fs::write(dir.join(".disabled"), b"").map_err(|e| e.to_string())
}
pub fn cli_plugins_uninstall(name: &str) -> Result<(), String> {
    let dir = aleph_plugins_dir().map_err(|e| e.to_string())?.join(name);
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())
}
```
Then change the `lifecycle.rs` handler calls from `crate::cli_plugins_*` to `crate::store::lifecycle_glue::cli_plugins_*`. Add `pub mod lifecycle_glue;` to `src/store/mod.rs`.

- [ ] **Step 5: Implement `catalog.rs`** (`extensions.installed` + `extensions.catalog`)

```rust
use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::gateway::handlers::plugins::handlers::get_extension_manager;
use crate::mcp::manager::handle::McpManagerHandle;
use crate::store::cache::{CatalogCache, CatalogFilter};
use crate::store::reconcile::{mcp_to_entry, plugin_to_entry, skill_to_entry};
use crate::store::types::{ExtensionCategory, ExtensionKind};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Default, Deserialize)]
pub struct CatalogParams {
    pub kind: Option<ExtensionKind>,
    pub category: Option<ExtensionCategory>,
    pub source_id: Option<String>,
    pub query: Option<String>,
}

/// extensions.catalog — filtered read of the cached catalog (offline-capable).
pub async fn handle_catalog(req: JsonRpcRequest, cache: Arc<CatalogCache>) -> JsonRpcResponse {
    let p: CatalogParams = match &req.params {
        Some(_) => match parse_params(&req) { Ok(p) => p, Err(e) => return e },
        None => CatalogParams::default(),
    };
    let filter = CatalogFilter { kind: p.kind, category: p.category, source_id: p.source_id, query: p.query };
    match cache.query(&filter).await {
        Ok(entries) => JsonRpcResponse::success(req.id, json!({ "extensions": entries })),
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, e.to_string()),
    }
}

/// extensions.installed — live reconciled list across all backends.
pub async fn handle_installed(req: JsonRpcRequest, mcp: McpManagerHandle) -> JsonRpcResponse {
    let mut out = Vec::new();

    match mcp.list_servers().await {
        Ok(servers) => out.extend(servers.iter().map(mcp_to_entry)),
        Err(e) => return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("mcp: {e}")),
    }

    match get_extension_manager() {
        Ok(mgr) => out.extend(mgr.plugin_registry().list_plugins().iter().map(|p| plugin_to_entry(p))),
        Err(resp) => return resp.with_id(req.id),
    }

    if let Some(system) = crate::skill::SkillSystem::current() {
        out.extend(system.full_status().await.iter().map(skill_to_entry));
    }

    JsonRpcResponse::success(req.id, json!({ "extensions": out }))
}
```
> Implementer notes: (a) confirm `ExtensionManager` exposes `plugin_registry()` returning something with `list_plugins()`; if access is behind a lock/accessor, adapt (e.g. `mgr.registry().read().list_plugins()`). (b) `JsonRpcResponse::with_id` is used by existing handlers (`get_extension_manager` returns an error response with `None` id — re-stamp it). Confirm `with_id` exists on `JsonRpcResponse`; if not, rebuild the error with `req.id`.

- [ ] **Step 6: Wire the façade module** — create `src/gateway/handlers/extensions/mod.rs`

```rust
pub mod catalog;
pub mod lifecycle;
```
Add to `src/gateway/handlers/mod.rs`:
```rust
pub mod extensions;
```

- [ ] **Step 7: Run the unit tests**

Run: `cargo test -p alephcore gateway::handlers::extensions::lifecycle::tests`
Expected: PASS (3 tests).

- [ ] **Step 8: Commit**

```bash
git add src/gateway/handlers/extensions/ src/gateway/handlers/mod.rs src/store/lifecycle_glue.rs src/store/mod.rs
git commit -m "feat(store): extensions.* façade (catalog/installed/toggle/uninstall)"
```

---

### Task 7: Register `extensions.*` handlers + build & smoke verification

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/mod.rs` (or the sibling that registers `mcp.*` — mirror `…/builder/handlers/mcp.rs`)
- Create: `src/bin/aleph-server/commands/start/builder/handlers/extensions.rs`

**Interfaces:**
- Consumes: the façade handlers (Task 6), the `McpManagerHandle`, and a shared `Arc<CatalogCache>` constructed at startup (open at `~/.aleph/store_catalog.db`).
- Produces: registered methods `extensions.catalog`, `extensions.installed`, `extensions.toggle`, `extensions.uninstall`.

- [ ] **Step 1: Construct the shared cache at startup**

In the builder where `McpManagerHandle` is created, add (using `crate::discovery::aleph_home_dir`):
```rust
let catalog_path = alephcore::discovery::aleph_home_dir()
    .map(|d| d.join("store_catalog.db"))
    .unwrap_or_else(|_| std::path::PathBuf::from("store_catalog.db"));
let catalog_cache = std::sync::Arc::new(
    alephcore::store::cache::CatalogCache::open(&catalog_path)
        .expect("open store catalog cache"),
);
```

- [ ] **Step 2: Create the registration fn** `…/builder/handlers/extensions.rs` (mirror `mcp.rs`'s `reg!` macro pattern)

```rust
use alephcore::gateway::handlers::extensions;
use alephcore::gateway::server::GatewayServer;
use alephcore::mcp::manager::handle::McpManagerHandle;
use alephcore::store::cache::CatalogCache;
use std::sync::Arc;

pub fn register(server: &mut GatewayServer, mcp: McpManagerHandle, cache: Arc<CatalogCache>) {
    {
        let cache = cache.clone();
        server.handlers_mut().register("extensions.catalog", move |req| {
            let cache = cache.clone();
            async move { extensions::catalog::handle_catalog(req, cache).await }
        });
    }
    {
        let mcp = mcp.clone();
        server.handlers_mut().register("extensions.installed", move |req| {
            let mcp = mcp.clone();
            async move { extensions::catalog::handle_installed(req, mcp).await }
        });
    }
    {
        let mcp = mcp.clone();
        server.handlers_mut().register("extensions.toggle", move |req| {
            let mcp = mcp.clone();
            async move { extensions::lifecycle::handle_toggle(req, mcp).await }
        });
    }
    {
        let mcp = mcp.clone();
        server.handlers_mut().register("extensions.uninstall", move |req| {
            let mcp = mcp.clone();
            async move { extensions::lifecycle::handle_uninstall(req, mcp).await }
        });
    }
}
```
Call `extensions::register(&mut server, mcp_handle.clone(), catalog_cache.clone());` where `mcp::register(...)` is called in the builder.

- [ ] **Step 3: Build the whole workspace**

Run: `cargo build -p alephcore && cargo build -p aleph-server`
Expected: compiles cleanly. Fix any signature drift flagged by the implementer-notes in Tasks 5–6.

- [ ] **Step 4: Smoke test the façade against a running daemon**

Start the server (per the project's run path), then call the RPC over the gateway. Using the panel dev tools or a websocket client, send:
```json
{ "jsonrpc": "2.0", "id": 1, "method": "extensions.installed", "params": {} }
```
Expected: `result.extensions` is an array; every currently-configured MCP server, installed plugin, and registered skill appears as an `ExtensionEntry` with `installed: true` and an `id` like `local:mcp:<id>` / `local:plugin:<id>` / `local:skill:<id>`.

Then toggle one off and confirm it reports `ok: true`:
```json
{ "jsonrpc": "2.0", "id": 2, "method": "extensions.toggle", "params": { "id": "local:mcp:<some-id>", "enabled": false } }
```

- [ ] **Step 5: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/handlers/extensions.rs src/bin/aleph-server/commands/start/builder/handlers/mod.rs
git commit -m "feat(store): register extensions.* handlers + startup catalog cache"
```

---

## Self-review (P0)

**Spec coverage (P0 scope):** unified types §5 → Tasks 1–3 ✓; SQLite catalog cache §6 → Task 4 ✓; installed reconciliation §7 → Task 5 ✓; `extensions.*` façade §8 (catalog/installed/toggle/uninstall) → Tasks 6–7 ✓. Install/configure/sources are P1/P2 (not P0) ✓.

**Placeholder scan:** no TBD/TODO; every code step shows complete code. Three "implementer notes" flag exact-field confirmations (skill status field names, `plugin_registry()` accessor, `SkillSystem::current()`/`with_id`) — these are verification pointers with concrete fallbacks, not placeholders.

**Type consistency:** `ExtensionEntry`/`ExtensionKind`/`ExtensionCategory`/`TrustTier`/`InstallSpec` defined in Task 1–3 are used identically in Tasks 4–6. `CatalogFilter` (Task 4) consumed in Task 6. `parse_local_id` (Task 6) returns `(&str, &str)` used consistently by toggle/uninstall. Façade ids `local:{kind}:{backend}` produced by reconcile (Task 5) and parsed by lifecycle (Task 6) match.

**Known compile-risk points** (call out for the executor, each with a fallback in-task): skill status field names; `ExtensionManager` plugin-registry accessor; global `SkillSystem` accessor; `JsonRpcResponse::with_id`. None block the design; all are local field/accessor confirmations.
