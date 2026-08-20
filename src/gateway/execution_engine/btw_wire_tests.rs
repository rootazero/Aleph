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
