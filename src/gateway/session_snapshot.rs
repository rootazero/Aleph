//! One reading of "what settings is this conversation running under".
//!
//! A session's knobs are persisted in two places and nowhere else: the
//! `identity_meta.custom` bag (`exec_tier` / `session_mode` / `think_level` /
//! `memory_mode` / `model_pin` / `project_root`) and the `sessions` row's own
//! columns (`model`, `model_provider`, the usage counters). Both have been
//! durable for a long time. What was missing is a **reader**: no response
//! handed them to a client that re-attached to a conversation by key, so a
//! client that reopened a thread painted the *install* defaults over a
//! conversation the server was still governing by its *own* values.
//!
//! This module is that reader, and there is deliberately only one of it. Two
//! surfaces need the answer — `chat.history` (attach) and `sessions.list`
//! (browse) — and a knob decoded two ways is a knob whose two surfaces can
//! disagree about whether it is set. `sessions.list` used to decode three of
//! them inline; it now goes through here, which is also how `think_level` (the
//! twin that was simply never added) reached that surface at all.
//!
//! Every getter returns `None` for *both* "absent" and "JSON null", because
//! that is what the writers mean: `sessions.patch` clears an override by
//! writing `null`, and the run loop treats absent and null identically. A
//! client must render `None` as "follows the global default" — never as a
//! concrete value of its own choosing, which is the mistake that made a
//! restarted terminal claim a tier the session did not have.

use aleph_protocol::{DanglingCallView, LastRunState, RunProgressView, SessionSnapshot};

use crate::gateway::session_store::types::SessionMetadata;
use crate::session::events::SessionEventRecord;
use crate::session::reduction::{
    reduce_run, DanglingProvenance, LogContradiction, RunDisposition, RunProgress,
};

/// `identity_meta.custom` key holding a session's user-chosen working
/// directory. Written by `sessions.set_project_root` and by
/// `resume_coordinator`; both spell it as a literal, so this constant is the
/// name the *readers* agree on.
pub const PROJECT_ROOT_SESSION_KEY: &str = "project_root";

/// The knob vocabulary a `RunStarted` envelope freezes, in the envelope's own
/// declaration order.
///
/// Four of these are `identity_meta.custom` keys — spelled here by their
/// constants, so a renamed key moves both readers at once. The last two are
/// the session row's own columns and are spelled literally because there is no
/// constant to borrow: nothing else keys a bag by them.
///
/// It exists so the two shapes cannot drift: a census test in
/// [`crate::session::events`] asserts this array equals
/// [`crate::session::events::RunEnvelopeSnapshot`]'s serialised key set. A
/// seventh knob therefore has to be added in both places before the build is
/// green.
///
/// What it does **not** catch, said out loud: a knob added to
/// [`SessionSnapshot`] and to neither of those two. `SessionSnapshot` carries
/// identity, usage counters and a label alongside its knobs, and nothing in
/// its shape says which fields are knobs — so there is no set to derive that
/// question from, only a list to keep.
pub const RUN_ENVELOPE_KNOB_KEYS: [&str; 6] = [
    crate::config::types::policies::EXEC_TIER_SESSION_KEY,
    crate::config::types::policies::MODE_SESSION_KEY,
    crate::agents::thinking::THINK_LEVEL_SESSION_KEY,
    crate::memory::session_memory_mode::MEMORY_MODE_SESSION_KEY,
    "model",
    "model_provider",
];

/// Read one `identity_meta.custom` string, treating JSON `null` as absent.
///
/// The single decoder for every knob. Inlining `im.custom.get(k)?.as_str()` at
/// each call site is how `sessions.list` came to decode three knobs and
/// `chat.history` zero — the shape is trivial, which is exactly why it gets
/// copied instead of shared.
#[must_use]
pub fn custom_str(meta: &SessionMetadata, key: &str) -> Option<String> {
    meta.identity_meta
        .as_ref()?
        .custom
        .get(key)?
        .as_str()
        .map(str::to_string)
}

/// Build the client-facing settings snapshot for a session.
///
/// Pure: it reads the metadata it is given and invents nothing. In particular
/// it does **not** fall back to the global tier/mode/think defaults — the
/// server resolves those per turn from live config, and a snapshot that baked
/// today's global value in would go stale the moment the config changed while
/// still looking authoritative.
#[must_use]
pub fn snapshot_from_metadata(meta: &SessionMetadata) -> SessionSnapshot {
    use crate::agents::thinking::THINK_LEVEL_SESSION_KEY;
    use crate::config::types::policies::{EXEC_TIER_SESSION_KEY, MODE_SESSION_KEY};
    use crate::memory::session_memory_mode::MEMORY_MODE_SESSION_KEY;
    use crate::providers::session_model_handle::{
        MODEL_PIN_PROVIDER_SESSION_KEY, MODEL_PIN_SESSION_KEY,
    };

    SessionSnapshot {
        session_key: meta.key.clone(),
        agent_id: meta.agent_id.clone(),
        mode: custom_str(meta, MODE_SESSION_KEY),
        exec_tier: custom_str(meta, EXEC_TIER_SESSION_KEY),
        think_level: custom_str(meta, THINK_LEVEL_SESSION_KEY),
        memory_mode: custom_str(meta, MEMORY_MODE_SESSION_KEY),
        model_pin: custom_str(meta, MODEL_PIN_SESSION_KEY),
        model_pin_provider: custom_str(meta, MODEL_PIN_PROVIDER_SESSION_KEY),
        model: meta.model.clone(),
        model_provider: meta.model_provider.clone(),
        input_tokens: meta.input_tokens,
        output_tokens: meta.output_tokens,
        total_tokens: meta.total_tokens,
        estimated_cost_usd: meta.estimated_cost_usd,
        message_count: meta.message_count,
        compaction_count: meta.compaction_count,
        project_root: custom_str(meta, PROJECT_ROOT_SESSION_KEY),
        label: meta.label.clone(),
        // Filled by the caller that has the log in hand — see
        // [`last_run_from_events`]. `None` here is "nobody asked", which is
        // what a snapshot built from metadata alone can honestly say.
        last_run: None,
    }
}

/// The attach face's answer to "what did the newest run do", from the whole log.
///
/// Wraps [`reduce_run`] and renders its result; it never re-derives a
/// disposition of its own. An `Err` — the reducer refusing a log whose seqs go
/// backwards, or a marker slice with a stray event in it — becomes
/// [`LastRunState::LOG_INCONSISTENT`] carrying the contradiction's tag.
/// **Not** `Clean`, and not a silent `None`: the reducer's refusal means "I do
/// not know what state this run is in", and the only reading that survives
/// contact with a client is one that says so (criterion #8).
///
/// `inspected` is `true`, which is what makes an empty `dangling` mean
/// something here: this face looked.
#[must_use]
pub fn last_run_from_events(events: &[SessionEventRecord]) -> LastRunState {
    let reduction = match reduce_run(events) {
        Ok(r) => r,
        Err(contradiction) => {
            return LastRunState {
                disposition: LastRunState::LOG_INCONSISTENT.to_string(),
                contradictions: vec![contradiction.tag().to_string()],
                inspected: true,
                ..LastRunState::default()
            }
        }
    };

    // "This session has never run anything" is not the same answer as "its
    // newest run finished cleanly", and both come back from the reducer as
    // `Clean`. The difference is whether the log holds a marker at all, and
    // that is derived from the reduction rather than re-scanned: `run_anchor`
    // is `Some` for every `RunStarted`, and a `RunFinished` with no start of
    // its own is exactly what `FinishWithoutStart` reports.
    let has_marker = reduction.run_anchor.is_some()
        || reduction
            .contradictions
            .iter()
            .any(|c| matches!(c, LogContradiction::FinishWithoutStart { .. }));

    let (disposition, trailing_starts) = match reduction.disposition {
        RunDisposition::Interrupted { trailing_starts } => (
            LastRunState::INTERRUPTED,
            u32::try_from(trailing_starts).unwrap_or(u32::MAX),
        ),
        RunDisposition::Clean if has_marker => (LastRunState::CLEAN, 0),
        RunDisposition::Clean => (LastRunState::NEVER_RAN, 0),
    };

    LastRunState {
        disposition: disposition.to_string(),
        run_id: reduction.run_id.clone(),
        trailing_starts,
        dangling: reduction.dangling.iter().map(dangling_view).collect(),
        // A run that recorded nothing says so with `None`; `inspected` is what
        // separates that from "this face does not carry progress".
        progress: (reduction.progress != RunProgress::default())
            .then(|| progress_view(&reduction.progress)),
        contradictions: reduction
            .contradictions
            .iter()
            .map(|c| c.tag().to_string())
            .collect(),
        inspected: true,
    }
}

/// The list face's answer, from one session's run markers alone.
///
/// Cheap enough to run for every row of `sessions.list`, and it can answer
/// exactly one question: the disposition word plus the two facts markers carry.
/// [`LastRunState::inspected`] is `false`, so a reader cannot mistake the empty
/// `dangling` list for "no tool calls were lost".
#[must_use]
pub fn last_run_from_markers(markers: &[SessionEventRecord]) -> LastRunState {
    if markers.is_empty() {
        return LastRunState::from_markers(LastRunState::NEVER_RAN, None, 0);
    }
    // `reduce_run` over a marker slice is the same derivation the attach face
    // uses, so the two faces cannot disagree about the word. It is fed markers
    // rather than a full log on purpose: `load_run_markers` is one indexed
    // query for every session, and reducing every session's whole log to paint
    // a list would be a different kind of wrong.
    let reduction = match reduce_run(markers) {
        Ok(r) => r,
        Err(contradiction) => {
            let mut state =
                LastRunState::from_markers(LastRunState::LOG_INCONSISTENT, None, 0);
            state.contradictions = vec![contradiction.tag().to_string()];
            return state;
        }
    };
    let (disposition, trailing_starts) = match reduction.disposition {
        RunDisposition::Interrupted { trailing_starts } => (
            LastRunState::INTERRUPTED,
            u32::try_from(trailing_starts).unwrap_or(u32::MAX),
        ),
        RunDisposition::Clean => (LastRunState::CLEAN, 0),
    };
    let mut state =
        LastRunState::from_markers(disposition, reduction.run_id.clone(), trailing_starts);
    state.contradictions = reduction
        .contradictions
        .iter()
        .map(|c| c.tag().to_string())
        .collect();
    state
}

fn dangling_view(call: &crate::session::reduction::DanglingCall) -> DanglingCallView {
    DanglingCallView {
        call_id: call.call_id.clone(),
        tool_name: call.tool_name.clone(),
        provenance: match call.provenance {
            DanglingProvenance::ThisRestart => DanglingCallView::THIS_RESTART,
            DanglingProvenance::EarlierRun => DanglingCallView::EARLIER_RUN,
        }
        .to_string(),
        denied: call.denied,
    }
}

fn progress_view(progress: &RunProgress) -> RunProgressView {
    RunProgressView {
        tool_calls_dispatched: u32::try_from(progress.tool_calls_dispatched).unwrap_or(u32::MAX),
        tool_calls_answered: u32::try_from(progress.tool_calls_answered).unwrap_or(u32::MAX),
        assistant_messages: u32::try_from(progress.assistant_messages).unwrap_or(u32::MAX),
        last_activity_ms: progress.last_activity_at,
    }
}

#[cfg(test)]
mod last_run_tests {
    use super::*;
    use aleph_protocol::LastRunDisposition;
    use crate::session::events::{RunOutcome, SessionEvent, ToolOutput, TurnId};

    fn rec(seq: u64, event: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event,
            created_at_ms: seq as i64 * 10,
        }
    }

    fn started(run: &str) -> SessionEvent {
        SessionEvent::RunStarted {
            run_id: run.to_string(),
            at: 1,
            project_root: None,
            envelope: None,
        }
    }

    fn finished(run: &str) -> SessionEvent {
        SessionEvent::RunFinished {
            run_id: run.to_string(),
            outcome: RunOutcome::Completed,
            at: 2,
        }
    }

    fn requested(call: &str) -> SessionEvent {
        SessionEvent::ToolCallRequested {
            turn_id: TurnId::new_v4(),
            call_id: call.to_string(),
            name: "bash_exec".to_string(),
            input: serde_json::json!({}),
            at: 3,
        }
    }

    fn result_for(call: &str) -> SessionEvent {
        SessionEvent::ToolResult {
            turn_id: TurnId::new_v4(),
            call_id: call.to_string(),
            output: ToolOutput {
                value: serde_json::json!("ok"),
                metadata: Default::default(),
            },
            at: 4,
        }
    }

    /// A run cut off mid-tool-call, rendered for a re-attaching client — and
    /// every number checked against the reduction itself rather than against a
    /// remembered figure, so the view can only ever say what the reducer said.
    #[test]
    fn an_interrupted_log_carries_the_reducers_numbers() {
        let events = vec![
            rec(1, started("run-a")),
            rec(2, requested("call-1")),
            rec(3, result_for("call-1")),
            rec(4, requested("call-2")),
        ];
        let reduction = reduce_run(&events).expect("reducible");
        let view = last_run_from_events(&events);

        assert_eq!(view.disposition(), LastRunDisposition::Interrupted);
        assert_eq!(view.run_id.as_deref(), Some("run-a"));
        assert_eq!(view.trailing_starts, 1);
        assert!(view.inspected);

        let dangling = view.dangling().expect("this face looked");
        assert_eq!(dangling.len(), reduction.dangling.len());
        assert_eq!(dangling[0].call_id, "call-2");
        assert_eq!(dangling[0].tool_name, "bash_exec");
        assert_eq!(
            dangling[0].provenance,
            aleph_protocol::DanglingCallView::THIS_RESTART,
            "the call was dispatched by the run that is still open"
        );
        assert!(!dangling[0].denied);

        let progress = view.progress.expect("the run recorded something");
        assert_eq!(
            progress.tool_calls_dispatched as usize,
            reduction.progress.tool_calls_dispatched
        );
        assert_eq!(
            progress.tool_calls_answered as usize,
            reduction.progress.tool_calls_answered
        );
        assert_eq!(progress.tool_calls_dispatched, 2);
        assert_eq!(progress.tool_calls_answered, 1);
        assert_eq!(progress.last_activity_ms, reduction.progress.last_activity_at);
    }

    /// The three words that are not "interrupted", and the one distinction the
    /// reducer cannot make on its own: `Clean` comes back for a session whose
    /// run finished AND for one that never ran, and those are different answers
    /// to the operator.
    #[test]
    fn a_finished_run_is_clean_and_a_log_with_no_markers_never_ran() {
        let finished_log = vec![rec(1, started("run-a")), rec(2, finished("run-a"))];
        assert_eq!(
            last_run_from_events(&finished_log).disposition(),
            LastRunDisposition::Clean
        );

        // A log with tool activity but no run marker at all — the L0 fast path,
        // a backfill, a session older than the markers.
        let markerless = vec![rec(1, requested("call-1")), rec(2, result_for("call-1"))];
        assert_eq!(
            last_run_from_events(&markerless).disposition(),
            LastRunDisposition::NeverRan
        );
        assert_eq!(
            last_run_from_events(&[]).disposition(),
            LastRunDisposition::NeverRan
        );
    }

    /// A `RunFinished` whose start is not in this log is still a marker, so the
    /// session HAS run — reading it as `never_ran` would be a fresh lie built
    /// on the reducer's `Clean`.
    #[test]
    fn a_closing_marker_with_no_start_still_means_this_session_ran() {
        let split_tail = vec![rec(1, finished("run-a"))];
        let view = last_run_from_events(&split_tail);
        assert_eq!(view.disposition(), LastRunDisposition::Clean);
        assert!(
            view.contradictions
                .contains(&"session-log-finish-without-start".to_string()),
            "the report must still name what was odd: {:?}",
            view.contradictions
        );
    }

    /// The arm that must never decay to `Clean`. The reducer refusing a log
    /// means "I do not know what state this run is in", and a client is told
    /// exactly that, with the tag that names the doctor finding.
    #[test]
    fn a_log_the_reducer_refuses_is_log_inconsistent_never_clean() {
        let backwards = vec![
            rec(9, started("run-a")),
            rec(2, requested("call-1")),
        ];
        let view = last_run_from_events(&backwards);
        assert_eq!(view.disposition(), LastRunDisposition::LogInconsistent);
        assert_eq!(
            view.contradictions,
            vec!["session-log-out-of-order-slice".to_string()]
        );
        assert!(
            view.inspected,
            "the log WAS opened; the answer is a refusal, not an absence"
        );
        assert_ne!(view.disposition(), LastRunDisposition::Clean);
    }

    /// The list face answers the same word off markers alone, and says it did
    /// not look at anything else — `dangling()` is `None`, not an empty list
    /// that would read as "no tool calls were lost".
    #[test]
    fn the_list_face_agrees_on_the_word_without_claiming_to_have_looked() {
        let markers = vec![
            rec(1, started("run-a")),
            rec(2, finished("run-a")),
            rec(3, started("run-b")),
        ];
        let listed = last_run_from_markers(&markers);
        assert_eq!(listed.disposition(), LastRunDisposition::Interrupted);
        assert_eq!(listed.run_id.as_deref(), Some("run-b"));
        assert_eq!(listed.trailing_starts, 1);
        assert!(!listed.inspected);
        assert_eq!(listed.dangling(), None);
        assert_eq!(listed.progress, None);
    }

    /// A session `load_run_markers` did not return has no markers, which is
    /// `never_ran` — not `clean`, and not an absent answer.
    #[test]
    fn a_session_with_no_markers_lists_as_never_ran() {
        let listed = last_run_from_markers(&[]);
        assert_eq!(listed.disposition(), LastRunDisposition::NeverRan);
        assert!(!listed.inspected);
    }

    /// Both faces reduce through the same function, so they cannot disagree
    /// about the word for the same session. This is the assertion that goes red
    /// if either face grows a derivation of its own.
    #[test]
    fn the_two_faces_never_disagree_about_the_word() {
        for (log, markers) in [
            (
                vec![rec(1, started("r")), rec(2, requested("c"))],
                vec![rec(1, started("r"))],
            ),
            (
                vec![rec(1, started("r")), rec(2, finished("r"))],
                vec![rec(1, started("r")), rec(2, finished("r"))],
            ),
        ] {
            assert_eq!(
                last_run_from_events(&log).disposition(),
                last_run_from_markers(&markers).disposition(),
                "the attach face and the list face gave one session two words"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_manager::SessionIdentityMeta;

    fn meta_with(custom: serde_json::Value) -> SessionMetadata {
        let mut identity = SessionIdentityMeta::default();
        if let serde_json::Value::Object(map) = custom {
            for (k, v) in map {
                identity.custom.insert(k, v);
            }
        }
        SessionMetadata {
            key: "agent:main:main".to_string(),
            agent_id: "main".to_string(),
            identity_meta: Some(identity),
            ..SessionMetadata::default()
        }
    }

    #[test]
    fn every_persisted_knob_reaches_the_snapshot() {
        let snap = snapshot_from_metadata(&meta_with(serde_json::json!({
            "session_mode": "code",
            "exec_tier": "ask",
            "think_level": "high",
            "memory_mode": "off",
            "model_pin": "claude-opus-5",
            "model_pin_provider": "anthropic",
            "project_root": "/tmp/proj",
        })));
        assert_eq!(snap.mode.as_deref(), Some("code"));
        assert_eq!(snap.exec_tier.as_deref(), Some("ask"));
        assert_eq!(snap.think_level.as_deref(), Some("high"));
        assert_eq!(snap.memory_mode.as_deref(), Some("off"));
        assert_eq!(snap.model_pin.as_deref(), Some("claude-opus-5"));
        assert_eq!(snap.model_pin_provider.as_deref(), Some("anthropic"));
        assert_eq!(snap.project_root.as_deref(), Some("/tmp/proj"));
        assert_eq!(snap.session_key, "agent:main:main");
        assert_eq!(snap.agent_id, "main");
    }

    /// `null` is how `sessions.patch` clears an override. Reading it as a value
    /// would make a cleared knob render as the string "null" in a pill.
    #[test]
    fn an_explicit_null_reads_as_no_override() {
        let snap = snapshot_from_metadata(&meta_with(serde_json::json!({
            "session_mode": serde_json::Value::Null,
            "exec_tier": serde_json::Value::Null,
            "think_level": serde_json::Value::Null,
        })));
        assert_eq!(snap.mode, None);
        assert_eq!(snap.exec_tier, None);
        assert_eq!(snap.think_level, None);
    }

    #[test]
    fn a_session_with_no_identity_meta_yields_no_overrides() {
        let meta = SessionMetadata {
            key: "agent:main:main".to_string(),
            ..SessionMetadata::default()
        };
        let snap = snapshot_from_metadata(&meta);
        assert_eq!(snap.mode, None);
        assert_eq!(snap.exec_tier, None);
        assert_eq!(snap.think_level, None);
        assert_eq!(snap.memory_mode, None);
        assert_eq!(snap.model_pin, None);
        assert_eq!(snap.project_root, None);
    }

    #[test]
    fn usage_columns_ride_verbatim() {
        let meta = SessionMetadata {
            key: "agent:main:main".to_string(),
            input_tokens: 1_200,
            output_tokens: 340,
            total_tokens: 1_540,
            estimated_cost_usd: 0.0421,
            message_count: 9,
            compaction_count: 2,
            model: Some("gpt-5".to_string()),
            model_provider: Some("openai".to_string()),
            ..SessionMetadata::default()
        };
        let snap = snapshot_from_metadata(&meta);
        assert_eq!(snap.input_tokens, 1_200);
        assert_eq!(snap.output_tokens, 340);
        assert_eq!(snap.total_tokens, 1_540);
        assert!((snap.estimated_cost_usd - 0.0421).abs() < f64::EPSILON);
        assert_eq!(snap.message_count, 9);
        assert_eq!(snap.compaction_count, 2);
        assert_eq!(snap.model.as_deref(), Some("gpt-5"));
        assert_eq!(snap.model_provider.as_deref(), Some("openai"));
    }

    /// Census, enforced against the source rather than a remembered list: every
    /// session-knob constant this crate defines must be read here.
    ///
    /// The defect this guards is the one that put `think_level` on no client
    /// surface at all for as long as it has existed — its two twins were added
    /// to `sessions.list`, it was not, and nothing noticed because a knob with
    /// no reader looks exactly like a knob nobody set.
    #[test]
    fn no_session_knob_constant_is_left_unread() {
        let src = include_str!("session_snapshot.rs");
        // Strip the test module so the constants named in *these* assertions do
        // not satisfy the check by themselves.
        let production = crate::utils::source_scan::production_prefix(src);
        for knob in [
            "MODE_SESSION_KEY",
            "EXEC_TIER_SESSION_KEY",
            "THINK_LEVEL_SESSION_KEY",
            "MEMORY_MODE_SESSION_KEY",
            "MODEL_PIN_SESSION_KEY",
            "PROJECT_ROOT_SESSION_KEY",
        ] {
            assert!(
                production.contains(knob),
                "{knob} is persisted on the session but never read into the snapshot — \
                 a client re-attaching cannot restore it"
            );
        }
    }
}
