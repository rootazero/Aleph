use super::*;
use crate::gateway::pty::{PtySession, SpawnOptions};

/// A screen carrying `bytes`, at the size the tests use throughout.
fn screen(bytes: &[u8]) -> Screen {
    let mut s = Screen::new(4, 40);
    s.feed(bytes);
    s
}

/// A sample with no foreground probe result — the shape every test written
/// before the probe existed was asserting, spelled once instead of sixteen
/// times.
///
/// `program: None` here is the honest value, not a convenience: these tests
/// are about the screen, the hold and the change predicate, and a probe that
/// did not run reports `None` (`RuntimeAgentEntry::program`'s doc). Tests
/// that ARE about the probe build [`SampleInput`] themselves.
fn sample_shell(
    agents: &RuntimeAgents,
    session_id: &str,
    shell: &str,
    cwd: &str,
    screen: &Screen,
    process_exited: bool,
    now: i64,
) -> bool {
    agents.sample(SampleInput {
        session_id,
        shell,
        program: None,
        argv: &[],
        cwd,
        screen,
        process_exited,
        frame_produced: true,
        now,
    })
}

/// 证伪守卫：剪断 osc_title 的接线，title 必须变空。
/// 一条不会变红的守卫不是守卫（判据 §3）。
///
/// `shell` is deliberately a name the OSC title does not equal, so
/// cutting the title read makes `label` fall back to it and this
/// assertion goes red. Falsified by hand on 2026-09-02 — see the task
/// report.
#[test]
fn the_title_wire_is_actually_connected() {
    let s = screen(b"\x1b]0;my-agent\x07idle");
    let agents = RuntimeAgents::default();
    sample_shell(&agents, "s1", "sh", "", &s, false, 0);
    assert_eq!(agents.snapshot()[0].label, "my-agent");
}

/// 证伪守卫：剪断 osc_progress 的接线，两条载荷必须给出同一个状态。
///
/// Falsifiable the same way the title wire above is, and for the same
/// reason: the two payloads differ ONLY in the `osc_progress` region, the
/// screen is empty in both, and `grok.toml` gives them opposite states
/// (`osc_progress_working` at priority 1150 on `^4;1;-1$`,
/// `osc_progress_idle` at 950 on `^4;0;0$`). Cut `screen.osc_progress()`
/// at the sample site and both fall to the same agent-known fallback, so
/// the inequality below is what proves the wire carries current.
///
/// This replaces `osc_progress_has_no_producer_this_phase`, which pinned
/// the deliberate absence of this producer. The producer now exists
/// (`Screen::osc_progress`), so the old pin was asserting a fact that had
/// stopped being true.
#[test]
fn the_osc_progress_wire_is_actually_connected() {
    let working = screen(b"\x1b]9;4;1;-1\x07");
    let idle = screen(b"\x1b]9;4;0;0\x07");

    let agents = RuntimeAgents::default();
    sample_shell(&agents, "s1", "grok", "", &working, false, 0);
    sample_shell(&agents, "s2", "grok", "", &idle, false, 0);
    let rows = agents.snapshot();

    assert_eq!(
        rows[0].state,
        RuntimeAgentState::Working,
        "4;1;-1 is grok's highest-priority working rule"
    );
    assert_eq!(
        rows[1].state,
        RuntimeAgentState::Idle,
        "4;0;0 is grok's osc_progress_idle rule"
    );
}

/// `cwd` is the SPAWN directory, and it has to be able to DIFFER from
/// empty — a field that is empty for every session is a predicate that
/// cannot vary (判据 §2), which is what pinning it to a constant was.
/// Empty still means "the spawn inherited the server's directory", never
/// the filesystem root.
#[test]
fn the_spawn_cwd_reaches_the_entry_and_empty_means_inherited() {
    let s = screen(b"$ ");
    let agents = RuntimeAgents::default();

    sample_shell(
        &agents,
        "chosen",
        "sh",
        "/tmp/aleph-cwd-probe",
        &s,
        false,
        0,
    );
    sample_shell(&agents, "inherited", "sh", "", &s, false, 0);

    let snap = agents.snapshot();
    let chosen = snap.iter().find(|e| e.session_id == "chosen").unwrap();
    let inherited = snap.iter().find(|e| e.session_id == "inherited").unwrap();
    assert_eq!(chosen.cwd, "/tmp/aleph-cwd-probe");
    assert_eq!(inherited.cwd, "");
}

/// A program the bundled manifest does not know is `agent: None` — never
/// a guessed name — and its state is Unknown, not Idle.
#[test]
fn an_unrecognised_program_is_none_and_unknown() {
    let s = screen(b"$ ");
    let agents = RuntimeAgents::default();
    sample_shell(&agents, "s1", "sh", "", &s, false, 0);
    let e = &agents.snapshot()[0];
    assert_eq!(e.agent, None);
    assert_eq!(e.state, RuntimeAgentState::Unknown);
}

/// The `process_exited` input is a real input, not a literal the caller
/// always passes `false`: the same screen answers differently either
/// side of it. Without this the exited arm of
/// `detection_update_for_publish_with_osc` would be unreachable from
/// here and the parameter would be decoration (判据 §2).
#[test]
fn an_exited_session_reads_idle_not_the_screens_answer() {
    let s = screen(b"$ ");
    let agents = RuntimeAgents::default();

    sample_shell(&agents, "live", "sh", "", &s, false, 0);
    sample_shell(&agents, "dead", "sh", "", &s, true, 0);

    let snap = agents.snapshot();
    let dead = snap.iter().find(|e| e.session_id == "dead").unwrap();
    let live = snap.iter().find(|e| e.session_id == "live").unwrap();
    assert_eq!(dead.state, RuntimeAgentState::Idle);
    assert_eq!(live.state, RuntimeAgentState::Unknown);
}

/// `updated_at` is the time of the last OBSERVABLE change, not of the
/// last sample. `RuntimeAgentEntry` derives `PartialEq`, so a timestamp
/// rewritten every frame turns task 6's natural `old != new` predicate
/// into a ~60 Hz broadcast of an unchanged state.
///
/// `now` is a parameter precisely so this test states the times instead
/// of sleeping for them.
#[test]
fn updated_at_advances_only_when_something_observable_changed() {
    let s = screen(b"$ ");
    let agents = RuntimeAgents::default();

    assert!(
        sample_shell(&agents, "s1", "sh", "", &s, false, 1_000),
        "a new session is a change"
    );
    assert_eq!(agents.snapshot()[0].updated_at, 1_000);

    assert!(
        !sample_shell(&agents, "s1", "sh", "", &s, false, 9_999),
        "an identical observation is not a change"
    );
    assert_eq!(
        agents.snapshot()[0].updated_at,
        1_000,
        "updated_at must not follow the clock"
    );

    assert!(
        sample_shell(&agents, "s1", "sh", "/elsewhere", &s, false, 12_000),
        "a different cwd is a change"
    );
    assert_eq!(agents.snapshot()[0].updated_at, 12_000);
}

/// Upstream damps the STATE, not the announcement: while the hold is
/// active the pane's own `state` stays Working (herdr `src/pane.rs:266`
/// is reached only from the `Publish` arm). Mirrors herdr's
/// `pending_idle_holds_working_to_plain_idle_until_confirmed`, with the
/// confirmation count replaced by wall clock — see [`IDLE_HOLD_MS`].
///
/// `claude`'s `osc_title_working` rule drives Working; a title matching
/// no rule leaves the engine on its known-agent idle fallback, which is
/// Idle with `visible_idle: false` — exactly the "plain idle" upstream
/// distrusts.
#[test]
fn a_plain_idle_after_working_is_held_until_the_cap() {
    let agents = RuntimeAgents::default();
    let working = screen("\x1b]0;⠋ building\x07".as_bytes());
    let plain_idle = screen("\x1b]0;claude\x07".as_bytes());

    sample_shell(&agents, "s1", "claude", "", &working, false, 1_000);
    assert_eq!(
        agents.snapshot()[0].state,
        RuntimeAgentState::Working,
        "the working chrome must be detected, or this test proves nothing"
    );

    sample_shell(&agents, "s1", "claude", "", &plain_idle, false, 2_000);
    assert_eq!(
        agents.snapshot()[0].state,
        RuntimeAgentState::Working,
        "plain idle is held, not written"
    );

    assert!(
        agents.release_expired(2_000 + IDLE_HOLD_MS - 1).is_empty(),
        "one millisecond before the cap nothing is released"
    );
    assert_eq!(agents.snapshot()[0].state, RuntimeAgentState::Working);

    let flipped = agents.release_expired(2_000 + IDLE_HOLD_MS);
    assert_eq!(flipped, vec!["s1".to_string()]);
    assert_eq!(agents.snapshot()[0].state, RuntimeAgentState::Idle);
    assert_eq!(
        agents.snapshot()[0].updated_at,
        2_000 + IDLE_HOLD_MS,
        "the flip is an observable change and carries its own time"
    );
}

/// Upstream's bypass: a screen carrying VISIBLE idle evidence is believed
/// at once. Mirrors herdr's `visible_idle_bypasses_plain_idle_hold`.
/// Without this arm the damper would delay every legitimate finish by
/// 700 ms.
#[test]
fn visible_idle_bypasses_the_hold() {
    let agents = RuntimeAgents::default();
    let working = screen("\x1b]0;⠋ building\x07".as_bytes());
    let visible_idle = screen("\x1b]0;✳ Claude Code\x07".as_bytes());

    sample_shell(&agents, "s1", "claude", "", &working, false, 1_000);
    assert_eq!(agents.snapshot()[0].state, RuntimeAgentState::Working);

    sample_shell(&agents, "s1", "claude", "", &visible_idle, false, 2_000);
    assert_eq!(
        agents.snapshot()[0].state,
        RuntimeAgentState::Idle,
        "visible idle evidence is not a guess, so it is not held"
    );
    assert!(
        agents.release_expired(2_000 + IDLE_HOLD_MS).is_empty(),
        "nothing was pending, so nothing can be released"
    );
}

/// THE WIRE, half one. The `sample()` tests above prove the function;
/// this one proves it has a caller. It spawns a real child, drives the
/// same per-session body the flush ticker drives, and asserts the session
/// landed in the PROCESS table — the one task 6 and task 11 read.
///
/// It also carries the cwd across the seam: without that, the
/// `PtySession.cwd` field and the argument at `manager.rs` would be
/// proven only by a direct `sample()` call, one field short of the same
/// 判据 §7 gap this test exists to close.
///
/// ⚠️ The cwd assertion now compares CANONICAL paths, because task A2 made
/// `cwd` the LIVE directory (foreground process, falling back to the spawn
/// directory) rather than the spawn string verbatim. The child never `cd`s,
/// so both sources name the same directory — but macOS spells the temp dir
/// `/var/folders/...` when you ask the environment and `/private/var/...` when
/// you ask the kernel where a process is, and the spawn string keeps a
/// trailing slash the kernel drops. Comparing the strings asserted a spelling;
/// comparing the canonical paths asserts the directory, which is what the
/// field means. It still goes red for an empty or wrong cwd.
#[tokio::test(flavor = "multi_thread")]
async fn a_published_frame_lands_the_session_in_the_table() {
    let id = "t-runtime-wire";
    agents().remove(id);

    let spawn_dir = std::env::temp_dir().to_string_lossy().into_owned();
    let opts = SpawnOptions {
        command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
        args: if cfg!(windows) {
            vec!["/C".into(), "echo ALEPH_RUNTIME_WIRE & pause".into()]
        } else {
            vec!["-c".into(), "printf 'ALEPH_RUNTIME_WIRE'; sleep 30".into()]
        },
        cwd: Some(spawn_dir.clone()),
        rows: 6,
        cols: 40,
        ..Default::default()
    };
    let session = PtySession::spawn(id.into(), &opts, None).expect("spawn");

    let mut framed = false;
    for _ in 0..100 {
        let now = chrono::Utc::now().timestamp_millis();
        if crate::gateway::pty::manager::flush_session(&session, now)
            .frame
            .is_some()
        {
            framed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(framed, "the child produced no frame in 2s");

    let entry = agents().snapshot().into_iter().find(|e| e.session_id == id);
    session.kill();
    agents().remove(id);

    let entry = entry.expect("a flushed frame must land the session in the process table");
    assert!(
        !entry.cwd.is_empty(),
        "a cwd that is empty for every session is a predicate that cannot vary \
         (判据 §2) -- the spawn directory was {spawn_dir}"
    );
    let canonical = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| p.into());
    assert_eq!(
        canonical(&entry.cwd),
        canonical(&spawn_dir),
        "the session's directory must cross the seam, not just reach sample(). \
         Entry said {}, spawn said {spawn_dir}",
        entry.cwd
    );
}

/// Whether a raw bus frame is a `runtime.agents.changed` topic event —
/// via the protocol constant, not a re-typed literal (fix round 1,
/// review Minor 6's reasoning applied here too: a rename of the
/// constant must redden every reader, not just the ones that remembered
/// to update their copy of the string).
fn is_agents_changed(raw: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v.get("topic").and_then(|t| t.as_str().map(str::to_owned)))
        .as_deref()
        == Some(aleph_protocol::runtime::RUNTIME_AGENTS_CHANGED_TOPIC)
}

/// THE EVENT WIRE (task 6, fix round 1). `sample()` reporting a change
/// must reach a real bus subscriber as `runtime.agents.changed`; a frame
/// whose detected state/agent/label/cwd are unchanged must publish
/// NOTHING — the "not on every frame" rule from R6-4 — and the exit path
/// must publish once more.
///
/// `start_flush_loop` cannot be driven from a test (see
/// `the_flush_loop_body_calls_the_sampler_and_the_release`'s doc), so
/// this drives the exact per-tick decision it makes instead: ONE call to
/// `crate::gateway::pty::manager::publish_agents_changed_if(changed, &bus)`
/// per frame — the `if` now lives INSIDE that function (fix round 1,
/// review F4/Minor 8), not re-implemented here, so this test asserts
/// what the helper actually does rather than a copy of its logic.
///
/// Assertions are by ORDER, not by a final count: `bus.publish` is
/// synchronous, so `try_recv` immediately after each call is
/// deterministic (no wait needed for the first two steps) — this closes
/// review Minor 4 (the old "seen >= 2" could not tell {frame1, frame2}
/// from {frame1, exit}; this can, because each step drains and asserts
/// before the next one runs).
///
/// Three specific deletions redden three specific steps:
/// - deleting the `if !changed { return; }` guard inside
///   `publish_agents_changed_if` reddens the FRAME 2 step (it would then
///   see an event instead of empty);
/// - deleting the `bus.publish(...)` line inside that same function
///   reddens the FRAME 1 step (it would see empty instead of an event);
/// - deleting the exit-site call in `session.rs` reddens the EXIT step.
///
/// The source pin (`the_flush_loop_body_calls_the_sampler_and_the_release`)
/// still covers the one thing this test cannot: that `start_flush_loop`'s
/// own body still calls `publish_agents_changed_if` at all, rather than
/// this test's own direct calls being the only production-shaped caller
/// left standing.
///
/// The command below writes "first", waits for the caller to unblock a
/// `read`/`pause`, then writes "second" — demand-driven rather than a
/// fixed sleep window (fix round 1, review Minor 5: a fixed-delay second
/// write can land before the FIRST `flush_session` poll on a loaded
/// runner, leaving no second frame to observe at all). What must NOT
/// change between the two frames is the *detected* state: `shell` here
/// is `"sh"`/`"cmd.exe"`, which `agent_detect::identify_agent` does not
/// recognise, and an unrecognised agent is `Unknown` "regardless of
/// screen content" (`agent_detect`'s own doc, judgment §8) — so
/// differing visible text is exactly the case that must NOT count as a
/// change here.
///
/// ⚠️ **The Windows command uses ONLY `cmd` builtins (`echo`, `set /p`,
/// `pause`), and that is load-bearing — do not put an external command in
/// it.** From 2026-09-05 this test was RED on Windows at `changed2`, and it
/// was a true positive: `changed` covers state / agent / **program** / label
/// / cwd, there is no `tcgetpgrp` on Windows, and the earlier command's
/// second half ran `ping`, so the foreground answer moved from `cmd.exe` (a
/// builtin has no child) to `PING.EXE`. Measured with an independent
/// `Win32_Process` walk, so the event this asserts must not fire was
/// CORRECTLY fired.
///
/// It was correctly fired about the FIXTURE, though, not about a defect. An
/// interactive Unix shell running `ping` answers `ping` too; this test is
/// green on Unix only because a non-interactive `sh -c` never creates a job
/// and `tcgetpgrp` keeps naming `sh`. Asserting on that difference made this
/// test's subject the platform's process model rather than "different text,
/// same detected state, no event".
///
/// The defect it pointed at is REAL and is fixed elsewhere: the walk now
/// prefers an agent-identifying candidate over the deepest one, so an agent
/// keeps `program` while its tools run
/// (`foreground::pick_foreground`, and
/// `foreground::tests::an_agent_outranks_the_tool_it_spawned_and_only_an_agent_does`
/// is the guard — it goes red if that preference is removed). Removing the
/// confound HERE is only legitimate because that guard exists; without it,
/// this would be retuning a command to hide a finding.
#[tokio::test(flavor = "multi_thread")]
async fn a_changed_sample_reaches_the_bus_an_unchanged_one_does_not_and_exit_publishes_once() {
    let id = "t-runtime-event-wire";
    agents().remove(id);

    let bus = crate::sync_primitives::Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
    let mut rx = bus.subscribe();

    let opts = SpawnOptions {
        command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
        args: if cfg!(windows) {
            // `set /p` reads a LINE from stdin — the Windows analogue of
            // `read x` below — and the trailing `pause` keeps the process
            // alive for the second observation. Both are `cmd` builtins, so
            // this whole command runs in ONE process and the foreground
            // answer is `cmd.exe` at both observation points. See the ⚠️
            // above for why that matters.
            vec![
                "/C".into(),
                // The OSC is ECHOED, not set with cmd's `title`: `title` calls
                // `SetConsoleTitle`, and whether ConPTY forwards that as an
                // OSC is the pseudoconsole's business, not this fixture's.
                // Measured — echoing the bytes reaches the screen; `title` did
                // not, within 2 s.
                "echo \x1b]0;qa-fixture\x07& echo first & set /p x= & echo second & pause".into(),
            ]
        } else {
            vec![
                "-c".into(),
                "printf '\\033]0;qa-fixture\\a'; printf 'first'; read x; \
                 printf 'second'; sleep 30"
                    .into(),
            ]
        },
        rows: 6,
        cols: 40,
        ..Default::default()
    };
    let session = PtySession::spawn(id.into(), &opts, Some(bus.clone())).expect("spawn");

    // Frame 1: a brand-new session's first observation is unconditionally
    // a change (`sample`'s `previous.is_none_or(...)` — nothing to
    // compare against yet).
    let mut changed1 = None;
    for _ in 0..100 {
        let now = chrono::Utc::now().timestamp_millis();
        {
            let outcome = crate::gateway::pty::manager::flush_session(&session, now);
            if outcome.frame.is_some() {
                changed1 = Some(outcome.agent_changed);
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let changed1 = changed1.expect("the child produced no first frame in 2s");
    assert!(
        changed1,
        "a brand-new session's first frame must be reported as a change, \
             or this test proves nothing"
    );

    crate::gateway::pty::manager::publish_agents_changed_if(changed1, &bus);
    assert!(
        matches!(rx.try_recv(), Ok(raw) if is_agents_changed(&raw)),
        "publish_agents_changed_if(true, ..) must deliver runtime.agents.changed \
             to a real subscriber"
    );
    assert!(
        rx.try_recv().is_err(),
        "exactly one event for frame 1 — no more"
    );

    // Settle the LABEL before the second observation, and wait for the
    // fixture's OWN title rather than for a delay.
    //
    // `label` is the OSC title when there is one and the spawn label
    // otherwise, so anything that sets a title moves it — and on Windows
    // `cmd.exe` sets the console title to its own image path a moment after
    // start. Measured: frame 1 carried `label: "cmd.exe"` (no title yet) and
    // frame 2 `label: "C:\\WINDOWS\\system32\\cmd.exe"`, so the row changed for
    // a reason that is CORRECT and is not this test's subject. Both commands
    // therefore claim the title as their first act, and the second observation
    // does not begin until that title is the one on the row: a settle written
    // as "flush until nothing changes" would have exited on the first tick
    // that produced no frame, which is a predicate that cannot fail (判据 §2).
    let mut labelled = false;
    for _ in 0..100 {
        let now = chrono::Utc::now().timestamp_millis();
        crate::gateway::pty::manager::flush_session(&session, now);
        if agents().entry(id).is_some_and(|e| e.label == "qa-fixture") {
            labelled = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        labelled,
        "the child never claimed the OSC title, so the label is still whatever \
         the platform put there and frame 2 would be judged against a moving row"
    );

    // Unblock the child's `read`/`pause` so it writes its second,
    // DIFFERENT visible text on demand rather than racing a fixed sleep.
    session.write_input(b"\r\n").expect("write to unblock read");

    // Frame 2: different visible text, same detected (Unknown) state —
    // must NOT be reported as a change.
    let mut changed2 = None;
    for _ in 0..100 {
        let now = chrono::Utc::now().timestamp_millis();
        {
            let outcome = crate::gateway::pty::manager::flush_session(&session, now);
            if outcome.frame.is_some() {
                changed2 = Some(outcome.agent_changed);
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let changed2 = changed2.expect("the child produced no second frame in 2s");
    assert!(
        !changed2,
        "a second frame with an unchanged detected state must not be \
             reported as a change"
    );

    crate::gateway::pty::manager::publish_agents_changed_if(changed2, &bus);
    assert!(
        rx.try_recv().is_err(),
        "publish_agents_changed_if(false, ..) must publish nothing"
    );

    // Exit path: kill the child; the reader thread's real EOF handling
    // must publish once more (`session.rs`, beside `agents().remove`) —
    // bounded poll, since this crosses a real thread boundary.
    session.kill();

    let mut seen_exit = false;
    for _ in 0..200 {
        if let Ok(raw) = rx.try_recv() {
            if is_agents_changed(&raw) {
                seen_exit = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    agents().remove(id);
    assert!(
        seen_exit,
        "the exit path must publish one more runtime.agents.changed"
    );
    assert!(
        rx.try_recv().is_err(),
        "exactly one event from the exit path — no more"
    );
}

/// F2 (fix round 1, review Important 2): a row landing in the process
/// table must actually reach the caller through `runtime.agents.list` —
/// a handler that always returned `agents: vec![]`, or whose filter
/// dropped every row, would pass every other test in this module.
///
/// Ownership is REAL here, not the `Unknown`-admits-unscoped-caller
/// fallback the other tests in this file lean on: the session is
/// spawned through `pty::manager().spawn()` (not the bare
/// `PtySession::spawn()` the flush-wire tests use), so
/// `PtyManager::owner_of` has an actual `Known(Some("u-owner"))` record
/// — the same mechanism `handlers::pty::require_owned` filters on. The
/// table row itself is seeded directly via `sample()` on a synthetic
/// screen (the `screen()` helper above): this test's subject is the RPC
/// face's ownership filter, not the flush wire, which
/// `a_published_frame_lands_the_session_in_the_table` above already
/// proves.
#[tokio::test(flavor = "multi_thread")]
#[serial_test::parallel(pty_global_manager)]
async fn a_row_reaches_the_caller_through_the_list_rpc_filtered_by_owner() {
    let opts = SpawnOptions {
        created_by: Some("u-owner".to_string()),
        ..Default::default()
    };
    let spawn = crate::gateway::pty::manager().spawn(&opts).expect("spawn");
    let id = spawn.session_id.clone();
    agents().remove(&id);

    let s = screen(b"$ ");
    sample_shell(agents(), &id, &spawn.shell, "", &s, false, 0);

    let list_req = || crate::gateway::protocol::JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "runtime.agents.list".to_string(),
        params: Some(serde_json::json!({})),
        id: Some(serde_json::json!(1)),
    };
    let agent_ids = |resp: &crate::gateway::protocol::JsonRpcResponse| -> Vec<String> {
        let parsed: aleph_protocol::runtime::RuntimeAgentsListResponse =
            serde_json::from_value(resp.result.clone().expect("list always succeeds"))
                .expect("must be the protocol shape");
        parsed.agents.into_iter().map(|e| e.session_id).collect()
    };

    let owner_resp = crate::gateway::caller_identity::CALLER_USER
        .scope(
            Some("u-owner".to_string()),
            crate::gateway::handlers::runtime::handle_list(list_req()),
        )
        .await;
    let owner_ids = agent_ids(&owner_resp);
    assert!(
        owner_ids.contains(&id),
        "the owner must see their own row through runtime.agents.list: {owner_ids:?}"
    );

    let other_resp = crate::gateway::caller_identity::CALLER_USER
        .scope(
            Some("u-other".to_string()),
            crate::gateway::handlers::runtime::handle_list(list_req()),
        )
        .await;
    let other_ids = agent_ids(&other_resp);
    assert!(
        !other_ids.contains(&id),
        "a different actor must not see another owner's row: {other_ids:?}"
    );

    crate::gateway::pty::manager().close(&id).ok();
    agents().remove(&id);
}

/// Spec §5: PTY 会话消失 ⇒ 条目消失. Asserts presence FIRST — otherwise a
/// child that exits before the first flush would let this test pass by
/// observing an absence that was never a presence (判据 §2).
///
/// ⚠️ **This was RED ON WINDOWS until 2026-09-05, and it was a true positive
/// about the product.** The entry never left because the whole settle —
/// `pty.exit`, `manager().remove`, `agents().remove` — hung off
/// `spawn_reader`'s read loop breaking on `Ok(0)`, and ConPTY does not close
/// the pseudoconsole's output pipe when the child exits, so on Windows that
/// break never came: measured `pty.exit` **never**, not late, for a child
/// that exits in ~2 s. Fixed by deriving "the child exited" from the child
/// rather than from the terminal — see `pty/session.rs::settle_exit` and its
/// own guard `a_child_that_exits_settles_the_session_without_needing_terminal_eof`,
/// which pins the mechanism this test only observes the effect of.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_that_exits_leaves_the_table() {
    let id = "t-runtime-exit";
    agents().remove(id);

    let opts = SpawnOptions {
        command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
        args: if cfg!(windows) {
            vec![
                "/C".into(),
                "echo ALEPH_RUNTIME_EXIT & ping -n 3 127.0.0.1 >nul".into(),
            ]
        } else {
            vec!["-c".into(), "printf 'ALEPH_RUNTIME_EXIT'; sleep 1".into()]
        },
        rows: 6,
        cols: 40,
        ..Default::default()
    };
    let session = PtySession::spawn(id.into(), &opts, None).expect("spawn");

    let mut present = false;
    for _ in 0..100 {
        let now = chrono::Utc::now().timestamp_millis();
        crate::gateway::pty::manager::flush_session(&session, now);
        if agents().snapshot().iter().any(|e| e.session_id == id) {
            present = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        present,
        "the session never landed in the table to begin with"
    );

    let mut gone = false;
    for _ in 0..200 {
        if !agents().snapshot().iter().any(|e| e.session_id == id) {
            gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    session.kill();
    agents().remove(id);
    assert!(gone, "the reader thread's exit path must drop the entry");
}

/// THE WIRE, half two — the frame the behavioural test cannot reach.
///
/// `a_published_frame_lands_the_session_in_the_table` calls
/// `flush_session` directly, so reverting `start_flush_loop`'s body to
/// `session.feed_and_take_frame()` — a plausible merge resolution — leaves
/// every test in this module and the whole `pty::` suite green while the
/// table is empty forever in production (判据 §7). `start_flush_loop`
/// takes `&'static self` and latches on a process-global `STARTED`, so it
/// cannot be driven from a test; the repo's answer for exactly this class
/// is a source-level pin (precedents:
/// `execution_engine/run_loop/flow_scope_census.rs`,
/// `orchestrator/dispatch.rs::the_harness_spawn_reestablishes_the_run_tree_originator`).
///
/// `release_expired` is pinned by the same test and for the same reason:
/// it is the only thing that ever releases a held idle, and it too has a
/// single unwitnessed call site.
///
/// Task 6 adds a third thing this same unwitnessed body must do: fold
/// both triggers (`flush_session` reporting `changed` for any session
/// touched this tick, and `release_expired` returning a non-empty `Vec`)
/// into one bool and call `publish_agents_changed_if` with it — ONE call
/// site (fix round 1, review F4/Minor 8 — coalesced per tick, so the
/// two triggers share the one call rather than each getting their own).
/// The behavioural half of this proof
/// (`a_changed_sample_reaches_the_bus_an_unchanged_one_does_not_and_exit_publishes_once`)
/// drives `publish_agents_changed_if` directly against a real bus and
/// cannot see whether `start_flush_loop` itself still calls it — this
/// pin covers the call site's existence, not whether the bool handed to
/// it is folded correctly from both triggers.
///
/// A missing file or a missing/renamed function FAILS — it does not skip.
/// A guard that goes quiet when it cannot find its subject is not a guard
/// (判据 §2).
#[test]
fn the_flush_loop_body_calls_the_sampler_and_the_release() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("gateway")
        .join("pty")
        .join("manager.rs");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

    // `code_text` rather than the bare `strip_comment_lines`: it also
    // drops string-literal PAYLOADS, so the brace walk below cannot be
    // thrown off by a `{` inside a message. Comments go either way.
    let code =
        crate::utils::source_scan::code_text(&crate::utils::source_scan::production_prefix(&src));

    let at = code.find("fn start_flush_loop").expect(
        "start_flush_loop not found in manager.rs — if it was renamed, \
             re-point this pin; if it was deleted, the flush loop is gone",
    );
    let open = code[at..].find('{').expect("start_flush_loop has no body") + at;
    let mut depth = 0usize;
    let mut close = None;
    for (i, ch) in code[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close.expect("start_flush_loop's body is not brace-balanced");
    let body = &code[open..=close];

    assert!(
        body.contains("flush_session("),
        "start_flush_loop must call flush_session — without it the sampler \
             has no production caller and the runtime table is empty forever \
             with every test green. Body was:\n{body}"
    );
    assert!(
        body.contains("release_expired("),
        "start_flush_loop must call release_expired — without it a held \
             working->idle observation is never believed and a finished agent \
             reads Working forever. Body was:\n{body}"
    );
    assert!(
        body.contains("mark_quiet("),
        "start_flush_loop must call mark_quiet — a session that goes quiet \
             produces no frame, so the per-session loop cannot reach it and \
             `quiet_since` would be a wire field with no producer, publishing \
             null forever with every unit test green. Body was:\n{body}"
    );
    assert!(
        body.contains("publish_agents_changed_if("),
        "start_flush_loop must call publish_agents_changed_if — without it \
             neither trigger (flush_session's `changed`, or a non-empty \
             release_expired()) can ever reach a subscriber, and the RPC list \
             and the change event can disagree forever with every test green. \
             Body was:\n{body}"
    );
}

/// Every `agent_panel.rs` frontend file in the repo, EXCEPT the shared
/// owner that legitimately sorts (`shared/ui_logic/src/state/agent_panel.rs`
/// — the single source `sort_entries` lives in).
///
/// Derived rather than hand-listed (Task 10, R10-5, 判据 §5): a fixed
/// two-path list only covers the frontends that exist on the day it is
/// written, so a THIRD frontend `agent_panel.rs` (a future mobile
/// client, a desktop-native surface) would sit silently outside a
/// hardcoded pair. Walking the tree instead means any file with that
/// exact name is picked up automatically, wherever it lands.
///
/// `target/`, `interfaces/webchat/node_modules/` and `graphify-out/` are
/// skipped, not for correctness (none can contain a `.rs` file this
/// guard cares about) but because `target/` alone is >100GB of build
/// output and `graphify-out/` (Task 10 fix round 1, F6) is a 1.3 GB,
/// `.gitignore`d, machine-generated tree that the reviewer measured at
/// 59% of this walk's 15,689 visited entries — this repo's existing
/// walker (`utils::source_scan::rust_sources_under`) has no such skip
/// because every current call site points it at `src/`, never at the
/// repo root, so this guard writes its own rather than pointing that
/// one somewhere it was never meant to run.
///
/// # Does not follow symlinks (Task 10 fix round 2, #9)
///
/// Recursion is gated on `DirEntry::file_type()`, not `Path::is_dir()`
/// — the latter follows symlinks, so a directory symlink would be
/// descended into with no visited-set and no depth cap, and a symlink
/// CYCLE would not merely run slow, it would stack-overflow the whole
/// `cargo test -p alephcore --lib` binary (an infrastructure failure,
/// not a guard result). No directory symlink is reachable here today —
/// the only ones in the repo live under `node_modules/` and
/// `desktop/macos/bridge/.build/`, both already skipped by name — so
/// this was a latent hazard rather than a live bug, but `file_type()`
/// removes it for the cost of one method call.
///
/// # False positives this walk can produce (判据 §3 — the expensive
/// direction; Task 10 fix round 2, #10)
///
/// This walk is not scoped to `interfaces/`: `archive/` (72 `.rs` files
/// measured at review time), `examples/`, `benches/`, `tests/` and
/// `docs/` are walked too. A file legitimately named `agent_panel.rs`
/// living in any of those trees — an archived copy, a doc example —
/// would be picked up by this walk and scanned by the ordering guard
/// below as if it were a live frontend, reddening on its own valid
/// `.sort_by`. Exactly three `agent_panel.rs` files exist in the repo
/// today (the two live frontends and the shared owner excluded below),
/// so this is hypothetical, not live — but the false-positive
/// direction is the one that gets a guard weakened by the next person
/// it wrongly blocks, so it is worth knowing before someone trips it.
fn agent_panel_frontend_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // `entry.file_type()` reports the entry itself and does NOT
            // follow symlinks (unlike `path.is_dir()`), so a symlinked
            // directory is neither recursed into nor treated as a
            // directory at all — see the symlink note above.
            let is_real_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if is_real_dir {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == "target"
                    || name == "node_modules"
                    || name == "graphify-out"
                    || name.starts_with('.')
                {
                    continue;
                }
                walk(&path, out);
            } else if path.file_name().and_then(|n| n.to_str()) == Some("agent_panel.rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(root, &mut files);
    let owner = root.join("shared/ui_logic/src/state/agent_panel.rs");
    files.retain(|p| p != &owner);
    files
}

/// Substrings this guard treats as "this file performs its own
/// ordering" (Task 10 fix round 1, F1+F2, reviewer-verified).
///
/// `.sort` — not `.sort_by` — is the token, because `.sort_by` alone
/// left `.sort_unstable_by` / `.sort_unstable_by_key` / `.sort_unstable()`
/// GREEN: a real `.sort_unstable_by` inserted into the TUI panel's
/// production code passed the old check, and `sort_unstable*` appears
/// 74× in this repo's own first-party code — an established idiom
/// here, not a theoretical dodge. `.sort` as a substring still catches
/// `.sort_by`, `.sort_by_key`, `.sort()` and every `.sort_unstable*`
/// spelling in one token. `.reverse()` is separate because it shares no
/// substring with `.sort` at all, and it is the cheapest possible way
/// to make the two frontends disagree without writing anything that
/// reads like sorting.
///
/// # What this guard cannot see (判据 §5 — name the gaps, don't
/// pretend they are closed)
///
/// Token-list gaps, specific to `BANNED_ORDERING_TOKENS`:
///
/// - `.collect::<BTreeMap<_, _>>()` / `BTreeSet` — ordering with no
///   ordering CALL to grep for at all.
/// - `.min_by` / `.max_by` / `.min_by_key` — picks one row rather than
///   reordering the rest, so it is a parity risk only for a "top
///   agent" affordance, not full-list ordering.
/// - A `binary_search`-and-insert that maintains order incrementally.
/// - An ordering call moved into a sibling file next to
///   `agent_panel.rs` (`agent_panel_rows.rs`, a `render_rows` in
///   `widgets/mod.rs`) — this guard is keyed on the file NAME, not the
///   widget. Left uncovered deliberately: no neighbour of either panel
///   (`btw_panel.rs`, `session_picker.rs`, `provider_picker.rs`, …)
///   currently sorts, and a directory-scoped scan trades this gap for
///   a false-positive one the day a legitimate sibling widget (a
///   picker) starts sorting its own rows — 判据 §3: the false-positive
///   direction is the expensive one, because the next person weakens
///   the guard to get past it.
///
/// A gap inherited from `production_prefix`, not introduced here (Task
/// 10 fix round 2, #11):
///
/// - A line whose TEXT begins with `#[cfg(test)]` while actually being
///   string- or comment-literal payload is read by `production_prefix`
///   as a live attribute, which discards everything from there to the
///   end of that (mis-detected) item — the silent-approval direction
///   (`production_prefix`'s own doc comment, "Known gap (F2, review
///   round 4, unfixed)", has the full account). Zero reachable
///   instances in either frontend file as of this writing; noted here
///   so a reader of THIS guard does not have to go find that fact in
///   another module's doc comment to know it applies here too.
const BANNED_ORDERING_TOKENS: [&str; 2] = [".sort", ".reverse()"];

/// `code_text(production_prefix(src))`, extracted so the guard below
/// and its true-negative fixture
/// (`the_stripper_survives_sort_by_named_only_in_prose`) call the exact
/// same stripping instead of each re-spelling it (Task 10 fix round 1,
/// F5). Before this extraction the fixture wrote its own composition
/// of the two calls, so weakening the guard to the weaker
/// `strip_comment_lines` (the `live_apply.rs:477` precedent) would have
/// left the fixture green while the guard started firing on string
/// literals and doc comments — 判据 §1: two representations of one
/// fact, and the weaker one is the one that ships.
fn scrub(src: &str) -> String {
    crate::utils::source_scan::code_text(&crate::utils::source_scan::production_prefix(src))
}

/// The two frontends this guard is known to protect today, asserted by
/// MEMBERSHIP in the derived walk's output rather than merely counted
/// (Task 10 fix round 2, #8 — REPLACES the earlier `files.len() >= 2`
/// floor rather than sitting beside it).
///
/// A count floor passes as long as the walk finds ANY two files named
/// `agent_panel.rs`, including two WRONG ones — if a real frontend were
/// ever renamed out from under the walk on the same day an unrelated
/// stray `agent_panel.rs` appeared under `archive/` or `examples/`
/// (判据 §3's false positive, noted on the walk above), the count would
/// still read 2 and the floor would stay silently green. Asserting
/// these two specific paths are members of the walk's output does not
/// have that hole: it can only pass if the walk actually reached the
/// frontends it is supposed to guard, identity and all. The derived
/// walk still catches a third, unlisted frontend automatically — this
/// only pins that these two specific ones are never silently dropped.
const KNOWN_FRONTENDS: [&str; 2] = [
    "interfaces/tui/src/tui/widgets/agent_panel.rs",
    "interfaces/webchat/src/components/sidebar/agent_panel.rs",
];

/// R2: sorting lives ONLY in `shared_ui_logic::state::agent_panel::sort_entries`.
/// Neither frontend's `agent_panel.rs` may perform its own ordering call.
///
/// `code_text` (not the weaker `strip_comment_lines` the `live_apply.rs`
/// precedent uses) is deliberate here (R10-8): it strips comments AND
/// string-literal payloads over one lexer walk, and the property this
/// guard checks is "this file performs no ordering call" — a `.sort_by`
/// spelled inside a string literal is not a call either, any more than
/// one spelled inside a doc comment is.
///
/// A missing known frontend, or an ordering call in one that IS found,
/// FAILS rather than vacuously passing (判据 §2 / §8): "I found nothing
/// to check" is not the same fact as "I checked and it's clean".
#[test]
fn no_frontend_sorts_its_own_agent_panel_entries() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = agent_panel_frontend_files(root);

    for known in KNOWN_FRONTENDS {
        let expected = root.join(known);
        assert!(
            files.contains(&expected),
            "expected {} among the derived agent_panel.rs frontend files, \
                 but it was not found (found: {files:?}); a missing known \
                 frontend means this walk is not finding what it is supposed \
                 to guard — a silent pass, not a clean one.",
            expected.display()
        );
    }

    for path in &files {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| {
            panic!(
                "{}: {e} — a missing frontend file is not a pass",
                path.display()
            )
        });
        let code = scrub(&src);
        let hit = BANNED_ORDERING_TOKENS
            .iter()
            .find(|token| code.contains(**token));
        assert!(
            hit.is_none(),
            "{} sorts its own agent-panel entries (found `{}`); sorting \
                 belongs to shared_ui_logic::state::agent_panel::sort_entries (R2)",
            path.display(),
            hit.unwrap_or(&"")
        );
    }
}

/// True-negative fixture for the guard above (R10-6, reversed by R10-8):
/// a comment or string literal that merely NAMES `.sort_by`/`.sort()`/
/// `.reverse()` — documenting the very rule this guard enforces — must
/// not redden it. Kept here, next to the assertion it proves, rather
/// than as a production doc comment in another crate that a future
/// author with no idea this guard exists could reword out from under
/// it. Calls the same `scrub` the guard above calls (F5) so weakening
/// one cannot silently stop tracking the other.
#[test]
fn the_stripper_survives_sort_by_named_only_in_prose() {
    let synthetic = "\
//! module doc naming `.sort_by`, `.sort()` and `.reverse()` so nobody re-adds them\n\
/// doc comment: this widget must never call `.sort_by`, `.sort()` or `.reverse()`\n\
// plain comment, also just prose: .sort_by(...) .sort() .reverse()\n\
pub fn render() {\n\
    // still just a comment inside a function body: .sort_by .reverse()\n\
    let _ = \"a string literal mentioning .sort_by and .reverse() too\";\n\
}\n";
    let code = scrub(synthetic);
    let hit = BANNED_ORDERING_TOKENS
        .iter()
        .find(|token| code.contains(**token));
    assert!(
        hit.is_none(),
        "code_text must strip `.sort`/`.reverse()` when they appear only \
             in `//`, `///` and `//!` comments or inside a string literal — \
             otherwise the guard above would redden on prose, and a guard \
             that fires on prose gets weakened by the next person who trips \
             it (判据 §3). Found `{}` in code after stripping:\n{code}",
        hit.unwrap_or(&"")
    );
}

/// The first link of the row-NAME chain. Any self-rolled derivation of
/// "what is running here" has to read `program`, whatever else it is
/// spelled with — `.or(..)`, a `match`, an `if let`, three nested
/// `unwrap_or`s — so this one token covers spellings a copy of
/// `entry_name`'s exact body would not.
///
/// Deliberately WIDER than "no copy of `entry_name`": if a face ever
/// genuinely needs the raw `program` for something that is not the row
/// name (a tooltip separating it from `agent`, say), that is a new
/// SHARED derivation and this guard is what forces the conversation,
/// instead of letting a second local answer appear the way the first
/// one did. Widening it is a deliberate edit with a reason, which is
/// the whole difference from a copy that arrives by accident.
///
/// Same gaps as [`BANNED_ORDERING_TOKENS`]: whitespace between the dot
/// and the field, a derivation moved into a sibling file, and
/// `production_prefix`'s mis-detected-`#[cfg(test)]` hole. Named here
/// rather than re-derived by the next reader.
const BANNED_NAME_DERIVATION_TOKENS: [&str; 1] = [".program"];

/// The call each frontend must make instead. Asserting only the
/// absence of `.program` would pass on a face that dropped the call
/// and rendered `entry.label` directly — the two faces would then name
/// the same row differently with nothing red (判据 §4: assert the
/// effect arrived, not that the copy is gone).
const REQUIRED_NAME_CALL: &str = "entry_name(";

/// R2's other half: the row NAME is derived ONCE, in
/// `shared_ui_logic::state::agent_panel::entry_name`.
///
/// Both frontends held a byte-identical copy of the
/// `program → agent → label` chain, and the Panel's doc asserted parity
/// with the TUI's in prose — a claim about another crate's file that
/// nothing checked (判据 §1 / §9). None of the existing machinery could
/// have caught it: `agent_panel_parity.rs` scopes itself to ORDERING in
/// its own header, and the guard above looks for a self-rolled `.sort`,
/// not for a self-rolled name.
///
/// Two halves, and neither alone is the property — see
/// [`BANNED_NAME_DERIVATION_TOKENS`] and [`REQUIRED_NAME_CALL`] for what
/// each one buys. The negative half runs over every file the walk finds
/// (a third frontend inherits it automatically); the positive half runs
/// over [`KNOWN_FRONTENDS`] only, because those are the two files known
/// to render rows and an unlisted `agent_panel.rs` may legitimately not.
///
/// `scrub` is shared with the ordering guard, so the doc comments above
/// and the frontends' own comments — which name `program` and
/// `entry_name` in prose, deliberately — cannot redden this.
#[test]
fn no_frontend_derives_its_own_agent_row_name() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = agent_panel_frontend_files(root);

    for known in KNOWN_FRONTENDS {
        let expected = root.join(known);
        assert!(
            files.contains(&expected),
            "expected {} among the derived agent_panel.rs frontend files, \
                 but it was not found (found: {files:?}); a missing known \
                 frontend means this walk is not finding what it is supposed \
                 to guard — a silent pass, not a clean one.",
            expected.display()
        );
    }

    for path in &files {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| {
            panic!(
                "{}: {e} — a missing frontend file is not a pass",
                path.display()
            )
        });
        let code = scrub(&src);

        let hit = BANNED_NAME_DERIVATION_TOKENS
            .iter()
            .find(|token| code.contains(**token));
        assert!(
            hit.is_none(),
            "{} derives its own agent-row name (found `{}`); the \
                 program → agent → label chain belongs to \
                 shared_ui_logic::state::agent_panel::entry_name, which both \
                 faces call — a second copy is how the two came to claim \
                 parity in prose with nothing checking it (判据 §1)",
            path.display(),
            hit.unwrap_or(&"")
        );

        if KNOWN_FRONTENDS
            .iter()
            .any(|known| path == &root.join(known))
        {
            assert!(
                code.contains(REQUIRED_NAME_CALL),
                "{} renders agent rows but never calls `{REQUIRED_NAME_CALL}`; \
                     dropping the call and naming rows some other way passes \
                     the ban above while giving the two faces different names \
                     for the same row",
                path.display()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Task A2: the foreground probe reaches identification, and silence is
// published as a fact rather than folded into the state.
// ---------------------------------------------------------------------------

/// A `SampleInput` carrying a probe result, for the tests that are about the
/// probe. The complement of [`sample_shell`].
fn sample_program(
    agents: &RuntimeAgents,
    session_id: &str,
    program: &str,
    screen: &Screen,
    now: i64,
) -> bool {
    agents.sample(SampleInput {
        session_id,
        shell: "zsh",
        program: Some(program),
        argv: &[],
        cwd: "",
        screen,
        process_exited: false,
        frame_produced: true,
        now,
    })
}

/// A screen that matches `claude.toml`'s `live_turn_working` rule
/// (priority 970, region `bottom_non_empty_lines(12)`,
/// `^\s*[⏸⏵].*esc to interrupt(?:\s|·|$)`). Nothing above it in priority can
/// match this text: the two 980 blocked rules and the 1000 transcript rule all
/// require `esc to cancel` or `showing detailed transcript`, and the 1100
/// working rule reads the OSC title, which is empty here.
const CLAUDE_WORKING_LINE: &str = "\u{23F5} pretending to work esc to interrupt";

/// THE WIRE THIS WHOLE ROUND EXISTS FOR.
///
/// It replaces the KNOWN GAP that used to stand in `sample`'s doc: production
/// identified agents from the SPAWN label, so a user who opened a terminal and
/// typed `claude` was published as `Unknown` forever. Every other guard in
/// this file could stay green through that, because they all pass the agent's
/// name in as `shell` themselves.
///
/// So this one refuses to name the agent anywhere. It starts a real shell on a
/// real PTY, puts a directory on its `PATH` containing an executable called
/// `claude`, and types `claude`. The only thing that can turn that into
/// `agent: Some("claude")` is the foreground probe: `PtySession::shell` is
/// `sh` throughout.
///
/// Falsification (task A2 step 4, run by hand — output in the report): comment
/// out the line in `manager::flush_session` that feeds `foreground_fact()`
/// into `SampleInput::program`, and this goes red while every other test in
/// the file stays green.
///
/// ⚠️ It carried `#[cfg(unix)]` until 2026-09-05, and Windows is the platform
/// that needs it MOST: there is no `tcgetpgrp` there, so the identification
/// this asserts can only arrive through
/// `foreground::foreground_fact_for_shell`, the one branch no developer or CI
/// job had ever executed. The per-platform halves are in [`fake_claude_on_path`],
/// which also says which single assertion Windows skips and why.
#[tokio::test(flavor = "multi_thread")]
async fn a_real_agent_started_after_spawn_is_identified() {
    let id = "t-runtime-foreground-identify";
    agents().remove(id);

    let dir = tempfile::tempdir().expect("tempdir");
    let fake = fake_claude_on_path(dir.path());

    let opts = SpawnOptions {
        command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
        rows: 6,
        cols: 60,
        ..Default::default()
    };
    let session = PtySession::spawn(id.into(), &opts, None).expect("spawn");

    // Let the shell reach its prompt before typing, so the line is not eaten
    // by start-up output.
    for _ in 0..20 {
        let now = chrono::Utc::now().timestamp_millis();
        if crate::gateway::pty::manager::flush_session(&session, now)
            .frame
            .is_some()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    session
        .write_input(fake.command_line.as_bytes())
        .expect("write the command");

    let mut identified = None;
    // Must outlast the GATE, not just the program. One frame (the echoed
    // command line) authorises probes for PROBE_FRAME_BUDGET *
    // PROBE_MIN_INTERVAL_MS = 3 s, and under a full-suite run this thread is
    // not getting a tick every 20 ms. A budget equal to the gate's own window
    // asserts at the exact moment the gate stops looking, which is how this
    // read green alone and red in the suite (measured 2026-09-05). Costs
    // nothing when the condition is met early: the loop breaks on the hit.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        let now = chrono::Utc::now().timestamp_millis();
        let _ = crate::gateway::pty::manager::flush_session(&session, now);
        if let Some(e) = agents().snapshot().into_iter().find(|e| e.session_id == id) {
            // Wait for everything the assertions below check, not just the
            // identification. The probe can name the agent a beat before the
            // fake `claude`'s chrome has been read into the screen, so a loop
            // that broke on identification alone gave up the rest of its
            // budget and then asserted on a screen that had only the echoed
            // command line — green when the machine was idle, red under a
            // full-suite run (observed 2026-09-04 during the task M merge:
            // `agent: Some("claude"), program: Some("claude"), state: Idle`).
            // A break condition weaker than the assertion is the assertion
            // racing itself.
            let hit = e.agent.as_deref() == Some("claude")
                && e.program.as_deref() == Some(fake.expected_program)
                && (!fake.paints_chrome || e.state == RuntimeAgentState::Working);
            identified = Some(e);
            if hit {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let observed = session.foreground_fact();
    let painted = session.with_screen(crate::gateway::pty::screen::Screen::visible_text);
    session.kill();
    agents().remove(id);

    let entry = identified.expect("the session never reached the runtime table at all");
    eprintln!("probe observed: {observed:?}");
    eprintln!("entry: {entry:?}");
    eprintln!("screen:\n{painted}");
    assert_eq!(
        entry.agent.as_deref(),
        Some("claude"),
        "an agent started AFTER spawn must be identified from the foreground \
         process, not from the spawn label ({:?}). Probe saw: {observed:?}",
        session.shell
    );
    assert_eq!(
        entry.program.as_deref(),
        Some(fake.expected_program),
        "the probed program must reach the wire, not just the identification. \
         Probe saw: {observed:?}"
    );
    if fake.paints_chrome {
        assert_eq!(
            entry.state,
            RuntimeAgentState::Working,
            "the screen paints claude.toml's live_turn_working chrome, which is \
             only reachable once the agent is identified"
        );
    }
}

/// The fake agent this guard plants on `PATH`, and the two things about it
/// the assertions have to know.
struct FakeClaude {
    /// What to type at the shell's prompt to put it in the foreground.
    command_line: String,
    /// What `program` must come back as. NOT the same string on both
    /// platforms, and that is a measured fact rather than a concession:
    /// `sysinfo` reports `claude.exe` on Windows, `normalized_agent_lookup_name`
    /// strips the extension to identify the AGENT, and
    /// `normalized_program_name` returns the token it looked at — so the
    /// panel prints `claude` on macOS and `claude.exe` on Windows. Written
    /// down here rather than normalised away because normalising it is a
    /// change to what every platform prints, which is a product call and not
    /// a test's to make.
    expected_program: &'static str,
    /// Whether the fake paints Claude's `live_turn_working` chrome, so the
    /// `state` assertion is reachable. See [`fake_claude_on_path`].
    paints_chrome: bool,
}

/// Plant a fake `claude` in `dir` and say how to run it.
///
/// Unix: a `#!/bin/sh` script that prints the working chrome and sleeps. The
/// process is a shell whose `argv[0]` is the script, which is the shape
/// `identify_agent_from_process` was written against.
///
/// Windows: a COPY of `ping.exe` named `claude.exe`. It has to be a real
/// image and not a `.cmd` shim, because a shim runs as a second `cmd.exe`
/// whose own child is the long-lived process — and the walk answers with the
/// DEEPEST descendant, so the fact would name the shim's child and never the
/// agent. A native `claude.exe` is also the shape this machine actually has
/// (`C:\Users\…\.local\bin\claude.exe`, measured 2026-09-05).
///
/// ⚠️ The Windows fake deliberately paints NO chrome, so the `state`
/// assertion is skipped there. `cmd.exe` echoes in the console output
/// codepage, so an `echo ⏸ … esc to interrupt` typed as UTF-8 arrives
/// mangled and the rule would not match — a guard that failed for that
/// reason would be reporting an encoding accident as a detection defect.
/// Screen-derived state is covered platform-independently by the dozen tests
/// in this file that feed the screen directly; what THIS guard owns is the
/// foreground-probe wire, and that half is asserted on both platforms.
fn fake_claude_on_path(dir: &std::path::Path) -> FakeClaude {
    #[cfg(windows)]
    {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let ping = std::path::Path::new(&system_root)
            .join("System32")
            .join("PING.EXE");
        let fake = dir.join("claude.exe");
        std::fs::copy(&ping, &fake)
            .unwrap_or_else(|e| panic!("copy {} -> {}: {e}", ping.display(), fake.display()));
        FakeClaude {
            // `>nul` keeps ping's own output off the screen; the assertions
            // here are about the process table, not about what it painted.
            command_line: format!(
                "set \"PATH={};%PATH%\" && claude -n 31 127.0.0.1 >nul\r\n",
                dir.display()
            ),
            expected_program: "claude.exe",
            paints_chrome: false,
        }
    }
    #[cfg(unix)]
    {
        let fake = dir.join("claude");
        std::fs::write(
            &fake,
            // The leading newline is load-bearing, not cosmetic. The PTY line
            // discipline echoes the typed command instantly while `sh` writes
            // its prompt whenever it gets scheduled, so under a full-suite run
            // the echo lands first and the prompt ends up on the NEXT line —
            // the fake's chrome is then appended to it as
            // `$ ⏵ pretending to work esc to interrupt`. `live_turn_working`
            // is anchored `^\s*[⏸⏵]`, so a `$ ` prefix means it cannot match;
            // the fake prints once and sleeps, so no later frame ever corrects
            // the row and the entry stays `Idle` forever (observed 2026-09-06
            // in a full `--lib` run: identified agent + program, chrome on
            // screen, `state: Idle`). Starting on a fresh line makes the
            // chrome begin at column 0 whatever the shell left behind.
            format!("#!/bin/sh\nprintf '\\n%s\\n' '{CLAUDE_WORKING_LINE}'\nsleep 30\n"),
        )
        .expect("write fake claude");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake claude");
        FakeClaude {
            command_line: format!("export PATH={}:$PATH; claude\n", dir.display()),
            expected_program: "claude",
            paints_chrome: true,
        }
    }
}

/// Silence is a fact about OUTPUT, not a state (spec R2-3). A working agent
/// that stops printing must report how long it has been quiet and stay
/// `Working` — anything that let the clock turn it `Idle` would be
/// manufacturing evidence.
#[test]
fn a_quiet_working_agent_reports_quiet_since_without_becoming_idle() {
    let agents = RuntimeAgents::default();
    let working = screen(CLAUDE_WORKING_LINE.as_bytes());
    sample_program(&agents, "s1", "claude", &working, 1_000);
    assert_eq!(agents.snapshot()[0].state, RuntimeAgentState::Working);
    assert_eq!(
        agents.snapshot()[0].quiet_since,
        None,
        "a session that just produced a frame is not quiet"
    );

    assert!(
        agents.mark_quiet(1_000 + QUIET_AFTER_MS - 1).is_empty(),
        "one millisecond before the threshold is not yet quiet"
    );

    assert_eq!(
        agents.mark_quiet(1_000 + QUIET_AFTER_MS),
        vec!["s1".to_string()],
        "the flip must name the session, so the caller can publish it"
    );
    let row = agents.snapshot().remove(0);
    assert_eq!(
        row.quiet_since,
        Some(1_000),
        "quiet_since is the moment of the LAST frame, not the moment we noticed"
    );
    assert_eq!(
        row.state,
        RuntimeAgentState::Working,
        "SILENCE IS NOT IDLE -- this is the assertion spec R2-3 exists for"
    );

    // A new frame ends the quiet.
    sample_program(&agents, "s1", "claude", &working, 40_000);
    assert_eq!(
        agents.snapshot()[0].quiet_since,
        None,
        "a frame must clear the quiet mark"
    );
}

/// The change predicate keys on the None<->Some FLIP, never on the value.
///
/// Keying on the value would fire an event on every tick a session stayed
/// quiet, because "how long has it been quiet" grows without anything
/// happening — the noise clients learn to ignore, and then they ignore the
/// real ones too (R6-4).
#[test]
fn quiet_flip_is_a_change_but_quiet_value_is_not() {
    let agents = RuntimeAgents::default();
    let working = screen(CLAUDE_WORKING_LINE.as_bytes());
    sample_program(&agents, "s1", "claude", &working, 1_000);
    let before = agents.generation();

    assert_eq!(
        agents.mark_quiet(1_000 + QUIET_AFTER_MS).len(),
        1,
        "the flip into quiet is a change"
    );
    let after_flip = agents.generation();
    assert!(after_flip > before, "the flip must bump the generation");

    for extra in [1_000, 10_000, 600_000] {
        assert!(
            agents.mark_quiet(1_000 + QUIET_AFTER_MS + extra).is_empty(),
            "staying quiet is not news -- only the transition is"
        );
    }
    assert_eq!(
        agents.generation(),
        after_flip,
        "an already-quiet session must not bump the generation as it ages"
    );
    assert_eq!(
        agents.snapshot()[0].quiet_since,
        Some(1_000),
        "and the value must not drift while it stays quiet"
    );
}

/// herdr's `!agent_changed` term in the idle hold (`agent_detection.rs:51`),
/// which this module could not carry until now.
///
/// The old comment said so explicitly: "a session's `shell` is fixed at spawn,
/// so the identified agent cannot change within one session — this term comes
/// back the day a phase identifies the agent from something mutable". The
/// probe is that day. A hold is an argument about ONE agent's transition; when
/// the agent underneath changes, the argument is about a different program and
/// must not survive.
#[test]
fn agent_change_clears_the_idle_hold() {
    let agents = RuntimeAgents::default();
    let working = screen(CLAUDE_WORKING_LINE.as_bytes());
    let blank = screen(b"$ ");

    sample_program(&agents, "s1", "claude", &working, 1_000);
    assert_eq!(agents.snapshot()[0].state, RuntimeAgentState::Working);

    // Same agent, plain idle: held at Working (the existing behaviour).
    sample_program(&agents, "s1", "claude", &blank, 2_000);
    assert_eq!(
        agents.snapshot()[0].state,
        RuntimeAgentState::Working,
        "without an agent change the hold still runs -- otherwise this test \
         proves nothing about the change clearing it"
    );

    // A DIFFERENT agent on the same session: the hold is about the old one.
    sample_program(&agents, "s1", "codex", &blank, 2_100);
    let row = agents.snapshot().remove(0);
    assert_eq!(
        row.agent.as_deref(),
        Some("codex"),
        "the probe changed the identified agent"
    );
    assert_eq!(
        row.state,
        RuntimeAgentState::Idle,
        "a hold started for claude must not keep codex at Working"
    );
    assert!(
        agents.release_expired(2_100 + 10_000).is_empty(),
        "and nothing may still be pending"
    );
}

/// `terminal{wait}` (task W-D) needs to be woken by any observable change, and
/// a waiter that misses one sleeps until its timeout while the thing it asked
/// about has already happened.
///
/// So the generation counter is asserted against every producer that can make
/// the table look different: a changed sample, a released idle hold, a quiet
/// flip, and a removal — a removal of an ABSENT row included, because a
/// session that exits before its first sample is exactly the case a waiter
/// cannot learn about any other way. An unchanged sample must NOT bump it — a
/// watch that fires at the 16 ms flush cadence is a busy loop with extra
/// steps, and that is the one case where "nothing to report" is the truth.
#[test]
fn subscribe_bumps_on_every_observable_change() {
    let agents = RuntimeAgents::default();
    let rx = agents.subscribe();
    let working = screen(CLAUDE_WORKING_LINE.as_bytes());
    let blank = screen(b"$ ");

    let start = *rx.borrow();
    assert_eq!(start, agents.generation(), "the receiver sees the counter");

    // 1. a changed sample
    assert!(sample_program(&agents, "s1", "claude", &working, 1_000));
    let after_sample = *rx.borrow();
    assert!(after_sample > start, "a changed sample must bump");

    // 2. an UNCHANGED sample must not
    assert!(!sample_program(&agents, "s1", "claude", &working, 1_100));
    assert_eq!(
        *rx.borrow(),
        after_sample,
        "an unchanged sample must not bump -- otherwise every 16ms tick wakes \
         every waiter"
    );

    // 3. a released idle hold
    sample_program(&agents, "s1", "claude", &blank, 2_000);
    let before_release = *rx.borrow();
    assert_eq!(
        agents.release_expired(2_000 + IDLE_HOLD_MS),
        vec!["s1".to_string()]
    );
    let after_release = *rx.borrow();
    assert!(after_release > before_release, "a released hold must bump");

    // 4. a quiet flip
    assert_eq!(agents.mark_quiet(2_000 + QUIET_AFTER_MS).len(), 1);
    let after_quiet = *rx.borrow();
    assert!(after_quiet > after_release, "a quiet flip must bump");

    // 5. a removal
    agents.remove("s1");
    assert!(*rx.borrow() > after_quiet, "a removal must bump");

    // 6. removing something that was not there ALSO bumps: a waiter on a
    // session that exited before it was ever sampled has no row to lose, and
    // the exit is precisely the answer it is waiting for
    // (`builtin_tools::terminal::tests::wait_reports_gone_when_an_unsampled_session_exits`).
    let after_remove = *rx.borrow();
    agents.remove("never-existed");
    assert!(
        *rx.borrow() > after_remove,
        "an exit must wake waiters even when the table had no row to drop"
    );
}

/// S3: building the screen text costs an allocation the size of the visible
/// grid, per session, per frame — and when no agent is identified the engine
/// discards it unread (`agent_detect::detect`'s permanent early return for
/// `agent: None`).
///
/// Counting is the only way to assert this: the text is a pure value, so its
/// absence is invisible from the outside. Move `screen.visible_text()` back
/// above the identification and the first assertion goes red.
/// The counter is per-table, so this test owns it outright — an earlier
/// version used a process-global `static` and read **9** where it expected 1,
/// because sibling tests in the same binary sample concurrently.
#[test]
fn identify_runs_before_the_screen_text_is_built() {
    let agents = RuntimeAgents::default();
    let s = screen(b"whatever the shell printed");

    sample_shell(&agents, "s1", "zsh", "", &s, false, 0);
    assert_eq!(
        agents.visible_text_builds(),
        0,
        "an unidentified program has no manifest to match, so the screen text \
         must never be built for it"
    );

    sample_program(&agents, "s2", "claude", &s, 0);
    assert_eq!(
        agents.visible_text_builds(),
        1,
        "and exactly once when there IS an agent -- a counter that can only \
         report zero is not measuring anything (判据 §2)"
    );
}

/// A sample that carries a probe result but NO frame — the shape
/// `flush_session` produces when the foreground program moved while the screen
/// stood still.
fn sample_program_without_a_frame(
    agents: &RuntimeAgents,
    session_id: &str,
    program: &str,
    cwd: &str,
    screen: &Screen,
    now: i64,
) -> bool {
    agents.sample(SampleInput {
        session_id,
        shell: "zsh",
        program: Some(program),
        argv: &[],
        cwd,
        screen,
        process_exited: false,
        frame_produced: false,
        now,
    })
}

/// I3: a sample is not evidence of a frame, and only a frame may end silence.
///
/// `flush_session` re-samples when the probe's believed process changed, and
/// `ForegroundState::observe` counts a moved `cwd` as a change. So an
/// identified agent that is thinking silently and `chdir`s gets re-sampled
/// with no frame — and the first version cleared `quiet_since` and restarted
/// the quiet clock for it, publishing "not quiet" with nothing behind it.
///
/// Falsify by writing `last_frame_at: now` / `quiet_since: None`
/// unconditionally again (the shape this replaced): both assertions below go
/// red, and nothing else in the file does.
#[test]
fn a_program_change_without_a_frame_does_not_clear_quiet_since() {
    let agents = RuntimeAgents::default();
    let working = screen(CLAUDE_WORKING_LINE.as_bytes());

    // A real frame at t=1_000, then silence long enough to be marked.
    sample_program(&agents, "s1", "claude", &working, 1_000);
    assert_eq!(agents.mark_quiet(1_000 + QUIET_AFTER_MS).len(), 1);
    assert_eq!(agents.snapshot()[0].quiet_since, Some(1_000));

    // The agent `chdir`s while still thinking. The probe notices; the screen
    // does not change; no frame is produced.
    let after = 1_000 + QUIET_AFTER_MS + 5_000;
    let changed =
        sample_program_without_a_frame(&agents, "s1", "claude", "/elsewhere", &working, after);

    let row = agents.snapshot().remove(0);
    assert_eq!(
        row.quiet_since,
        Some(1_000),
        "the session is still silent, so its quiet mark must survive a sample \
         that carried no frame -- clearing it publishes a fact with no producer"
    );
    assert_eq!(
        row.cwd, "/elsewhere",
        "the cwd move itself must still reach the wire, or this test is \
         asserting that nothing happened"
    );
    assert!(
        changed,
        "and the move IS an observable change, so a waiter must still be woken"
    );

    // The quiet clock did not restart either: it is still measured from the
    // last real frame, so this session stays quiet rather than needing another
    // full QUIET_AFTER_MS.
    assert!(
        agents.mark_quiet(after + 1).is_empty(),
        "already marked; staying quiet is not news"
    );
}

/// The cost guard (herdr's counting-architecture-test shape, spec §4.1).
///
/// Fifteen sessions driven for a hundred 16 ms ticks each. The gate's own
/// constants say how many probes that may cost, and the bound below is
/// DERIVED from them rather than written as a number — a hand-computed
/// literal would have to be edited every time a constant moves, and the
/// version that does not get edited is the one that stops constraining
/// anything (判据 §1).
///
/// ⚠️ The counters are the fifteen `ForegroundState`s this test owns, and that
/// is the whole reason it can assert anything. The first version read a
/// process-global `static` and measured 60 against a ceiling of 60 — zero
/// slack, while five untagged tests in the same binary drove `flush_session`
/// concurrently and one of them polled for three seconds. A guard whose
/// instrument other tests can move is not measuring this code (判据 §18).
/// With the counter per session there is no shared state left to serialise,
/// so this test carries no serial key.
#[test]
fn probe_count_is_bounded_at_fifteen_sessions() {
    use crate::gateway::pty::foreground::{
        fact_for_pid, probe_due, ForegroundState, PROBE_MIN_INTERVAL_MS,
    };

    const SESSIONS: i64 = 15;
    const TICKS: i64 = 100;
    const TICK_MS: i64 = 16; // manager::FLUSH_INTERVAL

    let me = std::process::id();
    let mut states: Vec<ForegroundState> =
        (0..SESSIONS).map(|_| ForegroundState::default()).collect();

    for tick in 0..TICKS {
        let now = tick * TICK_MS;
        for state in &mut states {
            // Every session produces a frame on every tick: the worst case
            // the rate gate exists to bound.
            state.note_frame(true);
            if probe_due(state.last_probe_at(), now, state.frame_budget_left(), false) {
                // A REAL process-table read, so what is counted is what
                // production pays and not a tally this test kept itself.
                state.observe(now, fact_for_pid(me));
            }
        }
    }

    // One free first look, then at most one per PROBE_MIN_INTERVAL_MS of
    // elapsed time.
    let span = (TICKS - 1) * TICK_MS;
    let per_session = 1 + span / PROBE_MIN_INTERVAL_MS;
    let ceiling = u64::try_from(SESSIONS * per_session).expect("small");

    let measured: u64 = states
        .iter()
        .map(crate::gateway::pty::foreground::ForegroundState::probes)
        .sum();
    assert!(
        measured <= ceiling,
        "{measured} probes for {SESSIONS} sessions over {TICKS} ticks exceeds \
         the gate's own ceiling of {ceiling}. Un-gated this would be {}",
        SESSIONS * TICKS
    );
    assert!(
        measured >= u64::try_from(SESSIONS).expect("small"),
        "only {measured} probes -- the gate cannot be so tight that the \
         first look never happens, or every session is unidentifiable \
         forever (判据 §2: a gate that never opens)"
    );
    assert!(
        measured < u64::try_from(SESSIONS * TICKS).expect("small"),
        "the gate must actually reduce something"
    );
    // Every session must be probed the SAME number of times: they were driven
    // identically, so a spread means one of them stopped being looked at.
    let first = states[0].probes();
    assert!(
        states.iter().all(|s| s.probes() == first),
        "identically driven sessions must cost identically; got {:?}",
        states
            .iter()
            .map(crate::gateway::pty::foreground::ForegroundState::probes)
            .collect::<Vec<_>>()
    );
}

/// THE GLUE (task M, spec §4.1): the live cwd has THREE sources and they
/// are ranked, so no reader downstream has to guess which one answered.
///
/// 1. `OSC 7` — the shell TELLING us where it is (`Screen::cwd()`, stream B).
/// 2. the foreground process's own cwd, read by the probe (stream A).
/// 3. the spawn directory, which never moves.
///
/// Neither stream could write this on its own: A had only sources 2 and 3
/// and left the marker comment in `flush_session` for source 1 to be
/// attached to; B produced source 1 with no consumer. A merge that brought
/// both halves in and left the order unwritten is the exact shape of 判据 §7
/// — two ends complete and no wire between them, invisible to dead-code
/// analysis because both ends have their own tests.
///
/// Both phases drive the REAL `flush_session`, so what is asserted is the
/// order production uses and not a re-implementation of it here.
///
/// Phase 1 pins source 1 over the other two: the child reports an `OSC 7`
/// path that is neither its own working directory nor its spawn directory,
/// so an entry showing anything else means the OSC 7 read is not wired in.
///
/// Phase 2 removes source 1 and pins 2 over 3: the child `cd`s away from
/// its spawn directory, and the expectation is DERIVED from the probe's own
/// answer (`session.foreground_fact()`) rather than written as a literal —
/// on a platform whose process table cannot report a cwd the probe answers
/// `None` and the spawn directory is then the correct answer, which is a
/// different assertion rather than a skipped one (判据 §2).
///
/// `PtySession::spawn` is used directly, not `pty::manager().spawn()`, so
/// this test owns its sessions and needs no serial key — the same reason
/// the other flush-wire tests in this file carry none.
#[tokio::test(flavor = "multi_thread")]
async fn cwd_prefers_osc7_then_foreground_then_spawn() {
    // Not created on disk: `OSC 7` is a REPORT, and the terminal's job is to
    // relay what the shell said, not to adjudicate it. A path that exists
    // would let a future implementation "validate" it and still pass.
    let osc_dir = "/tmp/aleph-tr2-osc7-reported";
    let spawn_dir = std::env::temp_dir().to_string_lossy().into_owned();
    let canonical = |p: &str| {
        std::fs::canonicalize(p)
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_else(|_| p.to_owned())
    };

    // ---- Phase 1: OSC 7 outranks the probe and the spawn directory. ----
    let id = "t-runtime-cwd-osc7";
    agents().remove(id);
    let opts = SpawnOptions {
        command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
        args: if cfg!(windows) {
            vec![
                // `/K`, NOT `/C`, and that is the whole Windows branch.
                // `prompt` is cmd.exe's only way to emit a raw ESC, but it
                // only sets the FORMAT — the bytes are written when cmd next
                // DISPLAYS a prompt, and `/C` never displays one. So the
                // `/C` version set a format nobody ever rendered and the
                // sequence was never emitted at all: the test failed on
                // Windows with "the child's OSC 7 never reached the screen",
                // which reads like a terminal defect and was a shell-flag
                // one. `/K` prints the prompt immediately and then blocks
                // reading stdin, which is also what keeps the child alive —
                // so the trailing `ping` the `/C` version needed for that is
                // gone. Verified outside Rust before being written here:
                // `cmd /K 'prompt $E]7;file:///tmp/x$E\$_'` emits
                // `ESC ] 7 ; file:///tmp/x ESC \` (2026-09-05, this machine).
                "/K".into(),
                format!("prompt $E]7;file://{osc_dir}$E\\$_"),
            ]
        } else {
            vec![
                "-c".into(),
                format!("printf '\\033]7;file://{osc_dir}\\007'; sleep 30"),
            ]
        },
        cwd: Some(spawn_dir.clone()),
        rows: 6,
        cols: 40,
        ..Default::default()
    };
    let session = PtySession::spawn(id.into(), &opts, None).expect("spawn");

    // Drive until the screen has actually parsed the OSC 7, not merely until
    // the first frame: the sequence can arrive on a later read than the one
    // that produced the first frame, and asserting on frame 1 would be a
    // race dressed up as a failure.
    let mut saw_osc = false;
    for _ in 0..150 {
        let now = chrono::Utc::now().timestamp_millis();
        let _ = crate::gateway::pty::manager::flush_session(&session, now);
        if session.with_screen(|s| s.cwd().map(str::to_owned)) == Some(osc_dir.to_owned()) {
            // One more pass so the sampler sees the parsed value.
            let now = chrono::Utc::now().timestamp_millis();
            let _ = crate::gateway::pty::manager::flush_session(&session, now);
            saw_osc = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let entry = agents().snapshot().into_iter().find(|e| e.session_id == id);
    session.kill();
    agents().remove(id);
    assert!(saw_osc, "the child's OSC 7 never reached the screen in 3s");
    let entry = entry.expect("a flushed session must be in the table");
    assert_eq!(
        entry.cwd,
        osc_dir,
        "OSC 7 is the shell telling us where it is and must outrank both the \
         probe (which would have said {}) and the spawn directory ({spawn_dir})",
        canonical(&spawn_dir)
    );

    // ---- Phase 2: with no OSC 7, the probe outranks the spawn directory. ----
    //
    // The child moves to a genuinely DIFFERENT directory rather than sitting
    // in the one it was spawned in. A child that never moves already produces
    // two distinct strings on macOS — `/var/folders/…` when you ask the
    // environment, `/private/var/…` when you ask the kernel — so an assertion
    // that leans on that is discriminating by accident, and on a platform that
    // spells the two the same it would pass no matter which source answered
    // (判据 §2).
    //
    // It prints, too, because probe rule 2 needs a frame since the last probe:
    // rule 1's free first look can land before the shell has run its `cd`, and
    // with no further output nothing would ever look again.
    let id2 = "t-runtime-cwd-foreground";
    let moved_to = if cfg!(windows) { "C:\\Windows" } else { "/" };
    agents().remove(id2);
    let opts2 = SpawnOptions {
        command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
        args: if cfg!(windows) {
            vec![
                "/C".into(),
                "cd /d C:\\Windows & echo MOVED & ping -n 30 127.0.0.1 >nul".into(),
            ]
        } else {
            vec!["-c".into(), "cd /; printf 'MOVED'; sleep 30".into()]
        },
        cwd: Some(spawn_dir.clone()),
        rows: 6,
        cols: 40,
        ..Default::default()
    };
    let session2 = PtySession::spawn(id2.into(), &opts2, None).expect("spawn");

    // Drive until the probe has SEEN the move. The flush that observes it is
    // the one that re-samples (a moved cwd is a changed `ForegroundFact`), so
    // by the time this loop breaks the table has already been told.
    let mut moved_seen = false;
    for _ in 0..200 {
        let now = chrono::Utc::now().timestamp_millis();
        let _ = crate::gateway::pty::manager::flush_session(&session2, now);
        if session2
            .foreground_fact()
            .and_then(|f| f.cwd)
            .is_some_and(|c| canonical(&c) == canonical(moved_to))
        {
            moved_seen = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let probe_cwd = session2.foreground_fact().and_then(|f| f.cwd);
    let entry2 = agents()
        .snapshot()
        .into_iter()
        .find(|e| e.session_id == id2);
    let no_osc7 = session2.with_screen(|s| s.cwd().is_none());
    session2.kill();
    agents().remove(id2);
    let entry2 = entry2.expect("a flushed session must be in the table");
    assert!(
        no_osc7,
        "phase 2 must have no OSC 7, or it is phase 1 again"
    );
    match probe_cwd {
        Some(live) => {
            assert_eq!(
                entry2.cwd, live,
                "the probe's live cwd must outrank the spawn directory \
                 ({spawn_dir}) — that is the whole point of probing"
            );
            assert!(
                moved_seen,
                "the probe answered {live} but never saw the child move to \
                 {moved_to} in 4s; without the move this phase separates the \
                 two sources only by their spelling of one directory, which is \
                 not a discrimination it can rely on (判据 §2)"
            );
        }
        None => assert_eq!(
            entry2.cwd, spawn_dir,
            "with no OSC 7 and no probe answer, the spawn directory is the \
             last resort and must still be reported"
        ),
    }
}
