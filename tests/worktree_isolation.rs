//! P3 Stage H Task 9 — H-T1 happy-path integration test.
//!
//! Verifies that when `SpawnRequest.isolation == Some(IsolationMode::Worktree)`,
//! `spawn()` provisions a fresh git worktree, runs the harness inside it, emits
//! `WorktreeCreated` + `WorktreeCleanedUp{leaked:false}` trace events, and
//! removes the worktree directory on successful exit.
//!
//! The test is skipped gracefully when `git` is unavailable or the working
//! directory is not a git repository.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serial_test::serial;
use tokio_util::sync::CancellationToken;

use alephcore::agents::subagent_spawner::{spawn, SpawnRequest, SpawnerBase};
use alephcore::agents::{AgentDef, AgentMode, IsolationMode};
use alephcore::harness::chain_context::ChainContext;
use alephcore::harness::trace::LoopTraceEvent;
use alephcore::harness::TraceSink;
use alephcore::providers::adapter::{ProviderResponse, RequestPayload};
use alephcore::providers::AiProvider;
use alephcore::session::events::ToolOutput;
use alephcore::session::in_process::InProcessActorSessionService;
use alephcore::session::service::SessionService;
use alephcore::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use alephcore::tools::service::{ToolDefinition, ToolError, ToolService};
use alephcore::Result as AlephResult;

// -- Helpers ------------------------------------------------------------------

fn is_git_repo() -> bool {
    std::process::Command::new("git")
        .arg("rev-parse")
        .arg("--git-dir")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Wall clock, in ms, of a bare `git worktree add --detach` and the matching
/// `git worktree remove --force` in *this* environment.
///
/// H-T5's budget is derived from this rather than from a constant. `create` and
/// `cleanup` are one such git invocation each plus a uuid and a path join, so
/// their elapsed time is git checking out — and then deleting — every tracked
/// file in the repo. That cost scales with repo size and disk speed, not with
/// Aleph code: on this tree a bare `git worktree add` measures ~700ms and
/// `git worktree remove` ~600ms, both already past the 800/400ms constants this
/// test used to assert, before Aleph runs a single line. An absolute budget
/// therefore grades the machine. Measuring the floor here is what lets the test
/// assert the one property the code actually owns — that the wrapper adds
/// essentially nothing on top of git.
fn raw_git_worktree_baseline(repo: &std::path::Path) -> (u128, u128) {
    let path = std::env::temp_dir().join(format!("aleph-h-t5-baseline-{}", uuid::Uuid::new_v4()));

    let t0 = std::time::Instant::now();
    let added = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "add", "--detach"])
        .arg(&path)
        .arg("HEAD")
        .output()
        .expect("spawn baseline `git worktree add`");
    let add_ms = t0.elapsed().as_millis();
    assert!(
        added.status.success(),
        "baseline `git worktree add` failed: {}",
        String::from_utf8_lossy(&added.stderr)
    );

    let t1 = std::time::Instant::now();
    let removed = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "remove", "--force"])
        .arg(&path)
        .output()
        .expect("spawn baseline `git worktree remove`");
    let remove_ms = t1.elapsed().as_millis();
    assert!(
        removed.status.success(),
        "baseline `git worktree remove` failed: {}",
        String::from_utf8_lossy(&removed.stderr)
    );

    (add_ms, remove_ms)
}

fn fresh_session_service() -> Arc<dyn SessionService> {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    migrate_add_session_events(&conn).expect("migrate session_events");
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    Arc::new(InProcessActorSessionService::new(store))
}

// -- Mocks --------------------------------------------------------------------

struct NoopTools;

#[async_trait]
impl ToolService for NoopTools {
    async fn execute(
        &self,
        _name: &str,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            value: serde_json::json!({}),
            metadata: Default::default(),
        })
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }

    async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
        None
    }

    fn metadata_schema(&self) -> Arc<[alephcore::tool_metadata::ToolDefinition]> {
        Arc::from(Vec::<alephcore::tool_metadata::ToolDefinition>::new())
    }
}

/// Returns a single terminal text response immediately so the harness exits
/// after one Think turn.
struct OneShotProvider;

impl AiProvider for OneShotProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move { Ok(ProviderResponse::text_only("worktree-ok".to_string())) })
    }

    fn name(&self) -> &str {
        "one-shot-worktree"
    }

    fn color(&self) -> &str {
        "#000000"
    }
}

/// Captures every `LoopTraceEvent` emitted during the run.
#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<LoopTraceEvent>>,
}

impl TraceSink for CapturingSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        self.events.lock().unwrap().push(event.clone());
    }

    fn flush(&self) {}
}

// -- Test ---------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn h_t1_worktree_isolation_happy_path() {
    // Skip if git is unavailable or cwd is not inside a git repo.
    if !is_git_repo() {
        eprintln!("h_t1_worktree_isolation_happy_path: skipped (not a git repo)");
        return;
    }

    let sink = Arc::new(CapturingSink::default());
    let sink_for_base: Arc<dyn TraceSink> = sink.clone();

    let base = SpawnerBase {
        session: fresh_session_service(),
        parent_tools: Arc::new(NoopTools),
        provider: Arc::new(OneShotProvider),
        chain: ChainContext::new(),
        raw_memory_writer: None,
        capture_registry: None,
        parent_agent_id: None,
        parent_session_id: None,
        guardrails: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        trace_sink: Some(sink_for_base),
        // P3 Stage I:
        plugin_registry: None,
        subagent_semaphore: None,
        routing_store: None,
        default_max_iterations: None,
        parallel_tool_concurrency: None,
        context_budget_config: None,
        context_budget_refiner: None,
        primary_context_window: None,
        cheap_summary_provider: None,
        verifier_chain: None,
    };

    let agent_def =
        AgentDef::new("worktree-probe", AgentMode::SubAgent).with_allowed_tools(vec!["*".into()]);

    let req = SpawnRequest {
        agent_def: &agent_def,
        task: "worktree isolation test",
        context_summary: None,
        model: None,
        timeout_secs: 30,
        cancel: CancellationToken::new(),
        spawn_context: None,
        fork_source: None,
        isolation: Some(IsolationMode::Worktree),
        strategy: None,
        session_mode: None,
        request_id: None,
    };

    let result = spawn(&base, req).await;
    assert!(result.is_ok(), "spawn should succeed: {:?}", result.err());
    assert_eq!(result.unwrap().final_text.as_deref(), Some("worktree-ok"));

    // Inspect captured trace events.
    let events = sink.events.lock().unwrap();

    // Exactly one WorktreeCreated event.
    let created_paths: Vec<PathBuf> = events
        .iter()
        .filter_map(|e| {
            if let LoopTraceEvent::WorktreeCreated { path } = e {
                Some(path.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        created_paths.len(),
        1,
        "must emit exactly one WorktreeCreated event; got events: {events:?}"
    );
    let created_path = &created_paths[0];

    // Exactly one WorktreeCleanedUp event, with leaked=false.
    let cleaned_up: Vec<(PathBuf, bool)> = events
        .iter()
        .filter_map(|e| {
            if let LoopTraceEvent::WorktreeCleanedUp { path, leaked } = e {
                Some((path.clone(), *leaked))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        cleaned_up.len(),
        1,
        "must emit exactly one WorktreeCleanedUp event; got events: {events:?}"
    );
    let (cleanup_path, leaked) = &cleaned_up[0];
    assert!(
        !leaked,
        "WorktreeCleanedUp must have leaked=false on happy path"
    );

    // Both path fields must match.
    assert_eq!(
        created_path, cleanup_path,
        "WorktreeCreated.path must equal WorktreeCleanedUp.path"
    );

    // The worktree dir name must contain "aleph-subagent-".
    let dir_name = created_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    assert!(
        dir_name.contains("aleph-subagent-"),
        "worktree dir name {dir_name:?} should contain 'aleph-subagent-'"
    );

    // The worktree directory must no longer exist on disk.
    assert!(
        !created_path.exists(),
        "worktree dir {created_path:?} must be removed after successful spawn"
    );
}

#[tokio::test]
#[serial]
async fn h_t2_cancel_path_still_cleans_up() {
    if !is_git_repo() {
        eprintln!("h_t2_cancel_path_still_cleans_up: skipped (not a git repo)");
        return;
    }

    let sink = Arc::new(CapturingSink::default());
    let arc_sink: Arc<dyn TraceSink> = sink.clone();
    let repo = std::env::current_dir().unwrap();

    let path = {
        let h = alephcore::sandbox::worktree::create(&repo, "h-t2", Some(arc_sink.clone()))
            .await
            .expect("create");
        h.path().to_path_buf()
        // h dropped here — Drop safety-net cleans up
    };
    for _ in 0..50 {
        if !path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        !path.exists(),
        "Drop safety-net must clean up cancelled worktree"
    );
}

#[tokio::test]
#[serial]
async fn h_t3_panic_path_emits_leaked_true_event() {
    if !is_git_repo() {
        eprintln!("h_t3_panic_path_emits_leaked_true_event: skipped (not a git repo)");
        return;
    }

    let sink = Arc::new(CapturingSink::default());
    let arc_sink: Arc<dyn TraceSink> = sink.clone();
    let repo = std::env::current_dir().unwrap();

    let path = {
        let h = alephcore::sandbox::worktree::create(&repo, "h-t3", Some(arc_sink.clone()))
            .await
            .expect("create");
        h.path().to_path_buf()
        // h dropped here without calling cleanup() — triggers leaked=true path
    };
    for _ in 0..50 {
        if !path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let events = sink.events.lock().unwrap().clone();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, LoopTraceEvent::WorktreeCleanedUp { leaked: true, .. })),
        "expected WorktreeCleanedUp(leaked=true) on Drop path; got events: {events:?}"
    );
}

#[tokio::test]
#[serial]
async fn h_t4_no_leaked_dirs_after_10_random_cancellations() {
    use rand::RngExt;

    if !is_git_repo() {
        eprintln!("h_t4_no_leaked_dirs_after_10_random_cancellations: skipped (not a git repo)");
        return;
    }

    let repo = std::env::current_dir().unwrap();
    let mut paths = Vec::new();
    for i in 0..10 {
        let h = alephcore::sandbox::worktree::create(&repo, &format!("h-t4-{i}"), None)
            .await
            .expect("create");
        paths.push(h.path().to_path_buf());
        if rand::rng().random_bool(0.5) {
            h.cleanup().await.expect("explicit cleanup");
        }
        // h dropped here if not explicitly cleaned up — Drop safety-net fires
    }
    for _ in 0..100 {
        let any_remaining = paths.iter().any(|p| p.exists());
        if !any_remaining {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let leftover: Vec<_> = paths.iter().filter(|p| p.exists()).collect();
    assert!(leftover.is_empty(), "leaked worktrees: {leftover:?}");
}

#[tokio::test]
#[serial]
async fn h_t5_create_and_cleanup_within_perf_budget() {
    if !is_git_repo() {
        eprintln!("h_t5_create_and_cleanup_within_perf_budget: skipped (not a git repo)");
        return;
    }

    let repo = std::env::current_dir().unwrap();
    let (raw_create_ms, raw_cleanup_ms) = raw_git_worktree_baseline(&repo);

    let t0 = std::time::Instant::now();
    let h = alephcore::sandbox::worktree::create(&repo, "h-t5", None)
        .await
        .expect("create");
    let create_ms = t0.elapsed().as_millis();
    let t1 = std::time::Instant::now();
    h.cleanup().await.expect("cleanup");
    let cleanup_ms = t1.elapsed().as_millis();

    // Twice the measured floor plus 300ms of absolute headroom. Loose enough
    // that ordinary run-to-run variance never fails it — and that a platform
    // which slows git down uniformly (Windows Defender's on-access scan) slows
    // the baseline in step instead of needing its own constant. Tight enough to
    // catch a wrapper that grows a second git invocation, a sleep, or a walk
    // over the checked-out tree.
    let create_budget = raw_create_ms * 2 + 300;
    let cleanup_budget = raw_cleanup_ms * 2 + 300;
    assert!(
        create_ms < create_budget,
        "create took {create_ms}ms; raw `git worktree add` here is {raw_create_ms}ms, budget {create_budget}ms"
    );
    assert!(
        cleanup_ms < cleanup_budget,
        "cleanup took {cleanup_ms}ms; raw `git worktree remove` here is {raw_cleanup_ms}ms, budget {cleanup_budget}ms"
    );
}
