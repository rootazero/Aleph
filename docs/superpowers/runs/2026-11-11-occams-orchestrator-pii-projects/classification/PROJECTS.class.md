# Classification & Routing Matrix: PROJECTS

**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-rev-orch-pii-proj`  
**Target Module**: `src/projects`  
**Files Audited (8 files)**:
- `src/projects/attribution_backfill.rs`
- `src/projects/authz.rs`
- `src/projects/binding.rs`
- `src/projects/events.rs`
- `src/projects/mod.rs`
- `src/projects/roster.rs`
- `src/projects/run_context.rs`
- `src/projects/store.rs`

---

## Executive Summary

The `src/projects` subsystem forms Aleph's project room architecture: room entity management, membership-as-authorization roster projection, channel-to-room conversation binding, task-local run context inheritance, and boot-time attribution backfill.

Under the Occam's Razor doctrine (*"Simplicity is the removal of unnecessary concepts, not merely line count reduction"*), this static review audited all 8 assigned files to identify dead code, accidental complexity, redundant allocations, and over-nested control flow.

### Key Findings
1. **Boilerplate Row Collection Consolidation**:
   Across `store.rs`, four query methods (`republish_roster_locked`, `list`, `members`, and `room_ids_for_member`) employ identical 5-line manual accumulator loops (`let mut out = Vec::new(); for row in rows { out.push(row.map_err(db_err)?); } Ok(out)`). These condense cleanly to `rows.collect::<Result<Vec<_>, _>>().map_err(db_err)`.
2. **Redundant SQL Projection String Allocations**:
   - In `row_to_project`, `row.get::<_, String>(4)?` allocates a heap `String` solely to pass a `&str` slice to `ProjectStatus::from_stored(&str)`. Reading `row.get_ref(4)?.as_str()?` directly borrows from the SQLite buffer, eliminating an allocation on every single project row returned by `list()`, `get()`, etc.
   - In `remove()`, `SELECT current_session_key FROM projects` parses an `Option<Option<String>>` solely to check whether the column is non-null (`Some(Some(_))`). Replacing this with a boolean probe `SELECT current_session_key IS NOT NULL FROM projects` avoids parsing and heap-allocating the session key string.
   - In `bind_conversation()`, `SELECT id FROM projects WHERE id = ?1 AND status = 'active'` fetches `id: String` into `Option<String>` only to test `exists.is_none()`. Replacing this with `SELECT 1 ...` and `|_| Ok(())` eliminates the redundant string allocation.
3. **Gratuitous Parameter `.to_string()` on `Cow<'_, str>`**:
   In `insert_project_row` and `bind_workspace`, `p.to_string_lossy().to_string()` forces a heap allocation, whereas `p.to_string_lossy()` already returns `Cow<'_, str>` which implements `rusqlite::ToSql` directly (as already done in lines 422 and 1537).
4. **Intermediate Allocation & Redundant Trimming in `add_for`**:
   `renamed` trims and converts `name` to an owned `Option<String>`, only to pass `renamed.as_deref()` to `touch_registered_row` (which expects `Option<&str>`). In addition, `resolve_display_name` receives `name` by value and re-trims it. Keeping `renamed` as `Option<&str>` eliminates both the intermediate String allocation and the redundant trim pass.
5. **Zero-Copy Borrowing in `conversation_of`**:
   `binding::conversation_of` returned `Option<(String, BindingPeerKind, String)>`, cloning `channel` and `peer_id` out of `SessionKey::Group` on every call. Changing the signature to `Option<(&str, BindingPeerKind, &str)>` eliminates two heap allocations per call across hot conversation routing paths.
6. **Direct Scope Rendering in Attribution Backfill**:
   `backfill_legacy_room_attribution` cloned `project.id` into `ScopeId::Project(project.id.clone()).render()`. Calling `aleph_protocol::scope::project_scope_id(&project.id)` directly avoids the intermediate enum allocation and `String` clone.

---

## Classification & Routing Matrix

| ID | Location | Taxonomy | Smell Description | Route | Action |
|:---|:---|:---|:---|:---|:---|
| **PROJ-1** | `src/projects/store.rs:463-466, 601-605, 1300-1304, 1333-1337` | `CONTROL-FLOW / INDIRECTION` | Four query methods (`republish_roster_locked`, `list`, `members`, `room_ids_for_member`) repeat identical manual row accumulation loops: `let mut out = Vec::new(); for row in rows { out.push(row.map_err(db_err)?); } Ok(out)`. | **FIX-NOW** | Replace manual loops with idiomatic `rows.collect::<Result<Vec<_>, _>>().map_err(db_err)`. Eliminates 16 lines of repetitive boilerplate and manual mutable accumulation. |
| **PROJ-2** | `src/projects/store.rs:1592-1600` | `REDUNDANT-ALLOCATION` | In `row_to_project`, `let status: String = row.get(4)?;` allocates a heap `String` solely to pass `&status` to `ProjectStatus::from_stored(&status)`. | **FIX-NOW** | Borrow directly from the SQLite row buffer via `row.get_ref(4)?.as_str()?` (or `row.get::<_, &str>(4)?`), passing `&str` directly to `ProjectStatus::from_stored`. Eliminates 1 heap allocation for every project row read across `list()`, `get()`, `find_by_path_for()`, etc. |
| **PROJ-3** | `src/projects/store.rs:553, 749` | `REDUNDANT-ALLOCATION` | In `insert_project_row` and `bind_workspace`, `p.to_string_lossy().to_string()` forces a heap `String` allocation. `p.to_string_lossy()` returns `Cow<'_, str>` which implements `rusqlite::ToSql` directly. | **FIX-NOW** | Remove `.to_string()` and bind `p.to_string_lossy()` directly as `Option<Cow<'_, str>>`. |
| **PROJ-4** | `src/projects/store.rs:1398-1403, 1418, 1445, 1550-1568, 1660-1675` | `REDUNDANT-ALLOCATION` | In `add_for`, `renamed` allocates an owned `Option<String>` via `.map(str::to_string)`, only to pass `renamed.as_deref()` to `touch_registered_row(conn, &existing.id, renamed.as_deref())` (which takes `Option<&str>`). Also, `resolve_display_name` receives `name` by value and re-trims it, creating another duplicate string allocation. | **FIX-NOW** | Retain `renamed: Option<&str> = name.as_deref().map(str::trim).filter(|n| !n.is_empty())`. Compute `display = renamed.map_or_else(|| absolute.file_name().and_then(|s| s.to_str()).unwrap_or("project").to_string(), str::to_string)`. Pass `renamed` directly to `touch_registered_row`. |
| **PROJ-5** | `src/projects/store.rs:950-960` | `REDUNDANT-ALLOCATION` | In `bind_conversation`, `SELECT id FROM projects WHERE id = ?1 AND status = 'active'` extracts `r.get::<_, String>(0)` into an owned `Option<String>` solely to check `exists.is_none()`. | **FIX-NOW** | Replace query with `SELECT 1 FROM projects WHERE id = ?1 AND status = 'active'` using row mapper `|_| Ok(())` and checking `.optional()?.is_some()`. Matches the pattern used in `has_channel_binding`. |
| **PROJ-6** | `src/projects/store.rs:693-711` | `REDUNDANT-ALLOCATION` | In `remove`, `SELECT current_session_key FROM projects WHERE id = ?1` returns `Option<Option<String>>`, allocating a `String` if set, solely to check `Some(Some(_))` without ever inspecting the string contents. | **FIX-NOW** | Query `SELECT current_session_key IS NOT NULL FROM projects WHERE id = ?1` mapped as `|r| r.get::<_, bool>(0)`. If `None` -> `NotFound`, `Some(true)` -> `Invalid`, `Some(false)` -> proceed. Eliminates `Option<Option<String>>` and avoids allocating the session key string. |
| **PROJ-7** | `src/projects/store.rs:930-1015` | `REDUNDANT-ALLOCATION` | In `bind_conversation`, `channel_key` and `peer_key` are cloned when building `ChannelBinding` because `params!` consumed them by value in earlier statements. | **FIX-NOW** | Pass `&channel_key` and `&peer_key` to `params!`, allowing `channel_key` and `peer_key` to be moved into `ChannelBinding` without calling `.clone()`. |
| **PROJ-8** | `src/projects/store.rs:741-744` | `CONTROL-FLOW / INDIRECTION` | In `bind_workspace`, verbose pattern matching: `let canonical = match path { Some(p) => Some(canonical_dir(p)?), None => None };`. | **FIX-NOW** | Simplify to `let canonical = path.map(canonical_dir).transpose()?;`. |
| **PROJ-9** | `src/projects/store.rs:1478-1484` | `CONTROL-FLOW / INDIRECTION` | In `create_blank`, verbose matching: `match self.add(...) { Ok(p) => Ok(p), Err(e) => { let _ = std::fs::remove_dir(&target); Err(e) } }`. | **FIX-NOW** | Simplify to `self.add(&target, Some(trimmed.to_string())).map_err(|e| { let _ = std::fs::remove_dir(&target); e })`. |
| **PROJ-10** | `src/projects/store.rs:1276-1284` | `REDUNDANT-ALLOCATION` | In `bindings_for`, each iteration inside `for row in rows` executes `project_id: project_id.to_string()` to populate `ChannelBinding.project_id`, re-allocating identical strings from `&str` on every row. | **FIX-NOW** | Pre-allocate `let project_id_owned = project_id.to_string();` outside the loop and use `project_id_owned.clone()` inside, cloning the existing heap string directly. |
| **PROJ-11** | `src/projects/binding.rs:191-201` | `REDUNDANT-ALLOCATION` | `conversation_of(key: &SessionKey) -> Option<(String, BindingPeerKind, String)>` clones `channel` and `peer_id` out of `SessionKey::Group` on every call. Both callers (`store.rs:1105`, `projects_channel.rs:219`) only need borrowed `&str` slices for lookup and equality comparison. | **FIX-NOW** | Change signature to `pub fn conversation_of(key: &SessionKey) -> Option<(&str, BindingPeerKind, &str)>` returning borrowed slices. Eliminates 2 heap allocations on every group key conversation lookup. |
| **PROJ-12** | `src/projects/attribution_backfill.rs:82, 151` | `REDUNDANT-ALLOCATION` | `let scope = ScopeId::Project(project.id.clone()).render();` clones `project.id` and allocates enum `ScopeId::Project` solely to call `.render()`, which simply calls `wire::project_scope_id(ref_id)`. | **FIX-NOW** | Replace with `let scope = aleph_protocol::scope::project_scope_id(&project.id);` and remove unused import `use crate::scope::ScopeId;`. Eliminates 1 `String` clone per project row. |
| **PROJ-13** | `src/projects/binding.rs:96-152` | `DEAD-CODE / ACCIDENTAL-COMPLEXITY` | ~60 lines of historical design comments documenting the deletion of `from_wire`, `wire_str`, `parse_wire`, and `peer_kind_str`. | **REJECT** | Do not prune. These tombstone comments document non-obvious cross-crate architectural invariants and prevent regressions of Shape 10 (cross-crate wire contract duplication) and Shape 2 (tests that cannot go red). |
| **PROJ-14** | `src/projects/authz.rs:189-195` | `REDUNDANT-ALLOCATION` | In `ambiguous_user_refusal`, `candidates.iter().map(...).collect::<Vec<_>>().join("; ")` creates an intermediate `Vec<String>`. | **REJECT** | Error path that only fires on ambiguous user name resolution. The allocation is transient and minor; micro-optimizing it with manual string formatting adds code complexity without runtime benefit. |
| **PROJ-15** | `src/projects/authz.rs:228-233` | `REDUNDANT-ALLOCATION` | In `principal_id_for_name`, intermediate `views: Vec<aleph_protocol::users::UserView>` collected from `records`. | **REJECT** | `aleph_protocol::users::resolve` strictly requires a slice `&[UserView]`, making intermediate vector allocation unavoidable. |
| **PROJ-16** | `src/projects/run_context.rs:41-43` | `REDUNDANT-ALLOCATION` | `current() -> Option<PathBuf>` clones the task-local `CURRENT_PROJECT_ROOT`, even though many external callers call `.as_deref()`. | **REJECT** | Returning owned `Option<PathBuf>` is required because references to task-local values cannot escape `try_with`. Callers that need owned `PathBuf` across `tokio::spawn` require this exact contract. |
| **PROJ-17** | `src/projects/events.rs:36-40` | `REDUNDANT-ALLOCATION` | `publish_changed` allocates `project_id.to_string()` and `affected_user.map(str::to_string)`. | **REJECT** | `GatewayEventFrame::ProjectsChanged` requires owned `String` fields for cross-thread event bus message passing. |
| **PROJ-18** | `src/projects/roster.rs:46-52` | `REDUNDANT-ALLOCATION` | `RosterSnapshot::from_pairs` initializes `HashMap::new()` without pre-reserving capacity from `pairs.into_iter().size_hint()`. | **REJECT** | Roster collections are typically very small (a handful of members); capacity hinting adds unnecessary code clutter for negligible gain. |

---

## Invariant Safety Pre-Flight

### 1. Storage & Transactional Invariants (`store.rs`, `authz.rs`)
- **Zero Schema Mutation**: None of the proposed actions alter SQLite tables (`projects`, `project_members`, `project_channel_bindings`), columns, indexes, or migration sequences.
- **Fail-Closed Semantics**:
  - `bind_conversation` existence check (PROJ-5) preserves the exact fail-closed `ProjectError::NotFound` refusal if no active project exists.
  - `remove` room check (PROJ-6) preserves both `NotFound` and `Invalid` guards with identical error messages.
  - Authorization checks in `authz.rs` (`project_for`, `manageable_given`, `is_active_admin`, `is_active_principal`, `principal_id_for_name`) remain untouched and fail closed on errors.
- **Lock Acquisition Invariants**:
  - Concurrency control via single mutex slot `self.conn.lock()` is preserved.
  - Atomicity of `tx.commit()` and subsequent `Self::republish_roster_locked(conn)` inside the same lock remains strictly preserved, preventing roster snapshot interleaving.

### 2. Attribution, Events & Context (`attribution_backfill.rs`, `events.rs`, `run_context.rs`, `binding.rs`)
- **Normalization Invariance**:
  - `conversation_of` (PROJ-11) returning `Option<(&str, BindingPeerKind, &str)>` retains exact `SessionKey::Group` field references without modifying the normalization performed at key construction. All call sites (`store.rs:1105`, `projects_channel.rs:219`) perform identical string comparisons and lookups.
  - `project_for_conversation` continues to normalize through `binding::normalize_component`.
- **Scope Identifier Formatting**:
  - In `attribution_backfill.rs` (PROJ-12), replacing `ScopeId::Project(project.id.clone()).render()` with `aleph_protocol::scope::project_scope_id(&project.id)` calls the exact underlying function that `ScopeId::render()` delegates to, guaranteeing byte-for-byte identical scope string generation (`"project:<id>"`).
- **Event Bus Ordering**:
  - `events::publish_changed` remains intact and retains at-least-once frame publication.
