# Jev Classification Report: Batch HUB2 (Hub Operations)

**Batch ID**: HUB2  
**Target Module**: `src/hub`  
**Files Audited**:
- `src/hub/hub_catalog.rs`
- `src/hub/install.rs`
- `src/hub/cache.rs`
- `src/hub/reconcile.rs`
- `src/hub/official_mcp.rs`
- `src/hub/official_plugins.rs`
- `src/hub/official_skills.rs`

---

## 1. Executive Summary & Routing Table

| Finding ID | File | Category | Smell Summary | Route |
|---|---|---|---|---|
| HUB2-01 | `hub_catalog.rs:167-183` | `CORRECTNESS` | Off-by-one budget check rejects valid schemas with exactly `MAX_PROPERTIES` | `FIX-NOW` |
| HUB2-02 | `hub_catalog.rs:223` | `REDUNDANT-ALLOCATION` | Redundant `.clone().or_else(|| Some(...))` in `to_entry` | `FIX-NOW` |
| HUB2-03 | `install.rs:163-168` | `DEFENSIVE-CHECK` | Subdir path traversal guard permits Windows drive prefix (`C:\...`) and absolute paths | `FIX-CAREFULLY` |
| HUB2-04 | `install.rs:211-215` | `CORRECTNESS` | Git checkout leaf allows symlink breakout outside repository root | `FIX-CAREFULLY` |
| HUB2-05 | `install.rs:441-447` | `CONTROL-FLOW` | Redundant nested match/if-let in `run_install` MCP branch | `FIX-NOW` |
| HUB2-06 | `cache.rs:93-113` | `REDUNDANT-ALLOCATION` | Gratuitous `Box<dyn ToSql>`, clones, `.to_string()`, and secondary `refs` vector | `FIX-NOW` |
| HUB2-07 | `cache.rs:26-37, 120-123` | `REDUNDANT-ALLOCATION` | `matches_query` repeatedly allocates lowercased string per catalog row | `FIX-NOW` |
| HUB2-08 | `cache.rs:199-212` | `CONTROL-FLOW` | `upsert_many` spawns blocking task and opens transaction for empty entries slice | `FIX-NOW` |
| HUB2-09 | `reconcile.rs:120-134` | `REDUNDANT-ALLOCATION` | `by_name` allocates String for static enum kind; `by_mcp_facade` clones owned String keys | `FIX-NOW` |
| HUB2-10 | `reconcile.rs:149-175, 183-189`| `REDUNDANT-ALLOCATION` | O(N) repeated linear scans over `origins` and String clone in `ledger_local_ref` | `FIX-NOW` |
| HUB2-11 | `official_mcp.rs:49-53` | `CONTROL-FLOW` | `.map(clean).unwrap_or(false)` instead of idiomatic `is_some_and` | `FIX-NOW` |
| HUB2-12 | `official_mcp.rs:149-150` | `REDUNDANT-ALLOCATION` | Redundant `.clone()` of owned `cfg.id` in `migrate_legacy_preset_servers` | `FIX-NOW` |
| HUB2-13 | `official_skills.rs:45` | `CONTROL-FLOW` | Redundant string conversion `.to_str() == Some(...)` on `OsStr` | `FIX-NOW` |
| HUB2-14 | `official_plugins.rs` | `NONE` | Clean and minimal; no accidental complexity detected | `REJECT` |

---

## 2. Detailed Findings

### HUB2-01: Off-by-one budget check in `validate_config_schema`
- **Location**: `src/hub/hub_catalog.rs:167-170, 176-179`
- **Category**: `CORRECTNESS` / `INVARIANT-TIGHTENING`
- **Impact**: `*budget = budget.saturating_sub(map.len()); if *budget == 0 { return Err(...); }`
  When a schema has exactly 1024 properties, `*budget` becomes 0, triggering the `Err` despite the contract explicitly specifying `<= 1024 total properties`.
- **Action**: Check `if map.len() > *budget` before subtracting.

### HUB2-02: Redundant `.clone().or_else(|| Some(...))` in `to_entry`
- **Location**: `src/hub/hub_catalog.rs:223`
- **Category**: `REDUNDANT-ALLOCATION` / `CONTROL-FLOW`
- **Impact**: Unnecessary option closure allocation and cloning.
- **Action**: Simplify to `Some(self.via.as_deref().unwrap_or(hub_id).to_string())`.

### HUB2-03: Subdir path traversal guard bypass on Windows drive prefixes & absolute paths
- **Location**: `src/hub/install.rs:163-168`
- **Category**: `DEFENSIVE-CHECK` / `CORRECTNESS`
- **Impact**: `leaf.split(['/', '\\']).any(|seg| seg == "." || seg == ".." || seg.is_empty())`
  On Windows, `C:\path` segments are `["C:", "path"]`. Neither is empty or `..`. When joined via `checkout.join(&leaf)`, `checkout.join` replaces the base with the absolute path, escaping checkout isolation.
- **Action**: Check `leaf.contains(':') || std::path::Path::new(&leaf).is_absolute() || leaf.split(['/', '\\']).any(...)`.

### HUB2-04: Symlink breakout in git checkout leaf
- **Location**: `src/hub/install.rs:211-215`
- **Category**: `CORRECTNESS` / `INVARIANT-TIGHTENING`
- **Impact**: `src_leaf.is_dir()` follows symlinks. If a repository contains a symlink pointing outside the checkout, `copy_skill_leaf` will read and copy arbitrary host files.
- **Action**: Enforce `!src_leaf.is_symlink()`, canonicalize both `checkout` and `src_leaf`, and require `src_leaf_canon.starts_with(&checkout_canon)`.

### HUB2-05: Redundant nested match/if-let in `run_install`
- **Location**: `src/hub/install.rs:441-447`
- **Category**: `CONTROL-FLOW`
- **Impact**: Matching `InstallSpec::McpStdio` then immediately matching `if let InstallSpec::McpStdio` introduces unnecessary nesting.
- **Action**: Use guard clause or flatten match pattern.

### HUB2-06: Gratuitous `Box<dyn ToSql>` and clones in `cache.rs::query_entries`
- **Location**: `src/hub/cache.rs:93-113`
- **Category**: `REDUNDANT-ALLOCATION`
- **Impact**: Every query allocates a vector of boxes, clones query filter strings, calls `to_string()` on static enum strings, and collects into a secondary `refs` vector.
- **Action**: Use `Vec<&dyn rusqlite::types::ToSql>` directly borrowing from `f` and outer static variables.

### HUB2-07: Per-entry query string lowercasing in `cache.rs::matches_query`
- **Location**: `src/hub/cache.rs:26-37, 120-123`
- **Category**: `REDUNDANT-ALLOCATION`
- **Impact**: When filtering SQLite results, `let q = query.trim().to_lowercase()` is computed for every entry in the result set.
- **Action**: Lowercase `query` once in `query_entries` before filtering.

### HUB2-08: Empty batch handling in `CatalogCache::upsert_many`
- **Location**: `src/hub/cache.rs:199-212`
- **Category**: `CONTROL-FLOW`
- **Impact**: An empty `entries` slice spawns a blocking thread, locks the SQLite mutex, and starts an empty transaction.
- **Action**: Early return `if entries.is_empty() { return Ok(()); }`.

### HUB2-09: Unnecessary String allocations in `reconcile.rs::mark_installed_state`
- **Location**: `src/hub/reconcile.rs:120-134`
- **Category**: `REDUNDANT-ALLOCATION`
- **Impact**: `e.kind.as_str().to_string()` allocates a String for every installed entry for an immutable enum variant. `by_mcp_facade` clones `e.id` into an owned String key.
- **Action**: Use `(&'static str, String)` for `by_name` and `&str` for `by_mcp_facade`.

### HUB2-10: Repeated linear scans over `origins` in `mark_installed_state`
- **Location**: `src/hub/reconcile.rs:149-175, 183-189`
- **Category**: `REDUNDANT-ALLOCATION` / `CONTROL-FLOW` / `DEAD-CODE`
- **Impact**: Disambiguation runs `origins.iter().any` and `ledger_local_ref` (which clones String) inside a candidate loop, followed by another `origins.iter().find`.
- **Action**: Find `origin` once per catalog entry, reuse reference, and delete `ledger_local_ref`.

### HUB2-11: Non-idiomatic bool option check in `official_mcp.rs::is_projectable`
- **Location**: `src/hub/official_mcp.rs:49-53`
- **Category**: `CONTROL-FLOW`
- **Impact**: `.map(clean).unwrap_or(false)` is verbose compared to `is_some_and(clean)`.
- **Action**: Replace with `is_some_and(clean)`.

### HUB2-12: Redundant clone of owned `cfg.id` in `migrate_legacy_preset_servers`
- **Location**: `src/hub/official_mcp.rs:149-150`
- **Category**: `REDUNDANT-ALLOCATION`
- **Impact**: `cfg` is owned by value in the loop; `cfg.id.clone()` is unnecessary.
- **Action**: Move `cfg.id`.

### HUB2-13: Redundant string conversion on `OsStr` in `official_skills.rs`
- **Location**: `src/hub/official_skills.rs:45`
- **Category**: `CONTROL-FLOW`
- **Impact**: `and_then(|n| n.to_str()) == Some("SKILL.md")` converts `OsStr` to `str` before equality check.
- **Action**: Use `is_some_and(|n| n == "SKILL.md")`.

### HUB2-14: Clean status in `official_plugins.rs`
- **Location**: `src/hub/official_plugins.rs`
- **Category**: `NONE`
- **Action**: No refactoring needed.
