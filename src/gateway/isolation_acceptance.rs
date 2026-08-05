//! P1 data-isolation acceptance tests (spec §9).
//!
//! These are the two acceptance tests the P1 branch is named for:
//! - [`two_users_cannot_see_each_other_end_to_end`] (spec §9-1)
//! - [`single_user_fixture_is_byte_identical_after_upgrade`] (spec §9-2)
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
///   (renames, does not copy) into `agents/main__u-owner/MEMORY.md` with
///   byte-identical content;
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
            legacy_warn_threshold: 0.95,
        })
        .with_curated_root_for_test(curated_dir.clone());

    let store = as_caller(OWNER_USER_ID, provider.get_or_load_curated_store("main"))
        .await
        .unwrap();
    assert!(
        !tokio::fs::try_exists(bare_memory_dir.join("MEMORY.md"))
            .await
            .unwrap(),
        "adoption renames the bare file away, it does not copy it"
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
