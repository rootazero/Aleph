//! A `/btw` turn is read-only — proven from the user's raw text down to a file
//! that is not on disk.
//!
//! Deliberately assembled end to end rather than split in two. The read-only
//! floor has unit tests and the routing has unit tests, and **both stay green
//! while the metadata key never reaches `TurnContext`**, because each half is
//! exercised with an input the other half never produces. That is the shape
//! that hid the `EXEC_WORKSPACE` defect for four rounds: sandbox tests built
//! the command by hand so no tool ever filled it in, tool tests ran against a
//! fake sandbox so no containment check ever read it, and the wire between
//! them was cut the whole time.
//!
//! So nothing between the raw input and the refusal is hand-built here:
//!
//! * the metadata key comes from [`stamp_btw`] — `execute()`'s first statement;
//! * the session move comes from [`redirect_to_side_session`] — its second;
//! * the tier and the `side_question` flag come from `resolve_turn_permissions`;
//! * the `TurnContext` comes from `TurnPermissions::turn_context`, which is the
//!   call the agent loop makes;
//! * the service comes from `build_request_tool_service`, likewise.
//!
//! The one stand-in is at the far end — the tool the service dispatches to —
//! and it is the same stand-in `tests/exec_workspace_jail.rs` uses for the same
//! reason. It carries the real name, so the permission decision is made from
//! the real declared facts for `file_write`, and it really writes the file, so
//! the assertion is about an **effect**: `proof.txt` existing is precisely the
//! failure this file exists to catch, and no amount of correct-looking plumbing
//! produces a passing run that also wrote it.
//!
//! [`stamp_btw`]: super::slash_command::stamp_btw
//! [`redirect_to_side_session`]: super::execute::redirect_to_side_session

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::execute::redirect_to_side_session;
use super::slash_command::stamp_btw;
use super::tests::{gate_test_agent, gate_test_request, test_engine};
use super::{build_request_tool_service, RunRequest};
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Arc;
use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult as LoopToolResult};
use crate::tools::service::ToolService;

/// The far end. Named `file_write` on purpose: the tier reads a tool's
/// DECLARED facts by name (`ScopedToolService::tool_facts` →
/// `is_idempotent_builtin_name`), so the permission decision under test is the
/// real one rather than one made about a name nothing has classified.
///
/// It declares nothing else, which is the fail-closed (mutating) shape, and it
/// really writes the file — the effect the assertions read.
struct RealWritingFileWrite;

#[async_trait::async_trait]
impl LoopTool for RealWritingFileWrite {
    fn name(&self) -> &str {
        "file_write"
    }
    fn description(&self) -> &str {
        "writes a file"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, input: Value, _cancel: CancellationToken) -> LoopToolResult {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let content = input
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match std::fs::write(&path, content) {
            Ok(()) => LoopToolResult::Success {
                output: json!({ "written": path }),
            },
            Err(e) => LoopToolResult::Error {
                error: e.to_string(),
                retryable: false,
            },
        }
    }
}

fn write_registry() -> Arc<LoopToolRegistry> {
    let mut r = LoopToolRegistry::new();
    r.register(Box::new(RealWritingFileWrite));
    Arc::new(r)
}

/// Everything one turn produces, from a raw user string.
struct Turn {
    tools: Arc<dyn ToolService>,
    /// The session the run was admitted on — the redirect's observable result.
    executes_on: SessionKey,
    /// The session the user typed in.
    typed_in: SessionKey,
}

/// Drive one turn's real resolution chain from `input`, stopping just short of
/// the harness: everything the agent loop does to decide what a tool call is
/// allowed to do, and nothing it does to ask a model for one.
async fn resolve_turn(input: &str, temp: &tempfile::TempDir) -> Turn {
    let engine = test_engine();
    let agent = gate_test_agent(temp, "btw-wire").await;
    let typed_in = SessionKey::main("btw-wire");

    let mut request: RunRequest = gate_test_request(&typed_in, "run-btw-wire");
    request.input = input.to_string();

    // `execute()`'s first two statements, in its order.
    stamp_btw(&request.input, &mut request.metadata);
    redirect_to_side_session(&mut request);

    let permissions = engine.resolve_turn_permissions(&request, &agent).await;
    let turn_context = permissions.turn_context(&request, &request.run_id, false);

    let tools = build_request_tool_service(
        write_registry(),
        BTreeSet::new(),
        None,
        Some(turn_context),
        None,
        request.session_key.to_key_string(),
        permissions.explicit.clone(),
        permissions.tier,
        false,
        &[],
        false,
        crate::tools::scoped::DeferredTools::empty(),
        None,
    );

    Turn {
        tools,
        executes_on: request.session_key.clone(),
        typed_in,
    }
}

fn write_call(path: &Path) -> Value {
    json!({ "path": path.to_string_lossy(), "content": "hi" })
}

/// The flagship. A side question asked in the ordinary way must not be able to
/// change anything — and "must not" is measured on the filesystem, not on the
/// return value.
#[tokio::test]
async fn a_side_question_cannot_write_a_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let proof: PathBuf = temp.path().join("proof.txt");

    let turn = resolve_turn(
        "/btw create a file called proof.txt with the word hi in it",
        &temp,
    )
    .await;

    // The run moved off the conversation it was typed in — the other half of
    // the promise, and the one that gives it its own busy-queue lane.
    assert_eq!(
        turn.executes_on.to_key_string(),
        crate::gateway::btw::side_key_for(&turn.typed_in).to_key_string(),
        "a side question must execute on its derived side session"
    );
    assert_ne!(
        turn.executes_on.to_key_string(),
        turn.typed_in.to_key_string(),
        "the main session must be untouched — this is the whole promise"
    );

    let outcome = turn.tools.execute("file_write", write_call(&proof)).await;

    // The effect is asserted before the receipt, on purpose: the filesystem is
    // the ground truth, and a refusal delivered alongside a written file would
    // be the more expensive of the two failures. It is also the assertion that
    // must be the one to fail when the ceiling is removed — a test that fails
    // first on the shape of an error is reporting the symptom it happened to
    // reach, not the thing it exists to catch.
    assert!(
        !proof.exists(),
        "the side question wrote {} — the read-only ceiling did not arrive",
        proof.display()
    );

    let err = outcome.expect_err("a mutating tool must be refused during a side question");
    let refusal = err.to_string();
    assert!(
        refusal.contains("/btw side question"),
        "the refusal must name the side question rather than the plan handoff \
         or a policy entry nobody wrote, got: {refusal}"
    );
}

/// The control, and it is what keeps the test above from passing for the wrong
/// reason. The same assembly, the same tool, the same call — only the leading
/// `/btw` removed — must write the file. Without this arm a rig that refused
/// everything (a broken registry, a mis-built service, a typo in the tool name)
/// would look exactly like a working ceiling.
#[tokio::test]
async fn the_same_turn_without_btw_writes_the_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let proof: PathBuf = temp.path().join("proof.txt");

    let turn = resolve_turn(
        "create a file called proof.txt with the word hi in it",
        &temp,
    )
    .await;

    assert_eq!(
        turn.executes_on.to_key_string(),
        turn.typed_in.to_key_string(),
        "an ordinary run must stay on the session it was typed in"
    );

    turn.tools
        .execute("file_write", write_call(&proof))
        .await
        .expect("an ordinary turn writes files");
    assert!(
        proof.exists(),
        "the control arm must actually write, or the refusal above proves nothing"
    );
}

/// `/btw promote` is a side question too — it is the verb that moves the last
/// side answer across, and it must be bound by the same ceiling rather than
/// slipping through because its body is empty.
#[tokio::test]
async fn promote_is_bound_by_the_same_ceiling() {
    let temp = tempfile::tempdir().expect("tempdir");
    let proof: PathBuf = temp.path().join("proof.txt");

    let turn = resolve_turn("/btw promote", &temp).await;

    assert_ne!(
        turn.executes_on.to_key_string(),
        turn.typed_in.to_key_string(),
        "promote runs on the side session like every other side question"
    );
    turn.tools
        .execute("file_write", write_call(&proof))
        .await
        .expect_err("promote is still a side question");
    assert!(!proof.exists());
}

/// The ordering `execute()` depends on, pinned in source.
///
/// Everything else here proves the metadata key *arrives*; none of it can prove
/// that `execute()` still asks in the order it does. Move
/// `redirect_to_side_session` below `admit_run` and every test in this repo
/// stays green while the lane claim, the `RunSlot` key, `mark_admitted` and the
/// busy-input policy all move back to the main session — the silent regression
/// the whole placement exists to prevent. A comment is not a guard.
///
/// Source-level because the two calls are indistinguishable at runtime: the
/// resulting run works, it is simply governed as if it were an ordinary message
/// in the main conversation.
#[test]
fn execute_redirects_before_it_admits() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/gateway/execution_engine/execute.rs");
    let text = std::fs::read_to_string(&path).expect("execute.rs");
    // Split on the bare attribute — never on `\n#[cfg(test)]\n`, which matches
    // nothing on a CRLF checkout and silently widens the "production prefix" to
    // the whole file.
    let production = text
        .replace('\r', "")
        .split("#[cfg(test)]")
        .next()
        .unwrap_or_default()
        .to_string();

    let redirect = production
        .find("redirect_to_side_session(&mut request)")
        .expect("execute() still redirects; if this moved, so did the invariant");
    let admit = production
        .find("self.admit_run(")
        .expect("execute() still admits; the scan stopped matching, so its green means nothing");

    assert!(
        redirect < admit,
        "`redirect_to_side_session` must run BEFORE `admit_run`: the admission \
         gate claims the run slot and applies the busy-input policy on \
         `request.session_key`, so a side question that has not moved yet is \
         steered, interrupted or queued against the run it was asked about — \
         and `mark_admitted` then withdraws its ticket from the wrong lane. \
         Found redirect at byte {redirect}, admit_run at byte {admit}."
    );
}

// ---------------------------------------------------------------------------
// Stopping
// ---------------------------------------------------------------------------

/// Register a `Running` run on `session` and hand back the receiver that a
/// cancellation would arrive on.
async fn park_running_run(
    engine: &super::ExecutionEngine<
        crate::thinker::SingleProviderRegistry,
        super::tests::EmptyToolRegistry,
    >,
    session: &SessionKey,
    run_id: &str,
) -> tokio::sync::mpsc::Receiver<()> {
    let (tx, rx) = tokio::sync::mpsc::channel::<()>(1);
    engine.active_runs.write().await.insert(
        run_id.to_string(),
        super::ActiveRun {
            request: gate_test_request(session, run_id),
            state: super::RunState::Running,
            started_at: chrono::Utc::now(),
            admitted_at: std::time::Instant::now(),
            completed_at: None,
            steps_completed: 0,
            current_tool: None,
            cancel_tx: Some(tx),
            seq_counter: Default::default(),
            chunk_counter: Default::default(),
        },
    );
    rx
}

/// `/stop` in a conversation has to reach the side question asked from it.
///
/// Both halves of stopping are session-keyed, and a btw run's `ActiveRun`
/// carries the SIDE key — so before this, `cancel_session` on the main session
/// simply did not find it. Combined with the TUI dropping every frame of a btw
/// run (so no client can show the user its `run_id` to aim `chat.abort` at), a
/// side question was unstoppable from every surface a user has.
///
/// This is the same reach `cancel_session` already has for delegated children:
/// a side session is a derived child, invisible to the walk only because nothing
/// told the walk about the derivation.
#[tokio::test]
async fn stopping_the_main_conversation_stops_its_side_question() {
    let engine = test_engine();
    let main = SessionKey::main("btw-stop");
    let side = crate::gateway::btw::side_key_for(&main);

    let mut main_rx = park_running_run(&engine, &main, "run-main").await;
    let mut side_rx = park_running_run(&engine, &side, "run-side").await;

    let stopped = engine
        .cancel_session(&main)
        .await
        .expect("cancel_session succeeds");

    assert_eq!(
        stopped.as_deref(),
        Some("run-main"),
        "the receipt names this session's own run when it has one"
    );
    assert!(
        main_rx.recv().await.is_some(),
        "the main run must be cancelled"
    );
    assert!(
        side_rx.recv().await.is_some(),
        "the side question must be cancelled too — nothing else can reach it"
    );
}

/// A side question with no main run still reports as stopped, or `/stop` tells
/// the user "nothing is running" while stopping something.
#[tokio::test]
async fn stopping_reports_a_side_question_it_stopped() {
    let engine = test_engine();
    let main = SessionKey::main("btw-stop-side-only");
    let side = crate::gateway::btw::side_key_for(&main);
    let mut side_rx = park_running_run(&engine, &side, "run-side-only").await;

    let stopped = engine.cancel_session(&main).await.expect("cancel_session");
    assert_eq!(stopped.as_deref(), Some("run-side-only"));
    assert!(side_rx.recv().await.is_some());
}

/// Register a `Running` run that cannot be cancelled — it is in `active_runs`
/// but has no cancel channel, which is what `cancel` reports as `RunNotActive`.
async fn park_uncancellable_run(
    engine: &super::ExecutionEngine<
        crate::thinker::SingleProviderRegistry,
        super::tests::EmptyToolRegistry,
    >,
    session: &SessionKey,
    run_id: &str,
) {
    engine.active_runs.write().await.insert(
        run_id.to_string(),
        super::ActiveRun {
            request: gate_test_request(session, run_id),
            state: super::RunState::Running,
            started_at: chrono::Utc::now(),
            admitted_at: std::time::Instant::now(),
            completed_at: None,
            steps_completed: 0,
            current_tool: None,
            cancel_tx: None,
            seq_counter: Default::default(),
            chunk_counter: Default::default(),
        },
    );
}

/// A side question that could not be stopped must not be reported as nothing
/// having run.
///
/// Only `Ok` may assert something about what was there. Folding the side walk's
/// `Err` into `Ok(None)` would answer "nothing was running" about a session
/// where something was running *and could not be stopped* — the one answer a
/// stop path must never give.
#[tokio::test]
async fn a_side_question_that_cannot_be_stopped_is_not_reported_as_nothing() {
    let engine = test_engine();
    let main = SessionKey::main("btw-stop-err");
    let side = crate::gateway::btw::side_key_for(&main);
    park_uncancellable_run(&engine, &side, "run-stuck").await;

    let outcome = engine.cancel_session(&main).await;
    assert!(
        matches!(
            outcome,
            Err(crate::gateway::execution_engine::ExecutionError::RunNotActive(_))
        ),
        "expected the side walk's failure to surface, got {outcome:?}"
    );
}

/// ...but a side fault must not void a receipt for a main run that really was
/// stopped. The error is reported only when there is nothing else to say, which
/// is why this is a `match` and not a `?` on both branches.
#[tokio::test]
async fn a_side_fault_does_not_void_a_main_run_that_was_stopped() {
    let engine = test_engine();
    let main = SessionKey::main("btw-stop-err-mixed");
    let side = crate::gateway::btw::side_key_for(&main);
    let mut main_rx = park_running_run(&engine, &main, "run-main").await;
    park_uncancellable_run(&engine, &side, "run-stuck").await;

    let stopped = engine
        .cancel_session(&main)
        .await
        .expect("a stop that worked stays a stop");
    assert_eq!(stopped.as_deref(), Some("run-main"));
    assert!(main_rx.recv().await.is_some());
}

/// The walk is one level deep. Stopping a side session must not derive a
/// side-of-side key and go looking in a session nothing ever ran on.
#[tokio::test]
async fn stopping_a_side_session_does_not_walk_further() {
    let main = SessionKey::main("btw-stop-depth");
    let side = crate::gateway::btw::side_key_for(&main);
    assert!(
        crate::gateway::btw::side_session_of(&side).is_none(),
        "a side session has no side session of its own"
    );

    let engine = test_engine();
    let mut side_rx = park_running_run(&engine, &side, "run-depth").await;
    let stopped = engine.cancel_session(&side).await.expect("cancel_session");
    assert_eq!(stopped.as_deref(), Some("run-depth"));
    assert!(side_rx.recv().await.is_some());
}

// ---------------------------------------------------------------------------
// Failing before admission
// ---------------------------------------------------------------------------

/// A seeding failure has to reach the user, and this is the frame that does it.
///
/// Two halves, and they are asserted separately on purpose. This one is the
/// producer: the frame is terminal, carries the sanitized receipt, and names the
/// **main** session — the conversation the person is looking at, and the only
/// key the delivery filter can resolve, since no `RunAccepted` has seeded the
/// run→session index and the side row may not exist yet.
#[tokio::test]
async fn a_pre_admission_failure_announces_itself_on_the_main_session() {
    let engine = test_engine();
    let emitter = super::tests::TestEmitter::new();
    let main = SessionKey::main("btw-fail");

    let mut request = gate_test_request(&main, "run-btw-fail");
    request.input = "/btw why?".to_string();
    stamp_btw(&request.input, &mut request.metadata);
    redirect_to_side_session(&mut request);
    assert_ne!(
        request.session_key.to_key_string(),
        main.to_key_string(),
        "the fixture must be past the redirect, or this asserts nothing about which key is chosen"
    );

    engine
        .emit_pre_admission_run_error(
            &emitter,
            &request.run_id,
            &main,
            &request,
            &crate::gateway::execution_engine::ExecutionError::Failed(
                "btw: seed cursor is not an event seq".to_string(),
            ),
        )
        .await;

    let events = emitter.get_events().await;
    let frame = events
        .iter()
        .find_map(|e| match e {
            crate::gateway::event_emitter::StreamEvent::RunError {
                run_id,
                session_key,
                error_code,
                ..
            } => Some((run_id.clone(), session_key.clone(), error_code.clone())),
            _ => None,
        })
        .expect("a pre-admission failure must put a terminal frame on the wire");

    assert_eq!(frame.0, request.run_id);
    assert_eq!(
        frame.1.as_deref(),
        Some(main.to_key_string().as_str()),
        "the receipt must be charged to the conversation the user is looking at, \
         not to a derived session their client has never heard of"
    );
    assert!(frame.2.is_some(), "the receipt carries a machine code");
}

/// ...and this one is the wiring: the seed failure path actually goes through
/// that producer rather than returning bare.
///
/// Source-level because the failure it guards is not reachable from a test
/// engine — `seed_side_session` degrades to a warn without an orchestrator, so
/// no in-process fixture can make it return `Err`. What makes this more than a
/// spelling check is that it is the *shape* that was wrong: the original wrote
/// `…await?`, and a bare `?` here is invisible on every surface, because both
/// delivery wrappers decline to report an attempt that "ran" on the assumption
/// the engine already did.
#[test]
fn a_seeding_failure_cannot_return_without_announcing_itself() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/gateway/execution_engine/execute.rs");
    let text = std::fs::read_to_string(&path).expect("execute.rs");
    let production = text
        .replace('\r', "")
        .split("#[cfg(test)]")
        .next()
        .unwrap_or_default()
        .to_string();

    assert!(
        !production.contains("self.seed_side_session(main, &request).await?"),
        "a bare `?` here returns from `execute()` before `RunAccepted` with \
         nothing on the wire — the failure is invisible on every surface"
    );

    // The handling block, bounded by its own braces rather than by a character
    // window: a fixed-width scan reads into whatever happens to sit after it.
    let call = production
        .find("if let Err(e) = self.seed_side_session(")
        .expect("the seed call moved; this guard's green would mean nothing");
    let open = production[call..]
        .find('{')
        .expect("the handling block opens")
        + call;
    let mut depth = 0usize;
    let mut end = open;
    for (i, c) in production[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = open + i;
                    break;
                }
            }
            _ => {}
        }
    }
    let block = &production[open..=end];
    assert!(
        block.contains("emit_pre_admission_run_error"),
        "the seeding-failure path must emit its terminal frame before it \
         returns; block was:\n{block}"
    );
    // ...and charged to the MAIN session. The producer guard above supplies
    // `&main` itself, so it pins the producer's contract and not this call
    // site's argument — which leaves the one thing the whole fix is about
    // unpinned. A one-token "simplification" to `&request.session_key` keeps
    // every other assertion green while sending the receipt to a derived
    // session the client has never heard of and the delivery filter cannot
    // resolve: the invisible failure, fully reinstated. Same lesson as
    // `execute_redirects_before_it_admits`, one function down — the placement
    // is correct and well commented, and a comment is not a guard.
    assert!(
        block.contains("&run_id, main,"),
        "the terminal frame must be charged to the MAIN session (`main`), not \
         to the side session the run was redirected onto; block was:\n{block}"
    );
    assert!(
        !block.contains("request.session_key"),
        "by the time this block runs, `request.session_key` IS the side \
         session — charging the receipt to it puts the only notice of a \
         permanently sticky failure where nobody is looking; block was:\n{block}"
    );
}

// ---------------------------------------------------------------------------
// The arrival lane
// ---------------------------------------------------------------------------

/// The arrival lane is registered **before** the engine is ever entered, and
/// only its FIFO front ticket attempts delivery — so a side question keyed on
/// the conversation it was typed in waits behind whatever is **waiting** there.
///
/// A *waiting* message is what this fixture builds, and it is deliberately not
/// a running one: a running run holds no ticket at all (`try_claim` calls
/// `mark_admitted`, which withdraws it — that is how `Steer` and `Interrupt`
/// reach the engine mid-run). The lane is a waiting room, not a run registry,
/// so "parks behind the running main turn" is a scenario production does not
/// produce and this test does not claim.
///
/// What it does claim is shape 1 of the real defect: the side lane is
/// independent of main-lane occupancy. Shape 2 — the run's own ticket stranded
/// on the main lane for the whole side question, because `mark_admitted` is
/// keyed on the session the run *claimed* — is covered by construction rather
/// than here: `register_run` is the only arrival entry point and it derives the
/// lane from the same function `admit_run` will claim on
/// (`no_production_arrival_path_picks_its_own_lane_key`).
///
/// Driven against the real `busy_queue` with the real key derivation. The
/// control arm is the same message with the stamp removed: it must time out, or
/// this test would pass on a lane that never blocks anyone.
#[tokio::test]
async fn a_side_question_does_not_queue_behind_a_waiting_main_message() {
    use crate::gateway::busy_queue::{
        deliver_with_ticket, register, BusyQueueConfig, DeliveryOutcome,
    };

    let cfg = BusyQueueConfig {
        max_per_session: 8,
        // Short: the control arm below waits it out on purpose.
        max_wait_secs: 1,
        wake_fallback_secs: 3600,
    };
    let main = SessionKey::main("btw-lane");
    let main_lane = main.to_key_string();

    // A message WAITING on the main lane, holding its front ticket for the whole
    // test — a `queue`-mode follower, a deferred steer, a burst. Not a running
    // run: that one has no ticket left to hold.
    let _main_waiter = register(&main_lane, cfg.max_per_session, "run-waiting")
        .expect("the waiting message takes the front ticket");

    let mut side_question = gate_test_request(&main, "run-btw");
    side_question.input = "/btw why?".to_string();
    stamp_btw(&side_question.input, &mut side_question.metadata);
    let side_lane =
        crate::gateway::btw::execution_session(&side_question.session_key, &side_question.metadata)
            .to_key_string();

    let ticket = register(&side_lane, cfg.max_per_session, "run-btw")
        .expect("the side question takes a ticket on its own lane");
    let outcome = deliver_with_ticket(ticket, cfg, &mut || async { Ok(()) }).await;
    assert!(
        matches!(outcome, DeliveryOutcome::Executed(Ok(()))),
        "a side question must be delivered while a waiting message holds the \
         main lane, got {outcome:?}"
    );

    // Control: the same message with no stamp resolves to the main lane, where
    // the occupied front ticket makes it wait — and it is that difference, not
    // anything else about this rig, that the assertion above is reading.
    let mut ordinary = gate_test_request(&main, "run-ordinary");
    ordinary.input = "why?".to_string();
    stamp_btw(&ordinary.input, &mut ordinary.metadata);
    let ordinary_lane =
        crate::gateway::btw::execution_session(&ordinary.session_key, &ordinary.metadata)
            .to_key_string();
    assert_eq!(ordinary_lane, main_lane);

    let ticket = register(&ordinary_lane, cfg.max_per_session, "run-ordinary")
        .expect("the ordinary message takes a ticket");
    let outcome = deliver_with_ticket(ticket, cfg, &mut || async { Ok(()) }).await;
    assert!(
        matches!(outcome, DeliveryOutcome::TimedOut),
        "an ordinary message must wait behind the main lane's waiting ticket — \
         if it does not, this lane is not the thing the side question needed to \
         escape, and the assertion above proves nothing. Got {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// The redirect on its own
// ---------------------------------------------------------------------------

/// The derivation is the shared one, and the main key comes back so the seed
/// has a source. Asserting the returned key rather than re-deriving it here
/// keeps this from being a second definition of what the side key is.
#[test]
fn a_stamped_request_is_moved_onto_the_derived_side_key() {
    let main = SessionKey::main("assistant");
    let mut request = gate_test_request(&main, "run-1");
    request.input = "/btw why?".to_string();
    stamp_btw(&request.input, &mut request.metadata);

    let returned = redirect_to_side_session(&mut request).expect("a side question is redirected");

    assert_eq!(returned.to_key_string(), main.to_key_string());
    assert_eq!(
        request.session_key.to_key_string(),
        crate::gateway::btw::side_key_for(&main).to_key_string()
    );
    assert_ne!(request.session_key.to_key_string(), main.to_key_string());
}

/// Asking twice must be free.
///
/// The property is structural rather than merely twice-true: after the first
/// redirect the key IS a side key, so `execution_session` is at a fixed point
/// and asks 3, 4, …n return it unchanged. This exercises the second ask, which
/// is the one a real re-entry performs.
///
/// `execute()` is re-enterable, and the one re-entry path that carries metadata
/// verbatim (`steering::build_steering_rescue_request`) clones both the stamp
/// and the already-redirected key. A derivation that ran again there would
/// produce the side key OF the side key: a third session nothing can address,
/// retire or list, seeded from the side transcript and answered into where no
/// one is looking. The rescue builder strips re-entry residue by NAME, so it
/// cannot be relied on to learn about a new marker — and stripping this one
/// would drop the ceiling with it.
#[test]
fn redirecting_an_already_redirected_request_is_a_no_op() {
    let main = SessionKey::main("assistant");
    let mut request = gate_test_request(&main, "run-1");
    request.input = "/btw why?".to_string();
    stamp_btw(&request.input, &mut request.metadata);

    redirect_to_side_session(&mut request).expect("the first ask redirects");
    let after_first = request.session_key.clone();

    // Exactly what the rescue builder hands back: same metadata, same key.
    assert!(
        redirect_to_side_session(&mut request).is_none(),
        "a request already on its side session must not be redirected again"
    );
    assert_eq!(
        request.session_key.to_key_string(),
        after_first.to_key_string(),
        "the second ask derived a side key OF the side key"
    );
    // The stamp survives, because it is what carries the read-only ceiling into
    // the rescue turn.
    assert!(request
        .metadata
        .contains_key(crate::gateway::btw::BTW_METADATA_KEY));
}

/// An unstamped request is left alone — byte-identical, including for input
/// that merely looks like a command.
#[test]
fn an_ordinary_request_is_untouched() {
    let main = SessionKey::main("assistant");
    for input in ["hello", "/help", "/btwlike this", "/btw"] {
        let mut request = gate_test_request(&main, "run-1");
        request.input = input.to_string();
        stamp_btw(&request.input, &mut request.metadata);

        assert!(
            redirect_to_side_session(&mut request).is_none(),
            "{input} is not a side question"
        );
        assert_eq!(
            request.session_key.to_key_string(),
            main.to_key_string(),
            "{input} must run where it was typed"
        );
    }
}

/// The redirect reads the metadata key, not the text. A request some other
/// surface already stamped is redirected even though its input no longer looks
/// like a command — one resolver, one answer.
#[test]
fn the_redirect_reads_the_stamp_not_the_text() {
    let main = SessionKey::main("assistant");
    let mut request = gate_test_request(&main, "run-1");
    request.input = "why?".to_string();
    request.metadata.insert(
        crate::gateway::btw::BTW_METADATA_KEY.to_string(),
        "why?".to_string(),
    );

    assert!(redirect_to_side_session(&mut request).is_some());
    assert_eq!(
        request.session_key.to_key_string(),
        crate::gateway::btw::side_key_for(&main).to_key_string()
    );
}

// ---------------------------------------------------------------------------
// From an inbound channel message
// ---------------------------------------------------------------------------
//
// Everything above starts at `execute()`, from a request whose `input` this
// file set by hand. That is one hand-built half: it proves the engine honours a
// `/btw` it is handed, and says nothing about whether any channel hands it one.
//
// For most of this feature's life no channel did. An inbound-router special
// case claimed `/btw` first, stripped the prefix and substituted a fresh
// `SessionKey::ephemeral` uuid, so the request that reached `execute()` was
// indistinguishable from an ordinary message: `stamp_btw` saw no prefix, the
// ceiling never applied, and the derived side session — with its seeding, its
// own lane and its retirement — was never addressed. Every test in this file
// stayed green throughout, because each was handed the input that special case
// never produced.
//
// So these start one layer further out: at `handle_message`, with an
// `InboundMessage` exactly as a channel adapter delivers one, and carry the
// request the router actually produced the rest of the way down.

use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry};
use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};
use crate::gateway::event_emitter::{EventEmitter, StreamEvent};
use crate::gateway::inbound_router::ChannelPermissionLevel;
use crate::gateway::{
    ChannelRegistry, DmPolicy, ExecutionAdapter, InboundMessageRouter, RouterChannelConfig,
    RoutingConfig, RunStatus, SqlitePairingStore,
};
use crate::tool_metadata::{ToolCatalog, ToolSource, UnifiedTool};

/// The router's own default agent id. Registering the rig's agent under any
/// other name makes `resolve_agent_id_async` fall back to this one and every
/// delivery dies as `AgentNotFound` — which is the deployment default, so the
/// rig uses it rather than working around it.
const RIG_AGENT: &str = "main";
const RIG_CHANNEL: &str = "telegram";

/// Records the `RunRequest` the router hands the engine, and — when armed —
/// answers through the emitter the router built for it.
///
/// The boundary matters: this is the last point at which the request is still
/// the router's work and not the engine's, so what it captures is exactly what
/// the channel path produced.
///
/// The script is empty by default, and has to be. Every other test on this rig
/// asserts on the request or on the router's *own* replies, and one of them
/// (`a_claimed_btw_reaches_the_engine_on_the_same_rig`) asserts that the router
/// sent nothing at all — an adapter that always answered would put a reply on
/// that wire and make the assertion mean something else.
#[derive(Default)]
struct CapturingAdapter {
    seen: std::sync::Mutex<std::collections::VecDeque<RunRequest>>,
    /// Frames this adapter emits, in order, on the emitter the router built.
    /// Set through the `ChannelRig::*_with` arming methods.
    script: std::sync::Mutex<Vec<RigFrame>>,
}

/// One frame in a [`CapturingAdapter`] script.
///
/// A script rather than three optional fields because the ORDER is what several
/// of these tests are about — the badge belongs on the answer and not on what
/// preceded it, so "what preceded it" has to be expressible.
#[derive(Clone)]
enum RigFrame {
    /// A standalone progress message: `is_intermediate: true`, non-empty. The
    /// emitter delivers these on their own, immediately, well before it has an
    /// answer — the exact traffic the `answering` latch exists to keep unbadged.
    Progress(String),
    /// An answer token: `is_intermediate: false`. Buffered, and — on a channel
    /// that can edit — streamed out as it arrives.
    Chunk(String),
    /// Explicit provider reasoning. The emitter buffers it and, on a streaming
    /// channel, sends it at `RunComplete` as its own `🤔 …` message — beside
    /// the answer, and after the badge latch is already open.
    Reasoning(String),
    /// A failure receipt. Today's drain never sends one after `RunComplete` —
    /// which is exactly why the latch's closing half needs a test that does.
    Error(String),
    /// The terminal frame, carrying `summary.final_response`.
    Complete(Option<String>),
}

impl CapturingAdapter {
    fn take(&self) -> Option<RunRequest> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }
}

#[async_trait::async_trait]
impl ExecutionAdapter for CapturingAdapter {
    async fn execute(
        &self,
        request: RunRequest,
        _agent: Arc<AgentInstance>,
        emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), super::ExecutionError> {
        let script = self
            .script
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(request.clone());
        // The frames a real run emits, in the order a real run emits them.
        // Nothing about the delivery is built here: which emitter this is,
        // whether it streams, whether it marks, and what it sends were all
        // decided by `executor.rs` before this adapter was called.
        for frame in script {
            let event = match frame {
                RigFrame::Progress(text) => StreamEvent::ResponseChunk {
                    run_id: request.run_id.clone(),
                    seq: 0,
                    delta: text.clone(),
                    full_text: text,
                    chunk_index: 0,
                    is_final: false,
                    is_intermediate: true,
                },
                RigFrame::Chunk(text) => StreamEvent::ResponseChunk {
                    run_id: request.run_id.clone(),
                    seq: 0,
                    delta: text.clone(),
                    full_text: text,
                    chunk_index: 0,
                    is_final: false,
                    is_intermediate: false,
                },
                RigFrame::Reasoning(content) => StreamEvent::Reasoning {
                    run_id: request.run_id.clone(),
                    seq: 0,
                    content,
                    is_complete: true,
                },
                RigFrame::Error(error) => StreamEvent::RunError {
                    run_id: request.run_id.clone(),
                    seq: 0,
                    error,
                    error_code: None,
                    session_key: None,
                },
                RigFrame::Complete(final_response) => StreamEvent::RunComplete {
                    run_id: request.run_id.clone(),
                    seq: 0,
                    summary: crate::gateway::event_emitter::RunSummary {
                        final_response,
                        ..Default::default()
                    },
                    total_duration_ms: 0,
                },
            };
            let _ = emitter.emit(event).await;
        }
        Ok(())
    }

    async fn cancel(&self, _run_id: &str) -> Result<(), super::ExecutionError> {
        Ok(())
    }

    async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
        Some(RunStatus {
            run_id: run_id.to_string(),
            state: super::RunState::Completed,
            started_at: None,
            completed_at: None,
            steps_completed: 0,
            current_tool: None,
        })
    }

    async fn active_run_count(&self) -> usize {
        0
    }
}

/// An outbound channel that accepts sends and remembers them.
///
/// **Registering this is not decoration — without it the rig cannot observe the
/// trap it exists to observe.** `try_send_unknown_command_help` has exactly two
/// `false` exits: no suggestions, and `channel_registry.send(...)` returning
/// `Err`. On an empty `ChannelRegistry` every send errors, so the helper always
/// reported "I did not answer" and the router fell through to the agent — the
/// same outcome as the claim, by a mechanism no deployment has. The rig read
/// green and the green meant nothing.
///
/// It records what it was asked to send so a swallowed message is visible as an
/// effect (a "did you mean" reply on the wire), not merely as the absence of a
/// run.
struct RigChannel {
    info: crate::gateway::channel::ChannelInfo,
    state: crate::gateway::channel::ChannelState,
    sent: Arc<std::sync::Mutex<Vec<String>>>,
    edits: Arc<std::sync::Mutex<Vec<String>>>,
}

impl RigChannel {
    /// `editing` is opt-in, and the default (`false`) is load-bearing.
    ///
    /// `apply_channel_capabilities` floors `stream_enabled` on a channel that
    /// cannot edit, so a non-editing rig channel routes a run's answer through
    /// the emitter's outbound chokepoint, and an editing one routes it through
    /// the `StreamingController` instead — two different delivery arms, two
    /// different application points for the badge. Turning this on globally
    /// would silently move every existing rig test onto the other arm.
    fn new(
        sent: Arc<std::sync::Mutex<Vec<String>>>,
        edits: Arc<std::sync::Mutex<Vec<String>>>,
        editing: bool,
    ) -> Self {
        Self {
            info: crate::gateway::channel::ChannelInfo {
                id: ChannelId::new(RIG_CHANNEL),
                name: RIG_CHANNEL.to_string(),
                channel_type: RIG_CHANNEL.to_string(),
                status: crate::gateway::channel::ChannelStatus::Connected,
                capabilities: crate::gateway::channel::ChannelCapabilities {
                    editing,
                    stream_protocol: if editing {
                        crate::gateway::channel::StreamProtocol::EditBased
                    } else {
                        crate::gateway::channel::StreamProtocol::None
                    },
                    ..Default::default()
                },
            },
            state: crate::gateway::channel::ChannelState::new(8),
            sent,
            edits,
        }
    }
}

#[async_trait::async_trait]
impl crate::gateway::channel::Channel for RigChannel {
    fn info(&self) -> &crate::gateway::channel::ChannelInfo {
        &self.info
    }
    fn state(&self) -> &crate::gateway::channel::ChannelState {
        &self.state
    }
    async fn start(&mut self) -> crate::gateway::channel::ChannelResult<()> {
        Ok(())
    }
    async fn stop(&mut self) -> crate::gateway::channel::ChannelResult<()> {
        Ok(())
    }
    async fn send(
        &self,
        message: crate::gateway::channel::OutboundMessage,
    ) -> crate::gateway::channel::ChannelResult<crate::gateway::channel::SendResult> {
        self.sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message.text.clone());
        Ok(crate::gateway::channel::SendResult {
            message_id: MessageId::new("rig-out"),
            timestamp: chrono::Utc::now(),
        })
    }

    /// Records the settled text of an edit-based stream.
    ///
    /// The default body is an `Err`, and an emitter that streams then settles
    /// drops that error on the floor — so without this the settling rewrite,
    /// which is where the badge goes on this arm, would be invisible AND
    /// unobservably broken.
    async fn edit(
        &self,
        _conversation_id: &ConversationId,
        _message_id: &MessageId,
        new_text: &str,
    ) -> crate::gateway::channel::ChannelResult<()> {
        self.edits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(new_text.to_string());
        Ok(())
    }
}

/// A router wired the way a channel deployment wires one: an agent registry, an
/// execution adapter, a `CommandParser` over a real builtin catalog, and — the
/// part that took a review round to get right — a **registered, connected
/// outbound channel**, so the router's own replies succeed the way they do in
/// production.
struct ChannelRig {
    router: InboundMessageRouter,
    adapter: Arc<CapturingAdapter>,
    agent: Arc<AgentInstance>,
    temp: tempfile::TempDir,
    next_id: std::sync::atomic::AtomicU64,
    /// Everything the router sent back to the channel. A message the router
    /// answered itself leaves a trace here and no trace in `adapter`.
    replies: Arc<std::sync::Mutex<Vec<String>>>,
    /// The settled text of every edit-based stream. Empty unless the rig was
    /// built [`ChannelRig::editing`].
    edits: Arc<std::sync::Mutex<Vec<String>>>,
}

impl ChannelRig {
    /// The ordinary rig: a channel that cannot edit, so a run's answer is
    /// delivered as one message through the emitter's outbound chokepoint.
    async fn new(catalog_resolves_btw: bool) -> Self {
        Self::build(catalog_resolves_btw, false).await
    }

    /// The same rig over an **edit-capable** channel, which is what turns
    /// streaming on (`apply_channel_capabilities` widens on `EditBased` and
    /// floors on `editing`). A run's answer then arrives as an initial send
    /// plus a settling rewrite, and the badge rides the rewrite — a different
    /// arm of `RunComplete` from the one `new()` exercises, and the one that
    /// `output_mode = "typewriter"` makes the default on any real channel that
    /// can edit.
    async fn editing() -> Self {
        Self::build(false, true).await
    }

    /// `catalog_resolves_btw` seeds the catalog with a tool literally named
    /// `btw`. That is not the shipped state — `no_shipped_command_word_resolves_as_a_side_question`
    /// pins that it is not — it is the world in which falling through to the
    /// unified interception has an observable consequence, which is what makes
    /// the ordering here testable rather than merely true today. Registering
    /// `btw` for discovery would create exactly this world on every surface;
    /// see that guard for why the listing was not shipped.
    async fn build(catalog_resolves_btw: bool, editing: bool) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");

        let sessions = Arc::new(
            crate::gateway::session_manager::SessionManager::new(
                crate::gateway::session_manager::SessionManagerConfig {
                    db_path: temp.path().join("sessions.db"),
                    ..Default::default()
                },
            )
            .expect("session manager"),
        );
        let agents = Arc::new(AgentRegistry::new());
        agents
            .register(
                AgentInstance::new(
                    AgentInstanceConfig {
                        agent_id: RIG_AGENT.to_string(),
                        workspace: temp.path().join("workspace"),
                        agent_dir: temp.path().join("agent"),
                        ..Default::default()
                    },
                    sessions,
                )
                .expect("agent instance"),
            )
            .await;
        let agent = agents.get(RIG_AGENT).await.expect("registered agent");

        let catalog = Arc::new(ToolCatalog::new());
        catalog.register_builtin_tools().await;
        if catalog_resolves_btw {
            catalog
                .register_with_conflict_resolution(UnifiedTool::new(
                    "builtin:btw",
                    "btw",
                    "a tool that happens to be called btw",
                    ToolSource::Builtin,
                ))
                .await;
        }

        let replies: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let edits: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let channels = Arc::new(ChannelRegistry::new());
        channels
            .register(Box::new(RigChannel::new(
                replies.clone(),
                edits.clone(),
                editing,
            )))
            .await;

        let adapter = Arc::new(CapturingAdapter::default());
        let execution: Arc<dyn ExecutionAdapter> = adapter.clone();
        let mut router = InboundMessageRouter::with_execution(
            channels,
            Arc::new(SqlitePairingStore::in_memory().expect("pairing store")),
            RoutingConfig::default(),
            agents,
            execution,
        )
        .with_command_parser(Arc::new(crate::command::CommandParser::new(catalog)));
        router.register_channel_config(
            RIG_CHANNEL,
            RouterChannelConfig {
                dm_policy: DmPolicy::Open,
                require_mention: false,
                // The channel is at its most privileged tier on purpose. At the
                // default `Chat` tier the router stamps `caller_role: guest`,
                // which clamps every turn to `Ask` — and with no approval
                // channel wired, an ordinary message is refused too. The
                // refusal under test would then be indistinguishable from the
                // ambient one, and the control arm below could not write.
                // At `Config` an ordinary turn really does execute, so the side
                // question's refusal is the side question's alone.
                permission_level: ChannelPermissionLevel::Config,
                ..Default::default()
            },
        );

        Self {
            router,
            adapter,
            agent,
            temp,
            next_id: std::sync::atomic::AtomicU64::new(0),
            replies,
            edits,
        }
    }

    /// Deliver `text` as an inbound DM and return the request the router handed
    /// the engine.
    ///
    /// Polls rather than awaits because `execute_for_context` takes the busy
    /// lane's FIFO ticket synchronously and then `tokio::spawn`s the delivery —
    /// there is no handle to join, by design.
    async fn deliver(&self, text: &str) -> RunRequest {
        let n = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let msg = InboundMessage {
            id: MessageId::new(format!("m-{n}")),
            channel_id: ChannelId::new(RIG_CHANNEL),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("u1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        };
        self.router
            .handle_message(msg)
            .await
            .expect("the router must not error on a plain DM");

        for _ in 0..400 {
            if let Some(request) = self.adapter.take() {
                return request;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!(
            "{text:?} never reached the engine — the router answered it itself. \
             It replied: {:?}",
            self.replies_so_far()
        );
    }

    /// Arm the adapter with the frames every subsequent run will emit, in
    /// order, on the emitter `executor.rs` built.
    ///
    /// Off until asked for: see [`CapturingAdapter::script`].
    fn script(&self, frames: Vec<RigFrame>) {
        *self
            .adapter
            .script
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = frames;
    }

    /// Finish every subsequent run with `text` as its final response, and
    /// nothing before it.
    fn answer_with(&self, text: &str) {
        self.script(vec![RigFrame::Complete(Some(text.to_string()))]);
    }

    /// Emit a standalone progress message, then finish with `answer`.
    ///
    /// The two arrive as two separate channel messages, which is what makes the
    /// `answering` latch observable: the first goes out before the run has an
    /// answer at all.
    fn progress_then_answer(&self, progress: &str, answer: &str) {
        self.script(vec![
            RigFrame::Progress(progress.to_string()),
            RigFrame::Complete(Some(answer.to_string())),
        ]);
    }

    /// Stream `chunk` as the run's answer and then complete with no
    /// `final_response`.
    ///
    /// On an [`ChannelRig::editing`] rig this is the shape that drives
    /// `StreamingController::finalize()` into `StreamAction::Done`: one chunk
    /// long enough to cross `min_initial_chars` is sent as the initial message,
    /// `record_sent` sets `last_edit_len` to the whole buffer, and nothing
    /// arrives after it — so the controller reports there is nothing left to
    /// write, and any badge that depends on a settling rewrite has to ask for
    /// one.
    fn stream_then_complete(&self, chunk: &str) {
        self.script(vec![
            RigFrame::Chunk(chunk.to_string()),
            RigFrame::Complete(None),
        ]);
    }

    /// Answer, then fail — the sequence that shows whether the badge latch was
    /// closed or merely opened.
    fn answer_then_fail(&self, answer: &str, error: &str) {
        self.script(vec![
            RigFrame::Complete(Some(answer.to_string())),
            RigFrame::Error(error.to_string()),
        ]);
    }

    /// The same stream, preceded by explicit provider reasoning.
    ///
    /// On a streaming channel the emitter delivers the reasoning as its own
    /// `🤔 …` message during `RunComplete` — i.e. after the badge latch has
    /// already opened, which is the one place a call site has to say "this is
    /// beside the answer, not the answer".
    fn reason_then_stream(&self, reasoning: &str, chunk: &str) {
        self.script(vec![
            RigFrame::Reasoning(reasoning.to_string()),
            RigFrame::Chunk(chunk.to_string()),
            RigFrame::Complete(None),
        ]);
    }

    /// Block until something reaches the channel, and return the first thing
    /// that did.
    ///
    /// Polls for the same reason [`ChannelRig::deliver`] does — the delivery is
    /// `tokio::spawn`ed with no handle to join — and separately from `deliver`,
    /// because `deliver` returns as soon as the adapter has *recorded* the
    /// request, which is one statement before it answers on the emitter.
    async fn wait_for_reply(&self) -> String {
        self.wait_for_replies(1).await.remove(0)
    }

    /// Block until at least `n` messages have reached the channel.
    async fn wait_for_replies(&self, n: usize) -> Vec<String> {
        for _ in 0..400 {
            let seen = self.replies_so_far();
            if seen.len() >= n {
                return seen;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!(
            "the run sent {} message(s), expected at least {n} — the adapter was \
             armed but the emitter did not deliver. Saw: {:?}",
            self.replies_so_far().len(),
            self.replies_so_far()
        );
    }

    /// Block until at least one edit has landed, and return the settled text.
    ///
    /// Only an [`ChannelRig::editing`] rig can produce these; on the ordinary
    /// rig `Channel::edit`'s default body is an `Err` the emitter drops, so a
    /// wait here would time out rather than mislead.
    async fn wait_for_edit(&self) -> String {
        for _ in 0..400 {
            if let Some(last) = self
                .edits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .last()
                .cloned()
            {
                return last;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!(
            "no settling edit reached the channel. The run streamed {:?} and \
             edited nothing.",
            self.replies_so_far()
        );
    }

    /// What the router sent back to the channel instead of running the agent.
    ///
    /// Only meaningful because the rig registers a channel that accepts sends;
    /// on an empty registry every one of these fails and the router's own
    /// answers are invisible.
    fn replies_so_far(&self) -> Vec<String> {
        self.replies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Deliver `text` and report whether it reached the engine at all.
    ///
    /// The twin of [`ChannelRig::deliver`] for the case where being swallowed
    /// is the outcome under test rather than a test failure.
    async fn try_deliver(&self, text: &str) -> Option<RunRequest> {
        let n = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let msg = InboundMessage {
            id: MessageId::new(format!("m-{n}")),
            channel_id: ChannelId::new(RIG_CHANNEL),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("u1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        };
        self.router
            .handle_message(msg)
            .await
            .expect("the router must not error on a plain DM");

        for _ in 0..40 {
            if let Some(request) = self.adapter.take() {
                return Some(request);
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        None
    }

    /// Carry a request the router produced the rest of the way down, through
    /// the same three calls the agent loop makes.
    ///
    /// `stamp_btw` and `redirect_to_side_session` are `execute()`'s first two
    /// statements and are run here in that order; on this path the stamp is
    /// already present, so the first is a no-op — which is the point, and is
    /// asserted by the caller before this is reached.
    async fn tools_for(&self, request: &RunRequest) -> (Arc<dyn ToolService>, SessionKey) {
        let engine = test_engine();
        let mut request = request.clone();
        stamp_btw(&request.input, &mut request.metadata);
        redirect_to_side_session(&mut request);

        let permissions = engine.resolve_turn_permissions(&request, &self.agent).await;
        let turn_context = permissions.turn_context(&request, &request.run_id, false);
        let tools = build_request_tool_service(
            write_registry(),
            BTreeSet::new(),
            None,
            Some(turn_context),
            None,
            request.session_key.to_key_string(),
            permissions.explicit.clone(),
            permissions.tier,
            false,
            &[],
            false,
            crate::tools::scoped::DeferredTools::empty(),
            None,
        );
        (tools, request.session_key)
    }
}

/// The arrival property, and the one the deleted special case broke on both
/// counts.
///
/// Nothing here is constructed by the test but the user's sentence: the
/// metadata key is whatever `executor.rs` stamped, and the session key is
/// whatever the router resolved for this conversation — compared against an
/// ordinary message in the same conversation rather than against a key this
/// test derives, because a derivation here would just be a second copy of the
/// router's.
#[tokio::test]
async fn a_channel_side_question_arrives_stamped_on_the_conversations_own_key() {
    let rig = ChannelRig::new(false).await;

    let ordinary = rig.deliver("what is the plan?").await;
    let side = rig.deliver("/btw what does epoch mean here?").await;

    assert!(
        side.metadata
            .contains_key(crate::gateway::btw::BTW_METADATA_KEY),
        "a channel `/btw` must arrive stamped — without the stamp there is no \
         read-only ceiling and no side session, and nothing downstream can tell \
         it apart from an ordinary message"
    );
    assert!(
        !ordinary
            .metadata
            .contains_key(crate::gateway::btw::BTW_METADATA_KEY),
        "the control must be unstamped, or the assertion above proves nothing"
    );

    assert_eq!(
        side.session_key.to_key_string(),
        ordinary.session_key.to_key_string(),
        "a side question must reach the engine on the conversation's OWN key: \
         `btw::execution_session` derives the persistent side session from it, \
         and a substituted key derives a session nothing can seed or retire"
    );

    assert!(
        side.input.trim_start().starts_with("/btw"),
        "the `/btw` prefix must survive the router: `stamp_btw` is the only \
         thing that recognises a side question, and it reads the text, got {:?}",
        side.input
    );
    assert!(
        !side
            .metadata
            .contains_key(crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY),
        "a side question must not carry a slash-command mode: that key sends \
         `execute()` down the fast path, which dispatches on the raw tool \
         registry and never builds the ScopedToolService the ceiling lives in"
    );
}

/// The same message, carried the rest of the way: a `/btw` typed into a channel
/// cannot change anything, measured on the filesystem.
///
/// This is `a_side_question_cannot_write_a_file` with its one hand-built half
/// replaced by a real inbound message. Under the deleted special case it fails
/// at the first assertion below rather than the last — the file gets written,
/// because the request that arrived was not a side question at all.
#[tokio::test]
async fn a_channel_side_question_cannot_write_a_file() {
    let rig = ChannelRig::new(false).await;
    let proof: PathBuf = rig.temp.path().join("proof.txt");

    let request = rig
        .deliver("/btw create a file called proof.txt with the word hi in it")
        .await;
    assert!(
        request
            .metadata
            .contains_key(crate::gateway::btw::BTW_METADATA_KEY),
        "the router did not stamp it, so what follows would be testing this \
         test's own input"
    );

    let (tools, executes_on) = rig.tools_for(&request).await;
    assert_eq!(
        executes_on.to_key_string(),
        crate::gateway::btw::side_key_for(&request.session_key).to_key_string(),
        "the run must move to the side session derived from the conversation"
    );

    let outcome = tools.execute("file_write", write_call(&proof)).await;
    assert!(
        !proof.exists(),
        "a channel side question wrote {} — the read-only ceiling did not reach \
         this surface",
        proof.display()
    );
    let refusal = outcome
        .expect_err("a mutating tool must be refused during a side question")
        .to_string();
    assert!(
        refusal.contains("/btw side question"),
        "the refusal must name the side question, got: {refusal}"
    );
}

/// The control. Same rig, same conversation, same tool — only the leading
/// `/btw` removed — must still write, or the refusal above could be any rig
/// that refuses everything.
#[tokio::test]
async fn an_ordinary_channel_message_still_writes_the_file() {
    let rig = ChannelRig::new(false).await;
    let proof: PathBuf = rig.temp.path().join("proof.txt");

    let request = rig
        .deliver("create a file called proof.txt with the word hi in it")
        .await;

    let (tools, executes_on) = rig.tools_for(&request).await;
    assert_eq!(
        executes_on.to_key_string(),
        request.session_key.to_key_string(),
        "an ordinary channel message stays on the conversation it was typed in"
    );

    tools
        .execute("file_write", write_call(&proof))
        .await
        .expect("an ordinary channel turn writes files");
    assert!(proof.exists(), "the control arm must actually write");
}

/// The ordering, in the world where it has consequences.
///
/// `/btw` is claimed **before** the unified slash interception. Below that
/// point there are two ways to lose it and they fail differently: a name the
/// `CommandParser` resolves is serialized into `SLASH_COMMAND_MODE_KEY` and
/// taken by the engine's fast path (no `ScopedToolService`, no ceiling); a name
/// it does not resolve reaches `try_send_unknown_command_help`, which answers
/// "did you mean …?" and returns without running the agent at all.
///
/// This rig gives the catalog a tool called `btw` so the first of those has an
/// observable effect. Nothing ships in that state — but "nothing currently
/// resolves as `btw`" is a fact about today's tool list, not a property of the
/// router, and the claim has to hold either way.
#[tokio::test]
async fn the_router_claims_btw_ahead_of_a_catalog_that_could_resolve_it() {
    let rig = ChannelRig::new(true).await;
    let request = rig.deliver("/btw what does epoch mean here?").await;

    assert!(
        !request
            .metadata
            .contains_key(crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY),
        "a resolvable `/btw` was sent through the slash fast path — that path \
         never builds the ScopedToolService the read-only ceiling lives in"
    );
    assert!(
        request
            .metadata
            .contains_key(crate::gateway::btw::BTW_METADATA_KEY),
        "the side-question stamp must survive a catalog that knows the name"
    );
    assert!(
        request.input.trim_start().starts_with("/btw"),
        "the prefix must survive, got {:?}",
        request.input
    );
}

/// The claim is load-bearing **today**, and this pins the fact that says so.
///
/// I reported the opposite in review round 1: that `suggest_commands("btw", 3)`
/// found nothing on the shipped catalog, so a fallen-through `/btw` would reach
/// the agent anyway and deleting the claim was a latent break. That was wrong,
/// and the way it was wrong is worth keeping. I walked the tool **names**
/// looking for one within two edits of `btw` and found none — but
/// `suggest_commands` scores canonical names **and aliases**
/// (`registry/query.rs`), and `session_new` carries the alias `new`:
/// `levenshtein("new", "btw") == 2`, and with `max(len) == 3 <= 6` the threshold
/// is 2. A hit. Enumerating one axis of a two-axis search and reporting the
/// result as the search's answer is the same列举法 shape this file's other
/// guards exist to prevent.
///
/// So Trap 2 fires now: without the claim, a channel `/btw why is X slow?` is
/// answered `Unknown command /btw. Did you mean: /session_new?` and the question
/// is thrown away. `an_unclaimed_btw_is_swallowed_by_the_did_you_mean_helper`
/// demonstrates that as an effect; this pins the input that makes it true, so
/// the day the alias table changes the record changes with it rather than
/// quietly becoming a story about a world that no longer exists.
///
/// Deliberately **not** asserting *which* command is suggested: the value under
/// test is "the fall-through has something to say", not `session_new`.
#[tokio::test]
async fn the_fall_through_has_a_near_match_for_btw_so_the_claim_is_load_bearing() {
    let catalog = ToolCatalog::new();
    catalog.register_builtin_tools().await;

    let suggestions = catalog.suggest_commands("btw", 3).await;
    assert!(
        !suggestions.is_empty(),
        "`btw` has no near-match in the shipped catalog any more. That makes the \
         router's claim look optional — it is not: it is also what keeps a \
         resolvable `/btw` out of the slash fast path. Re-read \
         `the_router_claims_btw_ahead_of_a_catalog_that_could_resolve_it` before \
         relaxing anything here, and correct the round-1 review record, which \
         states this set is non-empty."
    );
}

/// The trap, as an effect: an unclaimed `/btw` is thrown away.
///
/// This is the arm the round-1 rig could not reach. `try_send_unknown_command_help`
/// returns `false` — "I did not answer, fall through to the agent" — on exactly
/// two conditions: no suggestions, and the outbound send failing. The rig used
/// to register no channel, so the second was always true, the router always fell
/// through, and four channel tests stayed green with the claim disabled. That
/// green was reported as a fact about production. It was a fact about the rig.
///
/// Here the rig has a connected channel, so the helper behaves as it does on a
/// deployment, and the swallow is observable in the only two places it shows: no
/// run reached the engine, and a "did you mean" reply went out on the wire.
///
/// The fall-through is exercised **without disabling anything**: a bare `/btw`
/// has an empty body, `BtwTurn::resolve` rejects it by design, so it takes the
/// identical path a body-carrying `/btw` would take the moment the claim is
/// removed.
#[tokio::test]
async fn an_unclaimed_btw_is_swallowed_by_the_did_you_mean_helper() {
    let rig = ChannelRig::new(false).await;

    let reached = rig.try_deliver("/btw").await;

    assert!(
        reached.is_none(),
        "an unclaimed `/btw` reached the engine — then the fall-through is \
         harmless and this test is guarding nothing. Check whether \
         `try_send_unknown_command_help` still answers, and whether the rig's \
         channel is still registered."
    );
    let replies = rig.replies_so_far();
    assert!(
        replies.iter().any(|r| r.contains("Unknown command")),
        "the router neither ran the agent nor answered: {replies:?}"
    );
}

/// The control for the test above: the same rig, the same helper, a `/btw` that
/// **is** claimed — reaches the engine.
///
/// Without this arm, a rig that swallowed everything (a broken agent registry, a
/// permission denial, a channel config typo) would look exactly like a working
/// claim.
#[tokio::test]
async fn a_claimed_btw_reaches_the_engine_on_the_same_rig() {
    let rig = ChannelRig::new(false).await;

    let reached = rig.try_deliver("/btw what does epoch mean here?").await;

    assert!(
        reached.is_some(),
        "the claimed side question did not reach the engine; the router replied \
         {:?} instead",
        rig.replies_so_far()
    );
    assert!(
        rig.replies_so_far().is_empty(),
        "the router answered a claimed side question itself: {:?}",
        rig.replies_so_far()
    );
}

/// The marker **arrives**.
///
/// `format_side_answer` has a unit test, and a unit test on a formatter proves
/// the formatter. The thing that can silently stop being true is the wire:
/// nothing here constructs the emitter, chooses the config, or decides whether
/// to mark — `executor.rs` did all three before the adapter was handed the
/// emitter, and what is read at the end is a byte string a registered channel
/// was actually asked to send.
///
/// A side answer arrives in the same conversation as the main run's replies and
/// deliberately does not queue behind them, so it can land between two of them.
/// This assertion is the whole of what makes that legible.
#[tokio::test]
async fn a_channel_side_answer_reaches_the_channel_marked() {
    let rig = ChannelRig::new(false).await;
    rig.answer_with("the file is config.toml");

    rig.deliver("/btw what was that file called?").await;
    let reply = rig.wait_for_reply().await;

    assert!(
        reply.starts_with("💬 "),
        "the side answer reached the channel unmarked: {reply:?}. Check that \
         `executor.rs` still resolves `ReplyEmitterConfig::side_answer` through \
         `BtwTurn::resolve`, and that the emitter still marks at its outbound \
         chokepoint."
    );
    assert!(
        reply.contains("the file is config.toml"),
        "the marker replaced the answer instead of prefixing it: {reply:?}"
    );
}

/// The control for the test above, on the same rig, with the same adapter and
/// the same answer text: an ordinary message is delivered **unmarked**.
///
/// Without it, a marker applied unconditionally — to every reply this emitter
/// ever sends — reads exactly like a working feature.
#[tokio::test]
async fn an_ordinary_channel_answer_reaches_the_channel_unmarked() {
    let rig = ChannelRig::new(false).await;
    rig.answer_with("the file is config.toml");

    rig.deliver("what was that file called?").await;
    let reply = rig.wait_for_reply().await;

    assert_eq!(
        reply, "the file is config.toml",
        "an ordinary reply must reach the channel exactly as the run produced \
         it — a side-answer marker here means the predicate is not reading the \
         run's input at all"
    );
}

/// The badge lands on the **answer**, not on what the side question said while
/// it was working.
///
/// This is the `answering` latch's own guard, and until it existed the latch
/// could be deleted outright — predicate reduced to `config.side_answer` — with
/// the whole suite still green, because every other assertion here looks only at
/// a run whose single message IS its answer.
///
/// A side question can send before it has an answer: an approval prompt, a
/// scratchpad tick, any standalone intermediate chunk. Badging those puts two
/// `💬` messages in one conversation for one side question, which is the
/// interleaving the badge exists to disambiguate, inverted.
#[tokio::test]
async fn the_badge_is_on_the_answer_and_not_on_the_progress_before_it() {
    let rig = ChannelRig::new(false).await;
    rig.progress_then_answer("still looking...", "the file is config.toml");

    rig.deliver("/btw what was that file called?").await;
    let replies = rig.wait_for_replies(2).await;

    assert_eq!(
        replies[0], "still looking...",
        "the progress message a side question sent BEFORE it had an answer was \
         badged. `ReplyEmitter::answering` is what keeps the badge off it; if \
         the latch half of `is_marking()` is gone, this is what it costs."
    );
    assert!(
        replies[1].starts_with("💬 "),
        "the answer itself lost its badge: {:?}",
        replies[1]
    );
}

/// The badge reaches an **edit-based** channel too, on the arm where the last
/// debounced edit already delivered everything.
///
/// `StreamingController::finalize()` answers `Done` when `buffer.len() ==
/// last_edit_len`: the text is on screen, there is nothing left to write, and
/// so — before this was fixed — nothing wrote the badge either. That is not an
/// exotic path. `output_mode` defaults to `"typewriter"` and
/// `apply_channel_capabilities` keeps streaming on for any channel that can
/// edit, so it is the default one.
///
/// It needs an edit-capable rig, and that is the whole reason the escape could
/// exist with the suite green: `ChannelRig::new`'s channel cannot edit, so
/// streaming is floored off and every other arrival test here exercises the
/// outbound chokepoint instead.
#[tokio::test]
async fn a_streamed_side_answer_is_badged_when_it_settles_with_nothing_left_to_write() {
    let rig = ChannelRig::editing().await;
    // Long enough to cross `min_initial_chars` (30) so the controller sends an
    // initial message and records the whole buffer as delivered.
    rig.stream_then_complete("the file you are thinking of is config.toml");

    rig.deliver("/btw what was that file called?").await;
    let settled = rig.wait_for_edit().await;

    assert!(
        settled.starts_with("💬 "),
        "the streamed side answer settled unbadged: {settled:?}. \
         `StreamAction::Done` means no settling rewrite happens on its own — the \
         badge has to ask for one."
    );
    assert!(
        settled.contains("config.toml"),
        "the settling edit replaced the answer instead of badging it: {settled:?}"
    );
}

/// The control for the test above: the same edit-capable rig, the same stream,
/// an ordinary message — and **no edit at all**.
///
/// The fix issues an edit that would otherwise not happen. Asserting only that
/// an ordinary settle is unbadged would pass even if it had started making an
/// extra API call on every streamed reply on every channel that can edit; this
/// asserts the byte-identical no-op that claim rests on.
/// The badge latch closes with the answer, so nothing sent afterwards inherits
/// it.
///
/// `RunComplete` is the terminal frame for this purpose — the only event that
/// carries a run's answer. Today's drain sends exactly one terminal event, so
/// no shipped path emits anything after it; the latch's closing half therefore
/// has no producer to prove it, and a one-way flag would look identical. This
/// test supplies the producer: a failure receipt after the answer, badged if
/// and only if the latch was left open.
///
/// It is not a claim that the sequence happens — it is what makes "the latch is
/// a pair, not a flag" a fact about the code rather than about its comments.
#[tokio::test]
async fn the_badge_latch_closes_with_the_answer() {
    let rig = ChannelRig::new(false).await;
    rig.answer_then_fail("the file is config.toml", "provider hung up");

    rig.deliver("/btw what was that file called?").await;
    let replies = rig.wait_for_replies(2).await;

    assert!(
        replies[0].starts_with("💬 "),
        "the answer lost its badge: {:?}",
        replies[0]
    );
    assert!(
        !replies[1].starts_with("💬 "),
        "a message sent after the answer inherited its badge: {:?}. The latch \
         must be closed by `end_answering`, not left open for the emitter's \
         lifetime.",
        replies[1]
    );
}

/// The reasoning preview is **not** the answer, and does not get the badge —
/// even though it is sent after the latch opens.
///
/// It is the one message this emitter delivers between opening the latch and
/// settling the answer, and it goes out through the same chokepoint. Badging it
/// gives one side question TWO `💬` messages — the second of which is a chain
/// of thought — which is the interleaving the badge exists to disambiguate,
/// inverted. It also falsifies the invariant written on
/// `ReplyEmitter::answering`, and a doc that disagrees with the code is the
/// half a future author will trust.
#[tokio::test]
async fn the_reasoning_preview_is_never_badged_though_it_follows_the_latch() {
    let rig = ChannelRig::editing().await;
    rig.reason_then_stream(
        "weighing two candidates",
        "the file you are thinking of is config.toml",
    );

    rig.deliver("/btw what was that file called?").await;
    let settled = rig.wait_for_edit().await;
    let replies = rig.replies_so_far();

    let preview = replies
        .iter()
        .find(|r| r.contains("weighing two candidates"))
        .unwrap_or_else(|| panic!("the reasoning preview never reached the channel: {replies:?}"));
    assert!(
        preview.starts_with("🤔 "),
        "the reasoning preview was badged: {preview:?}. It travels beside the \
         answer, so it must go out through `send_aside_to_channel`, not the \
         marked path."
    );
    assert!(
        settled.starts_with("💬 "),
        "the answer itself lost its badge: {settled:?}"
    );
}

#[tokio::test]
async fn an_ordinary_streamed_answer_settles_without_an_extra_edit() {
    let rig = ChannelRig::editing().await;
    rig.stream_then_complete("the file you are thinking of is config.toml");

    rig.deliver("what was that file called?").await;
    let sent = rig.wait_for_reply().await;

    assert!(
        sent.contains("config.toml"),
        "the streamed answer never reached the channel: {sent:?}"
    );
    // Give the settle every chance to happen before concluding it did not.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(
        rig.edits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "an ordinary run took the `StreamAction::Done` arm and issued an edit \
         anyway: {:?}. That arm must stay a no-op unless the badge would change \
         the text.",
        rig.edits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    );
}

/// `/help` on a channel really carries the `/btw` line — asserted on the wire,
/// not on the constant.
///
/// The two guards next to `ROUTER_OWNED_HELP_LINES` in `command_handler.rs` both
/// read the constant and neither calls `handle_help`. Delete the `push_str` that
/// appends it and both stay green, `grep` still finds three hits, and `/btw`'s
/// **only** discovery surface disappears without a sound. That is the weaker
/// cousin of 守卫要断言「效果到达了」，不是「调用发生了」 — here neither the effect
/// nor the call was asserted.
///
/// It matters more than a missing test usually does: discovery is not a nicety
/// here. The catalog registration was refused because a listed `/btw` would also
/// be a dispatchable one, and the escape clause was re-taken specifically on the
/// strength of this line existing. If it silently stops being emitted, the
/// refusal loses the thing that justified it.
///
/// Uses the recording channel the rig grew when the arrival tests were fixed:
/// `/help` is answered by the router, so it never reaches the engine, and the
/// reply is only observable because an outbound send actually succeeds.
#[tokio::test]
async fn channel_help_really_carries_the_btw_line() {
    let rig = ChannelRig::new(false).await;

    let reached = rig.try_deliver("/help").await;
    assert!(
        reached.is_none(),
        "`/help` is answered by the router and must not reach the engine"
    );

    let replies = rig.replies_so_far();
    assert!(
        replies.iter().any(|r| r.contains("/btw")),
        "the channel `/help` listing does not mention `/btw`, which is its only \
         discovery surface anywhere — the catalog listing was deliberately \
         refused. Check that `handle_help` still appends \
         `ROUTER_OWNED_HELP_LINES`; the two guards on the constant itself cannot \
         see that append. Replies were: {replies:?}"
    );
}

/// Why `/btw` is not registered in the `ToolCatalog`, and what stops one
/// appearing there by accident.
///
/// `commands.list`, the TUI command tree and `/help` are all rendered from the
/// `ToolCatalog`, and `CommandParser::parse_async` resolves against that same
/// table — there is no listed-but-unresolvable state. So an entry added to make
/// `/btw` discoverable would also make it resolvable, and `execute()`'s own
/// `stamp_slash_mode` safety net resolves every `/`-prefixed input it is handed
/// — on Panel and TUI too, where the router's claim cannot help. The catalog
/// listing would cost the ceiling on every surface.
///
/// Channels get their discovery elsewhere and for free: the router owns its own
/// `/help` answer, so a line is appended there (`handle_help`) with no catalog
/// entry and therefore no resolvability. Panel and TUI stay undiscoverable until
/// the catalog grows a discoverable-but-not-dispatchable bit.
///
/// Stated as a rule rather than as the name `btw`: every shipped command word
/// is asked whether [`BtwTurn::resolve`] — the one resolver — would read it as
/// a side question. A second side-question spelling would be covered without
/// this guard being told about it.
///
/// # The five sources, and what makes the list stay complete
///
/// The first version of this guard named **three** tables and its doc asserted
/// that "no compile-time guard can prevent" a bundled extension shadowing
/// `/btw`. Both halves were wrong in the same way: `crate::bundled::BUNDLED_SKILLS`
/// and `BUNDLED_PLUGINS` are `include_dir!` trees embedded **into the binary**
/// and extracted to `~/.aleph/` on first start, so a bundled extension named
/// `btw` ships out of the box — and the trees are walkable. A sentence saying a
/// thing is impossible had foreclosed its own fix.
///
/// The second version fixed half of that and left the worse half open: it
/// walked bundled **skills** and declared bundled **plugins** out of scope. A
/// skill named `btw` overlays its instructions onto every side question and the
/// ceiling survives; a bundled **plugin tool** named `btw` reaches
/// `execute_direct_tool` through the raw registry with **no ceiling at all**. A
/// declared limit that happens to exclude exactly the worst case is not a limit,
/// it is the fail-soft skip this repo bills for. Both trees are read now.
///
/// So: five sources — the curated catalog entries, `BUILTIN_TOOL_DEFINITIONS`,
/// both halves of `SHORTHAND_ALIASES`, the bundled skills tree, and the bundled
/// plugins tree (its `[[tools]]` and its `commands/`, through the loader's own
/// parsers).
///
/// # A bundled plugin has five components; three cannot shadow a bare verb
///
/// `component_source` resolves skills / commands / agents / hooks / mcp-servers,
/// and the list above accounts for two of them. The other three are not an
/// omission — they are namespaced or are not command words at all, which is a
/// property worth stating rather than a gap worth hiding:
///
/// * **commands** and a plugin's own **skills** register under
///   `namespaced_component_key(plugin_id, name)` → `plugin-id:name`. A slash
///   token containing `:` can never equal a bare `btw`.
/// * **mcp-servers** register their tools as `{server_id}__{tool}`
///   (`McpHandler::qualified_name`, used verbatim as the `UnifiedTool` command
///   name in `tools/handlers/registration.rs`) — same argument, different
///   separator.
/// * **hooks** and **agents** are not slash commands; they never become catalog
///   command words.
///
/// So `[[tools]]` is the only bundled-plugin route that can shadow `/btw`, and
/// it is the route with no ceiling behind it. The commands arm is collected
/// anyway so that the guard keeps asking the one resolver about what boot
/// registers, instead of encoding today's namespacing as an assumption — but do
/// not read it as covering the tools arm.
///
/// The honest answer to "is that all of them" is **no, and nothing here makes
/// the list stay complete.** What is in reach is what the binary carries; what
/// is out of reach is genuinely out of reach — a skill, MCP server or plugin the
/// operator installs at runtime becomes a catalog command word this test never
/// sees, and an installed extension named `btw` would shadow the side question
/// on Panel and TUI. If a sixth compile-time registration surface is added, this
/// guard will not know, and the only thing that will catch it is someone reading
/// this paragraph.
///
/// ⚠️ Both bundled trees are git submodules. In a checkout that has not
/// initialised them the two arms contribute nothing, and this test **fails
/// loudly** on that rather than passing: the `words.len()` self-check is
/// dominated by the three in-binary tables and would read identically over two
/// empty directories, so a soft skip would report closure over a directory it
/// never opened. That is the shape
/// `every_bundled_plugin_passes_the_installers_own_validation`'s own failure
/// message warns about — "the scan, not the plugins, is what broke" — and in a
/// bare checkout the two tests fail together, for the one true reason.
///
/// [`BtwTurn::resolve`]: crate::gateway::btw::BtwTurn::resolve
#[tokio::test]
async fn no_shipped_command_word_resolves_as_a_side_question() {
    let catalog = ToolCatalog::new();
    catalog.register_builtin_tools().await;

    let mut words: Vec<String> = Vec::new();
    for tool in catalog.list_all_for_ui().await {
        words.push(tool.name);
        words.extend(tool.aliases);
    }
    for def in crate::executor::BUILTIN_TOOL_DEFINITIONS {
        words.push(def.name.to_string());
    }
    for (alias, canonical) in crate::tool_metadata::aliases::SHORTHAND_ALIASES {
        words.push((*alias).to_string());
        words.push((*canonical).to_string());
    }

    // Self-check for the three in-binary tables: a guard that iterates an empty
    // list is indistinguishable from one that passes.
    assert!(
        words.len() > 100,
        "only {} command words came back — one of the first three tables went \
         empty, so this guard checked almost nothing",
        words.len()
    );

    // Fourth and fifth sources: the two bundled trees, each read through the
    // loader's own parser rather than a second one written here.
    let bundled_skills = bundled_skill_command_words();
    let bundled_plugins = bundled_plugin_command_words();
    words.extend(bundled_skills.words.iter().cloned());
    words.extend(bundled_plugins.words.iter().cloned());

    // The bundled arms get their OWN non-empty check, and it is a hard failure.
    //
    // The `words.len() > 100` self-check above cannot see them — it is dominated
    // by the three in-binary tables and would read exactly the same with both
    // trees empty. Letting that stand would make this guard report closure over
    // a directory it never opened, which is the shape
    // `every_bundled_plugin_passes_the_installers_own_validation`'s own failure
    // message warns about ("the scan, not the plugins, is what broke"). An
    // uninitialised submodule is a fact about the checkout, and the honest
    // response is to say so out loud rather than to pass.
    //
    // ⚠️ Counts **units walked**, not words produced. A plugin whose components
    // live under a non-default directory name (`commands = "cmds"`) yields no
    // words while being entirely present, and a word-count check would then
    // accuse the submodules and send the reader to `git submodule update`, which
    // would not help. Units answer the question actually being asked — was the
    // tree there — and words answer the predicate.
    //
    // Deliberately paired: both trees are submodules of the same repo,
    // initialised by the same `--recursive`, so one message covers the one
    // condition that produces both.
    assert!(
        bundled_skills.units > 0 && bundled_plugins.units > 0,
        "the bundled walk parsed {} skill(s) and {} plugin director(ies) — \
         `skills/` and `plugins/` are git submodules and this checkout has not \
         initialised them, so those two sources were not checked at all. This is \
         a statement about the checkout, not about the guard: run \
         `git submodule update --init --recursive`. CI sets \
         `submodules: recursive`, which is where this arm earns its keep. Do not \
         relax this into a soft skip — a census that scans an empty directory and \
         reports closure is worse than one that is absent.",
        bundled_skills.units,
        bundled_plugins.units
    );

    for word in &words {
        assert!(
            crate::gateway::btw::BtwTurn::resolve(&format!("/{word} some argument")).is_none(),
            "`{word}` is a shipped command word that the one side-question \
             resolver reads as a side question. Discovery and resolution share \
             the catalog, so such an entry is also dispatchable: \
             `stamp_slash_mode` would stamp SLASH_COMMAND_MODE_KEY and the \
             engine's fast path would run it without the read-only ceiling."
        );
    }
}

/// What a bundled tree contributed, and how much of it the walk actually saw.
///
/// The second number exists because "produced no command words" and "was not
/// there" are different facts with different fixes, and the non-emptiness check
/// below must not confuse them. A plugin whose components live under a
/// non-default directory name contributes zero words while being perfectly
/// present; blaming that on an uninitialised submodule sends the reader to run
/// `git submodule update`, which will not help.
struct BundledWalk {
    /// Command words the catalog will register from this tree.
    words: Vec<String>,
    /// Units the walk parsed successfully — skills for the skills tree, plugin
    /// directories for the plugins tree. Zero means the tree is absent or
    /// unreadable; non-zero means it was walked, whatever it yielded.
    units: usize,
}

/// Every command word the bundled **skills** tree contributes to the catalog.
///
/// Walks `crate::bundled::BUNDLED_SKILLS` for `SKILL.md` files and parses each
/// with `skill::manifest::parse_skill_content` — the same function the loader
/// uses — so the id this returns is the id the catalog will register. A skill's
/// command word is its id, and the id comes from the `name:` frontmatter rather
/// than the directory name, which is why this asks the parser instead of
/// deriving it.
///
/// Empty in a bare checkout (the tree is a git submodule); non-empty in CI.
fn bundled_skill_command_words() -> BundledWalk {
    fn walk(dir: &include_dir::Dir<'_>, out: &mut BundledWalk) {
        for file in dir.files() {
            let is_skill_md = file
                .path()
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("SKILL.md"));
            if !is_skill_md {
                continue;
            }
            let Some(text) = file.contents_utf8() else {
                continue;
            };
            if let Ok(manifest) = crate::skill::manifest::parse_skill_content(
                text,
                crate::domain::skill::SkillSource::Bundled,
            ) {
                use crate::domain::Entity as _;
                out.units += 1;
                out.words.push(manifest.id().to_string());
            }
        }
        for sub in dir.dirs() {
            walk(sub, out);
        }
    }

    let mut out = BundledWalk {
        words: Vec::new(),
        units: 0,
    };
    walk(&crate::bundled::BUNDLED_SKILLS, &mut out);
    out
}

/// Every command word the bundled **plugins** tree contributes to the catalog.
///
/// This is the arm that matters most, and it was missing for a round. A bundled
/// plugin reaches the catalog by two routes, and only one of them can shadow a
/// bare verb:
///
/// * **`[[tools]]`** (top level in the deprecated `aleph.plugin.toml`,
///   `[[aleph.tools]]` in the CC format — both land in `tools_v2`) →
///   `register_plugin_tools` → `ToolSource::Plugin` → `CommandContext::Builtin`
///   → `execute_direct_tool`, i.e. the raw tool registry, **with no
///   `ScopedToolService` and therefore no read-only ceiling.** A bundled plugin
///   exposing a tool named `btw` would take every side question on Panel and TUI
///   straight past the thing this whole feature exists to deliver. **This is the
///   only bundled-plugin route that can shadow `/btw`.**
/// * **`commands/`** → `register_skills` → `ToolSource::Skill`, whose fast path
///   returns `Fallthrough`, so the ceiling survives and only the command's
///   prompt is overlaid. Its catalog word is
///   `SkillRegistration::qualified_name()` = `namespaced_component_key(plugin_id, name)`
///   = `plugin-id:name`, and boot registers that same qualified form — so a
///   plugin command called `btw` becomes `some-plugin:btw` and **cannot** equal
///   a bare `btw`. It is collected anyway, because the guard's job is to ask the
///   one resolver about what boot registers rather than to reason about which
///   answers are foregone conclusions; if `namespaced_component_key` ever stops
///   namespacing, this arm notices without being told.
///
/// ⚠️ **Do not read that as redundancy and trim the `tools_v2` arm.** The two
/// arms are not two ways of finding the same thing: the commands arm structurally
/// cannot fail the predicate, and the tools arm is the only thing standing
/// between a bundled plugin and an unceilinged `/btw`.
///
/// Both are collected through the loader's own calls rather than a second
/// parser: `manifest::parse_manifest_from_dir_sync` (the exact call
/// `tool_catalog_init` makes) for `tools_v2`, and `manifest::parsers::parse_commands_dir`
/// with `SkillRegistration::qualified_name()` (the exact derivation boot
/// registers) for commands.
///
/// # No pre-filter: the parser decides what a plugin is
///
/// This used to skip any directory without `.claude-plugin/plugin.{toml,json}`
/// before calling the parser — a two-entry enumeration standing in front of a
/// parser that accepts four shapes, and the shape it dropped was the one that
/// mattered: the deprecated `aleph.plugin.toml` puts `[[tools]]` at top level,
/// `toml_types.rs` maps it straight to `tools_v2`, and boot registers it exactly
/// like any other. A bundled plugin in that format declaring `name = "btw"` was
/// invisible to this guard — the same fail-soft skip the guard exists to
/// prevent, one level down, in its own detection.
///
/// The filter was also redundant, which is what makes deleting it the whole fix
/// rather than half of one: `Err(_) => continue` below already discards anything
/// the parser refuses, and a README-only directory falls through auto-discovery
/// to zero command words. The parser is the authority on what it can read; a
/// second opinion in front of it could only ever be narrower.
///
/// ⚠️ **Reads the tree from disk, not from `crate::bundled::BUNDLED_PLUGINS`.**
/// `parse_manifest_from_dir_sync` takes a `&Path` and a plugin manifest is a
/// directory shape, not a file — there is no content-string entry point to hand
/// an `include_dir` file to, and inventing one would be the second parser this
/// is written to avoid. `plugins/` on disk is the same tree `include_dir!`
/// embeds at build time.
///
/// ⚠️ **Known narrowing: the commands directory name is hard-coded.** Production
/// resolves it through `component_source::resolve_dirs(raw.commands, "commands", …)`,
/// so a manifest saying `commands = "cmds"` is walked at boot and missed here.
/// Reaching that field means re-parsing the raw manifest, i.e. the second parser
/// again. It cannot hide a `btw` — every command word from that route is
/// namespaced (above) — and it can no longer produce a false "the submodules are
/// missing" accusation either, because the non-empty check below counts **plugin
/// directories parsed**, not words produced.
fn bundled_plugin_command_words() -> BundledWalk {
    let mut out = BundledWalk {
        words: Vec::new(),
        units: 0,
    };

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return out;
    };

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }

        let plugin_id = match crate::extension::manifest::parse_manifest_from_dir_sync(&dir) {
            Ok(manifest) => {
                out.units += 1;
                for tool in manifest.tools_v2.clone().unwrap_or_default() {
                    out.words.push(tool.name);
                }
                manifest.id
            }
            // A manifest this crate cannot parse registers nothing, so it
            // contributes no command word — and a directory that is not a plugin
            // at all lands here too. It is not silently fine when it IS meant to
            // be a plugin: that is exactly what
            // `every_bundled_plugin_passes_the_installers_own_validation` exists
            // to fail on, and that guard is the one that should report it.
            Err(_) => continue,
        };

        if let Ok(commands) =
            crate::extension::manifest::parsers::parse_commands_dir(&dir, "commands", &plugin_id)
        {
            for cap in commands {
                if let crate::extension::CapabilityDeclaration::Skill(skill) = cap {
                    out.words.push(skill.qualified_name());
                }
            }
        }
    }
    out
}
