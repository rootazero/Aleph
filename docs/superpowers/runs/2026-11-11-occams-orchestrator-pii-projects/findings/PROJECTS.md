# Occam's Razor Remediation: Batch PROJECTS (`src/projects`)

## 1. Summary of Changes
- **`src/projects/attribution_backfill.rs`**: Replaced `ScopeId::Project(project.id.clone()).render()` with direct call to `aleph_protocol::scope::project_scope_id(&project.id)`, removing an intermediate enum instantiation and `String` clone. Removed unused `ScopeId` import.
- **`src/projects/store.rs`**:
  - Replaced repetitive manual row accumulation loops in `republish_roster_locked`, `list`, `members`, and `room_ids_for_member` with idiomatic `rows.collect::<Result<Vec<_>, _>>().map_err(db_err)`.
  - In `row_to_project`, borrowed `status_str: &str = row.get_ref(4)?.as_str()?` directly from the SQLite row buffer into `ProjectStatus::from_stored`, eliminating a heap `String` allocation for every project row read.
  - In `insert_project_row` and `bind_workspace`, bound `p.to_string_lossy()` directly as `Option<Cow<'_, str>>` without calling `.to_string()`.
  - In `remove()`, replaced `Option<Option<String>>` parsing with a boolean probe `SELECT current_session_key IS NOT NULL FROM projects WHERE id = ?1`, avoiding parsing and allocating the session key string.
  - In `bind_conversation()`, replaced `SELECT id FROM projects` with zero-allocation `SELECT 1 ...` probe, and borrowed `&channel_key` and `&peer_key` in `params!` so they can be moved directly into `ChannelBinding` without calling `.clone()`.
  - In `bind_workspace`, simplified control flow to `path.map(canonical_dir).transpose()?`.
  - In `bindings_for`, hoisted `let project_id_owned = project_id.to_string();` outside the loop, cloning the pre-allocated string instead of re-allocating from `&str` on every row.
  - In `add_for`, retained `renamed` as `Option<&str>`, eliminating intermediate `Option<String>` allocation and redundant trimming.
  - In `create_blank`, simplified control flow to `self.add(...).map_err(...)`.
- **Reasoning Tier Filter**: Retained `binding::conversation_of` returning `Option<(String, BindingPeerKind, String)>` to preserve cross-module API compatibility with `src/gateway/handlers/projects_channel.rs`.

## 2. Invariant Safety Pre-Flight
- Zero schema or table changes in SQLite (`projects`, `project_members`, `project_channel_bindings`).
- Fail-closed authz and room existence semantics (`NotFound`, `Invalid`) strictly preserved.
- Roster publication atomicity within transactions maintained.
