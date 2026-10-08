//! Terminal observation capability tests.
//!
//! Split out of `terminal.rs` unchanged (review round 1): that file was
//! 1,668 lines, of which ~900 were this module, against a 800-line project
//! ceiling. A `foo.rs` + `foo/tests.rs` pair needs no `mod.rs`.
//!
//! The file carries no `#[cfg(test)]` of its own — `terminal.rs` declares it
//! as `#[cfg(test)] mod tests;`. Source-level censuses that ask a file for
//! its `cfg_test_portion` are blind to that shape and must use
//! `source_scan::test_text` instead; the census this module's tests answer
//! to (`gateway::handlers::pty`'s
//! `every_test_that_reaches_the_global_pty_manager_is_tagged`) was fixed to
//! do so in the commit before this move, and names this path in its
//! `KNOWN_REACHERS` list so the coverage cannot lapse quietly.

use super::*;

fn observation_definitions() -> Vec<crate::tools::service::ToolDefinition> {
    let registry = crate::tools::ToolHandlerRegistry::new();
    let mut scope = crate::tools::ToolRegistrationScope::new("test:terminal-schema");
    capabilities::register_observation_capabilities(&registry, &mut scope).unwrap();
    let mut definitions: Vec<_> = registry
        .snapshot()
        .values()
        .map(|handler| handler.definition())
        .collect();
    definitions.sort_by(|a, b| a.name.cmp(&b.name));
    definitions
}

fn observation_definition(action: &str) -> crate::tools::service::ToolDefinition {
    observation_definitions()
        .into_iter()
        .find(|def| def.name == format!("terminal_sessions_{action}"))
        .unwrap()
}

async fn observe(
    action: &str,
    input: serde_json::Value,
) -> Result<TerminalOutput, crate::tools::ToolError> {
    let output =
        capabilities::invoke_observation(&format!("terminal_sessions_{action}"), input).await?;
    Ok(serde_json::from_value(output.value).expect("legacy terminal envelope"))
}

/// Every registered identity is read-only; adding any verb is deliberate.
#[test]
fn the_tool_exposes_no_write_verb() {
    let names: Vec<_> = observation_definitions()
        .into_iter()
        .map(|def| def.name)
        .collect();
    assert_eq!(
        names,
        [
            "terminal_sessions_attach",
            "terminal_sessions_explain",
            "terminal_sessions_list",
            "terminal_sessions_read",
            "terminal_sessions_status",
            "terminal_sessions_wait"
        ]
    );
}

/// DESCRIPTION 必须自己说清只读——这句话归这个工具所有，
/// 不进 system prompt（R9 第二把尺）。不写，模型会反复试着发命令。
#[test]
fn the_description_says_it_is_read_only() {
    for definition in observation_definitions() {
        assert!(definition.description.to_lowercase().contains("read-only"));
    }
}

/// Every `description` string the model actually receives, in schema order,
/// from the SHIPPED definition rather than from `schema_for!` — the schema
/// passes through `AlephTool::definition`, and a guard reading the macro
/// directly would assert about the producer instead of the wire (判据 §4).
///
/// Walks the whole `$defs` graph, not just this file's two types: `until`'s
/// item type is `aleph_protocol::runtime::RuntimeAgentState`, whose own doc
/// comment ships here too, from another crate, which is precisely how a
/// per-file reading of R9 misses it.
fn shipped_descriptions(schema: &serde_json::Value) -> Vec<String> {
    fn walk(node: &serde_json::Value, out: &mut Vec<String>) {
        match node {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    if key == "description" {
                        if let Some(text) = value.as_str() {
                            out.push(text.to_string());
                        }
                    }
                    walk(value, out);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(schema, &mut out);
    assert!(
        !out.is_empty(),
        "no descriptions found at all — a walk that finds nothing must not \
         read as 'nothing to complain about' (判据 §8); schema was {}",
        serde_json::to_string_pretty(schema).unwrap_or_default()
    );
    out
}

/// R9: the schema this tool ships carries nothing addressed to whoever
/// maintains it.
///
/// The retired action/args types derived `JsonSchema`, so every `///`
/// line on them — and on every type they referenced — became a `description`
/// the model paid for on each turn. Canonical handlers now ship their schemas
/// directly; this regression still walks the actual shipped descriptions. Three notes were
/// riding along, each a note ABOUT THE CODE rather than a runtime fact the
/// model cannot know:
///
/// * `List`'s second paragraph, a rule about saying all five field names,
///   naming the test that pins them;
/// * the retired action enum's own doc, pointing at a Rust constant by path;
/// * `RuntimeAgentState`'s type doc in `shared/protocol`, which is entirely
///   an argument for why that enum derives `JsonSchema` at all — it reaches
///   the model through `until`, from a crate nobody editing this tool reads.
///
/// # The predicate, and what it does NOT catch (判据 §5)
///
/// Rust path syntax (`::`). All three instances used it, it cannot occur in
/// a sentence written for a model, and it needs no list of banned names to
/// keep current — a test-name ban would go quietly vacuous the day that test
/// is renamed (判据 §2).
///
/// It does not catch maintainer prose with no symbol path in it ("say all
/// five or none, because…" on its own would pass). That half stays a reading
/// job. What this closes is the shape all three actual instances had.
#[test]
fn the_shipped_schema_addresses_the_model_and_not_the_maintainer() {
    // Schemas with no arguments (list/status) carry no descriptions, so the
    // "found nothing" guard must hold over the whole shipped set instead.
    let combined = serde_json::Value::Array(
        observation_definitions()
            .into_iter()
            .map(|def| def.input_schema)
            .collect(),
    );
    for description in shipped_descriptions(&combined) {
        assert!(
            !description.contains("::"),
            "a Rust path in a schema description means this sentence is \
             addressed to whoever maintains the code, not to the model that \
             receives it on every turn (R9). Move it to a `//` comment above \
             the item. Offending description:\n{description}"
        );
    }
}

#[test]
fn the_shipped_schema_preserves_each_terminal_action_semantics() {
    let action = |name: &str| observation_definition(name).description.to_lowercase();
    let list = action("list");
    assert!(
        list.contains("spawn") && list.contains("epoch") && list.contains("cd"),
        "{list}"
    );
    let status = action("status");
    assert!(status.contains("runtime.agents.list"), "{status}");
    let wait = action("wait");
    assert!(
        wait.contains("block") && wait.contains("timeout") && wait.contains("gone"),
        "{wait}"
    );
    let explain = action("explain");
    assert!(
        explain.contains("manifest") && explain.contains("version") && explain.contains("screen"),
        "{explain}"
    );

    let description = observation_definitions()
        .iter()
        .map(|def| def.description.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    for term in [
        "disabled in policy",
        "polling",
        "wrong detection",
        "idle agent",
    ] {
        assert!(
            description.contains(term),
            "tool description missing {term}: {description}"
        );
    }
}

#[test]
fn the_shipped_schema_explains_terminal_arguments_to_the_model() {
    let schema = observation_definition("wait").input_schema;
    let args = &schema;
    let session_id = args["properties"]["session_id"]["description"]
        .as_str()
        .expect("session_id has a model-facing description")
        .to_lowercase();
    let until = args["properties"]["until"]["description"]
        .as_str()
        .expect("until has a model-facing description")
        .to_lowercase();
    let timeout = args["properties"]["timeout_ms"]["description"]
        .as_str()
        .expect("timeout_ms has a model-facing description")
        .to_lowercase();
    assert!(session_id.contains("required") && session_id.contains("session_id"));
    assert!(until.contains("blocked") && until.contains("idle"));
    assert!(timeout.contains("60000") && timeout.contains("150000"));
    let descriptions = shipped_descriptions(&schema).join(" ").to_lowercase();
    for term in ["working", "blocked", "idle", "unknown", "default"] {
        assert!(
            descriptions.contains(term),
            "schema is missing state/default term {term}"
        );
    }
}

/// No `TurnContext` at all reads as operator (cron/A2A/internal
/// convention) — a caller with a scoped, non-operator role is refused.
///
/// Reaches the process-global `PtyManager` via `list_sessions`, so it
/// carries the same `pty_global_manager` parallel key every other test
/// in the crate that touches the singleton does — see the module doc on
/// `gateway::handlers::pty::every_test_that_reaches_the_global_pty_manager_is_tagged`,
/// which cannot see this reacher itself (it lives behind a function
/// call from the production half of this file, not inside a
/// `#[cfg(test)]` block the census scans — task-11 review F7).
#[tokio::test]
#[serial_test::parallel(pty_global_manager)]
async fn no_turn_context_is_treated_as_operator() {
    let out = observe("list", serde_json::json!({})).await.unwrap();
    assert!(out.success, "{}", out.message);
}

#[tokio::test]
async fn non_operator_caller_is_refused() {
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

    let ctx = TurnContext {
        session_key: SessionKey::Ephemeral {
            agent_id: "main".to_string(),
            ephemeral_id: "terminal-guest-test".to_string(),
        },
        run_id: String::new(),
        channel_id: String::new(),
        conversation_id: String::new(),
        caller_role: Some("guest".to_string()),
        channel_tool_permissions: None,
        unattended: false,
        plan_gate: None,
        side_question: false,
    };
    let out = TURN_CONTEXT
        .scope(ctx, async { observe("list", serde_json::json!({})).await })
        .await
        .unwrap();
    assert!(!out.success);
    assert!(out.message.contains("operator"), "{}", out.message);
    // A refusal that still carried session data would be a gate that
    // reports "no" and means "yes" (task-11 review F10) — discarding
    // the `data: None` in `invoke_observation`'s plain-refusal arms (the
    // operator gate and `TerminalRefusal::Message`; the tombstone arm
    // deliberately carries data, and is pinned on its own) and keeping
    // only the label check would leave this test green.
    assert!(out.data.is_none(), "a refusal must not carry session data");
}

#[tokio::test]
#[serial_test::parallel(pty_global_manager)]
async fn read_without_session_id_is_refused_not_panicking() {
    let out = observe("read", serde_json::json!({})).await.unwrap();
    assert!(!out.success);
    assert!(out.message.contains("session_id"), "{}", out.message);
}

/// Reaches the global `PtyManager` via `read_session`'s
/// `owner_of`/`visible_text` calls — same F7 rationale as
/// `no_turn_context_is_treated_as_operator` above.
#[tokio::test]
#[serial_test::parallel(pty_global_manager)]
async fn read_of_unknown_session_is_no_such_session() {
    let out = observe("read", serde_json::json!({"session_id": "does-not-exist"}))
        .await
        .unwrap();
    assert!(!out.success);
    assert!(out.message.contains("no such session"), "{}", out.message);
    // Same reasoning as `non_operator_caller_is_refused` (F10): the
    // refusal's payload, not just its label, must be asserted.
    assert!(out.data.is_none(), "a refusal must not carry session data");
}

/// A session that EXISTS but belongs to someone else must look
/// identical to one that does not exist at all — the assertion whose
/// absence let `read_session`'s ownership check (`terminal.rs:241`) be
/// deleted without reddening anything, since every existing test used an
/// id that never existed either way (task-11 review F8).
#[test]
#[serial_test::parallel(pty_global_manager)]
fn read_of_someone_elses_session_is_refused_like_unknown() {
    use crate::gateway::pty::SpawnOptions;

    let manager = pty::manager();
    let id = manager
        .spawn(&SpawnOptions {
            created_by: Some("u-owner".to_string()),
            ..Default::default()
        })
        .expect("spawn")
        .session_id;

    let result = read_session(manager, Some(&id), Some("u-someone-else"));

    // Close BEFORE asserting: this spawns on the process-global manager,
    // so a failing assert would leak a live PTY for the rest of the test
    // binary and every later test sharing that singleton would inherit it.
    let _ = manager.close(&id);

    assert_eq!(
        result,
        Err(TerminalRefusal::Message(pty::no_such_session(&id))),
        "an unowned session and a nonexistent one must produce byte-identical \
         refusals, or `read` becomes an id-enumeration oracle"
    );
}

/// D7: a caller with NO resolved identity sees only the sessions nobody
/// owns — and still sees those.
///
/// Both halves are asserted because the rule they separate is the whole
/// change: "actor-less admits everything" (what `pty::owner_admits` says,
/// and what this tool used to inherit) and "actor-less admits nothing"
/// both pass a test that only checks the owned session is hidden. The
/// unowned session is what says which of the two shipped.
///
/// The identified caller is asserted too: spec §10 ruled the narrowing
/// must not blind an operator to their own sessions, and that claim needs
/// a witness rather than a comment.
///
/// `status` reads the runtime table rather than the PTY registry, so the
/// owned session is sampled into it — otherwise the `status` half is
/// vacuous (an empty table hides everything, whatever the predicate says).
///
/// EVERY verb that can name or hand out a session id, not the subset
/// spec §4.4 lists: `wait` and `explain` take a `session_id` too, and a
/// gate applied to some of the addressed actions is not a gate — it is
/// the shape this tool's own module doc describes for `plugin_manage`
/// (one face closed, one open). Adding a verb without adding it here is
/// what this test exists to make expensive. `wait`'s window is zero so
/// the refusal (or the immediate timeout) is what is measured, not a
/// sleep.
#[tokio::test]
#[serial_test::parallel(pty_global_manager)]
async fn an_actorless_caller_sees_only_unowned_sessions() {
    use crate::gateway::pty::screen::Screen;
    use crate::gateway::pty::SpawnOptions;
    use crate::gateway::runtime::{agents, SampleInput};

    let manager = pty::manager();
    let owned = manager
        .spawn(&SpawnOptions {
            created_by: Some("u-owner".to_string()),
            ..Default::default()
        })
        .expect("spawn owned")
        .session_id;
    let unowned = manager
        .spawn(&SpawnOptions {
            created_by: None,
            ..Default::default()
        })
        .expect("spawn unowned")
        .session_id;

    let screen = Screen::new(4, 40);
    for id in [&owned, &unowned] {
        agents().sample(SampleInput {
            session_id: id,
            shell: "zsh",
            program: None,
            argv: &[],
            cwd: "",
            screen: &screen,
            process_exited: false,
            frame_produced: true,
            now: 0,
        });
    }

    let anon_list = list_sessions(manager, None).expect("list");
    let anon_status = status(manager, None).expect("status");
    let anon_read_owned = read_session(manager, Some(&owned), None);
    let anon_read_unowned = read_session(manager, Some(&unowned), None);
    let anon_wait_owned = wait_for_session(manager, Some(&owned), None, Some(0), None).await;
    let anon_wait_unowned = wait_for_session(manager, Some(&unowned), None, Some(0), None).await;
    let anon_explain_owned = explain_session(manager, Some(&owned), None);
    let anon_explain_unowned = explain_session(manager, Some(&unowned), None);
    let owner_list = list_sessions(manager, Some("u-owner")).expect("list as owner");

    // Close BEFORE asserting — same reason as
    // `read_of_someone_elses_session_is_refused_like_unknown`: a failing
    // assert would leak two live PTYs into every later test in this
    // binary.
    for id in [&owned, &unowned] {
        agents().remove(id);
        let _ = manager.close(id);
    }

    let ids = |v: &serde_json::Value, key: &str| -> Vec<String> {
        v[key]
            .as_array()
            .expect("array")
            .iter()
            .map(|e| e["session_id"].as_str().expect("session_id").to_string())
            .collect()
    };

    assert!(
        !ids(&anon_list, "sessions").contains(&owned),
        "an actor-less caller must not see a session someone else owns"
    );
    assert!(
        !ids(&anon_status, "agents").contains(&owned),
        "`status` must filter with the same predicate `list` does"
    );
    assert_eq!(
        anon_read_owned,
        Err(TerminalRefusal::Message(pty::no_such_session(&owned))),
        "an owned session must read as nonexistent to an actor-less caller"
    );
    assert_eq!(
        anon_wait_owned,
        Err(TerminalRefusal::Message(pty::no_such_session(&owned))),
        "`wait` is addressed by session id too — the same refusal, byte for byte, or it \
         becomes the oracle `read` refuses to be"
    );
    assert_eq!(
        anon_explain_owned,
        Err(TerminalRefusal::Message(pty::no_such_session(&owned))),
        "…and `explain`, which hands back the screen's title and tail: every face has to \
         answer the actor-less caller the same way, or the newest one is the hole"
    );

    assert!(
        ids(&anon_list, "sessions").contains(&unowned),
        "a session nobody owns is what the actor-less arm still admits"
    );
    assert!(
        ids(&anon_status, "agents").contains(&unowned),
        "…on the status face too"
    );
    assert!(
        anon_read_unowned.is_ok(),
        "…and it must still be readable: {anon_read_unowned:?}"
    );
    assert_eq!(
        anon_wait_unowned
            .as_ref()
            .map(|v| v["outcome"].as_str().unwrap_or_default().to_string()),
        Ok("timeout".to_string()),
        "…and waitable: an unowned session in `unknown` with a zero window times out, \
         which is the shape that proves the gate let the call through at all"
    );
    assert!(
        anon_explain_unowned.is_ok(),
        "…and explainable: {anon_explain_unowned:?}"
    );

    assert!(
        ids(&owner_list, "sessions").contains(&owned),
        "the narrowing must not blind an identified caller to its own session (spec §10)"
    );
}

// ── Step 1 (task D): what a loopback operator actually is ─────────────

/// The premise D7 turns on, asserted rather than assumed: a loopback
/// operator is NOT the actor-less caller.
///
/// Spec §10 left the arm's shape conditional on this — if a Panel-spawned
/// session carried `created_by: None`, narrowing the actor-less arm to
/// unowned rows would have been a no-op. It does not: the loopback
/// handshake resolves a user, that user is scoped as `CALLER_USER` around
/// every dispatched request, and `ambient_actor` reads it.
///
/// The identity is taken FROM the production resolver rather than written
/// here as a literal — a test that scopes its own constant and then reads
/// it back would be asserting `task_local`, not this chain (判据 §10).
/// The last link (that `handle_spawn` stamps `ambient_actor()` onto
/// `SessionInfo::created_by`) is already pinned, for an arbitrary user, by
/// `handlers::pty::tests::a_spawn_through_the_handler_carries_both_the_actor_and_the_scrollback`.
#[tokio::test]
async fn a_loopback_operator_is_not_an_actor_less_caller() {
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::security::store::SecurityStore;

    let store = SecurityStore::in_memory().expect("in-memory security store");
    let (user, role) =
        crate::gateway::handlers::connect::resolve_connection_identity(true, None, &store);
    assert_eq!(role, "operator", "loopback resolves to the implicit owner");

    let actor = CALLER_USER
        .scope(user.clone(), async {
            crate::gateway::visibility::ambient_actor()
        })
        .await;

    assert_eq!(
        actor, user,
        "the connection's resolved user must be the ambient actor a tool call sees"
    );
    assert!(
        actor.is_some(),
        "a loopback operator has an identity, so the actor-less arm is NOT its arm — \
         this is spec §10's second case, and the arm narrows to `created_by == None`"
    );
}

// ── wait ──────────────────────────────────────────────────────────────

struct WaitSession(String, &'static pty::PtyManager);

impl WaitSession {
    fn new(manager: &'static pty::PtyManager) -> Self {
        Self(
            manager
                .spawn(&pty::SpawnOptions::default())
                .expect("spawn live wait fixture")
                .session_id,
            manager,
        )
    }
}

impl Drop for WaitSession {
    fn drop(&mut self) {
        let _ = self.1.close(&self.0);
    }
}

/// An isolated table plus a screen, so a wait test never races the
/// process-global sampler.
fn sample_state(
    table: &crate::gateway::runtime::RuntimeAgents,
    session_id: &str,
    shell: &str,
    bytes: &[u8],
) {
    use crate::gateway::pty::screen::Screen;
    use crate::gateway::runtime::SampleInput;

    let mut screen = Screen::new(4, 40);
    screen.feed(bytes);
    table.sample(SampleInput {
        session_id,
        shell,
        program: None,
        argv: &[],
        cwd: "",
        screen: &screen,
        process_exited: false,
        frame_produced: true,
        now: 0,
    });
}

/// `grok`'s OSC 9;4 progress payload for "working" — the same wire
/// `gateway::runtime::tests::the_osc_progress_wire_is_actually_connected`
/// uses, so this test is not inventing a signal the engine may stop
/// honouring without anything going red.
const OSC_PROGRESS_WORKING: &[u8] = b"\x1b]9;4;1;-1\x07";

/// The wake-up edge: a state that arrives AFTER the wait started must end
/// it. Starting in `unknown` and waiting for `working` means an
/// implementation that answered from the first read alone cannot pass.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::parallel(pty_global_manager)]
async fn wait_returns_when_the_state_enters_the_until_set() {
    use aleph_protocol::runtime::RuntimeAgentState;
    use std::sync::Arc;

    let session = WaitSession::new(pty::manager());
    let table = Arc::new(crate::gateway::runtime::RuntimeAgents::default());
    // A shell is not an agent, so this row starts at `unknown`.
    sample_state(&table, &session.0, "zsh", b"");

    let writer = Arc::clone(&table);
    let id = session.0.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        sample_state(&writer, &id, "grok", OSC_PROGRESS_WORKING);
    });

    let outcome = wait_for_state(
        pty::manager(),
        &table,
        &session.0,
        &[RuntimeAgentState::Working],
        std::time::Duration::from_secs(5),
    )
    .await;

    match outcome {
        WaitOutcome::Reached(entry) => assert_eq!(entry.state, RuntimeAgentState::Working),
        other => panic!("the wait must end when the state arrives, got {other:?}"),
    }
}

/// A timeout carries the CURRENT entry, not a manufactured final state
/// (spec §5: `timeout` + the current entry, never "the last entry as if
/// it were the answer"). Asserting only the label would leave an
/// implementation that reports `timeout` with `agent: null` green, and a
/// caller cannot tell "still working" from "I lost sight of it".
#[tokio::test(flavor = "multi_thread")]
#[serial_test::parallel(pty_global_manager)]
async fn wait_times_out_with_the_current_entry() {
    use aleph_protocol::runtime::RuntimeAgentState;

    let session = WaitSession::new(pty::manager());
    let table = crate::gateway::runtime::RuntimeAgents::default();
    sample_state(&table, &session.0, "grok", OSC_PROGRESS_WORKING);

    let outcome = wait_for_state(
        pty::manager(),
        &table,
        &session.0,
        &[RuntimeAgentState::Blocked],
        std::time::Duration::from_millis(60),
    )
    .await;

    match outcome {
        WaitOutcome::Timeout(Some(entry)) => {
            assert_eq!(entry.state, RuntimeAgentState::Working);
            assert_eq!(entry.session_id, session.0);
        }
        other => {
            panic!("a window that closes with nothing reached is a timeout, got {other:?}")
        }
    }
}

/// The session ending is its own outcome. `gone` and `timeout` must not be
/// the same answer: a caller that gets `timeout` will wait again, and a
/// caller that gets `gone` knows there is nothing left to wait for.
///
/// Reaches the global PTY registry through `session_is_registered` — the
/// id below is in no registry, which is exactly the "the terminal ended"
/// shape.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::parallel(pty_global_manager)]
async fn wait_reports_gone_when_the_session_is_removed() {
    use aleph_protocol::runtime::RuntimeAgentState;
    use std::sync::Arc;

    let table = Arc::new(crate::gateway::runtime::RuntimeAgents::default());
    sample_state(&table, "s-gone", "grok", OSC_PROGRESS_WORKING);

    let remover = Arc::clone(&table);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        remover.remove("s-gone");
    });

    let outcome = wait_for_state(
        pty::manager(),
        &table,
        "s-gone",
        &[RuntimeAgentState::Blocked],
        std::time::Duration::from_secs(5),
    )
    .await;

    assert_eq!(
        outcome,
        WaitOutcome::Gone,
        "a session whose row is gone and which the registry does not know is `gone`"
    );
}

/// A row that is absent only because nothing has painted yet is NOT
/// `gone`. Without this, `wait` on a freshly spawned shell answers "the
/// terminal ended" — a wrong label, which reads as a fact.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::parallel(pty_global_manager)]
async fn wait_on_a_live_session_with_no_row_yet_keeps_waiting() {
    use crate::gateway::pty::SpawnOptions;
    use aleph_protocol::runtime::RuntimeAgentState;

    let live = pty::manager()
        .spawn(&SpawnOptions::default())
        .expect("spawn")
        .session_id;

    // Empty table: the session is registered, but nothing was ever
    // sampled for it.
    let table = crate::gateway::runtime::RuntimeAgents::default();
    let outcome = wait_for_state(
        pty::manager(),
        &table,
        &live,
        &[RuntimeAgentState::Blocked],
        std::time::Duration::from_millis(60),
    )
    .await;

    let _ = pty::manager().close(&live);

    assert_eq!(
        outcome,
        WaitOutcome::Timeout(None),
        "a live session that has not painted yet must time out, not read as gone"
    );
}

/// A session that ends BEFORE it was ever sampled must WAKE its waiter,
/// not be discovered when the window finally closes.
///
/// The elapsed assertion is the whole test, and asserting the outcome
/// word alone is not enough — I wrote it that way first and it passed
/// against the unfixed code. `wait_for_state` re-runs its verdict once
/// when the deadline fires, so the answer was already `gone`; it just
/// arrived a full window late. With the 60 s default that is a caller
/// told a minute after the fact.
///
/// The table never holds a row for this id, so the only thing that can
/// wake the waiter is `RuntimeAgents::remove` bumping the generation for
/// a row that was not there. It used to bump only when a row existed
/// (review round 1, Minor 2).
#[tokio::test(flavor = "multi_thread")]
#[serial_test::parallel(pty_global_manager)]
async fn wait_reports_gone_when_an_unsampled_session_exits() {
    use crate::gateway::pty::SpawnOptions;
    use aleph_protocol::runtime::RuntimeAgentState;
    use std::sync::Arc;

    let id = pty::manager()
        .spawn(&SpawnOptions::default())
        .expect("spawn")
        .session_id;
    // Never sampled: this table has no row for the session at any point.
    let table = Arc::new(crate::gateway::runtime::RuntimeAgents::default());

    let exiting = Arc::clone(&table);
    let exiting_id = id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        // `close` rather than `remove` so the child is killed too — a
        // bare registry removal would leak a live shell into the rest of
        // this test binary. Both spellings leave the waiter the same two
        // facts: gone from the registry, absent from the table.
        let _ = pty::manager().close(&exiting_id);
        exiting.remove(&exiting_id);
    });

    let window = std::time::Duration::from_secs(5);
    let started = std::time::Instant::now();
    let outcome = wait_for_state(
        pty::manager(),
        &table,
        &id,
        &[RuntimeAgentState::Blocked],
        window,
    )
    .await;
    let elapsed = started.elapsed();

    assert_eq!(outcome, WaitOutcome::Gone);
    assert!(
        elapsed < window / 5,
        "the exit must WAKE the waiter: it answered `gone` only after {elapsed:?} of a \
         {window:?} window, which means it slept to the deadline and found out on its way \
         out. The margin is deliberately wide — this fails on a mechanism, not on a \
         slow machine"
    );
}

/// The clamp, and the reason there is one. `600_000` is the brief's own
/// over-ask; the assertion below it is the one that matters — the ceiling
/// is checked against `bash_exec`'s budget constant, not against a second
/// copy of "150 seconds", so a shrunk foreground budget reddens here
/// instead of silently letting a blocking call outlive it.
#[test]
fn wait_timeout_is_capped_at_the_tool_budget() {
    assert_eq!(
        wait_window(Some(600_000)),
        std::time::Duration::from_millis(WAIT_MAX_TIMEOUT_MS),
        "an over-ask is clamped, not refused"
    );
    assert_eq!(
        wait_window(None),
        std::time::Duration::from_millis(WAIT_DEFAULT_TIMEOUT_MS)
    );
    assert_eq!(
        wait_window(Some(1_500)),
        std::time::Duration::from_millis(1_500),
        "a request under the ceiling is honoured exactly"
    );
}

/// See `WAIT_MAX_TIMEOUT_MS`'s doc: this is the constraint the number
/// exists to satisfy, and it is checked rather than restated.
#[test]
fn the_wait_ceiling_stays_under_the_foreground_tool_budget() {
    let budget_ms = crate::builtin_tools::bash_exec::WAIT_MAX_TIMEOUT_SECS * 1_000;
    assert!(
        WAIT_MAX_TIMEOUT_MS < budget_ms,
        "terminal{{wait}} may block for {WAIT_MAX_TIMEOUT_MS} ms, which is not under the \
         {budget_ms} ms a blocking builtin is allowed — the budget wrapper would kill the \
         call before it could report even its own timeout"
    );
}

/// An empty `until` can only produce a timeout, so it is refused with the
/// vocabulary instead of honoured literally for a full window.
///
/// Two things here are the fix for a guard that could not go red (review
/// round 1, I1), and both matter:
///
/// * the id is a REAL session nobody owns, so the actor-less caller is
///   admitted and the call reaches the `until` check. The first version
///   passed `"s-empty-until"`, which exists in no registry: the ownership
///   gate refused it first and the empty-`until` arm was never executed.
/// * the assertion is on wording only THIS refusal carries. The old one
///   looked for `"until"`, which `no such session: s-empty-until` contains
///   as part of the id — so deleting the refusal arm left the test green.
///
/// `timeout_ms: Some(0)` so that a deleted refusal arm fails in
/// milliseconds instead of defaulting to `[blocked, idle]` and blocking
/// for the full 60 s window before the assertion can fail.
#[tokio::test]
#[serial_test::parallel(pty_global_manager)]
async fn wait_refuses_an_empty_until_instead_of_stalling() {
    use crate::gateway::pty::SpawnOptions;

    let manager = pty::manager();
    let unowned = manager
        .spawn(&SpawnOptions {
            created_by: None,
            ..Default::default()
        })
        .expect("spawn")
        .session_id;

    let out = wait_for_session(manager, Some(&unowned), Some(&[]), Some(0), None).await;

    let _ = manager.close(&unowned);

    let TerminalRefusal::Message(message) =
        out.expect_err("an empty `until` is refused, not waited out")
    else {
        panic!("an empty `until` is a plain refusal, not a tombstone");
    };
    assert_eq!(
        message,
        "wait requires at least one state in `until` (blocked / idle / working / unknown); omit it for [blocked, idle]",
        "the adapter must preserve the old byte-identical empty-`until` message"
    );
}

// ── explain ───────────────────────────────────────────────────────────

/// `explain` names the rule that decided the state and the manifest
/// revision it came from — G3's mitigation (a stale manifest is invisible
/// until someone can see which one answered).
///
/// Driven through the OSC progress payload rather than screen text so the
/// assertion does not depend on chrome that upstream may repaint: the rule
/// id, its region and the state it carries all come from `grok.toml`.
#[test]
fn explain_names_the_matched_rule_and_manifest_version() {
    let screen = crate::gateway::pty::manager::DetectionInputs {
        text: String::new(),
        title: String::new(),
        osc_progress: "4;1;-1".to_string(),
    };
    let out = explain_detection(
        "s-explain",
        agent_detect::identify_agent("grok"),
        None,
        &screen,
    );

    let rule = out
        .matched_rule
        .expect("the osc-progress payload matches a grok rule");
    assert_eq!(rule.id, "osc_progress_working");
    assert_eq!(rule.region, "osc_progress");
    assert_eq!(
        out.state,
        aleph_protocol::runtime::RuntimeAgentState::Working
    );
    assert_eq!(out.agent.as_deref(), Some("grok"));
    assert_eq!(out.source.as_deref(), Some("bundled"));
    assert_eq!(
        out.manifest_version,
        agent_detect::manifest_version(
            agent_detect::identify_agent("grok").expect("grok is an agent")
        ),
        "the version reported must be the one the loaded manifest declares"
    );
    assert_eq!(
        out.inputs.osc_progress, "4;1;-1",
        "the explanation has to show what the engine was fed, or `no rule matched` and \
         `the input never arrived` are the same sentence"
    );
}

/// The two absences are different sentences. A session with no row has
/// never been looked at; a row whose program is not an agent has been.
#[test]
fn explain_tells_an_unsampled_session_from_an_unrecognised_program() {
    let screen = crate::gateway::pty::manager::DetectionInputs {
        text: String::new(),
        title: String::new(),
        osc_progress: String::new(),
    };

    let never_sampled = explain_detection("s-none", None, None, &screen);
    assert!(
        never_sampled
            .reason
            .as_deref()
            .expect("an unexplainable state carries a reason")
            .contains("no row"),
        "{:?}",
        never_sampled.reason
    );

    let row = aleph_protocol::runtime::RuntimeAgentEntry {
        session_id: "s-vim".to_string(),
        label: "zsh".to_string(),
        cwd: String::new(),
        agent: None,
        program: Some("vim".to_string()),
        state: aleph_protocol::runtime::RuntimeAgentState::Unknown,
        updated_at: 0,
        quiet_since: None,
    };
    let unrecognised = explain_detection("s-vim", None, Some(&row), &screen);
    assert!(
        unrecognised
            .reason
            .as_deref()
            .expect("reason")
            .contains("vim"),
        "the program that WAS found belongs in the sentence: {:?}",
        unrecognised.reason
    );
    assert_eq!(
        unrecognised.state,
        aleph_protocol::runtime::RuntimeAgentState::Unknown,
        "no agent means unknown, never idle"
    );
}

/// The wire between the tool and the live screen, which the pure test
/// above cannot see: cut `PtyManager::detection_inputs` down to empty
/// strings and this is what goes red.
///
/// The child paints an OSC 0 title and then sleeps, so the assertion is
/// about a value only the real screen can produce.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::parallel(pty_global_manager)]
async fn explain_reads_the_live_session_screen() {
    use crate::gateway::pty::SpawnOptions;

    let (command, args) = if cfg!(windows) {
        (
            "cmd.exe",
            vec![
                "/C".to_string(),
                "echo \x1b]0;ALEPH-EXPLAIN-TITLE\x07 & ping -n 20 127.0.0.1 > NUL".to_string(),
            ],
        )
    } else {
        (
            "sh",
            vec![
                "-c".to_string(),
                "printf '\\033]0;ALEPH-EXPLAIN-TITLE\\007'; sleep 20".to_string(),
            ],
        )
    };
    let manager = pty::manager();
    let id = manager
        .spawn(&SpawnOptions {
            command: Some(command.to_string()),
            args,
            created_by: Some("u-explain".to_string()),
            rows: 10,
            cols: 40,
            ..Default::default()
        })
        .expect("spawn")
        .session_id;

    // The reader thread feeds the screen; poll rather than sleep a fixed
    // amount, the shape every other PTY test in this crate uses.
    let mut seen = String::new();
    let mut found = false;
    for _ in 0..100 {
        let out = explain_session(manager, Some(&id), Some("u-explain")).expect("explain");
        seen = out["inputs"]["title"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if seen == "ALEPH-EXPLAIN-TITLE" {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let _ = manager.close(&id);
    assert!(
        found,
        "explain must read the LIVE screen, not an empty placeholder; title held: {seen:?}"
    );
}

/// `explain` is addressed by session id, so it is an id-enumeration
/// oracle unless it refuses exactly as `read` does.
#[test]
#[serial_test::parallel(pty_global_manager)]
fn a_closed_session_read_preserves_the_old_error_text() {
    let manager = pty::manager();
    let id = manager
        .spawn(&pty::SpawnOptions {
            created_by: Some("u-owner".to_string()),
            ..Default::default()
        })
        .expect("spawn")
        .session_id;
    manager.close(&id).expect("close");

    let result = read_session(manager, Some(&id), Some("u-owner"));
    assert_eq!(
        result,
        Err(TerminalRefusal::Message(pty::no_such_session(&id))),
        "closed-session read must not expose the service ToolError display prefix"
    );
}

#[test]
#[serial_test::parallel(pty_global_manager)]
fn explain_of_someone_elses_session_is_refused_like_unknown() {
    use crate::gateway::pty::SpawnOptions;

    let manager = pty::manager();
    let id = manager
        .spawn(&SpawnOptions {
            created_by: Some("u-owner".to_string()),
            ..Default::default()
        })
        .expect("spawn")
        .session_id;

    let stranger = explain_session(manager, Some(&id), Some("u-someone-else"));
    let unknown = explain_session(manager, Some("does-not-exist"), Some("u-someone-else"));

    let _ = manager.close(&id);

    assert_eq!(
        stranger,
        Err(TerminalRefusal::Message(pty::no_such_session(&id)))
    );
    assert_eq!(
        unknown,
        Err(TerminalRefusal::Message(pty::no_such_session(
            "does-not-exist"
        )))
    );
}

/// A session the PREVIOUS server owned answers its owner with what became of
/// its shell, and answers everyone else exactly as an id that never existed
/// — the journal fallback runs under the same predicate the live path uses,
/// so the tombstone cannot become the id-enumeration oracle `no_such_session`
/// exists to close. The whole face is driven through `call()` for the owner:
/// `lost_with_restart: true` on the wire, the pid and stop command as data,
/// the sentence plus its output clause as the message. `lost_with_restart:
/// false` is skipped on the wire, so every pre-existing envelope stays
/// byte-identical.
#[tokio::test]
#[serial_test::parallel(pty_global_manager)]
async fn a_tombstoned_terminal_answers_its_owner_and_nobody_else() {
    use crate::builtin_tools::process_journal as j;
    let _g = j::test_gate();
    let tmp = tempfile::tempdir().unwrap();
    j::enable_for_test(tmp.path().to_path_buf());
    j::record_pty_spawn("t-1", "pwsh", "", Some("alice"));
    j::record_pty_child("t-1", 777);
    j::disable_for_test();
    j::init_and_reconcile_with_probe(tmp.path().to_path_buf(), &|_, _| j::Liveness::StillRunning);

    let job = j::lookup_pty("t-1").expect("journal retains the interrupted PTY");
    let report = j::tombstone_report(&job).expect("interrupted PTY has a tombstone");
    assert!(report.text.contains("pid 777"), "{}", report.text);
    // The face itself, as the owner: the arm of `call()` that renders the
    // report, not only the resolver behind it.
    let out = crate::gateway::caller_identity::CALLER_USER
        .scope(
            Some("alice".to_string()),
            observe("read", serde_json::json!({"session_id": "t-1"})),
        )
        .await
        .unwrap();
    let wire = serde_json::to_value(&out).unwrap();
    assert_eq!(
        wire["lost_with_restart"],
        serde_json::json!(true),
        "the flag a reader keys on: {wire}"
    );
    assert_eq!(
        wire["data"]["stop_command"],
        serde_json::json!(report.stop_command),
        "{wire}"
    );
    assert!(
        !out.success && out.message.contains("pid 777"),
        "{}",
        out.message
    );
    assert_eq!(
        out.message,
        report.text_with_output(),
        "this envelope has no slot for the recorded output, so the clause rides the message"
    );
    assert!(
        out.message
            .ends_with("no output was recorded before the restart"),
        "{}",
        out.message
    );
    let bob = crate::gateway::caller_identity::CALLER_USER
        .scope(
            Some("bob".to_string()),
            observe("read", serde_json::json!({"session_id": "t-1"})),
        )
        .await
        .unwrap();
    assert!(!bob.success);
    assert_eq!(bob.message, pty::no_such_session("t-1"));
    assert!(bob.data.is_none() && !bob.lost_with_restart);
    assert_eq!(
        bob.message,
        pty::no_such_session("t-1"),
        "a stranger must read the tombstoned id as one that never existed"
    );
    let actorless = crate::gateway::caller_identity::CALLER_USER
        .scope(
            None,
            observe("read", serde_json::json!({"session_id": "t-1"})),
        )
        .await
        .unwrap();
    assert!(!actorless.success);
    assert_eq!(actorless.message, pty::no_such_session("t-1"));
    assert!(actorless.data.is_none() && !actorless.lost_with_restart);
    assert_eq!(
        actorless.message,
        pty::no_such_session("t-1"),
        "actorless sees nothing"
    );
    let out = serde_json::to_value(TerminalOutput {
        success: false,
        message: "x".into(),
        data: None,
        lost_with_restart: false,
    })
    .unwrap();
    assert!(
        out.get("lost_with_restart").is_none(),
        "false is skipped: existing envelopes byte-identical"
    );
    j::disable_for_test();
}

/// The tombstone envelope carries the report's recorded output IN the
/// message — this face has no `recorded_output` key the way the bash face
/// does, so the clause must ride the prose or the bytes reach nobody. Pure:
/// a PTY row has no live-tail twin today, so a real interrupted terminal
/// always reports "no output was recorded"; this pins the composition for
/// the report shape, not for a disk state production cannot produce.
#[test]
fn the_tombstone_envelope_carries_the_output_clause_in_its_message() {
    let report = crate::builtin_tools::process_journal::TombstoneReport {
        kind: "exited_during_restart",
        text: "Terminal t-9 (`pwsh`) was started by a previous server process.".to_string(),
        pid: Some(5),
        stop_command: None,
        output_clause: "last recorded output (as of 5): built 3 crates".to_string(),
    };
    let out = lost_with_restart_output(report.clone());
    assert!(
        out.message.starts_with(&report.text) && out.message.contains("built 3 crates"),
        "{}",
        out.message
    );
    assert!(out.lost_with_restart && !out.success);
    let data = out.data.expect("pid and stop command ride as data");
    assert_eq!(
        (&data["tombstone"], &data["pid"], &data["stop_command"]),
        (
            &serde_json::json!("exited_during_restart"),
            &serde_json::json!(5),
            &serde_json::Value::Null
        )
    );
}

// ---------------------------------------------------------------------------
// A4 RED — canonical terminal observation capabilities.
//
// All names carry the `terminal_capability_` prefix so one filter selects the
// whole A4 set. Every test below fails to COMPILE today because
// `terminal::capabilities` does not exist; GREEN converts `terminal.rs` into a
// module directory exposing `capabilities` with:
//
//   pub fn normalize_terminal_compat_call(name: &str, input: Value)
//       -> Result<(String, Value), ToolError>;
//   pub fn register_observation_capabilities(
//       registry: &ToolHandlerRegistry, scope: &mut ToolRegistrationScope)
//       -> Result<(), ToolError>;
//   pub(crate) fn require_operator_caller() -> Result<(), ToolError>;
//   pub(crate) async fn invoke_observation(name: &str, input: Value)
//       -> Result<ToolOutput, ToolError>;   // inline gate + dispatch, shared by all 6 handlers
// ---------------------------------------------------------------------------

const TERMINAL_CAPABILITY_CANONICAL: [&str; 6] = [
    "terminal_sessions_list",
    "terminal_sessions_read",
    "terminal_sessions_status",
    "terminal_sessions_wait",
    "terminal_sessions_explain",
    "terminal_sessions_attach",
];

fn terminal_capability_ctx(role: Option<&str>) -> crate::tools::turn_context::TurnContext {
    use crate::routing::session_key::SessionKey;
    crate::tools::turn_context::TurnContext {
        session_key: SessionKey::Ephemeral {
            agent_id: "main".to_string(),
            ephemeral_id: "terminal-capability-test".to_string(),
        },
        run_id: String::new(),
        channel_id: String::new(),
        conversation_id: String::new(),
        caller_role: role.map(str::to_owned),
        channel_tool_permissions: None,
        unattended: false,
        plan_gate: None,
        side_question: false,
    }
}

#[test]
fn terminal_capability_normalize_maps_each_read_action_to_its_canonical_name() {
    use super::capabilities::normalize_terminal_compat_call;
    use serde_json::json;

    let cases = [
        ("list", "terminal_sessions_list"),
        ("read", "terminal_sessions_read"),
        ("status", "terminal_sessions_status"),
        ("wait", "terminal_sessions_wait"),
        ("explain", "terminal_sessions_explain"),
    ];
    for (action, canonical) in cases {
        let input = json!({
            "action": action,
            "session_id": "s1",
            "until": "idle",
            "timeout_ms": 5
        });
        let (name, out) = normalize_terminal_compat_call("terminal", input)
            .unwrap_or_else(|e| panic!("action `{action}` must normalize: {e}"));
        assert_eq!(name, canonical, "action `{action}`");
        assert_eq!(
            out,
            json!({"session_id": "s1", "until": "idle", "timeout_ms": 5}),
            "action `{action}`: only the `action` key is stripped; other args pass through"
        );
    }

    // The documented minimal shape.
    let (name, out) =
        normalize_terminal_compat_call("terminal", json!({"action": "read", "session_id": "x"}))
            .unwrap();
    assert_eq!(name, "terminal_sessions_read");
    assert_eq!(out, json!({"session_id": "x"}));
}

#[test]
fn terminal_capability_normalize_rejects_unknown_missing_or_write_actions() {
    use super::capabilities::normalize_terminal_compat_call;
    use serde_json::json;

    for bad in [
        json!({"action": "spawn"}),
        json!({"action": "input", "session_id": "x"}),
        json!({"action": "attach", "session_id": "x"}),
        json!({"action": ""}),
        json!({"action": 7}),
        json!({"session_id": "x"}),
        json!("not an object"),
    ] {
        assert!(
            normalize_terminal_compat_call("terminal", bad.clone()).is_err(),
            "legacy `terminal` call {bad} must be refused, not guessed at"
        );
    }
}

#[test]
fn terminal_capability_normalize_passes_canonical_and_foreign_names_through() {
    use super::capabilities::normalize_terminal_compat_call;
    use serde_json::json;

    for name in
        TERMINAL_CAPABILITY_CANONICAL
            .iter()
            .copied()
            .chain(["file_read", "terminal_other", ""])
    {
        // Includes an `action` key: only the literal name `terminal` is
        // rewritten; everything else is returned byte-identical.
        let input = json!({"action": "read", "session_id": "x"});
        let (out_name, out_input) = normalize_terminal_compat_call(name, input.clone()).unwrap();
        assert_eq!(out_name, name);
        assert_eq!(out_input, input, "`{name}` input must be untouched");
    }
}

#[test]
fn terminal_capability_register_observation_capabilities_registers_exactly_the_six() {
    use super::capabilities::register_observation_capabilities;
    use crate::tools::{ToolHandlerRegistry, ToolRegistrationScope};

    let registry = ToolHandlerRegistry::new();
    let mut scope = ToolRegistrationScope::new("capability:terminal-observation");
    register_observation_capabilities(&registry, &mut scope).expect("registration succeeds");

    let mut names: Vec<String> = registry.snapshot().keys().cloned().collect();
    names.sort();
    let mut want: Vec<String> = TERMINAL_CAPABILITY_CANONICAL
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    want.sort();
    assert_eq!(names, want, "exactly the six canonical observation names");
    assert_eq!(
        scope.steps().len(),
        6,
        "every registration is scope-tracked"
    );
    assert!(
        registry.resolve("terminal").is_none(),
        "legacy `terminal` must not be registered as a second identity"
    );
    for name in TERMINAL_CAPABILITY_CANONICAL {
        let entry = registry.resolve_entry(name).expect("registered");
        assert_eq!(entry.descriptor.name, name);
        assert_eq!(entry.handler.definition().name, name);
    }
}

#[test]
fn terminal_capability_require_operator_caller_mirrors_legacy_caller_is_operator() {
    use super::capabilities::require_operator_caller;
    use crate::tools::turn_context::TURN_CONTEXT;

    // Absent TURN_CONTEXT is trusted (cron/internal/local daemon): legacy
    // `caller_is_operator()` is `current_turn_context().is_none_or(..)`.
    assert!(
        require_operator_caller().is_ok(),
        "absent context = trusted"
    );

    for (role, ok) in [
        (None, true),
        (Some("operator"), true),
        (Some("member"), false),
        (Some("guest"), false),
    ] {
        let got = TURN_CONTEXT.sync_scope(terminal_capability_ctx(role), require_operator_caller);
        assert_eq!(got.is_ok(), ok, "caller_role {role:?}");
    }
}

#[tokio::test]
async fn terminal_capability_canonical_handlers_refuse_non_operator_callers() {
    // `ToolHandlerRegistry` wraps handlers in `AdmissionHandler`, which refuses
    // any direct `invoke` lacking a dispatch verdict *before* the inner handler
    // runs — so the inline operator hard-refusal cannot be observed through the
    // registry. Every registered handler's `invoke` therefore routes through ONE
    // inner entrypoint, and that entrypoint is what is pinned here:
    //
    //   pub(crate) async fn invoke_observation(name: &str, input: Value)
    //       -> Result<ToolOutput, ToolError>
    //
    // Its first act is the operator check — before parsing `input`, before
    // touching the PTY manager — so a refusal never carries data. The refusal
    // keeps the LEGACY envelope (an `Ok` value with `success: false`, the shape
    // callers of the former `terminal` tool already parse), not an `Err`.
    use super::capabilities::invoke_observation;
    use super::TerminalOutput;
    use crate::tools::turn_context::TURN_CONTEXT;
    use serde_json::json;

    const LEGACY_REFUSAL: &str = "terminal requires operator; refused. An operator approving this call's own escalation card does not currently lift this refusal — nothing re-stamps the caller's role after approval.";

    for name in TERMINAL_CAPABILITY_CANONICAL {
        for role in ["member", "guest"] {
            let result = TURN_CONTEXT
                .scope(terminal_capability_ctx(Some(role)), async {
                    invoke_observation(name, json!({"session_id": "x"})).await
                })
                .await;
            let output = result.unwrap_or_else(|e| {
                panic!("`{name}`/{role}: refusal is an Ok legacy envelope, got Err({e:?})")
            });
            let envelope: TerminalOutput = serde_json::from_value(output.value.clone())
                .unwrap_or_else(|e| panic!("`{name}`/{role}: not a TerminalOutput: {e}"));
            assert!(!envelope.success, "`{name}`/{role}: must not succeed");
            assert_eq!(envelope.message, LEGACY_REFUSAL, "`{name}`/{role}");
            assert!(
                envelope.data.is_none(),
                "`{name}`/{role}: refusal carries no data"
            );
            assert!(!envelope.lost_with_restart, "`{name}`/{role}");
            assert!(
                output.value.get("lost_with_restart").is_none(),
                "`{name}`/{role}: legacy wire shape omits a false lost_with_restart"
            );
        }
    }
}
