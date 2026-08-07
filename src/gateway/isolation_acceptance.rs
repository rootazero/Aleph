//! Data-isolation acceptance tests (spec §9, §13).
//!
//! The two P1 acceptance tests this module is named for:
//! - [`two_users_cannot_see_each_other_end_to_end`] (spec §9-1)
//! - [`single_user_fixture_is_byte_identical_after_upgrade`] (spec §9-2)
//!
//! …and P2's, which is the same isolation read from the other side — what a
//! shared room DOES share, and what it still must not:
//! - [`two_members_share_one_room_memory_and_nobody_elses`] (spec §8/§13)
//! - [`the_model_can_tell_two_room_members_apart`] (spec §6.2)
//! - [`two_members_of_a_room_work_in_the_same_bound_folder`] (spec §8, Task 7)
//!
//! Every RPC surface below is exercised through its REAL handler function —
//! the exact same one `HandlerRegistry` wires up in production
//! (`src/bin/aleph-server/commands/start/builder/handlers/*.rs`) — wrapped in
//! [`as_caller`], which reproduces the task-local nesting
//! `server::handler::dispatch_with_caller_context` applies around every real
//! dispatch (the P1 `scope::with_scope` attribution AND the P0 `CALLER_USER`
//! identity, both seeded from the same caller id). This mirrors the pattern
//! every Task 6-9 visibility test in this branch already established (e.g.
//! `handlers::session::db_handlers::create::visibility_guards`,
//! `handlers::subagent::tests`) rather than calling `gateway::visibility`'s
//! predicates directly: `process_request` itself is `pub(super)` to
//! `gateway::server`, and the real `HandlerRegistry` wiring lives in the bin
//! crate — both are unreachable from `alephcore`'s test surface, so no test
//! in this crate can go through `process_request`/`HandlerRegistry` end to
//! end. Calling the handler functions directly, under the same task-locals a
//! real dispatch would apply, is as close to "real dispatch" as this crate's
//! boundary allows — see `src/gateway/CLAUDE.md`.

use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use crate::agents::background_tracker::{BackgroundAgentTracker, CompletedOutcome, SpawnMeta};
use crate::artifacts::{ArtifactOrigin, ArtifactStore};
use crate::gateway::caller_identity::CALLER_USER;
use crate::gateway::event_visibility::EventVisibilityIndex;
use crate::gateway::handlers::artifacts::handle_list as artifacts_handle_list;
use crate::gateway::handlers::memory::handle_search;
use crate::gateway::handlers::session::{handle_history_db, handle_list_db};
use crate::gateway::handlers::subagent::handle_tree;
use crate::gateway::protocol::{JsonRpcRequest, RESOURCE_NOT_FOUND};
use crate::gateway::router::SessionKey;
use crate::gateway::security::store::OWNER_USER_ID;
use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
use crate::gateway::session_store::SessionStore;
use crate::looping::types::{Cadence, LoopState};
use crate::looping::LoopRegistry;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use crate::memory::store::SqliteMemoryBackend;
use crate::scope::{with_scope, ScopeAttribution};

/// Build a minimal JSON-RPC request for a handler call in these tests.
fn req(method: &str, params: Option<serde_json::Value>) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: method.to_string(),
        params,
        id: Some(json!(1)),
    }
}

/// A collision-proof identifier for this test run, so the process-global
/// `BackgroundAgentTracker` singleton and shared session-key namespaces never
/// pick up state left over from another test in this module or another.
fn unique(label: &str) -> String {
    format!("{label}-{}", uuid::Uuid::new_v4().simple())
}

/// Reproduce, exactly, the task-local nesting a real dispatch applies around
/// every handler call: `scope::with_scope`'s P1 ownership attribution
/// wrapping `CALLER_USER`'s P0 identity task-local, both seeded from the same
/// caller id — see `server::handler::dispatch_with_caller_context`, whose
/// doc comment this mirrors. Use this for every simulated "caller" boundary
/// below (a write attributed to a user, or a read gated by one).
async fn as_caller<F, T>(user: &str, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    with_scope(
        Some(ScopeAttribution::personal(user)),
        CALLER_USER.scope(Some(user.to_string()), fut),
    )
    .await
}

/// The `main__u-<id>` grammar `memory.search`'s `agent_id` param addresses —
/// `session_write_id`'s personal-scope composition (`project_scope.rs`),
/// duplicated here as a literal so the test's expectations don't silently
/// track a refactor of that function's internals.
fn personal_partition(base: &str, user_id: &str) -> String {
    format!("{base}__{user_id}")
}

/// Register a background subagent under `root_session` on the
/// process-global tracker, unregistering it on drop — mirrors
/// `handlers::subagent::tests::RegisteredAgent` exactly, duplicated locally
/// so this module doesn't reach into another module's `#[cfg(test)]` items.
struct RegisteredAgent {
    request_id: String,
}

impl Drop for RegisteredAgent {
    fn drop(&mut self) {
        BackgroundAgentTracker::global()
            .mark_completed(&self.request_id, CompletedOutcome::ok_text("test cleanup"));
    }
}

fn register_subagent(request_id: &str, root_session: &str) -> RegisteredAgent {
    BackgroundAgentTracker::global().register_with_meta(
        request_id.to_string(),
        CancellationToken::new(),
        "a delegated task".to_string(),
        SpawnMeta {
            parent_id: None,
            depth: 1,
            root_session: root_session.to_string(),
            model: None,
        },
    );
    RegisteredAgent {
        request_id: request_id.to_string(),
    }
}

/// The acceptance test the branch is named for (spec §9-1).
///
/// Alice creates a session and, under her own attribution, captures a raw
/// memory row, publishes an artifact, starts a loop, and spawns a background
/// subagent — all through the real production write paths each already has
/// its own unit coverage for (Tasks 2-5). Bob then reads every one of those
/// surfaces through the SAME handler functions production dispatch calls,
/// and must see none of it: an addressed-key read gets the identical
/// NOT_FOUND a missing key would (no existence oracle), and an unaddressed
/// list/search read comes back empty rather than erroring. A legacy
/// (pre-P1, unscoped) session proves the owner isn't just "sees everything"
/// — they see the legacy fixture (owner-by-absence) but NOT alice's, exactly
/// like any other member.
#[tokio::test]
async fn two_users_cannot_see_each_other_end_to_end() {
    let temp = TempDir::new().unwrap();
    let sessions: Arc<dyn SessionStore> = Arc::new(
        SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("sessions.db"),
            ..Default::default()
        })
        .unwrap(),
    );
    let memory: Arc<SqliteMemoryBackend> =
        Arc::new(SqliteMemoryBackend::new(&temp.path().join("memory.db")).unwrap());
    let artifacts = Arc::new(ArtifactStore::new(temp.path().join("artifacts")));
    let events = EventVisibilityIndex::new();
    let loops = LoopRegistry::default();

    // ══════════════════════ Alice: creates and captures ══════════════════════

    let alice_agent = unique("alice-agent");
    let alice_key = SessionKey::main(alice_agent);
    let alice_key_str = alice_key.to_key_string();
    let alice_partition = personal_partition("main", "u-alice");
    let alice_run_id = unique("run-alice");
    let alice_request_id = unique("alice-req");

    as_caller("u-alice", async {
        sessions.get_or_create(&alice_key).await.unwrap();

        memory
            .insert_raw_memory(
                &RawMemory::new(
                    "alice's captured note".to_string(),
                    RawMemorySource::Reflection,
                )
                .with_agent(alice_partition.clone())
                .with_session(alice_key_str.clone()),
            )
            .await
            .unwrap();

        artifacts
            .put(
                &alice_key_str,
                None,
                ArtifactOrigin::Deliverable,
                "report.md",
                "text/markdown",
                b"alice's report",
            )
            .await
            .unwrap();

        // A loop, owner-stamped from the ambient scope exactly like
        // `builtin_tools::loop_manage`'s real call site.
        let scope_now = crate::scope::current_scope();
        let loop_state = LoopState::new(
            &alice_key_str,
            "keep going",
            Cadence::Fixed {
                interval_ms: 300_000,
            },
            0,
        )
        .with_owner_scope(scope_now.as_ref());
        assert_eq!(loop_state.owner_user_id.as_deref(), Some("u-alice"));
        loops.put(loop_state);

        events
            .note_frame(
                "stream.run_accepted",
                Some(&json!({ "run_id": alice_run_id, "session_key": alice_key_str })),
            )
            .await;
    })
    .await;

    let _subagent = register_subagent(&alice_request_id, &alice_key_str);

    // A legacy (pre-P1, unscoped) session — no `as_caller` wrapper, matching
    // an internal/cron creator: `owner_user_id` stays `None`, which reads as
    // owned by `OWNER_USER_ID` (owner-by-absence, `visibility::effective_owner`).
    let legacy_key = SessionKey::main(unique("legacy-agent"));
    sessions.get_or_create(&legacy_key).await.unwrap();

    let alice_trace_frame = json!({
        "run_id": alice_run_id,
        "seq": 1,
        "event": { "kind": "turn_started", "iteration": 1 },
    });

    // ══════════════════════ Bob: sees none of it ══════════════════════

    let bob_list = as_caller(
        "u-bob",
        handle_list_db(req("sessions.list", None), sessions.clone()),
    )
    .await;
    let bob_sessions = bob_list.result.expect("success, not an error")["sessions"]
        .as_array()
        .expect("sessions array")
        .clone();
    assert!(
        !bob_sessions
            .iter()
            .any(|s| s["key"] == alice_key_str.as_str()),
        "bob's sessions.list must not include alice's session: {bob_sessions:?}"
    );

    let bob_history = as_caller(
        "u-bob",
        handle_history_db(
            req(
                "sessions.history",
                Some(json!({ "session_key": alice_key_str })),
            ),
            sessions.clone(),
        ),
    )
    .await;
    assert_eq!(
        bob_history.error.as_ref().map(|e| e.code),
        Some(RESOURCE_NOT_FOUND),
        "bob addressing alice's session by key must get NOT_FOUND: {bob_history:?}"
    );

    let bob_memory = as_caller(
        "u-bob",
        handle_search(
            req(
                "memory.search",
                Some(json!({ "agent_id": alice_partition })),
            ),
            memory.clone(),
        ),
    )
    .await;
    let bob_memories = bob_memory.result.expect("success, not an error")["memories"]
        .as_array()
        .expect("memories array")
        .clone();
    assert!(
        bob_memories.is_empty(),
        "bob must not see alice's memory partition: {bob_memories:?}"
    );

    let bob_artifacts = as_caller(
        "u-bob",
        artifacts_handle_list(
            req(
                "artifacts.list",
                Some(json!({ "session_key": alice_key_str })),
            ),
            artifacts.clone(),
            sessions.clone(),
        ),
    )
    .await;
    assert_eq!(
        bob_artifacts.error.as_ref().map(|e| e.code),
        Some(RESOURCE_NOT_FOUND),
        "bob addressing alice's artifacts by session_key must get NOT_FOUND: {bob_artifacts:?}"
    );

    let bob_tree = as_caller(
        "u-bob",
        handle_tree(req("subagent.tree", None), sessions.clone()),
    )
    .await;
    let bob_nodes = bob_tree.result.expect("success, not an error")["nodes"]
        .as_array()
        .expect("nodes array")
        .clone();
    assert!(
        !bob_nodes
            .iter()
            .any(|n| n["root_session"] == alice_key_str.as_str()),
        "bob's omitted-root subagent.tree must not include alice's root: {bob_nodes:?}"
    );

    assert!(
        !events
            .event_admits(
                "stream.agent_trace",
                Some(&alice_trace_frame),
                Some("u-bob"),
                &sessions,
            )
            .await,
        "bob must not be admitted to alice's simulated run event"
    );

    // ══════════════════ Owner: sees the legacy fixture, not alice's ══════════════════

    let owner_list = as_caller(
        OWNER_USER_ID,
        handle_list_db(req("sessions.list", None), sessions.clone()),
    )
    .await;
    let owner_sessions = owner_list.result.expect("success")["sessions"]
        .as_array()
        .expect("sessions array")
        .clone();
    assert!(
        owner_sessions
            .iter()
            .any(|s| s["key"] == legacy_key.to_key_string().as_str()),
        "owner must see the legacy (unstamped) session: {owner_sessions:?}"
    );
    assert!(
        !owner_sessions
            .iter()
            .any(|s| s["key"] == alice_key_str.as_str()),
        "owner is not exempt from alice's ownership boundary: {owner_sessions:?}"
    );

    assert!(
        !events
            .event_admits(
                "stream.agent_trace",
                Some(&alice_trace_frame),
                Some(OWNER_USER_ID),
                &sessions,
            )
            .await,
        "the operator is not exempt from session ownership for live events either"
    );

    // ══════════════════════ Alice: sees her own everything ══════════════════════

    let alice_list = as_caller(
        "u-alice",
        handle_list_db(req("sessions.list", None), sessions.clone()),
    )
    .await;
    let alice_sessions = alice_list.result.expect("success")["sessions"]
        .as_array()
        .expect("sessions array")
        .clone();
    assert!(
        alice_sessions
            .iter()
            .any(|s| s["key"] == alice_key_str.as_str()),
        "alice must see her own session: {alice_sessions:?}"
    );
    assert!(
        !alice_sessions
            .iter()
            .any(|s| s["key"] == legacy_key.to_key_string().as_str()),
        "alice must not see the legacy/owner session: {alice_sessions:?}"
    );

    let alice_memory = as_caller(
        "u-alice",
        handle_search(
            req(
                "memory.search",
                Some(json!({ "agent_id": alice_partition })),
            ),
            memory.clone(),
        ),
    )
    .await;
    let alice_memories = alice_memory.result.expect("success")["memories"]
        .as_array()
        .expect("memories array")
        .clone();
    assert!(
        !alice_memories.is_empty(),
        "alice must see her own captured note"
    );

    assert!(
        events
            .event_admits(
                "stream.agent_trace",
                Some(&alice_trace_frame),
                Some("u-alice"),
                &sessions,
            )
            .await,
        "alice must be admitted to her own run event — the guard must not be a false positive"
    );
}

/// The `teams.*` half of §9-1, added when the family was tightened
/// (2026-08-06). Same acceptance shape as the test above, driven through the
/// real handlers: alice creates a team and a task in it; bob — a legitimate,
/// logged-in second user — sees an empty list and gets the SAME `not found`
/// response for every addressed method, whether he names the team or one of
/// its tasks.
#[tokio::test]
async fn two_users_cannot_see_each_others_teams() {
    use crate::gateway::handlers::teams::{
        handle_create_task, handle_get, handle_list, handle_list_tasks, handle_rename,
        handle_task_skip,
    };
    use crate::teams::{ScopedTeamStore, SqliteTeamStore, TeamStore};

    let raw = SqliteTeamStore::new(rusqlite::Connection::open_in_memory().unwrap());
    raw.migrate().await.unwrap();
    let teams: Arc<dyn TeamStore> = ScopedTeamStore::wrap(Arc::new(raw));

    let coord_conn = rusqlite::Connection::open_in_memory().unwrap();
    let coord_raw = crate::agents::swarm::tasks::store::SqliteCoordTaskStore::new(coord_conn);
    coord_raw.migrate().await.unwrap();
    let coord: Arc<dyn crate::agents::swarm::tasks::CoordTaskStore> = Arc::new(coord_raw);

    // --- alice creates a team and one task in it -----------------------------
    let team = as_caller(
        "u-alice",
        teams.create_team(crate::teams::NewTeam {
            name: "Alice Squad".to_string(),
            description: String::new(),
            leader_id: "agent-main".to_string(),
        }),
    )
    .await
    .expect("alice creates her team");

    let created = as_caller(
        "u-alice",
        handle_create_task(
            req(
                "teams.create_task",
                Some(json!({ "team_id": team.id, "subject": "ship it" })),
            ),
            teams.clone(),
            coord.clone(),
        ),
    )
    .await;
    assert!(created.error.is_none(), "alice creates her own task");
    let task_id = created.result.expect("result")["task"]["id"]
        .as_str()
        .expect("task id")
        .to_string();

    // --- bob sees nothing ----------------------------------------------------
    let bob_list = as_caller("u-bob", handle_list(req("teams.list", None), teams.clone())).await;
    assert_eq!(
        bob_list.result.expect("success")["teams"]
            .as_array()
            .expect("teams array")
            .len(),
        0,
        "bob must not see alice's team in teams.list"
    );

    let team_addressed = json!({ "team_id": team.id });
    let by_team: Vec<crate::gateway::protocol::JsonRpcResponse> = vec![
        as_caller(
            "u-bob",
            handle_get(
                req("teams.get", Some(team_addressed.clone())),
                teams.clone(),
                coord.clone(),
            ),
        )
        .await,
        as_caller(
            "u-bob",
            handle_list_tasks(
                req("teams.list_tasks", Some(team_addressed.clone())),
                teams.clone(),
                coord.clone(),
            ),
        )
        .await,
        as_caller(
            "u-bob",
            handle_rename(
                req(
                    "teams.rename",
                    Some(json!({ "team_id": team.id, "name": "Bob Squad" })),
                ),
                teams.clone(),
                Arc::new(crate::gateway::event_bus::GatewayEventBus::new()),
            ),
        )
        .await,
    ];
    for resp in &by_team {
        let err = resp.error.as_ref().expect("bob must be refused");
        assert_eq!(err.code, RESOURCE_NOT_FOUND);
        assert_eq!(
            err.message,
            format!("Team '{}' not found", team.id),
            "the denial must be the SAME response an absent team produces"
        );
    }

    // Task-addressed: the half a team_id-keyed sweep would have missed.
    let skip = as_caller(
        "u-bob",
        handle_task_skip(
            req("teams.task.skip", Some(json!({ "task_id": task_id }))),
            teams.clone(),
            coord.clone(),
        ),
    )
    .await;
    let err = skip.error.as_ref().expect("bob must be refused");
    assert_eq!(err.code, RESOURCE_NOT_FOUND);
    assert_eq!(err.message, format!("Task '{task_id}' not found"));

    // --- and alice's team is untouched --------------------------------------
    let alice_get = as_caller(
        "u-alice",
        handle_get(
            req("teams.get", Some(team_addressed)),
            teams.clone(),
            coord.clone(),
        ),
    )
    .await;
    let result = alice_get.result.expect("alice reads her own team");
    assert_eq!(result["team"]["name"], "Alice Squad");
    assert_eq!(
        result["tasks"].as_array().expect("tasks").len(),
        1,
        "alice's task survived bob's skip attempt"
    );
}

/// Reproduce the task-local nesting a real GATEWAY DISPATCH applies to an RPC
/// issued from inside a project room: the ambient scope is the ROOM's
/// (`ScopeId::Project`), while `CALLER_USER` is the individual member who is
/// asking. The divergence is the point — see
/// `gateway::handlers::agent::resolve_attribution` and
/// `execution_engine::AUTHOR_USER_KEY`: every member's turn in a room carries
/// the room's attribution (that is what shares the memory partition), so
/// "whose turn is this" needs its own fact.
///
/// NOT the shape of a turn's INTERIOR — see [`as_room_turn`], which is the one
/// to use for anything an agent run emits.
async fn as_room_member<F, T>(user: &str, project_id: &str, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    with_scope(
        Some(ScopeAttribution {
            owner_user_id: user.to_string(),
            scope: crate::scope::ScopeId::Project(project_id.to_string()),
        }),
        CALLER_USER.scope(Some(user.to_string()), fut),
    )
    .await
}

/// Reproduce the task-local nesting the INTERIOR of a room's agent run sees —
/// the context every `SessionEvent::UserMessage` writer actually runs in.
///
/// Four facts, and taking any of them from the wrong place is a defect this
/// suite has to be able to see:
///
/// - **The ambient scope's `owner_user_id` is `room_owner`, not the speaker**,
///   identically for every member. That is what `resolve_attribution` path 1
///   rebuilds from the session row, and it is what puts both members' memory in
///   one partition. A helper that set it to the speaker would make the label's
///   fallback agree with the correct answer by accident and pass while
///   production stamped the room's creator on everyone's message.
/// - **The speaker rides its own task-local**, seeded by
///   `run_loop::with_request_scope` from the request's `AUTHOR_USER_KEY`.
/// - **A real `tokio::spawn` sits between the seeding and the emission**, and
///   both task-locals are captured before it and re-seeded inside — the shape
///   `Orchestrator::dispatch` uses. Nesting them in ONE task is a sequence
///   production never presents, and it is what made the first fix round pass
///   while every room message carried the wrong name.
/// - **`CALLER_USER` is dead** on the far side, which is why nothing here
///   scopes it.
///
/// Division of labour: this proves the prompt RENDERS a correct author. That
/// `dispatch` really performs the re-seed is a different claim, proven against
/// the production wiring by
/// `tests/gateway_chat_room_author_across_spawn.rs`.
async fn as_room_turn<F, T>(speaker: &str, room_owner: &str, project_id: &str, fut: F) -> T
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    with_scope(
        Some(ScopeAttribution {
            owner_user_id: room_owner.to_string(),
            scope: crate::scope::ScopeId::Project(project_id.to_string()),
        }),
        crate::scope::with_room_author(Some(speaker.to_string()), async move {
            let captured_scope = crate::scope::current_scope();
            let captured_author = crate::scope::current_room_author();
            tokio::spawn(with_scope(
                captured_scope,
                crate::scope::with_room_author(captured_author, fut),
            ))
            .await
            .expect("the emission task must not panic")
        }),
    )
    .await
}

/// Spec §8/§13 (P2 acceptance): **两用户在同一项目群聊协作，项目记忆共享.**
///
/// The storage half of the P2 acceptance criterion, driven through the real
/// derivation (`project_scope::session_read_ids`) and the real `memory.search`
/// handler under the same task-locals a dispatch applies. Four claims, and the
/// last two are the ones that fail silently if the room arms are wrong:
///
/// 1. Alice writes in the room; **Bob's recall union contains that partition**
///    — one room, one memory, whoever is speaking.
/// 2. A non-member addressing the room's partition gets an EMPTY result, not
///    an error: `memory.search` must not become an existence oracle.
/// 3. Neither member's recall union contains ANY personal partition — a room
///    recalls the org tier and itself, and nobody's private memory. Asserted on
///    the whole vec, because a third entry sneaking in leaks with no error.
/// 4. The profile floor is ABSENT in the room. "Whose USER.md" has no answer
///    here; a predicate spelled `current_scope().is_some()` would admit the
///    creator's and report nothing.
#[tokio::test]
async fn two_members_share_one_room_memory_and_nobody_elses() {
    use crate::memory::project_scope::{profile_floor_id, session_read_ids, session_write_id};
    use crate::projects::ProjectStore;

    let _guard = crate::projects::roster::TEST_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let temp = TempDir::new().unwrap();
    let memory: Arc<SqliteMemoryBackend> =
        Arc::new(SqliteMemoryBackend::new(&temp.path().join("memory.db")).unwrap());

    // A real room with a real roster: alice owns it, bob is on it, mallory is
    // not. The roster IS the authorization (spec §6.1) — there is no second
    // per-resource grant to forget to set.
    let projects = ProjectStore::new(rusqlite::Connection::open_in_memory().unwrap());
    projects.create_schema().unwrap();
    let room = projects
        .create("shared room", Some("u-alice"), None)
        .unwrap();
    projects.add_member(&room.id, "u-bob").unwrap();

    // ── 1. Alice speaks in the room ──
    let alice_wrote_to = as_room_member("u-alice", &room.id, async {
        let partition = session_write_id("main", false, None);
        memory
            .insert_raw_memory(
                &RawMemory::new(
                    "the room agreed to ship on friday".to_string(),
                    RawMemorySource::Reflection,
                )
                .with_agent(partition.clone()),
            )
            .await
            .unwrap();
        partition
    })
    .await;

    // ── 2. Bob, a different member of the SAME room, recalls it ──
    let bob_reads = as_room_member("u-bob", &room.id, async {
        session_read_ids("main", false, None)
    })
    .await;
    assert!(
        bob_reads.contains(&alice_wrote_to),
        "one room, one memory: bob's recall union must contain the partition \
         alice wrote to ({alice_wrote_to}): {bob_reads:?}"
    );

    let bob_search = as_room_member(
        "u-bob",
        &room.id,
        handle_search(
            req(
                "memory.search",
                Some(json!({ "agent_id": alice_wrote_to.clone() })),
            ),
            memory.clone(),
        ),
    )
    .await;
    let rows = bob_search.result.expect("success, not an error")["memories"]
        .as_array()
        .expect("memories array")
        .clone();
    assert!(
        rows.iter().any(|m| m["user_input"]
            .as_str()
            .unwrap_or_default()
            .contains("friday")),
        "bob must recall what alice taught the agent in their shared room: {rows:?}"
    );

    // ── 3. A stranger gets emptiness, not an error ──
    let mallory_search = as_caller(
        "u-mallory",
        handle_search(
            req(
                "memory.search",
                Some(json!({ "agent_id": alice_wrote_to.clone() })),
            ),
            memory.clone(),
        ),
    )
    .await;
    let mallory_rows = mallory_search
        .result
        .expect("a denial must look like an empty result, never an error")["memories"]
        .as_array()
        .expect("memories array")
        .clone();
    assert!(
        mallory_rows.is_empty(),
        "a non-member must not read the room's partition: {mallory_rows:?}"
    );

    // ── 4. The room recalls nobody's personal memory, and has no profile ──
    assert_eq!(
        bob_reads,
        vec!["main".to_string(), alice_wrote_to.clone()],
        "a room recalls the org tier and itself — a third entry here is a \
         cross-user leak that reports nothing"
    );
    let alice_reads = as_room_member("u-alice", &room.id, async {
        session_read_ids("main", false, None)
    })
    .await;
    assert_eq!(
        alice_reads, bob_reads,
        "the room resolves identically for every member — that IS the sharing"
    );
    for ids in [&alice_reads, &bob_reads] {
        assert!(
            !ids.iter().any(|id| id.contains("__u-")),
            "not even the creator's personal memory is recalled in a room: {ids:?}"
        );
    }
    assert_eq!(
        as_room_member("u-alice", &room.id, async {
            profile_floor_id("main", false, None)
        })
        .await,
        None,
        "a room has no 'the user' whose profile could be the floor"
    );
}

/// Spec §9-2: a pre-P1 single-user fixture must read back byte-identical
/// after the P1 code is deployed. Three data shapes, each hand-authored
/// exactly as P0-era code would have left it on disk (never through the new
/// write paths), then opened through the new code as the owner (loopback
/// attribution — `OWNER_USER_ID`, matching `visibility.rs`'s "(u-owner,
/// None) -> true" rule for legacy rows):
///
/// - a session `metadata.json` with no `owner_user_id`/`scope_id` keys at
///   all — reading it must not retroactively stamp them, and re-serializing
///   must not introduce the keys (`skip_serializing_if` holds);
/// - a bare `agents/main/MEMORY.md` — the first owner-scoped load adopts it
///   (copies, leaving the base instance in place — see
///   `adopt_owner_curated_file`'s doc) into
///   `agents/main__u-owner/MEMORY.md` with byte-identical content;
/// - a base-partition note under `note/main/...` — `session_read_ids` must
///   still query the org partition FIRST in the union, and the note itself
///   must survive untouched.
#[tokio::test]
async fn single_user_fixture_is_byte_identical_after_upgrade() {
    use crate::config::types::memory::MemoryInjectionMode;
    use crate::gateway::session_store::file_backend::{
        sanitize_key_for_dir, FileSessionStore, FileSessionStoreConfig,
    };
    use crate::memory::curated::CuratedConfig;
    use crate::memory::project_scope::{scoped_agent_id, session_read_ids};
    use crate::thinker::memory_context_provider::MemoryContextProvider;

    let temp = TempDir::new().unwrap();
    let sessions_dir = temp.path().join("sessions");
    let curated_dir = temp.path().join("curated");
    let notes_dir = temp.path().join("memory_dir");
    tokio::fs::create_dir_all(&sessions_dir).await.unwrap();
    tokio::fs::create_dir_all(&curated_dir).await.unwrap();

    // ── Fixture 1: a pre-P1 session dir. `metadata.json` carries none of the
    // new P1 keys — exactly the shape P0-era code wrote. ──
    let legacy_key = SessionKey::main(unique("legacy-single-user-agent"));
    let key_str = legacy_key.to_key_string();
    let session_dir = sessions_dir.join(sanitize_key_for_dir(&key_str));
    tokio::fs::create_dir_all(&session_dir).await.unwrap();
    let legacy_metadata_json = format!(
        r#"{{"key":"{key_str}","agent_id":"legacy-single-user-agent","session_type":"main","created_at":1000,"last_active_at":1000,"message_count":3,"total_tokens":42}}"#
    );
    tokio::fs::write(session_dir.join("metadata.json"), &legacy_metadata_json)
        .await
        .unwrap();
    tokio::fs::write(session_dir.join("transcript.jsonl"), "")
        .await
        .unwrap();

    // ── Fixture 2: a bare `agents/main/MEMORY.md`, never adopted. ──
    let bare_memory_dir = curated_dir.join("main");
    tokio::fs::create_dir_all(&bare_memory_dir).await.unwrap();
    let legacy_curated_content = "legacy fact one\n\u{a7}\n";
    tokio::fs::write(bare_memory_dir.join("MEMORY.md"), legacy_curated_content)
        .await
        .unwrap();

    // ── Fixture 3: a base-partition note. ──
    let note_dir = notes_dir.join("note").join("main").join("general");
    tokio::fs::create_dir_all(&note_dir).await.unwrap();
    let legacy_note_content = "---\ncategory: general\n---\n\n- a fact from before P1\n";
    tokio::fs::write(note_dir.join("legacy-fact.md"), legacy_note_content)
        .await
        .unwrap();

    // ══════════════════ Open through the new code, as the owner ══════════════════

    // -- 1. session metadata: byte-identical round trip --
    let sessions: Arc<dyn SessionStore> = Arc::new(
        FileSessionStore::new(FileSessionStoreConfig {
            base_dir: sessions_dir.clone(),
            ..Default::default()
        })
        .unwrap(),
    );

    let meta = as_caller(OWNER_USER_ID, sessions.get_metadata(&legacy_key))
        .await
        .unwrap()
        .expect("legacy session readable");
    assert!(
        meta.owner_user_id.is_none() && meta.scope_id.is_none(),
        "reading an existing pre-P1 row must never retroactively stamp it: {meta:?}"
    );
    let reserialized = serde_json::to_string(&meta).unwrap();
    assert!(
        !reserialized.contains("owner_user_id") && !reserialized.contains("scope_id"),
        "skip_serializing_if must hold — no P1 keys leak into a pre-P1-shaped row: {reserialized}"
    );

    let owner_list = as_caller(
        OWNER_USER_ID,
        handle_list_db(req("sessions.list", None), sessions.clone()),
    )
    .await;
    let rows = owner_list.result.expect("success")["sessions"]
        .as_array()
        .expect("sessions array")
        .clone();
    assert!(
        rows.iter().any(|s| s["key"] == key_str.as_str()),
        "the owner must see their own pre-P1 session unchanged: {rows:?}"
    );

    // -- 2. curated envelope: post-adoption move is byte-identical --
    let provider = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
        .with_curated_config(CuratedConfig {
            memory_char_limit: 4000,
            user_char_limit: 4000,
            ..CuratedConfig::default()
        })
        .with_curated_root_for_test(curated_dir.clone());

    let store = as_caller(OWNER_USER_ID, provider.get_or_load_curated_store("main"))
        .await
        .unwrap();
    assert_eq!(
        tokio::fs::read(bare_memory_dir.join("MEMORY.md"))
            .await
            .unwrap(),
        legacy_curated_content.as_bytes(),
        "adoption COPIES the bare file: the base path is the org-tier instance \
         every unscoped principal still resolves to, and emptying it is the \
         P1 final review's I5 (see adopt_owner_curated_file's doc)"
    );
    let owner_scoped_id = scoped_agent_id("main", OWNER_USER_ID);
    let adopted_path = curated_dir.join(&owner_scoped_id).join("MEMORY.md");
    let adopted_bytes = tokio::fs::read(&adopted_path).await.unwrap();
    assert_eq!(
        adopted_bytes,
        legacy_curated_content.as_bytes(),
        "post-adoption content must be byte-identical to the pre-P1 file"
    );
    let entries = store.current_entries();
    assert!(
        entries.iter().any(|e| e.contains("legacy fact one")),
        "the adopted content must still be what the envelope renders: {entries:?}"
    );

    // -- 3. notes recall: org partition still first in the union --
    let ids = as_caller(OWNER_USER_ID, async {
        session_read_ids("main", true, None)
    })
    .await;
    assert_eq!(
        ids,
        vec!["main".to_string(), owner_scoped_id],
        "org partition must still be queried first in the union"
    );
    let note_bytes_after = tokio::fs::read(note_dir.join("legacy-fact.md"))
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8(note_bytes_after).unwrap(),
        legacy_note_content,
        "the base-partition note must survive the P1 upgrade untouched"
    );
}

/// Spec §6.2 (P2 acceptance, other half): **两用户在同一项目群聊协作** — the
/// collaboration half of the criterion whose storage half is
/// [`two_members_share_one_room_memory_and_nobody_elses`].
///
/// Sharing one memory partition is what makes the room a room; being able to
/// tell the members apart is what makes it a *conversation*. Both are needed,
/// and only the second exists in bytes the model reads — which is why this
/// asserts on the built prompt and not on the events.
///
/// End to end through the real path: [`crate::scope::room_author`] decides
/// whether a message gets an author at all, `SessionEvent::UserMessage` carries
/// it, and `harness::agent::prompt::build_prompt` — the ONE place events become
/// model messages — renders it. A personal-scope turn is included as the
/// control, because the failure this guards against is not "the label is
/// wrong": it is the label being absent, which merges two people into one voice
/// and reports nothing.
#[tokio::test]
async fn the_model_can_tell_two_room_members_apart() {
    use crate::providers::message::{ContentBlock, UnifiedMessage};
    use crate::session::events::{MessageContent, SessionEvent, SessionEventRecord};

    crate::scope::directory::record("u-room-alice", "Alice");
    crate::scope::directory::record("u-room-bob", "Bob");

    // Alice created the room, so hers is the `owner_user_id` on the session
    // row and therefore on EVERY member's run — including Bob's.
    async fn said_in_room(user: &str, text: &str, seq: u64) -> SessionEventRecord {
        let author = as_room_turn(user, "u-room-alice", "p-standup", async {
            crate::scope::ambient_room_author()
        })
        .await;
        assert_eq!(
            author.as_deref(),
            Some(user),
            "the label names the speaker; deriving it from the run's scope \
             would name the room's creator on every message"
        );
        user_event(text, seq, author)
    }

    fn user_event(text: &str, seq: u64, author: Option<String>) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            created_at_ms: 1_000 + seq as i64,
            event: SessionEvent::UserMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent {
                    text: text.to_string(),
                    blocks: Vec::new(),
                    thinking: None,
                    thinking_signature: None,
                },
                at: 1_000 + seq as i64,
                synthetic: false,
                author_user_id: author,
            },
        }
    }

    // The control: a personal session produces no author even when a speaker
    // IS seeded, so a solo session's prompt stays byte-identical to pre-P2.
    // Seeding the author is what makes this a real control — without it the
    // assertion would also pass for a build that simply never labels anything.
    let solo = with_scope(
        Some(ScopeAttribution::personal("u-room-alice")),
        crate::scope::with_room_author(Some("u-room-alice".to_string()), async {
            crate::scope::ambient_room_author()
        }),
    )
    .await;
    assert_eq!(
        solo, None,
        "a personal session has no second speaker to name"
    );

    let events = vec![
        said_in_room("u-room-alice", "let's ship on friday", 1).await,
        said_in_room("u-room-bob", "i need one more day", 2).await,
        user_event("and nobody typed this one", 3, None),
    ];

    let texts: Vec<String> = crate::harness::agent::prompt::build_prompt(&events, 0)
        .into_iter()
        .filter_map(|m| match m {
            UnifiedMessage::User { content } => content.into_iter().find_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    assert_eq!(
        texts,
        vec![
            "[Alice]: let's ship on friday".to_string(),
            "[Bob]: i need one more day".to_string(),
            "and nobody typed this one".to_string(),
        ]
    );
}

/// Spec §8 (P2 acceptance, Task 7): **collaborating in a room means working in
/// the same folder** — the third leg beside shared memory and distinguishable
/// speakers.
///
/// Driven through the real `handlers::agent::build_run_request`, the one
/// gateway→engine hand-off, under the task-locals a real dispatch applies. The
/// two claims that fail silently:
///
/// 1. **Both members resolve to the same directory**, from a binding neither of
///    them named on the request. A per-member fallback to the agent workspace
///    would look like a working room right up until one of them saved a file.
/// 2. **A remote, chat-tier member is not gated.** The config-tier gate guards
///    *choosing* a directory; using one the owner already chose is not
///    choosing. Getting this wrong locks every remote member out of the room's
///    entire purpose, and it fails as a permission error nobody would connect
///    to project rooms.
///
/// The personal-session control is what proves the workspace came from the
/// room and not from something ambient in the fixture.
#[tokio::test]
async fn two_members_of_a_room_work_in_the_same_bound_folder() {
    use crate::gateway::caller_identity::{CALLER_IS_LOOPBACK, CALLER_ROLE};
    use crate::gateway::handlers::agent::{build_run_request, AgentRunParams};
    use crate::gateway::router::AgentRouter;

    let _guard = crate::projects::roster::TEST_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let folder = TempDir::new().expect("tempdir");
    let canonical = std::fs::canonicalize(folder.path()).expect("canonical");

    let store = crate::projects::ProjectStore::shared();
    let room = store
        .create(
            "shared workspace room",
            Some("u-ws-alice"),
            Some(&canonical),
        )
        .expect("create room");
    store.add_member(&room.id, "u-ws-bob").expect("bob joins");

    // Remote AND chat-tier for both: the tier `project_root` refuses.
    async fn cwd_for(user: &str, project_id: Option<&str>) -> Option<std::path::PathBuf> {
        let session_key = AgentRouter::new().route(None, None, None, None).await;
        let params = AgentRunParams {
            input: "where are we working".to_string(),
            session_key: None,
            channel: Some("gui:chat".to_string()),
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: None,
            model_override: None,
            exec_tier: None,
            mode: None,
            voice_input: false,
            project_id: project_id.map(str::to_string),
        };
        CALLER_USER
            .scope(
                Some(user.to_string()),
                CALLER_ROLE.scope(
                    Some("member".to_string()),
                    CALLER_IS_LOOPBACK.scope(
                        false,
                        build_run_request(format!("run-{user}"), &session_key, params, None, None),
                    ),
                ),
            )
            .await
            .expect("a room member's turn must build")
            .workspace_override
    }

    let alice = cwd_for("u-ws-alice", Some(&room.id)).await;
    let bob = cwd_for("u-ws-bob", Some(&room.id)).await;
    assert_eq!(alice.as_deref(), Some(canonical.as_path()));
    assert_eq!(bob, alice, "one room, one working directory");

    // Control: outside the room the same person gets no override at all, so
    // the folder above can only have come from the binding.
    assert_eq!(cwd_for("u-ws-alice", None).await, None);
}
