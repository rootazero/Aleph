use super::audio::{CapturedUtterance, MicError, MicSession, TtsPlayer, PENDING_FRAME_CAP};
use super::caption_state::{apply_delta, apply_formatted, lock, CaptionState, Delta};
use super::level::LevelSmoother;
use super::machine::{on_event, Action, VoiceEvent, VoicePhase};
use super::sentence::SentenceSplitter;
use super::turn::{RecognitionPath, TurnToken, VoiceTurn};
use super::vad::{barge_step, vad_step, BargeConfig, BargeState, VadConfig, VadEvent, VadState};
use super::{drive_step, Caption, VoiceMode};
use crate::api::chat::ChatApi;
use crate::context::{DashboardState, GatewayEvent};
use crate::i18n::{t_string, I18nCtx};
use crate::state::sessions::{ConvId, SessionMap, VoiceRunEnd};
use crate::views::chat::ChatState;
use gloo_timers::{callback::Interval, future::TimeoutFuture};
use leptos::{prelude::*, task::spawn_local};
use serde_json::json;
use std::{rc::Rc, time::Duration};

const FRAME_ACK_GRACE_MS: u32 = 250;
const STREAM_STRIKE_LIMIT: u32 = 2;
const TRANSCRIPT_TOPIC: &str = "voice.transcribe.delta";

#[derive(Clone, Copy)]
pub(super) struct VoiceUi {
    pub phase: RwSignal<VoicePhase>,
    pub level: RwSignal<f64>,
    pub caption: RwSignal<Caption>,
    pub caption_state: RwSignal<CaptionState>,
    pub error_flash: RwSignal<bool>,
    pub mic_error: RwSignal<Option<MicError>>,
}

impl VoiceUi {
    pub fn new() -> Self {
        Self {
            phase: RwSignal::new(VoicePhase::Listening),
            level: RwSignal::new(0.0),
            caption: RwSignal::new(Caption::Idle),
            caption_state: RwSignal::new(CaptionState::default()),
            error_flash: RwSignal::new(false),
            mic_error: RwSignal::new(None),
        }
    }
}

#[derive(Clone, Copy)]
struct VoiceSession {
    ui: VoiceUi,
    dash: DashboardState,
    chat: ChatState,
    sessions: SessionMap,
    mode: VoiceMode,
    i18n: I18nCtx,
    epoch: u64,
    conv: ConvId,
    turn: RwSignal<VoiceTurn>,
    player: StoredValue<Option<Rc<TtsPlayer>>, LocalStorage>,
    splitter: StoredValue<SentenceSplitter, LocalStorage>,
    mic: StoredValue<Option<Rc<MicSession>>, LocalStorage>,
    tick: StoredValue<Option<Interval>, LocalStorage>,
    subscription: StoredValue<Option<usize>>,
    closed: StoredValue<bool>,
    strikes: StoredValue<u32>,
    streaming_off: StoredValue<bool>,
    misses: StoredValue<u32>,
}

pub(super) fn mount(
    ui: VoiceUi,
    dash: DashboardState,
    chat: ChatState,
    sessions: SessionMap,
    mode: VoiceMode,
    i18n: I18nCtx,
) {
    let Some(epoch) = dash.current_connection_epoch() else {
        Effect::new(move |_| mode.set_open(false));
        return;
    };
    let agent = chat
        .agent_id
        .get_untracked()
        .unwrap_or_else(|| "main".into());
    let conv = sessions.ensure_active(chat, &agent, String::new);
    VoiceSession {
        ui,
        dash,
        chat,
        sessions,
        mode,
        i18n,
        epoch,
        conv,
        turn: RwSignal::new(VoiceTurn::default()),
        player: StoredValue::new_local(None),
        splitter: StoredValue::new_local(SentenceSplitter::default()),
        mic: StoredValue::new_local(None),
        tick: StoredValue::new_local(None),
        subscription: StoredValue::new(None),
        closed: StoredValue::new(false),
        strikes: StoredValue::new(0),
        streaming_off: StoredValue::new(false),
        misses: StoredValue::new(0),
    }
    .start();
}

impl VoiceSession {
    fn current(self) -> bool {
        self.closed.try_get_value() == Some(false)
            && self.mode.open.try_get_untracked() == Some(true)
            && self.dash.current_connection_epoch() == Some(self.epoch)
            && self.sessions.active.try_get_untracked().flatten() == Some(self.conv)
    }

    fn owns(self, token: TurnToken) -> bool {
        self.current()
            && self
                .turn
                .try_with_untracked(|turn| turn.owns(token))
                .unwrap_or(false)
    }

    fn dispatch(self, event: VoiceEvent) {
        if !self.current() {
            return;
        }
        let Some(phase) = self.ui.phase.try_get_untracked() else {
            return;
        };
        let (next, action) = on_event(phase, event);
        self.ui.phase.set(next);
        match action {
            Action::None => {}
            Action::StopPlayback => {
                self.player.with_value(|p| p.as_ref().map(|p| p.stop_all()));
            }
            Action::ShowError => {
                let message = if event == VoiceEvent::RunFailed {
                    t_string!(self.i18n, voice.reply_failed)
                } else {
                    t_string!(self.i18n, voice.not_heard)
                };
                self.ui.caption.set(Caption::Error(message.to_string()));
                self.ui.error_flash.set(true);
                set_timeout(
                    move || {
                        self.ui.error_flash.try_set(false);
                    },
                    Duration::from_millis(900),
                );
            }
        }
    }

    fn start(self) {
        let player = TtsPlayer::new(
            self.epoch,
            move || self.current(),
            move || self.dispatch(VoiceEvent::FirstAudioReady),
            move || self.dispatch(VoiceEvent::PlaybackDrained),
            move |sentence| self.ui.caption.set(Caption::Assistant(sentence)),
        );
        self.player.set_value(Some(Rc::clone(&player)));
        let subscription = self
            .dash
            .subscribe_events(move |event| self.on_gateway_event(event));
        self.subscription.set_value(Some(subscription));
        self.mode.on_close(move || self.shutdown());
        on_cleanup(move || self.shutdown());
        Effect::new(move |_| {
            self.dash.connection_epoch.track();
            self.dash.is_connected.track();
            self.sessions.active.track();
            if self.closed.try_get_value() == Some(false) && !self.current() {
                self.mode.set_open(false);
            }
        });
        Effect::new(move |previous: Option<VoicePhase>| {
            let phase = self.ui.phase.get();
            if phase == VoicePhase::Listening && previous != Some(VoicePhase::Listening) {
                if let Some(mic) = self.mic.get_value() {
                    self.begin_listening(mic);
                }
            }
            phase
        });
        Effect::new(move |_| self.drive_reply());
        spawn_local(async move {
            match MicSession::open(move || self.current()).await {
                Ok(Some(mic)) => {
                    self.mic.set_value(Some(Rc::clone(&mic)));
                    player.resume_output();
                    self.begin_listening(Rc::clone(&mic));
                    self.tick
                        .set_value(Some(Interval::new(50, self.tick_callback(mic, player))));
                }
                Ok(None) => {}
                Err(error) if self.current() => self.ui.mic_error.set(Some(error)),
                Err(_) => {}
            }
        });
    }

    fn shutdown(self) {
        if self
            .closed
            .try_update_value(|closed| std::mem::replace(closed, true))
            != Some(false)
        {
            return;
        }
        self.revoke_turn();
        drop(self.tick.try_update_value(Option::take));
        if let Some(mic) = self.mic.try_update_value(Option::take).flatten() {
            mic.close();
        }
        if let Some(player) = self.player.try_update_value(Option::take).flatten() {
            player.close();
        }
        if let Some(id) = self.subscription.try_update_value(Option::take).flatten() {
            self.dash.unsubscribe_events(id);
        }
    }

    fn tick_callback(self, mic: Rc<MicSession>, player: Rc<TtsPlayer>) -> impl FnMut() + 'static {
        let mut vad = VadState::default();
        let mut barge = BargeState::default();
        let vad_config = VadConfig::default();
        let barge_config = BargeConfig::default();
        let mut smoother = LevelSmoother::default();
        move || {
            if !self.current() {
                return;
            }
            let phase = self.ui.phase.get_untracked();
            let rms = mic.rms();
            let output = if phase == VoicePhase::Speaking {
                player.output_level()
            } else {
                0.0
            };
            self.ui
                .level
                .set(f64::from(smoother.step(if phase == VoicePhase::Speaking {
                    output
                } else {
                    rms
                })));
            match phase {
                VoicePhase::Speaking => {
                    let (next, fire) = barge_step(barge, rms, output, &barge_config);
                    barge = next;
                    if fire {
                        self.revoke_turn();
                        vad = VadState::default();
                        self.dispatch(VoiceEvent::BargeIn);
                        mic.start_segment();
                    }
                }
                VoicePhase::Processing => {
                    vad = VadState::default();
                    barge = BargeState::default();
                }
                VoicePhase::Listening => {
                    barge = BargeState::default();
                    let (next, event) = vad_step(vad, rms, &vad_config);
                    vad = next;
                    match event {
                        Some(VadEvent::SpeechStart) => mic.start_segment(),
                        Some(VadEvent::Discarded) => mic.discard_segment(),
                        Some(VadEvent::UtteranceEnd { .. }) => self.seal(Rc::clone(&mic)),
                        None => {}
                    }
                }
            }
        }
    }

    fn revoke_turn(self) {
        let Some(old) = self.turn.try_update(VoiceTurn::invalidate) else {
            return;
        };
        if let Some(ticket) = old.receipt {
            self.sessions.discard_voice_receipt(ticket);
        }
        if let Some(token) = old.token() {
            if let Some(id) = old.stream_id {
                self.stop_stream(token, id);
            }
            if let Some(run) = old.run.filter(|_| old.terminal.is_none()) {
                self.abort(run);
            }
        }
        self.mic.try_with_value(|mic| {
            if let Some(mic) = mic {
                mic.stop_streaming();
            }
        });
        self.player.try_with_value(|player| {
            if let Some(player) = player {
                player.stop_all();
            }
        });
    }

    fn begin_listening(self, mic: Rc<MicSession>) {
        if !self.current() {
            return;
        }
        self.revoke_turn();
        let Some(token) = self.turn.try_update(|turn| turn.begin(self.epoch)) else {
            return;
        };
        self.ui.caption_state.set(CaptionState::default());
        if self.ui.caption.get_untracked() == Caption::Live {
            self.ui.caption.set(Caption::Idle);
        }
        if self.streaming_off.get_value() {
            return;
        }
        mic.start_streaming();
        spawn_local(async move {
            let subscribed = self
                .dash
                .subscribe_topic_at_epoch(token.epoch, TRANSCRIPT_TOPIC)
                .await;
            if !self.owns(token) {
                return;
            }
            let opened = match subscribed {
                Ok(()) => {
                    self.dash
                        .rpc_call_at_epoch(token.epoch, "voice.stream.start", json!({}))
                        .await
                }
                Err(error) => Err(error),
            };
            let id = opened
                .as_ref()
                .ok()
                .and_then(|value| value["stream_id"].as_str())
                .map(str::to_owned);
            let live = self.owns(token) && self.turn.with_untracked(VoiceTurn::capture_live);
            if !live {
                if let Some(id) = id {
                    self.stop_stream(token, id);
                }
                return;
            }
            match id {
                Some(id) => {
                    self.turn.update(|turn| turn.stream_id = Some(id.clone()));
                    mic.set_frame_sink(move |frame| self.send_frame(token, &id, frame));
                }
                None => {
                    mic.stop_streaming();
                    if opened.as_ref().is_ok_and(|value| {
                        value
                            .get("stream_id")
                            .is_some_and(serde_json::Value::is_null)
                    }) {
                        self.streaming_off.set_value(true);
                    } else {
                        self.turn.update(|turn| turn.frame_loss = true);
                    }
                }
            }
        });
    }

    fn send_frame(self, token: TurnToken, id: &str, frame: String) {
        if !self.owns(token) {
            return;
        }
        let reserved = self.turn.try_update(|turn| {
            if turn.stream_id.as_deref() != Some(id) {
                return false;
            }
            if turn.frame_requests == PENDING_FRAME_CAP {
                turn.frame_loss = true;
                return false;
            }
            turn.frame_requests += 1;
            true
        });
        if reserved != Some(true) {
            return;
        }
        let id = id.to_owned();
        spawn_local(async move {
            if !self.owns(token)
                || !self
                    .turn
                    .with_untracked(|turn| turn.stream_id.as_deref() == Some(id.as_str()))
            {
                return;
            }
            let accepted = self
                .dash
                .rpc_call_at_epoch(
                    token.epoch,
                    "voice.stream.audio",
                    json!({
                        "stream_id": id, "pcm_base64": frame, "ack": true,
                    }),
                )
                .await
                .is_ok_and(|reply| reply["accepted"].as_bool() == Some(true));
            if self.owns(token) {
                self.turn.update(|turn| {
                    turn.frame_requests -= 1;
                    turn.frame_loss |= !accepted;
                    if turn.frame_requests == 0 || turn.frame_loss {
                        if let Some(wake) = turn.frame_drain.take() {
                            let _ = wake.send(());
                        }
                    }
                });
            }
        });
    }

    fn on_gateway_event(self, event: GatewayEvent) {
        if !self.current() {
            return;
        }
        if event.topic == TRANSCRIPT_TOPIC {
            let id = event.data["stream_id"].as_str();
            if id.is_none()
                || !self
                    .turn
                    .with_untracked(|turn| turn.stream_id.as_deref() == id && !turn.recognizing())
            {
                return;
            }
            if event.data["closed"].as_bool() == Some(true)
                || event.data["delta"]["error"].is_string()
            {
                self.turn.update(|turn| {
                    turn.frame_loss = true;
                    if let Some(wake) = turn.frame_drain.take() {
                        let _ = wake.send(());
                    }
                });
                if let Some(mic) = self.mic.get_value() {
                    mic.stop_streaming();
                }
                return;
            }
            let Some(delta) = event.data.get("delta") else {
                return;
            };
            self.ui.caption_state.update(|caption| {
                apply_delta(
                    caption,
                    Delta {
                        committed: delta["committed"].as_str().unwrap_or_default().to_owned(),
                        interim: delta["interim"].as_str().unwrap_or_default().to_owned(),
                    },
                )
            });
            self.ui.caption.set(Caption::Live);
        } else if let Some(suffix) = event.topic.strip_prefix("run.") {
            let kind = event.data["type"].as_str().unwrap_or(suffix);
            let key = self.chat.session_key.get_untracked();
            if let (Some(run), Some(end)) =
                (event.data["run_id"].as_str(), VoiceRunEnd::from_event(kind))
            {
                if event.data["session_key"]
                    .as_str()
                    .is_none_or(|source| key.as_deref() == Some(source))
                {
                    self.turn.update(|turn| turn.observe_terminal(run, end));
                }
            }
        }
    }

    fn seal(self, mic: Rc<MicSession>) {
        let Some(token) = self.turn.with_untracked(VoiceTurn::token) else {
            return;
        };
        if self.turn.try_update(VoiceTurn::seal_capture) != Some(true) {
            return;
        }
        let clip = mic.take_utterance();
        let lost = mic.seal_streaming();
        self.turn.update(|turn| turn.frame_loss |= lost);
        self.dispatch(VoiceEvent::UtteranceSent);
        spawn_local(async move {
            // Only outstanding frames of this turn can hold the final choice.
            // The timeout starts once, after the final partial frame is sealed.
            if !self.owns(token) {
                return;
            }
            let drained = self
                .turn
                .try_update(|turn| {
                    if turn.stream_id.is_none() || turn.frame_requests == 0 || turn.frame_loss {
                        return None;
                    }
                    let (wake, drained) = futures::channel::oneshot::channel();
                    turn.frame_drain = Some(wake);
                    Some(drained)
                })
                .flatten();
            if let Some(drained) = drained {
                let _ =
                    futures::future::select(drained, TimeoutFuture::new(FRAME_ACK_GRACE_MS)).await;
            }
            if self.owns(token) {
                self.recognize(token, clip).await;
            }
        });
    }

    async fn recognize(self, token: TurnToken, clip: Option<CapturedUtterance>) {
        let preview = self.ui.caption_state.with_untracked(|caption| {
            let raw =
                aleph_protocol::voice_text::merge_utterance(&caption.committed, &caption.interim);
            aleph_protocol::voice_text::filter_transcript(&raw)
        });
        let Some((path, stream)) = self
            .turn
            .try_update(|turn| {
                let intact =
                    turn.stream_id.is_some() && !turn.frame_loss && turn.frame_requests == 0;
                let path = turn.finish_recognition(token, !preview.trim().is_empty(), intact)?;
                Some((path, turn.stream_id.take()))
            })
            .flatten()
        else {
            return;
        };
        if let Some(id) = stream {
            self.stop_stream(token, id);
        }
        let text = match path {
            RecognitionPath::Streaming => {
                self.strikes.set_value(0);
                self.ui.caption_state.update(lock);
                self.ui
                    .caption_state
                    .update(|caption| caption.committed.clone_from(&preview));
                self.ui.caption.set(Caption::Live);
                let raw = preview.clone();
                spawn_local(async move {
                    self.format_caption(token, raw).await;
                });
                Some(preview)
            }
            RecognitionPath::Batch => {
                if !self.streaming_off.try_get_value().unwrap_or(true) {
                    let strikes = self
                        .strikes
                        .try_update_value(|strikes| {
                            *strikes += 1;
                            *strikes
                        })
                        .unwrap_or_default();
                    if strikes >= STREAM_STRIKE_LIMIT {
                        self.streaming_off.set_value(true);
                    }
                }
                if self.ui.caption.try_get_untracked() == Some(Caption::Live) {
                    self.ui.caption.set(Caption::Idle);
                }
                let transcript = match clip {
                    Some(clip) => self.transcribe(token, clip).await,
                    None => None,
                };
                if !self.owns(token) {
                    return;
                }
                if let Some(text) = &transcript {
                    self.ui.caption.set(Caption::User(text.clone()));
                }
                transcript
            }
        };
        if let Some(text) = text {
            self.misses.set_value(0);
            self.send_utterance(token, text).await;
        } else {
            self.dispatch(VoiceEvent::TranscribeFailed);
            let misses = self
                .misses
                .try_update_value(|misses| {
                    *misses += 1;
                    *misses
                })
                .unwrap_or_default();
            if misses >= 3 {
                self.ui.caption.set(Caption::Error(
                    t_string!(self.i18n, voice.repeated_misses).to_string(),
                ));
            }
        }
    }

    async fn format_caption(self, token: TurnToken, text: String) {
        if !self.owns(token) {
            return;
        }
        let reply = self
            .dash
            .rpc_call_at_epoch(token.epoch, "voice.format", json!({ "text": text }))
            .await;
        if self.owns(token) && self.ui.caption.try_get_untracked() == Some(Caption::Live) {
            if let Some(text) = reply
                .as_ref()
                .ok()
                .and_then(|r| r["formatted"].as_str())
                .filter(|s| !s.is_empty())
            {
                self.ui
                    .caption_state
                    .update(|caption| apply_formatted(caption, text));
            }
        }
    }

    async fn transcribe(self, token: TurnToken, clip: CapturedUtterance) -> Option<String> {
        let base64 = clip.encode_wav();
        let reply = self
            .dash
            .rpc_call_at_epoch(
                token.epoch,
                "voice.transcribe",
                json!({
                    "audio_base64": base64, "mime_type": "audio/wav",
                }),
            )
            .await
            .ok()?;
        reply["text"]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    }

    async fn send_utterance(self, token: TurnToken, text: String) {
        if !self.owns(token) || self.turn.try_update(|turn| turn.begin_send(token)) != Some(true) {
            return;
        }
        let Some(player) = self.player.get_value() else {
            return;
        };
        player.reset();
        self.splitter.set_value(SentenceSplitter::default());
        let chat = self.chat;
        let key = chat.session_key.get_untracked();
        let agent = chat.agent_id.get_untracked();
        let room = chat.room_project_id.get_untracked();
        let root = if room.is_none() {
            chat.active_project_root.get_untracked()
        } else {
            None
        };
        let model = chat.selected_model.get_untracked();
        let dials =
            shared_ui_logic::state::session_dials_for_send(key.is_some(), &chat.session_knobs());
        let Some(ticket) = self
            .sessions
            .begin_voice_receipt(self.epoch, self.conv, key.clone())
        else {
            return;
        };
        self.turn.update(|turn| turn.receipt = Some(ticket));
        chat.push_user_message(&text);
        let response = ChatApi::send_voice_at_epoch(
            &self.dash,
            self.epoch,
            &text,
            key.as_deref(),
            agent.as_deref(),
            root.as_deref(),
            room.as_deref(),
            model.as_ref(),
            &dials,
        )
        .await;
        match response {
            Ok(response) => {
                let terminal = if self.owns(token) {
                    self.sessions.finish_voice_receipt(
                        self.dash.current_connection_epoch(),
                        ticket,
                        chat,
                        &response.run_id,
                        &response.session_key,
                    )
                } else {
                    self.sessions.discard_voice_receipt(ticket);
                    Err(())
                };
                if let Ok(terminal) = terminal {
                    self.turn.update(|turn| {
                        turn.receipt = None;
                        turn.accept_run(token, response.run_id, terminal);
                    });
                } else {
                    self.abort(response.run_id);
                    if self.owns(token) {
                        self.dispatch(VoiceEvent::RunFailed);
                    }
                }
            }
            Err(error) => {
                self.sessions.discard_voice_receipt(ticket);
                if self.owns(token) {
                    self.turn.update(|turn| turn.receipt = None);
                    self.dispatch(VoiceEvent::RunFailed);
                    if error.code.is_none() {
                        self.ui.caption.set(Caption::Error(
                            t_string!(self.i18n, voice.send_unknown).to_string(),
                        ));
                    }
                }
            }
        }
    }

    fn drive_reply(self) {
        if !self.current() {
            return;
        }
        let (run, terminal) = self.turn.with(|turn| (turn.run.clone(), turn.terminal));
        let Some(run) = run else { return };
        let Some(player) = self.player.get_value() else {
            return;
        };
        if matches!(terminal, Some(VoiceRunEnd::Failed | VoiceRunEnd::Joined)) {
            player.stop_all();
            self.turn.update_untracked(|turn| turn.run = None);
            self.dispatch(if terminal == Some(VoiceRunEnd::Failed) {
                VoiceEvent::RunFailed
            } else {
                VoiceEvent::PlaybackDrained
            });
            return;
        }
        let content = self.chat.messages.with(|messages| {
            let target = format!("assistant-{run}");
            messages
                .iter()
                .rev()
                .find(|message| message.id == target)
                .map(|message| message.content.clone())
        });
        let complete = terminal == Some(VoiceRunEnd::Complete);
        if let Some(content) = content {
            if let Some((sentences, _)) = self
                .splitter
                .try_update_value(|splitter| drive_step(splitter, &content, !complete))
            {
                for sentence in sentences {
                    player.enqueue(self.dash, sentence);
                }
            }
        }
        if complete {
            self.turn.update_untracked(|turn| turn.run = None);
            player.finalize(self.dash);
        }
    }

    fn stop_stream(self, token: TurnToken, id: String) {
        spawn_local(async move {
            let _ = self
                .dash
                .rpc_call_at_epoch(token.epoch, "voice.stream.stop", json!({ "stream_id": id }))
                .await;
        });
    }

    fn abort(self, run: String) {
        spawn_local(async move {
            let _ = ChatApi::abort_voice_at_epoch(&self.dash, self.epoch, &run).await;
        });
    }
}
