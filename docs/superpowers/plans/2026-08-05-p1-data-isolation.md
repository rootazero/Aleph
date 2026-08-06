# P1 Data Isolation Implementation Plan / P1「数据隔离」实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two users on one Aleph server cannot see each other's sessions, memory, artifacts, or live events; the single-user experience is byte-identical to today.

**Architecture:** Partition-key composition (Plan B, spec §3) — a new `src/scope/` vocabulary (`org` / `personal:<user_id>` / `project:<project_id>`) rides the *existing* `project_scope.rs` suffix mechanism for memory, new `owner_user_id`/`scope_id` fields on sessions and background-work stores, one ambient `ScopeAttribution` task-local seeded at gateway dispatch and at every `tokio::spawn` run boundary, and a `src/gateway/visibility.rs` predicate family consumed by every scoped-data RPC handler plus a 4th `&&` term in the WS event-delivery filter chain. Legacy rows (no owner field) are read as owner-owned — adoption by absence, zero backfill.

**Tech Stack:** Rust (tokio task-locals, rusqlite, serde), existing SQLite stores. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-04-multi-user-org-project-design.md` §5 (scopes), §8 P1 row, §9 (testing), §10 (edge semantics), §11 (hardening). User rulings 2026-08-05: P0 parked items (member event wildcard verify, `restamp_live_connections` panel filter, `tools.*` member carve-out) fold into P1; §11 low-cost hardening (member default exec tier `Ask`, role-aware tool permissions) is in scope.

**Recon dossiers** (verbatim code for every anchor named below; implementers read these before touching a file):
- `<workspace>/… (SDD workspace copies)` — controller: copy the three recon files into the SDD workspace at setup:
  - `p1-recon-gateway.md` (sessions/RPC/event chain), `p1-recon-memory.md` (partition seams), `p1-recon-background.md` (goals/loops/crons/subagents) from `C:\Users\zou\AppData\Local\Temp\claude\D--Workspace-Aleph\dadd5dff-cb09-40fe-a838-766ee6e0b45b\scratchpad\`.

---

## Global Constraints

1. **Acceptance (spec §8 P1):** 两用户会话/记忆互不可见；隔离守卫测试全绿；single-user experience unchanged (migration invariant: existing single-user data answers existing queries byte-identically).
2. **Legacy = owner:** any row whose `owner_user_id` is absent/None is owned by `OWNER_USER_ID` (`"u-owner"`, `src/gateway/security/store/users.rs`). This single rule replaces all backfill. The predicate lives in ONE place (`visibility::effective_owner`); never re-derive it inline.
3. **Fail-closed (P0 human ruling, still binding):** store `Err` or unparseable state at an authorization decision ⇒ deny, never fall through to allow. `.ok().flatten()` at a gate is the forbidden shape.
4. **No existence oracle:** addressed-key visibility failures (`sessions.history`, `artifacts.read_text`, …) return the same `RESOURCE_NOT_FOUND` error as a genuinely missing key — never "not authorized" (privacy-grade isolation must not confirm existence).
5. **Do NOT touch these dormant/deprecated mechanisms:** `NamespaceScope`/`namespace` column (`src/memory/namespace.rs` — parallel deprecated mechanism, stays inert), `SessionIdentityMeta` (`src/gateway/session_manager/mod.rs:102` — frozen guest-share identity, a DIFFERENT concept from owner; spec §3's "复活 SessionIdentityMeta" is implemented as first-class `SessionMetadata` columns instead, deviation recorded in Task 2).
6. **`agent_ids.first()` is forbidden** on any scope-union result — per-hit `fact.agent` stamping only (in-repo landmine, `src/memory/note_retrieval/mod.rs:763-767`).
7. **Prefix-cache discipline (CLAUDE.md §2.18):** per-USER bytes are per-SESSION stable (session owner is immutable, spec §10) — curated envelope stays session-frozen; nothing new may vary per-turn in the Stable zone.
8. **Verification set** (Windows; `cargo check -p alephcore` alone proves nothing): `cargo test -p alephcore --lib --no-run` must accompany any `pub` deletion; full gate per task = scoped `cargo test -p alephcore --lib <module>` foreground, no pipes, `timeout 600000`, prefix `CARGO_PROFILE_TEST_DEBUG=line-tables-only`. Final task runs the 4-command set (full lib test + `cargo check -p aleph-panel` + `cargo check -p aleph-desktop-windows` + `cargo clippy --all-targets`).
9. **Commits:** `<scope>: <description>`, English, one commit per task minimum. Single-branch main. **Do not push.**
10. **Tests assert effect, not invocation** (仓内纪律): isolation tests are "A creates data, B sees empty/NOT_FOUND", never "the filter function was called".
11. **R7/R10:** no LLM-judgment replacement, no `src/harness/` changes (nothing in this plan touches it; if an implementer thinks they need to, STOP and escalate).

---

## File Structure (new / modified)

| File | Responsibility |
|---|---|
| **Create** `src/scope/mod.rs` | Scope vocabulary: `ScopeId`, `ScopeAttribution`, task-local + `with_scope`/`current_scope`, metadata keys, `scope_from_metadata` |
| **Create** `src/gateway/visibility.rs` | Caller-visibility predicates: `visible_owner_filter`, `effective_owner`, `session_visible`, `partition_visible`, `not_found_response` |
| **Create** `src/gateway/method_visibility.rs` | Pin-list registry of every scoped-data RPC + treatment + guard tests (sibling of `method_admin.rs`) |
| **Create** `src/gateway/event_visibility.rs` | Run→session and session→owner bounded caches + `event_admits` (4th delivery filter) |
| Modify `src/gateway/session_store/types.rs` | `SessionMetadata` + `owner_user_id`/`scope_id`; `SessionFilter` + `owner_visible_to` |
| Modify `src/gateway/session_store/{file_backend,sqlite_backend}/…`, `src/gateway/session_manager/mod.rs` | create-time stamping; SQLite column migration; list filtering |
| Modify `src/gateway/server/handler.rs` | 4th task-local layer at both dispatch stations; 4th `&&` term in event write-loop |
| Modify `src/gateway/handlers/{session/db_handlers/*,memory.rs,artifacts.rs,clarification.rs,subagent.rs,graph/query.rs,users.rs}` | consume visibility predicates; restamp panel filter; deactivation freeze hook |
| Modify `src/gateway/handlers/agent.rs`, `src/gateway/inbound_router/executor.rs` | origin-side attribution metadata (the voice-registry two-writer sites) |
| Modify `src/gateway/execution_engine/{run_loop/mod.rs,execute.rs}`, `src/orchestrator/dispatch.rs`, `src/agents/subagent_tool/spawn.rs` | seed scope task-local inside spawn boundaries; `carry_policy_metadata` allowlist |
| Modify `src/memory/project_scope.rs`, `src/memory/assembler/gather.rs`, `src/thinker/memory_context_provider/*`, `src/memory/dreaming/mod.rs` | scope-aware write id + read union; floors split; curated per-scope + owner adoption; dream scan generalization |
| Modify `src/goal/{types.rs,store.rs}`, `src/looping/{types.rs,mod.rs}`, `src/tasks/cron/{config.rs,executor.rs}`, `src/gateway/execution_engine/goal_wait.rs` | owner/scope fields + wake metadata + `pause_all_owned_by` |
| Modify `src/gateway/method_admin.rs` | `MEMBER_CARVE_OUTS` += `tools.invoke`; module-doc P1 note updated to point at `method_visibility.rs` |
| Modify `docs/reference/SECURITY.md`, `src/gateway/CLAUDE.md` | P1 trust-model subsection; new landmines |

---

### Task 1: Scope vocabulary + ambient attribution (`src/scope/`)

**Files:**
- Create: `src/scope/mod.rs`
- Modify: `src/lib.rs` (add `pub mod scope;` alongside existing top-level modules)

**Interfaces (produced — later tasks consume these exact names):**
```rust
pub enum ScopeId { Org, Personal(String), Project(String) }
impl ScopeId {
    pub fn render(&self) -> String;                  // "org" | "personal:u-…" | "project:p-…"
    pub fn parse(s: &str) -> Option<ScopeId>;
    pub fn partition_suffix(&self) -> Option<&str>;  // Org→None, Personal(u)→Some(u), Project(p)→Some(p)
}
pub struct ScopeAttribution { pub owner_user_id: String, pub scope: ScopeId }
impl ScopeAttribution { pub fn personal(user_id: &str) -> Self; }
pub async fn with_scope<F: Future>(attr: Option<ScopeAttribution>, fut: F) -> F::Output;
pub fn current_scope() -> Option<ScopeAttribution>;
pub const OWNER_META_KEY: &str = "scope_owner_user_id";
pub const SCOPE_META_KEY: &str = "scope_id";
pub fn scope_from_metadata(meta: &std::collections::HashMap<String, String>) -> Option<ScopeAttribution>;
pub fn stamp_metadata(meta: &mut std::collections::HashMap<String, String>, attr: &ScopeAttribution);
```

- [ ] **Step 1: Write failing tests** (in `src/scope/mod.rs` `#[cfg(test)]`):

```rust
#[test]
fn render_parse_round_trips_all_three_kinds() {
    for s in [ScopeId::Org, ScopeId::Personal("u-alice".into()), ScopeId::Project("p-x7f2".into())] {
        assert_eq!(ScopeId::parse(&s.render()), Some(s.clone()));
    }
    assert_eq!(ScopeId::parse("personal:"), None, "empty ref is invalid");
    assert_eq!(ScopeId::parse("group:x"), None, "unknown kind is invalid — fail closed");
}

#[test]
fn partition_suffix_is_the_ref_verbatim() {
    assert_eq!(ScopeId::Org.partition_suffix(), None);
    assert_eq!(ScopeId::Personal("u-alice".into()).partition_suffix(), Some("u-alice"));
    assert_eq!(ScopeId::Project("p-x7f2".into()).partition_suffix(), Some("p-x7f2"));
}

#[tokio::test]
async fn task_local_scopes_and_does_not_cross_spawn() {
    // mirror src/projects/run_context.rs::task_local_does_not_cross_spawn_boundary
    let attr = ScopeAttribution::personal("u-alice");
    with_scope(Some(attr), async {
        assert_eq!(current_scope().unwrap().owner_user_id, "u-alice");
        let handle = tokio::spawn(async { current_scope() });
        assert!(handle.await.unwrap().is_none(), "task-locals must not cross spawn");
    }).await;
    assert!(current_scope().is_none(), "scope pops on future completion");
}

#[test]
fn metadata_round_trip() {
    let mut m = std::collections::HashMap::new();
    stamp_metadata(&mut m, &ScopeAttribution::personal("u-alice"));
    let back = scope_from_metadata(&m).unwrap();
    assert_eq!(back.owner_user_id, "u-alice");
    assert_eq!(back.scope, ScopeId::Personal("u-alice".into()));
    assert!(scope_from_metadata(&std::collections::HashMap::new()).is_none(), "absent keys → None (legacy)");
    // Corrupt scope_id with a present owner: fail closed to None, never guess.
    let mut bad = std::collections::HashMap::new();
    bad.insert(OWNER_META_KEY.to_string(), "u-alice".into());
    bad.insert(SCOPE_META_KEY.to_string(), "garbage".into());
    assert!(scope_from_metadata(&bad).is_none());
}
```

- [ ] **Step 2: Run to verify FAIL** — `cargo test -p alephcore --lib scope::` → compile error (module absent).
- [ ] **Step 3: Implement `src/scope/mod.rs`.** Module doc must state: (a) this is the spec §5.1 vocabulary; (b) `Personal`/`Project` refs are the P0 id formats verbatim (`u-<uuid>` from `users.rs:162`, `p-…` reserved for P2) so `partition_suffix` composes directly with `project_scope::scoped_agent_id` — the three suffix families `proj-*` (legacy directory feature), `u-*` (personal), `p-*` (project) are siblings, never nested; (c) the task-local follows `src/projects/run_context.rs`'s contract verbatim (children spawned via `tokio::spawn` MUST capture before the boundary — copy that doc language); (d) `scope_from_metadata` requires BOTH keys coherent, else `None` (Global Constraint 3). Implementation is mechanical from the tests; `with_scope`/`current_scope` copy `run_context.rs`'s `scope`/`try_with` shape exactly.
- [ ] **Step 4: Run to verify PASS** — `cargo test -p alephcore --lib scope::`.
- [ ] **Step 5: Commit** — `scope: add org/personal/project scope vocabulary and ambient attribution task-local`

---

### Task 2: Session `owner_user_id`/`scope_id` + create-time stamping + dispatch seeding

**Files:**
- Modify: `src/gateway/session_store/types.rs` (`SessionMetadata` at :76, `SessionFilter` at :320)
- Modify: `src/gateway/session_manager/mod.rs` (`run_migrations` static list, :365-451)
- Modify: `src/gateway/session_store/sqlite_backend/mod.rs` + `src/gateway/session_store/file_backend/mod.rs` (create branch of `get_or_create`; column read/write following the `derived_title` precedent)
- Modify: `src/gateway/server/handler.rs` (both dispatch stations)
- Test: co-located `#[cfg(test)]` + existing session-store test modules

**Interfaces:**
- Consumes: Task 1 (`ScopeAttribution`, `with_scope`, `current_scope`).
- Produces: `SessionMetadata.owner_user_id: Option<String>`, `SessionMetadata.scope_id: Option<String>`, `SessionMetadata::stamp_attribution(&mut self)`, `SessionFilter.owner_visible_to: Option<String>`.

**Spec deviation (record in commit message):** spec §3 says "复活 `SessionIdentityMeta`"; recon proved that type is a *different* dormant concept (frozen guest-share identity for tool scoping, `Default` = Owner). Owner/scope land as first-class queryable fields instead. Intent (session carries owner+scope) is unchanged.

- [ ] **Step 1: Write failing tests:**

```rust
// session_store tests (run against BOTH backends via the existing backend-parametrized test helpers)
#[tokio::test]
async fn create_under_scope_stamps_owner_and_scope() {
    let store = /* fresh backend */;
    let key = SessionKey::parse("gui:chat:alice-1:s0").unwrap();
    let meta = crate::scope::with_scope(
        Some(crate::scope::ScopeAttribution::personal("u-alice")),
        store.get_or_create(&key),
    ).await.unwrap();
    assert_eq!(meta.owner_user_id.as_deref(), Some("u-alice"));
    assert_eq!(meta.scope_id.as_deref(), Some("personal:u-alice"));
    // Round-trip: read back from disk, both fields survive.
    let read = store.get_metadata(&key).await.unwrap().unwrap();
    assert_eq!(read.owner_user_id.as_deref(), Some("u-alice"));
}

#[tokio::test]
async fn create_without_scope_leaves_fields_none_and_serializes_without_them() {
    let meta = store.get_or_create(&key).await.unwrap();
    assert!(meta.owner_user_id.is_none() && meta.scope_id.is_none());
    // Migration invariant: a None-owner metadata.json contains neither key (skip_serializing_if)
    let json = serde_json::to_string(&meta).unwrap();
    assert!(!json.contains("owner_user_id") && !json.contains("scope_id"));
}

#[tokio::test]
async fn get_or_create_on_existing_session_never_restamps() {
    // A created it; B's later get_or_create must not steal ownership.
    let first = crate::scope::with_scope(Some(ScopeAttribution::personal("u-alice")), store.get_or_create(&key)).await.unwrap();
    let second = crate::scope::with_scope(Some(ScopeAttribution::personal("u-bob")), store.get_or_create(&key)).await.unwrap();
    assert_eq!(second.owner_user_id.as_deref(), Some("u-alice"), "stamping is create-only");
}

#[tokio::test]
async fn list_sessions_filters_by_owner_visible_to() {
    // alice creates 2, bob creates 1, one legacy (no scope) row
    // owner_visible_to: Some("u-alice") → alice's 2 only
    // owner_visible_to: Some("u-owner") → legacy row only (legacy = owner)
    // owner_visible_to: None → all 4
}
```

- [ ] **Step 2: Run to verify FAIL** — fields don't exist.
- [ ] **Step 3: Implement:**
  - `SessionMetadata` gains, directly below `parent_session_key` (both `#[serde(skip_serializing_if = "Option::is_none")]` — this IS the byte-identical migration invariant for legacy rows):
    ```rust
    /// Authenticated user who created this session (`users.user_id`). `None` on
    /// rows created before P1 or outside any dispatch scope — read as owned by
    /// `OWNER_USER_ID` (adoption-by-absence; single predicate:
    /// `gateway::visibility::effective_owner`). Stamped once at creation, immutable
    /// thereafter (spec §10: 会话 scope 不可变).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<String>,
    /// Rendered `scope::ScopeId` ("personal:u-…"); `None` = legacy = org-era owner session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    ```
  - `SessionMetadata::stamp_attribution(&mut self)`: no-op when `self.owner_user_id.is_some()`; else copy from `crate::scope::current_scope()` (owner → `owner_user_id`, `scope.render()` → `scope_id`; `None` context → leave `None`). Both backends call it on their CREATE branch only.
  - SQLite: `run_migrations` list += `("sessions", "owner_user_id", "TEXT")`, `("sessions", "scope_id", "TEXT")`; column read/write wired exactly like `derived_title` (follow that identifier through the backend and mirror every site).
  - `SessionFilter` += `pub owner_visible_to: Option<String>`; both backends' `list_sessions` apply `effective-owner == owner_visible_to` when `Some` (SQLite: `AND COALESCE(owner_user_id, 'u-owner') = ?` — the literal must be `crate::gateway::security::store::OWNER_USER_ID`, bound as a parameter, not inlined; file backend: filter in the metadata loop with the same expression through a shared helper).
  - **Dispatch seeding** (`server/handler.rs`, BOTH stations — `do_lane_dispatch` closure and the idempotency `Proceed` arm): wrap the existing three-layer `CALLER_ROLE`/`CALLER_USER`/`CALLER_IS_LOOPBACK` nest with a 4th layer:
    ```rust
    crate::scope::with_scope(
        caller_user.clone().map(|u| crate::scope::ScopeAttribution::personal(&u)),
        /* existing three-layer nest */,
    )
    ```
    Both stations or neither — P0's tests for station parity are the model; add `both_dispatch_stations_seed_scope` asserting a probe method observes `current_scope()` through each station.
- [ ] **Step 4: Run to verify PASS** — `cargo test -p alephcore --lib session_store` + `--lib server::handler`.
- [ ] **Step 5: Commit** — `gateway: stamp owner_user_id/scope_id on session creation and filter list_sessions by owner (spec deviation: first-class columns instead of reviving SessionIdentityMeta)`

---

### Task 3: Attribution plumbing through run boundaries

**Files:**
- Modify: `src/gateway/handlers/agent.rs` (Panel origin — the `build_run_request` site; same function the voice-registry write rides, CLAUDE.md §2.4)
- Modify: `src/gateway/inbound_router/executor.rs` (channel origin — the second voice-registry write site)
- Modify: `src/gateway/execution_engine/run_loop/mod.rs:173-200` (the existing task-local wrapping stack)
- Modify: `src/orchestrator/dispatch.rs:872-923` (the spawned-harness wrapping stack; also `FlowRequest` if it lacks a metadata carrier — see Step 3)
- Modify: `src/gateway/execution_engine/execute.rs::carry_policy_metadata` (:1012-1035)
- Modify: `src/agents/subagent_tool/spawn.rs::spawn_background` (:66-166)

**Interfaces:**
- Consumes: Task 1 (`stamp_metadata`, `scope_from_metadata`, `with_scope`, `current_scope`, both key consts).
- Produces: every agent run — Panel, channel, continuation, cron (Task 5), background subagent — executes with `scope::current_scope()` reflecting its owner; `RunRequest.metadata`/`FlowRequest` carry the two keys.

**Why this task exists (brief must carry this):** `CALLER_USER` lives only inside `process_request`'s task tree; every real run is `tokio::spawn`ed (task-locals do NOT cross spawn — `run_context.rs` doc). So attribution must ride request metadata and be re-seeded inside the spawn, exactly like `originator_user_id` → `with_originator` already does (`run_loop/mod.rs:158-163` — the precedent to sit beside, NOT to reuse: originator = "who sent this message" for the approval gate; owner = "who is accountable for this run's data").

- [ ] **Step 1: Write failing tests:**

```rust
// run_loop (or execution_engine tests): metadata → task-local
#[tokio::test]
async fn run_loop_seeds_scope_from_request_metadata() {
    // Build a minimal RunRequest whose metadata carries stamp_metadata(personal u-alice);
    // drive the wrapping layer (extract the wrap into a testable fn if needed) and assert
    // a probe future inside observes current_scope().owner_user_id == "u-alice".
}
#[tokio::test]
async fn run_loop_without_keys_runs_unscoped() { /* absent keys → current_scope() None */ }

// carry_policy_metadata: continuation inheritance
#[test]
fn continuation_inherits_owner_and_scope_keys() {
    let mut src = HashMap::new();
    crate::scope::stamp_metadata(&mut src, &ScopeAttribution::personal("u-alice"));
    src.insert("caller_role".into(), "member".into());
    let out = carry_policy_metadata(&src);
    assert_eq!(out.get(crate::scope::OWNER_META_KEY).map(String::as_str), Some("u-alice"));
    assert_eq!(out.get(crate::scope::SCOPE_META_KEY).map(String::as_str), Some("personal:u-alice"));
}

// spawn_background: the pre-existing task-local omission, fixed
#[tokio::test]
async fn background_subagent_reseeds_scope_and_project_root() {
    // Inside with_scope(personal u-alice) + with_project_root(Some(dir)):
    // spawn_background a trivial runtime stub; assert the spawned body observed BOTH
    // current_scope() == alice AND projects::current() == Some(dir).
    // (This also closes the pre-existing cross-project note-leak: spawn.rs:158 spawns
    // the harness with zero task-local re-establishment, unlike run_loop/dispatch.)
}
```

- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement:**
  - **Origin stamping (both origin sites):** where the request metadata map is built, insert:
    ```rust
    if let Some(user) = crate::gateway::caller_identity::current_caller_user() {
        crate::scope::stamp_metadata(&mut metadata, &crate::scope::ScopeAttribution::personal(&user));
    }
    ```
    Channel side (`inbound_router/executor.rs`): the user comes from `pairing_store.sender_user(channel, sender_id)` (P0's link, `src/gateway/pairing_store.rs`) instead of the task-local; `None` (unlinked peer) → stamp nothing (legacy owner semantics, spec §11-3). Anchor by the voice-registry write in the same function; if `FlowRequest` (Panel path) has no metadata carrier, add `pub scope_owner: Option<ScopeAttribution>` is NOT the shape — add the two string fields `owner_user_id: Option<String>` / `scope_id: Option<String>` next to the doc-commented `session_hint` (whose own doc already begs for this) and convert to metadata where the dispatch builds the spawn.
  - **Spawn-boundary seeding:** in `run_loop/mod.rs`, add one more layer to the existing nest (sibling of `with_originator`): `crate::scope::with_scope(crate::scope::scope_from_metadata(&request.metadata), …)`. In `orchestrator/dispatch.rs:892`, add the same layer inside the `tokio::spawn` next to the existing `with_agent_id`/`with_project_root` re-establishment (their doc comment explains why — extend it to name scope). Do NOT build a grand unified wrapper helper — the two stacks differ (fs_scope/media vs not); the only shared new code is `scope_from_metadata` (Task 1).
  - **`spawn_background` fix:** capture before the boundary, re-seed inside:
    ```rust
    let captured_scope = crate::scope::current_scope();
    let captured_root = crate::projects::current_project_root();
    let captured_agent = crate::agents::current_agent_id();
    tokio::spawn(async move {
        let _cancel_guard = CancelGuard::new(bridge_cancel.clone());
        crate::agents::with_agent_id(captured_agent,
            crate::projects::with_project_root(captured_root,
                crate::scope::with_scope(captured_scope, async move {
                    /* existing body: runtime.run(...).catch_unwind() … */
                }))).await;
    });
    ```
  - **`carry_policy_metadata`:** allowlist += both consts; extend its doc-comment contract sentence ("a background run is never MORE privileged…") to name owner/scope as part of what a continuation must inherit.
- [ ] **Step 4: Run to verify PASS** — scoped test runs for `execution_engine`, `orchestrator`, `agents::subagent_tool`.
- [ ] **Step 5: Commit** — `scope: carry owner/scope attribution through run request metadata and reseed at every spawn boundary (fixes background-subagent task-local omission)`

---

### Task 4: Memory layer — scope-aware write id, read union, floors split, curated per-scope + owner adoption

**Files:**
- Modify: `src/memory/project_scope.rs` (module doc rewrite + two new fns; existing fns untouched)
- Modify: `src/memory/assembler/gather.rs` (:49-50 floors; :106-112, :255-258 read union)
- Modify: `src/thinker/memory_context_provider/constructor.rs` (+ `curated.rs` path resolution) — per-scope curated instance + one-time owner adoption
- Modify: `src/memory/dreaming/mod.rs:1105` consumer + `project_scope.rs::list_scoped_agent_ids`
- Test: co-located + `src/memory/assembler/` tests

**Interfaces:**
- Consumes: Task 1 (`current_scope`), Task 3 (attribution present during runs).
- Produces:
```rust
// src/memory/project_scope.rs
pub fn session_write_id(base: &str, project_scoped: bool, project_root: Option<&Path>) -> String;
pub fn session_read_ids(base: &str, project_scoped: bool, project_root: Option<&Path>) -> Vec<String>;
```

**Semantics (from spec §5.2, locked here):**
- `session_write_id`: `current_scope()` = `Personal(u)` → `scoped_agent_id(base, u)` (personal wins over the legacy `proj-` directory feature — a personal session's writes are the user's even inside a project directory); `Org`/`None` → exactly today's `scoped_or_base(base, project_scoped, project_root)`.
- `session_read_ids`: `Personal(u)` → `[base, scoped_agent_id(base, u)]` (org first — same order contract as today, and Global Constraint 6 applies); `Org`/`None` → today's `read_scope_ids` behavior via `project_namespace`.
- Every existing `scoped_or_base` call site that runs *inside a session/run* migrates to `session_write_id` (six sites, recon-memory Q1 table: `note_manage.rs:410`, `post_turn_compress.rs:42`, `prepare_history.rs:55`, `runner_impl.rs:505` ← the reuse-read MUST mirror the write, `memory_search.rs:371`, `tool_registry_impl.rs:1448`). `gather.rs`'s two `read_scope_ids` sites migrate to `session_read_ids`.
- **Floors split (spec §5.2 "Floors 分床"):** `gather.rs:49-50` — `self.profile.load(&user_floor_id)` where `user_floor_id = session_write_id(...)` (user-profile floor follows personal scope), `self.feedback_floor.load(&input.agent_id)` stays base (org). Rewrite `project_scope.rs`'s "Floors stay global" module-doc invariant to the split form.
- **Curated per-scope:** the curated trio (`MEMORY.md`, `USER.md`, `OPEN_LOOPS.md`) resolves under the *composed* id's dir (`~/.aleph/agents/<base>__<u>/` for personal sessions) — instancing falls out of the existing `agent_id`-keyed stores for free.
- **Owner adoption (one-time, lazy):** at curated-store/profile load for composed id `<base>__u-owner`: if the scoped dir lacks the file AND the bare `agents/<base>/` file exists → `fs::rename` it in (per-file, idempotent, crash-safe: a partial adoption just re-runs for the missing file next load). Existing single-user content thereby BECOMES the owner's personal instance — the owner's envelope is byte-identical before/after. Members start fresh. The bare dir remains as the (empty) org instance; nothing injects an org curated layer in P1 (deliberate — noted in module doc; notes/raw org sharing is the recall union, curated was never a sharing mechanism).
- **Dream daemon:** `list_scoped_agent_ids` gains the `u-` family: scan `{base}__proj-*` AND `{base}__u-*` (one function, one prefix list `["proj-", "u-", "p-"]` — silent-skip of personal dirs is the landmine recon flagged).

- [ ] **Step 1: Write failing tests:**

```rust
// project_scope.rs
#[tokio::test]
async fn personal_scope_wins_the_write_id() {
    crate::scope::with_scope(Some(ScopeAttribution::personal("u-alice")), async {
        assert_eq!(session_write_id("main", true, Some(Path::new("/repo"))), "main__u-alice");
    }).await;
}
#[tokio::test]
async fn unscoped_write_id_is_byte_identical_to_scoped_or_base() {
    // No task-local: for (project_scoped, root) in [(false,None),(true,Some(..))]:
    // session_write_id == scoped_or_base — the single-user zero-change pin.
}
#[tokio::test]
async fn personal_read_union_is_org_then_personal() {
    crate::scope::with_scope(Some(ScopeAttribution::personal("u-alice")), async {
        assert_eq!(session_read_ids("main", false, None), vec!["main", "main__u-alice"]);
    }).await;
}
#[test]
fn dream_scan_lists_personal_and_project_dirs() { /* tmpdir with main__proj-x, main__u-a, main__junk → first two only */ }

// gather.rs — floors split (effect assertion: the loader receives the right id)
#[tokio::test]
async fn user_floor_is_scoped_feedback_floor_is_not() {
    // Fixture loaders that record the id they were asked for; run gather under personal(u-alice):
    // profile loader saw "main__u-alice", feedback loader saw "main".
}

// curated adoption
#[tokio::test]
async fn owner_adoption_moves_the_trio_once_and_is_idempotent() {
    // bare agents/main/{MEMORY.md,USER.md} with content; load curated store for "main__u-owner"
    // → files now under agents/main__u-owner/, bare gone, content byte-identical; second load: no-op.
}
#[tokio::test]
async fn member_scope_gets_a_fresh_instance_not_the_owners() {
    // After adoption, load for "main__u-alice" → empty store, and alice's remember-add
    // never touches main__u-owner/MEMORY.md.
}
```

- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement** per the semantics block. Two hard rules for the implementer: (a) before moving the curated path derivation, grep every consumer of `agent_memory_path`/`PROFILE_FILENAME`/`OPEN_LOOPS` and route ALL of them through the same composed-id resolution — a reader left on the bare path after adoption reads an empty file silently (the §2.5 failure mode: memory fails silently); (b) `CuratedMemoryLayer` stays `Stable`/session-frozen — the composed id is constant for a session's lifetime (session scope immutable, spec §10), so the existing `(agent_id, session_key)` snapshot key keeps §2.18 intact; assert nothing new varies per-turn. Also verify (recon TODO): `remember.rs` resolves its store through the same provider path as `note_manage::resolve_agent_id` — if it has a private resolution, unify on `session_write_id`.
- [ ] **Step 4: Run to verify PASS** — `cargo test -p alephcore --lib memory::` + `--lib thinker::memory_context_provider`.
- [ ] **Step 5: Commit** — `memory: session-scope-aware partition ids, floors split, per-scope curated instances with one-time owner adoption`

---

### Task 5: Background work ownership (goals / loops / crons) + deactivation freeze

**Files:**
- Modify: `src/goal/types.rs` (:60 `Goal`), `src/goal/store.rs` (`commit_field_update` :819-859, new `pause_all_owned_by` beside `pause_all_active` :617)
- Modify: `src/looping/types.rs` (:102 `LoopState`), `src/looping/mod.rs` (new `pause_all_owned_by`)
- Modify: `src/tasks/cron/config.rs` (:330 `CronJob`), `src/tasks/cron/executor.rs` (:180 fire path)
- Modify: `src/gateway/execution_engine/goal_wait.rs` (`spawn_wake_run`/`wake_identity` :335-386), the goal/loop creation tool paths (`src/builtin_tools/` goal & loop tools — stamp at creation)
- Modify: `src/gateway/handlers/users.rs` (deactivation branch)

**Interfaces:**
- Consumes: Tasks 1, 3 (`current_scope` is live inside runs; `stamp_metadata`; `carry_policy_metadata` already carries the keys).
- Produces: `Goal.owner_user_id/scope_id: Option<String>`, `LoopState.owner_user_id/scope_id: Option<String>`, `CronJob.owner_user_id/scope_id: Option<String>`; `GoalStore::pause_all_owned_by(&self, user_id: &str) -> Result<usize>`; `LoopRegistry::pause_all_owned_by(&self, user_id: &str) -> usize`.

**Shape rule (from recon):** clone the `Goal.workspace` discipline exactly — `#[serde(default)]` fields, stamped when the unit is created (from `scope::current_scope()` inside the creating run), preserved by `commit_field_update`'s merge-by-owner (`merged.owner_user_id = live.owner_user_id.clone()` beside the existing `merged.workspace` line), and re-emitted into continuation metadata on hook-less wakes (`spawn_wake_run`: `stamp_metadata(&mut policy_meta, attr)` from the goal's persisted fields before `spawn_continuation_run`). Cron: stamp at job creation (cron.* is admin-gated, so the owner is the operating admin), emit in `build_cron_metadata`. Legacy rows (`None`) emit nothing → run unscoped → legacy owner semantics (zero-change).

**Deactivation (spec §10):** in `users.rs`'s deactivation branch (after the device-revoke loop): `goal_store.pause_all_owned_by(&user)` + `loop_registry.pause_all_owned_by(&user)`, both full-scan+filter copies of `pause_all_active`'s shape, `warn!` the counts. Crons are skipped deliberately (admin-gated creation ⇒ a deactivated member owns none; module doc records this). Freeze is one-way; no auto-resume on reactivation (spec is silent — record in ledger).

- [ ] **Step 1: Write failing tests:** field round-trip through `try_claim_continuation` + `commit_field_update` (copy the existing workspace-preservation test's shape — a tool snapshot write must not roll back `owner_user_id`); `spawn_wake_run` emits both metadata keys from persisted fields (assert on the metadata map it builds); loop tick parity; cron `build_cron_metadata` includes keys when set; `pause_all_owned_by` pauses exactly alice's active goals/loops and returns the count; legacy `None` rows are untouched by `pause_all_owned_by("u-alice")` (legacy = owner, not alice).
- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement** per the shape rule. Goal/loop creation stamping goes where the unit is first persisted (the goal/loop tool's create/start path — locate via the store's `put`/insert callers; stamp from `current_scope()`, `None` context → leave `None`).
- [ ] **Step 4: Run to verify PASS** — `cargo test -p alephcore --lib goal::` + `--lib looping::` + `--lib tasks::cron` + `--lib gateway::handlers::users`.
- [ ] **Step 5: Commit** — `background: goals/loops/crons record owner and scope, wakes rehydrate attribution, deactivation freezes owned work`

---

### Task 6: Visibility predicates + sessions/chat RPC enforcement + pin-list registry

**Files:**
- Create: `src/gateway/visibility.rs`
- Create: `src/gateway/method_visibility.rs`
- Modify: `src/gateway/handlers/session/db_handlers/query.rs` (`handle_list_db` :40, `handle_history_db` :158, `handle_usage_db` :276, `handle_preview_db` :340), `modify.rs` (`handle_delete_db_inner`, `handle_reset_db`), plus the `chat.*` session-addressed handlers (`chat.send`'s session resolution, `chat.abort`)
- Modify: `src/gateway/method_admin.rs` (module doc: the "P1 doesn't exist" note now points at `method_visibility.rs`)

**Interfaces:**
- Consumes: Task 2 (`owner_user_id` fields, `SessionFilter.owner_visible_to`).
- Produces:
```rust
// src/gateway/visibility.rs — ALL user-visibility decisions live here (spec §5.4 唯一强制点)
pub fn visible_owner_filter() -> Option<String>;           // None = unrestricted (no CALLER_USER task-local: internal/in-process callers)
pub fn effective_owner(meta: &SessionMetadata) -> &str;    // owner_user_id.as_deref().unwrap_or(OWNER_USER_ID)
pub fn session_visible(meta: &SessionMetadata) -> bool;    // visible_owner_filter() maps None→true, Some(u)→ u == effective_owner(meta)
pub fn partition_visible(partition_id: &str) -> bool;      // Task 7 consumes
pub fn not_found_response(id: RequestId) -> JsonRpcResponse; // RESOURCE_NOT_FOUND, "session not found" — Global Constraint 4
```

**Enforcement pattern (identical at every addressed-key site):**
```rust
let meta = match manager.get_metadata(&session_key).await {
    Ok(Some(m)) => m,
    Ok(None) => return visibility::not_found_response(request.id),
    Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
};
if !visibility::session_visible(&meta) {
    return visibility::not_found_response(request.id);           // same error as missing (GC 4)
}
```
List sites instead set `filter.owner_visible_to = visibility::visible_owner_filter()`.

**`method_visibility.rs`** is the durable registry + regression net (NOT a dispatch gate — enforcement stays in handlers because filtering needs per-method data access; the doc must say this): a const table `SCOPED_METHODS: &[(&str, Treatment)]` where `enum Treatment { ListFiltered, KeyChecked, PartitionChecked, OrgShared }`, mechanically derived the same way P0 derived the 74-family admin table (sweep all four registration patterns: `register_handler!` files under `src/bin/aleph-server/commands/start/builder/handlers/`, `handlers/mod.rs` direct `registry.register`, `agent_init` inline). `OrgShared` entries need a one-line reason (`teams.*` — org-level AI-team infrastructure, project scoping arrives in P2; `fs.*` — bounded by `allowed_roots`; config reads — admin-gated upstream). Pin tests copy `method_admin.rs`'s suite shape: `every_session_addressed_method_is_registered` (curated pin list, one per family), `org_shared_entries_all_carry_reasons`, and a cross-check test asserting no method appears in both this table's `OrgShared` and `method_admin`'s `ADMIN_PREFIXES`-uncovered daily surface without one of the two tables claiming it.

- [ ] **Step 1: Write failing tests:**

```rust
// visibility.rs unit
#[tokio::test]
async fn session_visible_matrix() {
    // (caller task-local, meta.owner) → expected:
    // (None-unset, any) → true            // unrestricted internal caller
    // (u-alice, Some(u-alice)) → true
    // (u-alice, Some(u-bob)) → false
    // (u-alice, None) → false             // legacy rows belong to owner
    // (u-owner, None) → true
}

// handler-level isolation guards (spec §9-1: A creates, B sees empty) — one per method:
#[tokio::test]
async fn sessions_list_is_scoped_to_the_caller() {
    // alice creates 2 sessions (scope task-local), bob creates 1, 1 legacy row.
    // Dispatch sessions.list as bob (CALLER_USER=u-bob scoped around the handler call)
    // → exactly bob's 1. As owner → bob's absent, legacy present. Unrestricted → all.
}
#[tokio::test]
async fn sessions_history_denies_cross_user_as_not_found() {
    // bob calls sessions.history with alice's key → RESOURCE_NOT_FOUND, and the error body
    // is byte-identical to a genuinely nonexistent key (no existence oracle).
}
// … same-shape tests pinned for: sessions.preview / session.usage / sessions.delete /
// sessions.reset / chat.abort / chat.send-to-foreign-session. sessions.delete's test must
// also assert the foreign session STILL EXISTS afterwards (deny means no side effect).
```

- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement** `visibility.rs` (module doc: the resolution is `CALLER_USER` + pure derivation — "resolve once" per spec §5.4 is satisfied by the P0 task-local, no second resolution exists; any handler writing its own owner comparison instead of calling these predicates is the bypass this module exists to prevent), then apply the enforcement pattern at every Step-1-pinned handler, then write `method_visibility.rs` with the mechanical sweep (script-derived listing in the module doc, P0 methodology).
- [ ] **Step 4: Run to verify PASS** — `cargo test -p alephcore --lib gateway::visibility` + `--lib gateway::method_visibility` + `--lib gateway::handlers::session`.
- [ ] **Step 5: Commit** — `gateway: per-user session visibility chokepoint — list filtering, addressed-key checks, scoped-method registry`

---

### Task 7: Memory / artifacts / clarification / subagent / graph RPC enforcement

**Files:**
- Modify: `src/gateway/handlers/memory.rs` (`handle_search` :90, `handle_list_facts` :272, `handle_stats` :364), `src/gateway/handlers/graph/query.rs` (:13-133), `src/gateway/handlers/artifacts.rs` (`handle_list` :102, `handle_read_text`, `handle_export_html`), `src/gateway/handlers/clarification.rs` (`handle_pending`, `handle_resolve`), `src/gateway/handlers/subagent.rs` (`handle_tree`)
- Modify: `src/gateway/visibility.rs` (add `partition_visible` + `resolve_readable_partition`)

**Interfaces:**
- Consumes: Task 6 predicates; Task 4's partition grammar.
- Produces: `visibility::partition_visible(partition_id: &str) -> bool` — split once on `"__"`: no suffix → `true` (org layer is shared by design, spec §11-1c); suffix starting `proj-` → `true` (legacy directory feature, org-tier); otherwise → suffix == caller user, or unrestricted caller. Unknown suffix families fail closed for members.

**Per-surface semantics (locked):**
- `memory.search` / `memory.listFacts` / `graph.query`: run the caller-supplied (or defaulted) `agent_id` through `partition_visible`; invisible → same empty-result shape as an unknown agent (no oracle). `memory.stats`: omitted `agent_id` = whole-store rollup remains **unrestricted-caller only**; a member's omitted `agent_id` is treated as `DEFAULT_AGENT_ID` (org partition) instead — its doc comment updated to say so.
- `artifacts.*`: storage is already session-keyed and cross-session-proof (recon Q5b); add the Task-6 addressed-key pattern on the resolved session (defense in depth — `sessions.list` was the key-harvesting front door and is now filtered, but these handlers must not depend on that).
- `clarification.pending`: filter each pending item by its session's visibility (list is small; per-item `get_metadata` + `session_visible`); `clarification.resolve`: addressed-key pattern.
- `subagent.tree`: `root_session` given → addressed-key pattern; omitted → unrestricted callers keep whole-process view; a scoped caller gets the tree filtered to roots whose session is visible (reuse the flat_nodes root_session plumbing; never an error — an empty tree is a valid answer).
- `method_admin.rs` module doc: replace the "needs the per-user visibility work (P1-adjacent)" follow-up note with a pointer to `method_visibility.rs` + these handlers; **verify while there** whether the ~L186 event-scope justification comment ("event_scope already restricts approval.* to admin") matches post-Task-8 reality and correct it in Task 8 if not.

- [ ] **Step 1: Write failing tests:** `partition_visible` matrix (org/proj-/own-u/foreign-u/unknown suffix × member/unrestricted); per-surface A建B空 guards: alice-partition raw memories invisible to bob via `memory.search(agent_id="main__u-alice")` (empty, same shape as unknown agent); member `memory.stats` without `agent_id` returns org-partition stats, unrestricted returns whole-store; bob cannot `artifacts.read_text` alice's session artifact (NOT_FOUND, storage row intact); `clarification.pending` as bob omits alice's question; `subagent.tree` omitted-root as member lists only own-session trees.
- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement** per the semantics block; register every touched method in `method_visibility.rs`'s table (`PartitionChecked` / `KeyChecked` / `ListFiltered`).
- [ ] **Step 4: Run to verify PASS** — scoped runs for `gateway::handlers::{memory,artifacts,clarification,subagent}` + `graph`.
- [ ] **Step 5: Commit** — `gateway: partition and session visibility on memory/artifacts/clarification/subagent/graph surfaces`

---

### Task 8: Event-stream visibility (owner filter + run→session cache) + P0 wildcard verification

**Files:**
- Create: `src/gateway/event_visibility.rs`
- Modify: `src/gateway/server/handler.rs` (:1462-1500 — the `should_forward` chain; RunAccepted cache seeding in the same loop)
- Modify: `src/gateway/event_scope.rs` (tests only, unless Step 1 finds red), `src/gateway/method_admin.rs` (~L186 comment, per Task 7 note)

**Interfaces:**
- Consumes: Task 2 (`effective_owner` via session metadata), `ConnectionState.caller_user` (P0, `server/mod.rs:84`).
- Produces:
```rust
// src/gateway/event_visibility.rs
pub struct EventVisibilityIndex { /* bounded run_id→session_key + session_key→Option<owner> maps,
                                     tokio::sync::RwLock, capacity caps + eviction on RunComplete/RunError
                                     + insertion-order overflow eviction (StreamRegistry hygiene) */ }
impl EventVisibilityIndex {
    pub async fn note_frame(&self, topic: &str, data: Option<&serde_json::Value>);   // seeds from RunAccepted {run_id, session_key}; evicts on RunComplete/RunError
    pub async fn event_admits(&self, topic: &str, data: Option<&serde_json::Value>,
                              caller_user: Option<&str>,
                              store: &Arc<dyn SessionStore>) -> bool;
}
pub fn session_identity_of(topic: &str, data: Option<&serde_json::Value>) -> SessionIdentity; // enum: BySessionKey(String) | ByRunId(String) | Global
```

**Semantics (locked):**
- `event_admits`: `Global` → `true` (unattributable topics — `tools.changed`, lifecycle, config — are org-level, and the admin-only ones among them are already gated by `EventScopeGuard`); `BySessionKey`/resolved `ByRunId` → owner-of-session == effective caller (`caller_user` None on a connection = walled → the wall already refused it; treat as deny here for defense in depth); unresolvable `ByRunId` (cache miss — event raced ahead of `RunAccepted` or predates the filter) → **deny** (fail closed; a dropped early frame is recoverable via `run_complete` reconciliation, a leaked frame is not).
- `session_identity_of` is an exhaustive `match` over `GatewayEventFrame`'s topic/method names with a **compile-anchored pin test**: a test that constructs one of every `GatewayEventFrame` variant (non_exhaustive-proof: match on the real enum in the test so a new variant breaks compilation here) and asserts each classifies to the intended `SessionIdentity` — a future variant that carries session content but classifies `Global` must be caught by this test's review, and the test's doc comment says exactly that.
- The 4th filter term in `handler.rs`: after the existing three (`scope_allowed && audience_allows && should_receive`), append `&& ctx.event_visibility.event_admits(topic, event_data, state_caller_user.as_deref(), &ctx.session_store).await` — reading `caller_user` in the same `ConnectionState` lock acquisition as `permissions`/`channel_kind` (extend the existing tuple, don't take the lock twice). `note_frame` is called unconditionally before the filter so every connection's loop keeps the shared index warm (first writer wins; the index is process-shared via `ctx`).
- Session-owner lookups go through the bounded `session_key→owner` cache (fill on miss from the store; the per-event cost is one RwLock read after warmup).
- **Cluster/node caveat** (ledger note, not code): node connections resolve to owner, so they will not receive member-session events; acceptable in P1 — nodes execute owner-tier infrastructure.

**Step 1 also settles the P0 parked item:** recon found `handler.rs:1169-1173` already assigns `state.permissions = scope_for_role(resolved_role)` with member → `[]` (NOT `["*"]`), which would mean the parked "member event wildcard" gap is already closed on main. Pin it rather than trust it.

- [ ] **Step 1: Write the pin + failing tests:**

```rust
// event_scope.rs / handler.rs tests — P0 parked-item verification (expected GREEN; if RED, fix here)
#[test]
fn member_role_has_no_event_wildcard() {
    assert!(crate::gateway::event_scope::scope_for_role("member").is_empty());
    let guard = EventScopeGuard::default_rules();
    for t in ["approval.request", "pairing.started", "config.changed", "guest.x", "surface.approval"] {
        assert!(!guard.can_receive(t, &scope_for_role("member")), "{t} must not reach members");
    }
}
// + a connect-path test asserting a member connection's ConnectionState.permissions is empty
// (and restamp_live_connections parity — same scope_for_role source).

// event_visibility.rs — failing until implemented
#[tokio::test]
async fn run_events_are_owner_scoped_via_the_run_accepted_seed() {
    // RunAccepted{run_id r1, session_key K(owner alice)} → note_frame;
    // AgentTrace{run_id r1} → event_admits: alice true, bob false, owner false.
}
#[tokio::test]
async fn unseeded_run_id_denies() { /* fail closed on cache miss */ }
#[tokio::test]
async fn session_key_bearing_topic_events_are_owner_scoped() { /* sessions.changed{session_key} */ }
#[tokio::test]
async fn global_topics_pass_for_everyone() { /* tools.changed → both alice and bob admit */ }
#[test]
fn every_frame_variant_is_classified() { /* the exhaustive-match pin described above */ }
#[tokio::test]
async fn index_is_bounded_and_evicts_on_run_completion() { /* capacity + RunComplete eviction */ }
```

- [ ] **Step 2: Run to verify FAIL** (the `event_visibility` ones; the P0 pins are expected green — if any is red, the fix is `scope_for_role`/connect assignment and it happens in this task).
- [ ] **Step 3: Implement** `event_visibility.rs` + the handler.rs wiring per semantics; while in `method_admin.rs`, re-read the ~L186 justification comment against this now-true reality and rewrite it to name both halves (role gate on admin topics = `EventScopeGuard` + owner gate on session topics = `event_visibility`).
- [ ] **Step 4: Run to verify PASS** — `cargo test -p alephcore --lib gateway::event` + `--lib server::handler`.
- [ ] **Step 5: Commit** — `gateway: owner-scoped event delivery via run→session index; pin member event scope (P0 follow-up closed)`

---

### Task 9: Hardening — member default exec tier, role-fed tool gate on tools.invoke, restamp panel filter

**Files:**
- Modify: the two `TurnEnvelope` build sites (`src/gateway/handlers/agent.rs::build_run_request` and `src/gateway/inbound_router/executor.rs` — same functions Task 3 touched; exec-tier resolution happens where the envelope's `exec_tier` is chosen)
- Modify: `src/gateway/handlers/tools.rs` (or wherever `tools.invoke` dispatches — locate via `method_visibility` sweep) + `src/gateway/method_admin.rs` (`MEMBER_CARVE_OUTS`)
- Modify: `src/gateway/handlers/users.rs::restamp_live_connections`

**Interfaces:** consumes Task 3's metadata plumbing (`caller_role` already rides `carry_policy_metadata`; the tool gate `role_is_operator` already reads run-metadata role — P0).

**Semantics (locked):**
1. **Member default exec tier = `Ask`** (spec §11 hardening, user-approved): at envelope build, when the resolved caller role is `member` AND the session/pill carries no explicit tier, the default becomes `ExecTier::Ask` instead of the global default. An explicit per-session choice (composer pill / `session_set_mode`-adjacent tier setting) still wins — this is a default, not a clamp (the clamp remains `[sandbox.command_policy]`). Operator paths byte-identical.
2. **`tools.invoke` member carve-out, done right:** P0 blanket-gated `tools.` because member `tools.invoke` reached the tool layer with `CALLER_ROLE=None` (= trusted) — the C2 escalation. The fix is identity propagation, not a hole: the `tools.invoke` handler stamps `caller_role` + scope attribution (Task 1 keys) into the invocation's run/turn metadata so the existing tool gate (`role_is_operator`) and Task 3 seeding see the member; THEN `MEMBER_CARVE_OUTS += "tools.invoke"`. The C2 regression test is mandatory (Step 1). `tools.list`/`tools.schema` stay admin-gated (Panel `team_from_template` needs only `invoke`; widen later on demand).
3. **`restamp_live_connections` panel filter** (P0 parked): the device→connection restamp loop must consider only `device_type = 'panel'` rows (the devices table is the shared panel/node namespace — `PANEL_DEVICE_TYPE` is the sole predicate, gateway/CLAUDE.md mine 3); add the filter at its store lookup.
4. **Dropped deliberately (record in ledger):** a new `[security] member_tool_permissions` config knob — zero consumers at default (R10 YAGNI); the tool gate + exec tier + command policy already fire for members once identity propagates.

- [ ] **Step 1: Write failing tests:** member envelope with no explicit tier resolves `Ask` (operator resolves today's default, byte-identical); member envelope with explicit pill keeps the pill; **C2 regression**: member `tools.invoke` of an operator-tier tool (e.g. the cron family) → denied by the tool gate (not by method_admin — the carve-out is open, the gate must catch it); member `tools.invoke` of an ordinary read tool succeeds; `method_admin::method_requires_admin("tools.invoke") == false` while `("tools.list") == true`; `restamp_live_connections` skips a node-namespace device row with a matching id.
- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement** per semantics. The `tools.invoke` change must route the invocation through the same `ScopedToolService` chokepoint as every other execution surface (CLAUDE.md exec-tier rule: any surface not through `src/tools/scoped/` is a bypass) — if it already does, the change is only the metadata stamping; verify, don't assume.
- [ ] **Step 4: Run to verify PASS** — scoped runs for the touched modules.
- [ ] **Step 5: Commit** — `gateway: member hardening — default Ask tier, identity-propagated tools.invoke carve-out, panel-scoped restamp`

---

### Task 10: Isolation sweep, migration invariant, docs

**Files:**
- Create: `src/gateway/tests/isolation_guard.rs`-style integration module (or extend the existing gateway test module the P0 acceptance tests live in)
- Modify: `docs/reference/SECURITY.md` (`### 多用户角色层（P0）` section grows a P1 subsection), `src/gateway/CLAUDE.md` (landmines), `src/gateway/method_admin.rs` + `src/memory/project_scope.rs` doc drift check
- Test: the full 4-command verification set

- [ ] **Step 1: End-to-end isolation guards** (spec §9-1, through real dispatch with task-locals scoped, not handler-internals):

```rust
// The acceptance test the branch is named for:
#[tokio::test]
async fn two_users_cannot_see_each_other_end_to_end() {
    // alice: create session, run a turn that captures a note + an artifact, start a loop.
    // bob: sessions.list → empty of alice; sessions.history(alice) → NOT_FOUND;
    //      memory.search(main__u-alice) → empty; artifacts.list(alice session) → NOT_FOUND;
    //      subagent.tree omitted → no alice roots; simulated alice run event → not admitted.
    // owner: sees legacy fixtures, not alice's.
}

#[tokio::test]
async fn single_user_fixture_is_byte_identical_after_upgrade() {
    // Spec §9-2. Build a pre-P1 fixture: file-backend session dir whose metadata.json
    // lacks the new fields + a bare agents/main/MEMORY.md + base-partition notes.
    // Open through the new code as the owner (loopback attribution):
    // - sessions.list rows: serialized metadata byte-identical (skip_serializing_if holds);
    // - curated envelope content identical (post-adoption move);
    // - notes recall results identical (org partition still first in the union).
}
```

- [ ] **Step 2: Run the full verification set** (foreground, no pipes, `timeout 600000` each):
  1. `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib`
  2. `cargo check -p aleph-panel`
  3. `cargo check -p aleph-desktop-windows`
  4. `cargo clippy --all-targets` (baseline: 24 pre-existing warnings; zero new)
- [ ] **Step 3: Docs:**
  - `SECURITY.md`: P1 subsection under `#auth-ux` — visibility chokepoint (`visibility.rs` predicates + `method_visibility.rs` registry), scope vocabulary, event owner filter, deactivation freeze, the §11 honesty boundary restated (privacy-grade, not malicious-member-grade), NOT_FOUND-over-forbidden rule.
  - `src/gateway/CLAUDE.md` new landmines: (a) "any new RPC returning scoped data registers in `method_visibility.rs` and calls `visibility::` predicates — an inline owner comparison is the bypass"; (b) "any new `GatewayEventFrame` variant must be classified in `event_visibility::session_identity_of` (the exhaustive-match pin will force you)"; (c) "any new `tokio::spawn` of run work must re-seed `scope::current_scope()` (and project root) — `spawn_background` was the cautionary tale".
  - `project_scope.rs` + `method_admin.rs`: confirm module docs match shipped reality (Floors split; P1-exists-now).
- [ ] **Step 4: Commit** — `p1: end-to-end isolation guards, migration invariant fixture, security docs`

---

## Self-Review Notes (writing-plans checklist, done)

- **Spec coverage:** §5.1 vocabulary → T1; §5.2 memory → T4; §5.3 session归属 → T2; §5.4 咽喉 → T6/T7 (+ resolution note in `visibility.rs` doc); §5.5 后台归属 → T3/T5; §9 three test classes → T6-T8 guards / T10 invariant / T6 registry; §10 停用冻结 → T5, scope-immutable → T2 stamp-once test; §11 hardening → T9. P0 parked trio → T8 (wildcard verify), T9 (restamp, tools.invoke). Push routing to members is P3 (spec §8), deliberately absent.
- **Known deviations from spec text (surface to user):** (1) `SessionIdentityMeta` not revived — first-class columns (T2, recon-proved it's a different concept); (2) member `tool_permissions` config knob dropped as YAGNI — hardening delivered via identity propagation + Ask default (T9); (3) org curated layer is not injected in P1 (T4 — curated was never a sharing mechanism; notes/raw org sharing is the recall union).
- **Type consistency:** `ScopeAttribution`/`ScopeId` names identical across T1-T9; `session_write_id`/`session_read_ids` (T4) consumed nowhere before T4; `effective_owner` single-sourced (T6) and reused by T8.
- **No placeholders:** every step carries code or an exact anchor + mechanical rule; the two "locate via sweep" instructions (T9 tools.invoke handler, T5 goal/loop creation sites) name the search seam and the invariant the implementer must satisfy.
