use aleph_protocol::telecom_relay::{
    CallTurn, CallTurnRole, PlaybackEvent, PlaybackState, PlayedExtent, TelecomEnvelope,
    TelecomKind, TranscriptState, TELECOM_PROTOCOL_VERSION,
};
use alephcore::gateway::interfaces::voice_call::{
    ApplyOutcome, CallAvailability, CallCommand, CallSession, CallStateError, IgnoredEvent,
    OfflineCallTransport, PlayedRange,
};
use chrono::{TimeZone, Utc};

fn session() -> CallSession {
    CallSession {
        provider_account_id: "acct-1".into(),
        phone_call_id: "call-1".into(),
        provider_call_id: "provider-call-1".into(),
        binding_id: "binding-1".into(),
        connection_generation: 7,
    }
}

fn turn(turn_id: &str, epoch: u64, seq: u64, role: CallTurnRole, text: &str) -> CallTurn {
    CallTurn {
        provider_account_id: "acct-1".into(),
        phone_call_id: "call-1".into(),
        provider_call_id: "provider-call-1".into(),
        turn_id: turn_id.into(),
        role,
        transcript_state: TranscriptState::Committed,
        output_epoch: epoch,
        output_seq: seq,
        text: text.into(),
    }
}

fn envelope(kind: TelecomKind) -> TelecomEnvelope {
    TelecomEnvelope {
        protocol_version: TELECOM_PROTOCOL_VERSION,
        binding_id: "binding-1".into(),
        connection_generation: 7,
        event_id: "event-1".into(),
        kind,
        created_at: Utc.timestamp_opt(1_800_000_000, 0).unwrap(),
        expires_at: Utc.timestamp_opt(1_800_000_060, 0).unwrap(),
    }
}

fn playback(output_seq: u64, state: PlaybackState, extent: PlayedExtent) -> PlaybackEvent {
    PlaybackEvent {
        provider_account_id: "acct-1".into(),
        phone_call_id: "call-1".into(),
        turn_id: "turn-1".into(),
        output_epoch: 1,
        output_seq,
        state,
        extent,
    }
}

fn started_transport() -> OfflineCallTransport {
    let mut transport = OfflineCallTransport::new();
    transport.start_call(session()).unwrap();
    transport
}

#[test]
fn interim_transcript_never_starts_a_turn() {
    let mut transport = started_transport();
    let mut interim = turn("turn-1", 1, 1, CallTurnRole::UserTranscript, "in progress");
    interim.transcript_state = TranscriptState::Interim;

    let result = transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(interim)))
        .unwrap();

    assert_eq!(result, ApplyOutcome::InterimIgnored);
    assert!(transport.state().unwrap().active_turn.is_none());
}

#[test]
fn stale_epoch_is_ignored_and_new_turn_clears_old_queue() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "first",
        ))))
        .unwrap();
    let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "old answer");
    transport.record_generated_output(&output).unwrap();
    transport.record_output_queued(&output).unwrap();
    assert_eq!(
        transport
            .state()
            .unwrap()
            .active_turn
            .as_ref()
            .unwrap()
            .queued_text,
        "old answer"
    );

    let stale = turn("late-old", 0, 1, CallTurnRole::UserTranscript, "late");
    assert_eq!(
        transport
            .receive_committed_transcript(&envelope(TelecomKind::CallTurn(stale)))
            .unwrap(),
        ApplyOutcome::Ignored(IgnoredEvent::StaleOutputEpoch)
    );

    let next = turn("turn-2", 2, 1, CallTurnRole::UserTranscript, "second");
    assert_eq!(
        transport
            .receive_committed_transcript(&envelope(TelecomKind::CallTurn(next)))
            .unwrap(),
        ApplyOutcome::TurnStarted {
            turn_id: "turn-2".into()
        }
    );
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(active.turn_id, "turn-2");
    assert!(active.queued_text.is_empty());

    let stale_playback = PlaybackEvent {
        provider_account_id: "acct-1".into(),
        phone_call_id: "call-1".into(),
        turn_id: "turn-1".into(),
        output_epoch: 1,
        output_seq: 1,
        state: PlaybackState::Cleared,
        extent: PlayedExtent::Uncertain,
    };
    assert_eq!(
        transport
            .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(stale_playback)))
            .unwrap(),
        ApplyOutcome::Ignored(IgnoredEvent::StaleOutputEpoch)
    );
}

#[test]
fn exact_playback_truncates_heard_text_but_uncertain_stays_unknown() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "你好 world");
    transport.record_generated_output(&output).unwrap();
    transport.record_output_queued(&output).unwrap();

    let exact = PlaybackEvent {
        provider_account_id: "acct-1".into(),
        phone_call_id: "call-1".into(),
        turn_id: "turn-1".into(),
        output_epoch: 1,
        output_seq: 1,
        state: PlaybackState::Played,
        extent: PlayedExtent::Exact { chars: 3 },
    };
    transport
        .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(exact)))
        .unwrap();
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(active.played_range, PlayedRange::Exact { chars: 3 });
    assert_eq!(active.played_text().as_deref(), Some("你好 "));
    assert_eq!(active.heard_text().as_deref(), Some("你好 "));
    assert_eq!(active.played_text_for_output(1).as_deref(), Some("你好 "));
    assert!(active.queued_text.is_empty());

    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer");
    transport.record_generated_output(&output).unwrap();
    let uncertain = PlaybackEvent {
        provider_account_id: "acct-1".into(),
        phone_call_id: "call-1".into(),
        turn_id: "turn-1".into(),
        output_epoch: 1,
        output_seq: 1,
        state: PlaybackState::Played,
        extent: PlayedExtent::Uncertain,
    };
    transport
        .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(uncertain)))
        .unwrap();
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(active.played_range, PlayedRange::Unknown);
    assert_eq!(active.played_text(), None);
}

#[test]
fn playback_ack_is_scoped_to_one_output_chunk() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    let first = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "first");
    let second = turn("turn-1", 1, 2, CallTurnRole::AssistantText, "second");
    transport.record_generated_output(&first).unwrap();
    transport.record_generated_output(&second).unwrap();
    transport.record_output_queued(&first).unwrap();
    transport.record_output_queued(&second).unwrap();

    transport
        .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(PlaybackEvent {
            provider_account_id: "acct-1".into(),
            phone_call_id: "call-1".into(),
            turn_id: "turn-1".into(),
            output_epoch: 1,
            output_seq: 2,
            state: PlaybackState::Played,
            extent: PlayedExtent::Exact { chars: 2 },
        })))
        .unwrap();

    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(active.queued_text, "first");
    assert_eq!(active.played_range, PlayedRange::Unknown);
    assert_eq!(active.heard_text(), None);
    assert_eq!(active.played_text_for_output(2).as_deref(), Some("se"));
    assert_eq!(
        active.playback_state_for_output(2),
        Some(PlaybackState::Played)
    );

    transport
        .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(PlaybackEvent {
            provider_account_id: "acct-1".into(),
            phone_call_id: "call-1".into(),
            turn_id: "turn-1".into(),
            output_epoch: 1,
            output_seq: 1,
            state: PlaybackState::Cleared,
            extent: PlayedExtent::None,
        })))
        .unwrap();
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert!(active.queued_text.is_empty());
    assert_eq!(active.played_text_for_output(1), None);
    assert_eq!(active.played_text_for_output(2).as_deref(), Some("se"));
}

#[test]
fn uncertain_or_missing_playback_never_claims_an_exact_range() {
    for state in [
        PlaybackState::Queued,
        PlaybackState::Started,
        PlaybackState::Cleared,
        PlaybackState::Failed,
    ] {
        let mut transport = started_transport();
        transport
            .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
                "turn-1",
                1,
                7,
                CallTurnRole::UserTranscript,
                "hello",
            ))))
            .unwrap();
        transport
            .record_generated_output(&turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer"))
            .unwrap();
        transport
            .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
                1,
                state,
                PlayedExtent::Exact { chars: 3 },
            ))))
            .unwrap();
        let active = transport.state().unwrap().active_turn.as_ref().unwrap();
        assert_eq!(active.played_range_for_output(1), PlayedRange::Unknown);
        assert_eq!(active.played_text_for_output(1), None);
    }

    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            7,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    transport
        .record_generated_output(&turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer"))
        .unwrap();
    transport
        .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
            1,
            PlaybackState::Played,
            PlayedExtent::Exact { chars: 3 },
        ))))
        .unwrap();
    assert_eq!(
        transport
            .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
                1,
                PlaybackState::Played,
                PlayedExtent::Uncertain,
            ))))
            .unwrap(),
        ApplyOutcome::Ignored(IgnoredEvent::DuplicateSequence)
    );
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(
        active.played_range_for_output(1),
        PlayedRange::Exact { chars: 3 }
    );
    assert_eq!(active.played_text_for_output(1).as_deref(), Some("ans"));
}

#[test]
fn transcript_and_assistant_sequences_are_role_scoped() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            7,
            CallTurnRole::UserTranscript,
            "first transcript",
        ))))
        .unwrap();

    transport
        .record_generated_output(&turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer"))
        .unwrap();

    assert_eq!(
        transport
            .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
                "turn-1",
                1,
                8,
                CallTurnRole::UserTranscript,
                "updated transcript",
            ))))
            .unwrap(),
        ApplyOutcome::Applied
    );
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(active.committed_transcript, "updated transcript");
    assert_eq!(active.committed_transcript_seq, 8);
    assert_eq!(active.output_seq, 1);

    assert_eq!(
        transport
            .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
                "turn-1",
                1,
                8,
                CallTurnRole::UserTranscript,
                "duplicate transcript",
            ))))
            .unwrap(),
        ApplyOutcome::Ignored(IgnoredEvent::DuplicateSequence)
    );
    assert_eq!(
        transport
            .state()
            .unwrap()
            .active_turn
            .as_ref()
            .unwrap()
            .committed_transcript,
        "updated transcript"
    );
}

#[test]
fn generated_queued_and_playback_are_distinct_facts() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer");
    transport.record_generated_output(&output).unwrap();
    assert!(transport
        .state()
        .unwrap()
        .active_turn
        .as_ref()
        .unwrap()
        .queued_text
        .is_empty());
    transport.send_text_output(output.clone()).unwrap();
    assert!(transport
        .state()
        .unwrap()
        .active_turn
        .as_ref()
        .unwrap()
        .queued_text
        .is_empty());
    assert_eq!(
        transport.commands(),
        &[CallCommand::SendText(output.clone())]
    );
    transport.record_output_queued(&output).unwrap();
    assert_eq!(
        transport
            .state()
            .unwrap()
            .active_turn
            .as_ref()
            .unwrap()
            .queued_text,
        "answer"
    );
}

#[test]
fn cancel_permanently_fences_generated_output_without_emitting_a_second_command() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer");
    transport.record_generated_output(&output).unwrap();

    transport.cancel_output("call-1", 1).unwrap();
    let commands_after_cancel = transport.commands().to_vec();
    assert_eq!(
        transport.send_text_output(output.clone()),
        Err(CallStateError::CancelledOutputEpoch { output_epoch: 1 }.into())
    );
    assert_eq!(transport.commands(), commands_after_cancel.as_slice());

    let late_output = turn("turn-1", 1, 2, CallTurnRole::AssistantText, "late answer");
    assert_eq!(
        transport.record_generated_output(&late_output),
        Err(CallStateError::CancelledOutputEpoch { output_epoch: 1 }.into())
    );
    assert_eq!(
        transport.record_output_queued(&output),
        Err(CallStateError::CancelledOutputEpoch { output_epoch: 1 }.into())
    );
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(active.generated_text, "answer");
    assert!(active.queued_text.is_empty());

    for state in [PlaybackState::Failed, PlaybackState::Played] {
        assert_eq!(
            transport
                .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
                    1,
                    state,
                    PlayedExtent::Exact { chars: 6 },
                ))))
                .unwrap(),
            ApplyOutcome::Ignored(IgnoredEvent::CancelledOutputEpoch)
        );
        assert_eq!(
            transport.send_text_output(output.clone()),
            Err(CallStateError::CancelledOutputEpoch { output_epoch: 1 }.into())
        );
        assert_eq!(transport.commands(), commands_after_cancel.as_slice());
    }
}

#[test]
fn cleared_playback_permanently_fences_the_acknowledged_output_epoch() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer");
    transport.record_generated_output(&output).unwrap();
    transport
        .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(PlaybackEvent {
            provider_account_id: "acct-1".into(),
            phone_call_id: "call-1".into(),
            turn_id: "turn-1".into(),
            output_epoch: 1,
            output_seq: 1,
            state: PlaybackState::Cleared,
            extent: PlayedExtent::None,
        })))
        .unwrap();

    assert_eq!(
        transport.send_text_output(output),
        Err(CallStateError::CancelledOutputEpoch { output_epoch: 1 }.into())
    );
    assert!(transport.commands().is_empty());
}

#[test]
fn explicit_cancel_ignores_late_played_without_mutating_prior_facts() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer");
    transport.record_generated_output(&output).unwrap();
    transport.record_output_queued(&output).unwrap();
    transport
        .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
            1,
            PlaybackState::Queued,
            PlayedExtent::None,
        ))))
        .unwrap();
    transport.cancel_output("call-1", 1).unwrap();

    assert_eq!(
        transport
            .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
                1,
                PlaybackState::Played,
                PlayedExtent::Exact { chars: 6 },
            ))))
            .unwrap(),
        ApplyOutcome::Ignored(IgnoredEvent::CancelledOutputEpoch)
    );
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(
        active.playback_state_for_output(1),
        Some(PlaybackState::Queued)
    );
    assert_eq!(active.played_text_for_output(1), None);
    assert_eq!(active.heard_text(), None);
    assert_eq!(active.queued_text, "answer");
}

#[test]
fn cleared_playback_ignores_late_played_without_heard_text() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer");
    transport.record_generated_output(&output).unwrap();
    transport.record_output_queued(&output).unwrap();
    transport
        .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
            1,
            PlaybackState::Cleared,
            PlayedExtent::None,
        ))))
        .unwrap();

    assert_eq!(
        transport
            .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
                1,
                PlaybackState::Played,
                PlayedExtent::Exact { chars: 6 },
            ))))
            .unwrap(),
        ApplyOutcome::Ignored(IgnoredEvent::CancelledOutputEpoch)
    );
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(
        active.playback_state_for_output(1),
        Some(PlaybackState::Cleared)
    );
    assert_eq!(active.played_text_for_output(1), None);
    assert_eq!(active.heard_text(), None);
}

#[test]
fn terminal_played_ignores_started_and_failed_without_mutation() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer");
    transport.record_generated_output(&output).unwrap();
    transport
        .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
            1,
            PlaybackState::Played,
            PlayedExtent::Exact { chars: 3 },
        ))))
        .unwrap();

    for state in [PlaybackState::Started, PlaybackState::Failed] {
        assert_eq!(
            transport
                .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
                    1,
                    state,
                    PlayedExtent::None,
                ))))
                .unwrap(),
            ApplyOutcome::Ignored(IgnoredEvent::DuplicateSequence)
        );
        let active = transport.state().unwrap().active_turn.as_ref().unwrap();
        assert_eq!(
            active.playback_state_for_output(1),
            Some(PlaybackState::Played)
        );
        assert_eq!(active.played_text_for_output(1).as_deref(), Some("ans"));
        assert_eq!(active.heard_text().as_deref(), Some("ans"));
    }
}

#[test]
fn terminal_playback_rejects_late_queue_and_send_without_repopulating_queue() {
    for state in [PlaybackState::Played, PlaybackState::Failed] {
        let mut transport = started_transport();
        transport
            .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
                "turn-1",
                1,
                1,
                CallTurnRole::UserTranscript,
                "hello",
            ))))
            .unwrap();
        let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer");
        transport.record_generated_output(&output).unwrap();
        transport
            .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
                1,
                state,
                if state == PlaybackState::Played {
                    PlayedExtent::Exact { chars: 6 }
                } else {
                    PlayedExtent::None
                },
            ))))
            .unwrap();

        assert_eq!(
            transport.record_output_queued(&output),
            Err(CallStateError::OutputAlreadyTerminal { seq: 1 }.into())
        );
        assert_eq!(
            transport.send_text_output(output),
            Err(CallStateError::OutputAlreadyTerminal { seq: 1 }.into())
        );
        assert!(transport.commands().is_empty());
        assert!(transport
            .state()
            .unwrap()
            .active_turn
            .as_ref()
            .unwrap()
            .queued_text
            .is_empty());
    }
}

#[test]
fn played_then_cleared_fences_later_output_sequences() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    let first = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer");
    transport.record_generated_output(&first).unwrap();
    transport
        .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
            1,
            PlaybackState::Played,
            PlayedExtent::Exact { chars: 6 },
        ))))
        .unwrap();

    assert_eq!(
        transport
            .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
                1,
                PlaybackState::Cleared,
                PlayedExtent::None,
            ))))
            .unwrap(),
        ApplyOutcome::Ignored(IgnoredEvent::DuplicateSequence)
    );
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(
        active.playback_state_for_output(1),
        Some(PlaybackState::Played)
    );
    assert_eq!(active.heard_text().as_deref(), Some("answer"));

    let later = turn("turn-1", 1, 2, CallTurnRole::AssistantText, "later");
    assert_eq!(
        transport.record_generated_output(&later),
        Err(CallStateError::CancelledOutputEpoch { output_epoch: 1 }.into())
    );
    assert_eq!(
        transport
            .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
                2,
                PlaybackState::Queued,
                PlayedExtent::None,
            ))))
            .unwrap(),
        ApplyOutcome::Ignored(IgnoredEvent::CancelledOutputEpoch)
    );
}

#[test]
fn unknown_cleared_sequence_fences_the_output_epoch() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();

    assert_eq!(
        transport.publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
            99,
            PlaybackState::Cleared,
            PlayedExtent::None,
        )))),
        Err(CallStateError::UnknownOutputSequence { seq: 99 }.into())
    );
    let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer");
    assert_eq!(
        transport.record_generated_output(&output),
        Err(CallStateError::CancelledOutputEpoch { output_epoch: 1 }.into())
    );
    assert_eq!(
        transport
            .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
                1,
                PlaybackState::Queued,
                PlayedExtent::None,
            ))))
            .unwrap(),
        ApplyOutcome::Ignored(IgnoredEvent::CancelledOutputEpoch)
    );
}

#[test]
fn queued_started_played_lifecycle_is_accepted_in_order() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer");
    transport.record_generated_output(&output).unwrap();

    for state in [PlaybackState::Queued, PlaybackState::Started] {
        assert_eq!(
            transport
                .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
                    1,
                    state,
                    PlayedExtent::None,
                ))))
                .unwrap(),
            ApplyOutcome::Applied
        );
    }
    assert_eq!(
        transport
            .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(playback(
                1,
                PlaybackState::Played,
                PlayedExtent::Exact { chars: 6 },
            ))))
            .unwrap(),
        ApplyOutcome::Applied
    );
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(
        active.playback_state_for_output(1),
        Some(PlaybackState::Played)
    );
    assert_eq!(active.heard_text().as_deref(), Some("answer"));
}

#[test]
fn non_played_or_overlong_exact_playback_is_unknown() {
    for state in [
        PlaybackState::Queued,
        PlaybackState::Started,
        PlaybackState::Cleared,
        PlaybackState::Failed,
    ] {
        let mut transport = started_transport();
        transport
            .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
                "turn-1",
                1,
                1,
                CallTurnRole::UserTranscript,
                "hello",
            ))))
            .unwrap();
        transport
            .record_generated_output(&turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer"))
            .unwrap();
        transport
            .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(PlaybackEvent {
                provider_account_id: "acct-1".into(),
                phone_call_id: "call-1".into(),
                turn_id: "turn-1".into(),
                output_epoch: 1,
                output_seq: 1,
                state,
                extent: PlayedExtent::Exact { chars: 3 },
            })))
            .unwrap();

        let active = transport.state().unwrap().active_turn.as_ref().unwrap();
        assert_eq!(active.playback_state_for_output(1), Some(state));
        assert_eq!(active.played_range_for_output(1), PlayedRange::Unknown);
        assert_eq!(active.played_range, PlayedRange::Unknown);
        assert_eq!(active.played_text_for_output(1), None);
    }

    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    transport
        .record_generated_output(&turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer"))
        .unwrap();
    transport
        .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(PlaybackEvent {
            provider_account_id: "acct-1".into(),
            phone_call_id: "call-1".into(),
            turn_id: "turn-1".into(),
            output_epoch: 1,
            output_seq: 1,
            state: PlaybackState::Played,
            extent: PlayedExtent::Exact { chars: 7 },
        })))
        .unwrap();

    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(
        active.playback_state_for_output(1),
        Some(PlaybackState::Played)
    );
    assert_eq!(active.played_range_for_output(1), PlayedRange::Unknown);
    assert_eq!(active.played_range, PlayedRange::Unknown);
    assert_eq!(active.played_text_for_output(1), None);
}

#[test]
fn cleared_playback_fences_local_generation_and_queue_facts() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer");
    transport.record_generated_output(&output).unwrap();
    transport
        .publish_playback_ack(&envelope(TelecomKind::PlaybackEvent(PlaybackEvent {
            provider_account_id: "acct-1".into(),
            phone_call_id: "call-1".into(),
            turn_id: "turn-1".into(),
            output_epoch: 1,
            output_seq: 1,
            state: PlaybackState::Cleared,
            extent: PlayedExtent::Exact { chars: 3 },
        })))
        .unwrap();

    let late_output = turn("turn-1", 1, 2, CallTurnRole::AssistantText, "late answer");
    assert_eq!(
        transport.record_generated_output(&late_output),
        Err(CallStateError::CancelledOutputEpoch { output_epoch: 1 }.into())
    );
    assert_eq!(
        transport.record_output_queued(&output),
        Err(CallStateError::CancelledOutputEpoch { output_epoch: 1 }.into())
    );
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(active.generated_text, "answer");
    assert!(active.queued_text.is_empty());
}

#[test]
fn same_epoch_cannot_start_a_different_turn_or_erase_the_active_output() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "first",
        ))))
        .unwrap();
    let output = turn("turn-1", 1, 1, CallTurnRole::AssistantText, "answer");
    transport.record_generated_output(&output).unwrap();
    transport.record_output_queued(&output).unwrap();

    assert_eq!(
        transport.receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-2",
            1,
            1,
            CallTurnRole::UserTranscript,
            "second",
        ),))),
        Err(CallStateError::TurnEpochReuse {
            output_epoch: 1,
            active_turn_id: "turn-1".into(),
            got_turn_id: "turn-2".into(),
        }
        .into())
    );
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(active.generated_text, "answer");
    assert_eq!(active.queued_text, "answer");

    assert_eq!(
        transport
            .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
                "turn-1",
                1,
                1,
                CallTurnRole::UserTranscript,
                "replayed first",
            ))))
            .unwrap(),
        ApplyOutcome::Ignored(IgnoredEvent::DuplicateSequence)
    );
    let active = transport.state().unwrap().active_turn.as_ref().unwrap();
    assert_eq!(active.turn_id, "turn-1");
    assert_eq!(active.generated_text, "answer");
}

#[test]
fn committed_tool_side_effect_survives_interruption() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "do it",
        ))))
        .unwrap();
    transport
        .commit_tool_side_effect("turn-1", 1, "tool-effect-1")
        .unwrap();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-2",
            2,
            1,
            CallTurnRole::UserTranscript,
            "interrupt",
        ))))
        .unwrap();
    assert_eq!(
        transport.state().unwrap().committed_tool_side_effects,
        vec!["tool-effect-1"]
    );
    assert_eq!(
        transport
            .state()
            .unwrap()
            .active_turn
            .as_ref()
            .unwrap()
            .committed_tool_side_effects,
        vec!["tool-effect-1"]
    );
}

#[test]
fn hangup_releases_output_and_makes_session_unavailable() {
    let mut transport = started_transport();
    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    transport.hangup("call-1").unwrap();
    assert_eq!(
        transport.state().unwrap().availability,
        CallAvailability::Unavailable
    );
    assert!(transport.state().unwrap().active_turn.is_none());
    assert_eq!(
        transport.commands().last(),
        Some(&CallCommand::Hangup {
            phone_call_id: "call-1".into()
        })
    );
    assert_eq!(
        transport.hangup("call-1"),
        Err(CallStateError::SessionUnavailable.into())
    );
}

#[test]
fn scope_and_generation_mismatches_are_rejected_and_cancel_is_bound() {
    let mut transport = started_transport();
    let wrong_binding = TelecomEnvelope {
        binding_id: "other-binding".into(),
        ..envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        )))
    };
    assert!(matches!(
        transport.receive_committed_transcript(&wrong_binding),
        Err(
            alephcore::gateway::interfaces::voice_call::CallTransportError::State(
                CallStateError::BindingMismatch { .. }
            )
        )
    ));
    assert_eq!(
        transport.cancel_output("wrong-call", 1),
        Err(CallStateError::PhoneCallMismatch {
            expected: "call-1".into(),
            got: "wrong-call".into(),
        }
        .into())
    );

    transport
        .receive_committed_transcript(&envelope(TelecomKind::CallTurn(turn(
            "turn-1",
            1,
            1,
            CallTurnRole::UserTranscript,
            "hello",
        ))))
        .unwrap();
    transport.cancel_output("call-1", 1).unwrap();
    assert_eq!(
        transport.commands().last(),
        Some(&CallCommand::CancelOutput {
            phone_call_id: "call-1".into(),
            output_epoch: 1,
        })
    );
    assert_eq!(
        transport.cancel_output("call-1", 2),
        Err(CallStateError::OutputEpochMismatch {
            expected: 1,
            got: 2
        }
        .into())
    );
}
