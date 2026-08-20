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

/// Asking twice must be free — including the third and fourth ask.
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
