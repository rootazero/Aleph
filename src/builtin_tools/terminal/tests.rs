//! `TerminalTool`'s tests.
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

/// Pull the accepted action strings out of a tool schema, whichever of
/// the two shapes schemars emitted.
///
/// schemars 1.2 renders a fieldless enum as a flat `enum` array ONLY when
/// no variant carries a doc comment; the moment one does — and every
/// `TerminalAction` variant does, because the model reads them — it emits
/// `oneOf` of `{const, description}` instead, to have somewhere to put
/// the per-variant text. Both shapes mean the same thing to a provider,
/// and which one ships is decided by something as innocent as deleting a
/// `///` line, so the guard reads both rather than pinning the accident.
///
/// Panics rather than returning an empty list when it recognises neither:
/// "I cannot find the actions" must not be answerable as "there are no
/// write verbs" (判据 §8).
fn declared_actions(schema: &serde_json::Value) -> Vec<String> {
    let action = &schema["$defs"]["TerminalAction"];
    if let Some(flat) = action["enum"].as_array() {
        return flat
            .iter()
            .map(|v| v.as_str().expect("enum member is a string").to_string())
            .collect();
    }
    if let Some(variants) = action["oneOf"].as_array() {
        return variants
            .iter()
            .map(|v| {
                v["const"]
                    .as_str()
                    .expect("oneOf member carries a const")
                    .to_string()
            })
            .collect();
    }
    panic!(
        "neither $defs.TerminalAction.enum nor .oneOf found; schema was {}",
        serde_json::to_string_pretty(schema).unwrap_or_default()
    );
}

/// 本期没有写入动词。多一个就是多一个授权面。
///
/// 2026-09-04 (task D): `wait` and `explain` join the list. Both are still
/// reads — `wait` blocks on the agent table's change watch and returns a
/// row, `explain` re-runs the detection engine over the screen — so the
/// claim this test pins ("no write verb") is unchanged, and
/// `the_description_says_it_is_read_only` stays true beside it. The
/// EXPECTED list is spelled out rather than counted so adding a verb is a
/// deliberate edit here and not a number that quietly grows.
///
/// Read out of `$defs`, not `properties.action`: schemars 1.2 emits a
/// NAMED type as a `$ref`, so `properties.action` carries no action
/// vocabulary at all and a guard reading it asserts against `Null`.
/// That is the shape every sibling tool with an enum-typed argument
/// already ships, and `schema_strictify` rewrites those refs explicitly.
///
/// Not to be "fixed" by forcing `#[schemars(inline)]` to match
/// `moa_manage`'s flat schema: that tool hand-writes `impl JsonSchema`
/// because `#[serde(tag = "action")]` puts a `oneOf` at the ROOT, which
/// grammar-constrained endpoints cannot compile — they answer with EMPTY
/// arguments. `TerminalArgs` is a plain struct; its root is already a
/// flat object, so that hazard is not this tool's to carry, and inlining
/// would make `terminal` the one tool shipping a shape its nine siblings
/// do not.
#[test]
fn the_tool_exposes_no_write_verb() {
    let def = TerminalTool.definition();
    let actions = declared_actions(&def.parameters);
    assert_eq!(actions, ["list", "read", "status", "wait", "explain"]);
}

/// DESCRIPTION 必须自己说清只读——这句话归这个工具所有，
/// 不进 system prompt（R9 第二把尺）。不写，模型会反复试着发命令。
#[test]
fn the_description_says_it_is_read_only() {
    assert!(TerminalTool::DESCRIPTION
        .to_lowercase()
        .contains("read-only"));
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
/// `TerminalAction` and `TerminalArgs` derive `JsonSchema`, so every `///`
/// line on them — and on every type they reference — becomes a `description`
/// the model pays for on each turn that loads this tool. Three notes were
/// riding along, each a note ABOUT THE CODE rather than a runtime fact the
/// model cannot know:
///
/// * `List`'s second paragraph, a rule about saying all five field names,
///   naming the test that pins them;
/// * `TerminalAction`'s own doc, pointing at a Rust constant by path;
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
    let def = TerminalTool.definition();
    for description in shipped_descriptions(&def.parameters) {
        assert!(
            !description.contains("::"),
            "a Rust path in a schema description means this sentence is \
             addressed to whoever maintains the code, not to the model that \
             receives it on every turn (R9). Move it to a `//` comment above \
             the item. Offending description:\n{description}"
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
    let out = TerminalTool
        .call(TerminalArgs {
            action: TerminalAction::List,
            session_id: None,
            until: None,
            timeout_ms: None,
        })
        .await
        .unwrap();
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
        .scope(ctx, async {
            TerminalTool
                .call(TerminalArgs {
                    action: TerminalAction::List,
                    session_id: None,
                    until: None,
                    timeout_ms: None,
                })
                .await
        })
        .await
        .unwrap();
    assert!(!out.success);
    assert!(out.message.contains("operator"), "{}", out.message);
    // A refusal that still carried session data would be a gate that
    // reports "no" and means "yes" (task-11 review F10) — discarding
    // the `data: None` in the two arms of `TerminalTool::call` and
    // keeping only the label check would leave this test green.
    assert!(out.data.is_none(), "a refusal must not carry session data");
}

#[tokio::test]
async fn read_without_session_id_is_refused_not_panicking() {
    let out = TerminalTool
        .call(TerminalArgs {
            action: TerminalAction::Read,
            session_id: None,
            until: None,
            timeout_ms: None,
        })
        .await
        .unwrap();
    assert!(!out.success);
    assert!(out.message.contains("session_id"), "{}", out.message);
}

/// Reaches the global `PtyManager` via `read_session`'s
/// `owner_of`/`visible_text` calls — same F7 rationale as
/// `no_turn_context_is_treated_as_operator` above.
#[tokio::test]
#[serial_test::parallel(pty_global_manager)]
async fn read_of_unknown_session_is_no_such_session() {
    let out = TerminalTool
        .call(TerminalArgs {
            action: TerminalAction::Read,
            session_id: Some("does-not-exist".to_string()),
            until: None,
            timeout_ms: None,
        })
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

    let id = pty::manager()
        .spawn(&SpawnOptions {
            created_by: Some("u-owner".to_string()),
            ..Default::default()
        })
        .expect("spawn")
        .session_id;

    let result = read_session(Some(&id), Some("u-someone-else"));

    // Close BEFORE asserting: this spawns on the process-global manager,
    // so a failing assert would leak a live PTY for the rest of the test
    // binary and every later test sharing that singleton would inherit it.
    let _ = pty::manager().close(&id);

    assert_eq!(
        result,
        Err(pty::no_such_session(&id)),
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

    let owned = pty::manager()
        .spawn(&SpawnOptions {
            created_by: Some("u-owner".to_string()),
            ..Default::default()
        })
        .expect("spawn owned")
        .session_id;
    let unowned = pty::manager()
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
            argv0: None,
            cmdline: None,
            cwd: "",
            screen: &screen,
            process_exited: false,
            frame_produced: true,
            now: 0,
        });
    }

    let anon_list = list_sessions(None).expect("list");
    let anon_status = status(None).expect("status");
    let anon_read_owned = read_session(Some(&owned), None);
    let anon_read_unowned = read_session(Some(&unowned), None);
    let anon_wait_owned = wait_for_session(Some(&owned), None, Some(0), None).await;
    let anon_wait_unowned = wait_for_session(Some(&unowned), None, Some(0), None).await;
    let anon_explain_owned = explain_session(Some(&owned), None);
    let anon_explain_unowned = explain_session(Some(&unowned), None);
    let owner_list = list_sessions(Some("u-owner")).expect("list as owner");

    // Close BEFORE asserting — same reason as
    // `read_of_someone_elses_session_is_refused_like_unknown`: a failing
    // assert would leak two live PTYs into every later test in this
    // binary.
    for id in [&owned, &unowned] {
        agents().remove(id);
        let _ = pty::manager().close(id);
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
        Err(pty::no_such_session(&owned)),
        "an owned session must read as nonexistent to an actor-less caller"
    );
    assert_eq!(
        anon_wait_owned,
        Err(pty::no_such_session(&owned)),
        "`wait` is addressed by session id too — the same refusal, byte for byte, or it \
         becomes the oracle `read` refuses to be"
    );
    assert_eq!(
        anon_explain_owned,
        Err(pty::no_such_session(&owned)),
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
        argv0: None,
        cmdline: None,
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

    let table = Arc::new(crate::gateway::runtime::RuntimeAgents::default());
    // A shell is not an agent, so this row starts at `unknown`.
    sample_state(&table, "s-wait", "zsh", b"");

    let writer = Arc::clone(&table);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        sample_state(&writer, "s-wait", "grok", OSC_PROGRESS_WORKING);
    });

    let outcome = wait_for_state(
        &table,
        "s-wait",
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

    let table = crate::gateway::runtime::RuntimeAgents::default();
    sample_state(&table, "s-timeout", "grok", OSC_PROGRESS_WORKING);

    let outcome = wait_for_state(
        &table,
        "s-timeout",
        &[RuntimeAgentState::Blocked],
        std::time::Duration::from_millis(60),
    )
    .await;

    match outcome {
        WaitOutcome::Timeout(Some(entry)) => {
            assert_eq!(entry.state, RuntimeAgentState::Working);
            assert_eq!(entry.session_id, "s-timeout");
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
    let outcome = wait_for_state(&table, &id, &[RuntimeAgentState::Blocked], window).await;
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

    let unowned = pty::manager()
        .spawn(&SpawnOptions {
            created_by: None,
            ..Default::default()
        })
        .expect("spawn")
        .session_id;

    let out = wait_for_session(Some(&unowned), Some(&[]), Some(0), None).await;

    let _ = pty::manager().close(&unowned);

    let message = out.expect_err("an empty `until` is refused, not waited out");
    assert!(
        message.contains("at least one state"),
        "the refusal must be the empty-`until` one and not the ownership gate's, or this \
         guard passes with the behaviour deleted: {message}"
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
    assert_eq!(out.source, Some("bundled"));
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
    let id = pty::manager()
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
        let out = explain_session(Some(&id), Some("u-explain")).expect("explain");
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

    let _ = pty::manager().close(&id);
    assert!(
        found,
        "explain must read the LIVE screen, not an empty placeholder; title held: {seen:?}"
    );
}

/// `explain` is addressed by session id, so it is an id-enumeration
/// oracle unless it refuses exactly as `read` does.
#[test]
#[serial_test::parallel(pty_global_manager)]
fn explain_of_someone_elses_session_is_refused_like_unknown() {
    use crate::gateway::pty::SpawnOptions;

    let id = pty::manager()
        .spawn(&SpawnOptions {
            created_by: Some("u-owner".to_string()),
            ..Default::default()
        })
        .expect("spawn")
        .session_id;

    let stranger = explain_session(Some(&id), Some("u-someone-else"));
    let unknown = explain_session(Some("does-not-exist"), Some("u-someone-else"));

    let _ = pty::manager().close(&id);

    assert_eq!(stranger, Err(pty::no_such_session(&id)));
    assert_eq!(unknown, Err(pty::no_such_session("does-not-exist")));
}
