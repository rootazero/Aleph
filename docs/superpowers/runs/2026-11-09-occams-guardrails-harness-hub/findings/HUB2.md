# Occam's Razor Findings & Refactoring Report: Batch HUB2

**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-occams-guard-harness-hub`  
**Target Module**: `src/hub` (Operations: Catalog, Install, Cache, Reconcile, Official Extensions)  
**Target Files**:
- `src/hub/hub_catalog.rs`
- `src/hub/install.rs`
- `src/hub/cache.rs`
- `src/hub/reconcile.rs`
- `src/hub/official_mcp.rs`
- `src/hub/official_plugins.rs`
- `src/hub/official_skills.rs`

**Commit Message**: `fix(hub): apply Occam's razor simplifications in install, cache, and reconcile operations`

---

## 1. Summary of Work Done

During this review of Batch HUB2, we conducted a rigorous Occam's Razor static analysis and identified multiple performance bottlenecks, control flow indirections, and critical security/invariant boundary risks:
1. **Security & Path Traversal Guards (`src/hub/install.rs`)**:
   - Closed a Windows drive prefix (`C:\...`) and absolute path bypass in `install_git_skill` subdir traversal checks.
   - Closed a symlink breakout vulnerability where symlinked leaves inside cloned git repositories could copy arbitrary files outside the repository checkout root.
   - Enforced symlink rejection on `src_leaf` and verified canonical path prefixes (`src_leaf_canon.starts_with(&checkout_canon)`).
2. **Schema Validation Correctness (`src/hub/hub_catalog.rs`)**:
   - Fixed an off-by-one budget check in `validate_config_schema` where schemas with exactly `1024` properties (the documented threshold) were erroneously rejected.
   - Simplified option chaining in `HubCatalogEntry::to_entry`.
3. **Database & Query Allocations (`src/hub/cache.rs`)**:
   - Completely eliminated gratuitous `Box<dyn ToSql>`, `.clone()`, `.to_string()`, and secondary `refs` vector allocations across SQLite catalog queries in `query_entries`.
   - Prevented N redundant string lowercasing allocations inside the catalog query filtering loop by computing the search needle once.
   - Added zero-overhead early return for empty `upsert_many` slices to prevent acquiring the SQLite mutex and beginning empty transactions.
4. **Reconciliation Allocations & Invariant Preservation (`src/hub/reconcile.rs`)**:
   - Eliminated repetitive heap String allocations in `by_name` HashMap by using static `&'static str` for `ExtensionKind` enum variants instead of `e.kind.as_str().to_string()`.
   - Replaced owned `String` keys with borrowed `&str` keys in `by_mcp_facade`.
   - Eliminated O(N) redundant linear scans and String cloning over `origins` by performing a single lookup per catalog entry and removing dead helper `ledger_local_ref`.
5. **Extension Catalog Simplifications (`src/hub/official_mcp.rs`, `src/hub/official_skills.rs`)**:
   - Replaced verbose `.map(...).unwrap_or(false)` with idiomatic `is_some_and`.
   - Eliminated duplicate `.clone()` of owned `cfg.id` in `migrate_legacy_preset_servers`.
   - Replaced indirect `OsStr` string conversions in `official_skills.rs` with direct equality checks.

---

## 2. Invariants & Preservation Proofs

| Invariant | How Preserved |
|:---|:---|
| **Public API Contracts & Schemas** | All public functions (`run_install`, `collect_installed`, `mark_installed_state`, `primer_entries`, `CatalogCache::*`) retain their exact signatures and behavior. Wire formats and serde serialization contracts are untouched. |
| **Path Traversal & Sandboxing** | Directory escape via relative segments (`..`), empty components, Windows drive letters (`C:`), absolute paths, and git symlink breakouts are strictly rejected before any file operations occur. |
| **Atomic Cache & Lock Safety** | SQLite transactional semantics in `upsert_many` and `replace_source` are preserved. The off-runtime-thread execution model via `with_conn` and tokio mutex locking remains intact. |
| **Official Catalog Fallbacks & Schemas** | In-memory and embedded official presets (`official_mcp`, `official_plugins`, `official_skills`) preserve full compatibility with catalog JSON schemas, fallback lists, and wire formats. |

---

## 3. Surgical Changes Applied

### A. Strict Path Traversal and Symlink Breakout Guards (`src/hub/install.rs`)
- **Before**:
  `leaf.split(['/', '\\']).any(|seg| seg == "." || seg == ".." || seg.is_empty())`
  On Windows or crafted catalog entries, absolute paths or drive-prefixed paths (such as `C:\Windows`) bypassed the split checks because the leading segment was `"C:"`. Additionally, `src_leaf.is_dir()` followed symlinks, allowing a git repository containing a symlink to escape checkout boundaries during `copy_skill_leaf`.
- **After**:
  Added explicit checks for `leaf.contains(':') || std::path::Path::new(&leaf).is_absolute()`. Added symlink rejection (`!src_leaf.is_symlink()`) and canonicalization containment verification (`src_leaf_canon.starts_with(&checkout_canon)`). Added unit tests verifying rejection of drive-qualified paths, absolute paths, and symlink breakouts.

### B. Flattened MCP Command Validation in Install Router (`src/hub/install.rs`)
- **Before**:
  `match spec` matched `InstallSpec::McpStdio | McpRemote`, and then immediately nested an `if let InstallSpec::McpStdio { command, .. } = spec` check.
- **After**:
  Extracted early error match arm:
  `InstallSpec::McpStdio { command, .. } if !command_available(command) => Err(...)`
  Flattening control flow and removing redundant nested pattern matching.

### C. Off-by-one Config Schema Budget Fix (`src/hub/hub_catalog.rs`)
- **Before**:
  `*budget = budget.saturating_sub(map.len()); if *budget == 0 { return Err(...); }`
  When a schema had exactly 1024 properties, `budget` saturated to 0 and immediately failed with `>1024 properties`, violating the documented requirement that schemas with `<= 1024` properties are accepted.
- **After**:
  Replaced with `if map.len() > *budget { return Err(...); } *budget -= map.len();`, ensuring valid schemas with up to 1024 properties pass validation. Added unit test `validate_config_schema_budget`.

### D. Zero-Box Dynamic SQL Parameters (`src/hub/cache.rs`)
- **Before**:
  `query_entries` allocated `args: Vec<Box<dyn rusqlite::types::ToSql>>`, cloned `id` and `source_id`, called `.to_string()` on `kind.as_str()` and `category.as_str()`, and collected everything into a second `refs: Vec<&dyn ToSql>` vector.
- **After**:
  `args` is typed directly as `Vec<&dyn rusqlite::types::ToSql>` with `Vec::with_capacity(4)`. Binds directly borrow from `id`, `kind_str`, `cat_str`, and `source_id`, eliminating all `Box` allocations, string clones, and the secondary vector.

### E. Pre-Lowercased Search Needle in Catalog Cache Query (`src/hub/cache.rs`)
- **Before**:
  `matches_query(e, q)` called `let q = query.trim().to_lowercase()` for every single row returned from the database.
- **After**:
  Lowercased `q` once in `query_entries` before the retain loop (`matches_query_lowercased(e, &q_lc)`), reducing allocation overhead proportional to result set size.

### F. Fast Path for Empty Batch Upserts (`src/hub/cache.rs`)
- **Before**:
  Calling `CatalogCache::upsert_many(&[])` spawned a blocking task, locked the SQLite mutex, opened an empty transaction, and committed it.
- **After**:
  Added `if entries.is_empty() { return Ok(()); }` early return.

### G. Borrowed Keys and Single-Pass Origin Reconciliation (`src/hub/reconcile.rs`)
- **Before**:
  `by_name` was keyed by `(String, String)` and allocated `e.kind.as_str().to_string()` for every entry. `by_mcp_facade` cloned `e.id` into an owned String key. During disambiguation, `origins` was repeatedly scanned via `origins.iter().any` and `ledger_local_ref` (which cloned String per candidate), and scanned again via `origins.iter().find`.
- **After**:
  - `by_name` uses `(&'static str, String)` avoiding String allocation for static enum variants.
  - `by_mcp_facade` uses borrowed `&str` keys (`HashMap<&str, &ExtensionEntry>`).
  - `origins.iter().find(|o| o.entry_id == e.id)` is evaluated once per catalog entry and reused for both collision disambiguation and update badge determination.
  - Dead private helper `ledger_local_ref` was removed.

### H. Idiomatic Helpers in Official Presets (`src/hub/official_mcp.rs`, `src/hub/official_skills.rs`)
- Replaced `.map(clean).unwrap_or(false)` with `.is_some_and(clean)` in `is_projectable`.
- Avoided double-cloning owned `cfg.id` in `migrate_legacy_preset_servers`.
- Replaced `f.path().file_name().and_then(|n| n.to_str()) == Some("SKILL.md")` with `f.path().file_name().is_some_and(|n| n == "SKILL.md")`.
