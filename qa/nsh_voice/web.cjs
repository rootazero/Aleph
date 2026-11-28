// Drive the packaged Panel, not its callbacks. Pairing and TLS pinning follow S3 web-chat.cjs.
const { createHash, X509Certificate } = require('node:crypto');
const fs = require('node:fs/promises');
const { performance } = require('node:perf_hooks');
const { setTimeout: sleep } = require('node:timers/promises');
const { TRANSCRIBE, SYNTHESIZE, CANCEL, FORBIDDEN, MAX_PCM, observe, requests, replies, runEvents, traffic, stats } = require('./wire.cjs');
const { classicTurn, operatorConnect, streamingTurn } = require('./classic.cjs');

class Check extends Error {}
function check(condition, name) {
  if (!condition) throw new Check(name);
}

async function until(read, name, ms = 30000) {
  const deadline = performance.now() + ms;
  while (performance.now() < deadline) {
    const result = await read();
    if (result) return result;
    await sleep(40);
  }
  throw new Check(name);
}

function instrument({ socketPath, bypassSyntheticSignalProcessing, delayFrameAcks, holdCapture, holdChatReceipt }) {
  const tracks = [], sockets = [], stopStates = [];
  const trackIds = new WeakMap();
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  const state = { capture: 0, captureResolved: 0, captureError: null, denied: 0, captureGraph: 0,
    workletRequested: 0, workletSettled: 0, scriptTap: 0,
    trackStops: 0, contextCloses: 0,
    playback: 0, ended: 0, stoppedEarly: 0, recognition: 0, decodedFrames: 0 };
  const activeSources = new WeakSet();
  Object.defineProperty(window, '__voiceQa', { value: {
    snapshot: () => ({ ...state, live: tracks.filter((track) => track.readyState === 'live').length,
      trackStates: tracks.map((track, index) => ({ index, state: track.readyState })), stopStates: [...stopStates] }),
    release,
    close: () => {
      const socket = sockets.findLast((s) => s.readyState === WebSocket.OPEN && new URL(s.url).pathname === socketPath);
      if (!socket) return false;
      socket.close(4001, 'fixture socket interruption');
      return true;
    },
  } });
  const originalCapture = MediaDevices.prototype.getUserMedia;
  MediaDevices.prototype.getUserMedia = async function (constraints) {
    state.capture++;
    try {
      if (bypassSyntheticSignalProcessing) {
        // Chromium suppresses the stationary limit-fixture tone with default mic processing.
        constraints = { ...constraints, audio: {
          ...constraints.audio, echoCancellation: false, noiseSuppression: false, autoGainControl: false,
        } };
      }
      if (holdCapture) await held;
      const stream = await originalCapture.call(this, constraints);
      const audioTracks = stream.getAudioTracks();
      audioTracks.forEach((track, index) => trackIds.set(track, tracks.length + index));
      tracks.push(...audioTracks);
      state.captureResolved++;
      return stream;
    } catch (error) {
      const known = ['NotAllowedError', 'NotFoundError', 'NotReadableError', 'SecurityError', 'AbortError',
        'OverconstrainedError', 'NotSupportedError', 'InvalidStateError', 'TypeError', 'ReferenceError'];
      state.captureError = known.includes(error.name) ? error.name : 'other';
      if (error.name === 'NotAllowedError') state.denied++;
      throw error;
    }
  };
  const trackStop = MediaStreamTrack.prototype.stop;
  MediaStreamTrack.prototype.stop = function (...args) {
    state.trackStops++;
    const id = trackIds.get(this) ?? null;
    const before = this.readyState;
    const result = trackStop.apply(this, args);
    stopStates.push({ id, before, after: this.readyState });
    return result;
  };
  const contextClose = AudioContext.prototype.close;
  AudioContext.prototype.close = function (...args) {
    state.contextCloses++;
    return contextClose.apply(this, args);
  };
  const captureGraph = AudioContext.prototype.createMediaStreamSource;
  AudioContext.prototype.createMediaStreamSource = function (...args) {
    state.captureGraph++;
    return captureGraph.apply(this, args);
  };
  const scriptTap = AudioContext.prototype.createScriptProcessor;
  AudioContext.prototype.createScriptProcessor = function (...args) {
    state.scriptTap++;
    return scriptTap.apply(this, args);
  };
  if (typeof AudioWorklet !== 'undefined') {
    const addModule = AudioWorklet.prototype.addModule;
    AudioWorklet.prototype.addModule = async function (...args) {
      state.workletRequested++;
      try {
        return await addModule.apply(this, args);
      } finally {
        state.workletSettled++;
      }
    };
  }
  const bufferSource = AudioBufferSourceNode.prototype;
  const scheduledSource = AudioScheduledSourceNode.prototype;
  // Chromium implements start() on the buffer source itself, not just on its parent.
  const start = bufferSource.start;
  const stop = scheduledSource.stop;
  bufferSource.start = function (...args) {
    start.apply(this, args);
    state.playback++;
    state.decodedFrames += this.buffer?.length ?? 0;
    activeSources.add(this);
    void window.__voiceQaStarted();
    // Classic sets onended before start(); observe the event before its handler runs cleanup.
    this.addEventListener('ended', () => { activeSources.delete(this); state.ended++; }, { capture: true, once: true });
  };
  scheduledSource.stop = function (...args) {
    stop.apply(this, args);
    if (activeSources.has(this)) state.stoppedEarly++;
  };
  const Speech = window.SpeechRecognition ?? window.webkitSpeechRecognition;
  if (Speech) {
    Speech.prototype.start = function () {
      state.recognition++;
      throw new DOMException('Browser speech is unavailable in this fixture', 'NotAllowedError');
    };
  }
  const Socket = window.WebSocket;
  window.WebSocket = class extends Socket {
    constructor(url, ...args) {
      if (new URL(url, location.href).href !== `${location.origin.replace(/^https:/, 'wss:')}${socketPath}`) {
        throw new DOMException('Fixture only permits the selected Gateway socket', 'SecurityError');
      }
      super(url, ...args);
      sockets.push(this);
      if (delayFrameAcks || holdChatReceipt) {
        const delayedIds = new Set();
        const method = holdChatReceipt ? 'chat.send' : 'voice.stream.audio';
        const send = this.send.bind(this);
        this.send = (message) => {
          const rpc = JSON.parse(message);
          if (rpc.method === method) delayedIds.add(rpc.id);
          send(message);
        };
        this.addEventListener('message', (event) => {
          if (!event.isTrusted) return;
          const response = JSON.parse(event.data);
          if (!delayedIds.delete(response.id)) return;
          event.stopImmediatePropagation();
          const deliver = () => this.dispatchEvent(new MessageEvent('message', { data: event.data }));
          if (holdChatReceipt) void held.then(deliver);
          else setTimeout(deliver, 800);
        });
      }
    }
  };
}

const step = (page, name) => page.getByRole('button', { name, exact: typeof name === 'string' });
const clickOpen = (page, tab) => page.getByLabel(`Open ${tab}`, { exact: true }).click();
const snapshot = (page) => page.evaluate(() => window.__voiceQa.snapshot());
const sttRequest = (row) => row && typeof row.turn === 'string' && row.inputPcmBytes > 0 && !row.captureId;
const hello = (rows) => requests(rows, 'system.hello').map((req) => {
  const [reply, extra] = replies(rows, req);
  return !extra && !reply?.failed && reply?.features.includes('live_events') ? reply : null;
}).find(Boolean);
const stopVoice = (page) => step(page, 'Stop voice conversation').click();

function ownsChatAbort(request, receipt, run) {
  return !!run.session && request.run === run.run && request.runOnly && !request.sessionPresent && request.socket === run.socket &&
    receipt.id === request.id && receipt.socket === request.socket && !receipt.failed && receipt.run === run.run &&
    receipt.aborted === true && receipt.dropped === 0;
}

function cancelledChatTerminal(rows, run, abort) {
  const completed = runEvents(rows, 'stream.run_complete', run);
  const errors = runEvents(rows, 'stream.run_error', run);
  if (completed.length !== 0 || errors.length !== 1) return null;
  const terminal = errors[0];
  const abortIndex = rows.indexOf(abort);
  if (abortIndex < 0 || rows.indexOf(terminal) <= abortIndex || terminal.runErrorCode !== 'CANCELLED' ||
    (terminal.session !== undefined && terminal.session !== run.session)) {
    return null;
  }
  return { run_complete: completed.length, run_error: errors.length };
}

function chatReceipt(rows, handshake) {
  const [send, extra] = requests(rows, 'chat.send');
  const [reply, duplicate] = replies(rows, send);
  if (!handshake || send?.socket !== handshake.socket || extra || duplicate || reply?.failed || !reply?.run || !reply.session ||
      (send.sessionPresent && send.session !== reply.session)) return null;
  return { send, reply };
}

function completedChat(rows, handshake) {
  const chat = chatReceipt(rows, handshake);
  if (!chat) return null;
  const { send, reply } = chat;
  const [accepted, extraAccepted] = runEvents(rows, 'stream.run_accepted', reply);
  const [terminal, extraTerminal] = runEvents(rows, 'stream.run_complete', reply);
  // Acceptance proves the session; completion can precede the RPC reply and does not name the session.
  if (!accepted || extraAccepted || accepted.session !== reply.session || accepted.at < send.at ||
      !terminal?.final || extraTerminal || terminal.at < accepted.at || runEvents(rows, 'stream.run_error', reply).length) return null;
  return { ...chat, accepted, terminal };
}

function completedVoiceTurn(rows, handshake, fingerprint) {
  const chat = completedChat(rows, handshake);
  const [capture, extraCapture] = requests(rows, TRANSCRIBE);
  const [stt, extraStt] = replies(rows, capture);
  const [synth, extraSynth] = requests(rows, SYNTHESIZE);
  const [audio, extraAudio] = replies(rows, synth);
  if (!chat || extraCapture || extraStt || extraSynth || extraAudio || !stt?.text || stt.failed || !sttRequest(capture) ||
      capture.socket !== handshake.socket || synth?.socket !== handshake.socket ||
      !chat.send.voice || !chat.send.runOnly || chat.send.idempotencyKey !== capture.turn ||
      chat.send.text !== stt.text || chat.send.at < stt.at ||
      synth.turn !== capture.turn || synth.text !== chat.terminal.final || synth.at < Math.max(chat.terminal.at, chat.reply.at) ||
      !audio?.outputAudioBytes || audio.outputAudioFingerprint !== fingerprint || audio.failed) return null;
  return { ...chat, capture, stt, synth, audio };
}

function sampleReceipt(rows, badge, stt, send, accepted, terminal, tts, player, firstStart) {
  return {
    temperature: badge, matched_terminal: true, chat_idempotency_key_present: Boolean(send.req.idempotencyKey),
    input_pcm_bytes: stt.req.inputPcmBytes, input_pcm_sample_rate_hz: stt.req.inputSampleRate,
    input_base64_bytes: stt.req.inputAudioBytes, output_base64_bytes: tts.reply.outputAudioBytes,
    player,
    timings_ms: {
      transcribe_frame_to_reply: stt.reply.at - stt.req.at,
      stt_reply_to_run_accepted_frame: accepted.at - stt.reply.at,
      stt_reply_to_chat_send_receipt: send.reply.at - stt.reply.at,
      final_frame_to_player_start_hook: firstStart - terminal.at,
      transcribe_frame_to_player_start_hook: firstStart - stt.req.at,
    },
    wire: traffic(rows),
  };
}

function playerReceipt(before, after, firstStart, audio) {
  const decoded = after.decodedFrames - before.decodedFrames;
  check(Number.isFinite(firstStart), 'browser_player_start_timestamp');
  check(firstStart >= audio.at, 'player_hook_after_wire_reply_observation');
  check(after.playback - before.playback === 1 && decoded > 0, 'one_nonempty_source_start');
  check(after.ended - before.ended === 1, 'one_source_end');
  check(after.stoppedEarly === before.stoppedEarly, 'no_stop_before_source_end');
  check(after.recognition === 0 && after.denied === before.denied && after.live === 0, 'no_browser_speech_or_live_track_left');
  return { scheduled_starts: 1, ended: 1, decoded_frames: decoded, fixture_reply_bytes_match: true, real_audible: null };
}

async function successfulTurn(page, rows, before, playbackTimes, badge, fingerprint, handshake) {
  await until(() => completedVoiceTurn(rows(), handshake, fingerprint), 'paired_voice_stt_chat_terminal_tts');
  await until(async () => (await snapshot(page)).playback > before.playback, 'scheduled_audio_start');
  await until(async () => (await snapshot(page)).ended > before.ended, 'scheduled_source_ended');
  await until(async () => (await snapshot(page)).capture > before.capture + 1, 'rearm_after_playback');
  await stopVoice(page);
  await until(async () => (await snapshot(page)).live === 0, 'stop_releases_browser_tracks');
  const after = await snapshot(page);
  const turn = completedVoiceTurn(rows(), handshake, fingerprint);
  check(turn, 'paired_voice_stt_chat_terminal_tts_once');
  const { capture, stt, send, reply, accepted, terminal, synth, audio } = turn;
  const firstStart = playbackTimes[before.playback];
  const player = playerReceipt(before, after, firstStart, audio);
  check(!await page.getByText('Fixture voice input.', { exact: true }).isVisible() &&
    !await page.getByText('PHASE9_RUNTIME_PROOF', { exact: true }).isVisible(), 'no_transcript_or_reply_rendered_in_voice');
  return { ...sampleReceipt(rows(), badge, { req: capture, reply: stt }, { req: send, reply }, accepted, terminal,
    { req: synth, reply: audio }, player, firstStart), paired_send_chat_audio: true };
}

async function startClassic(page) {
  const voice = page.locator('.voice-stage');
  await page.getByTitle('Tap for voice mode · hold to dictate', { exact: true }).press('Enter');
  await voice.waitFor();
  return voice;
}

async function closeClassic(voice, page) {
  await voice.getByRole('button', { name: '✕ esc to exit', exact: true }).click();
  await voice.waitFor({ state: 'hidden' });
  await until(async () => (await snapshot(page)).live === 0, 'classic_exit_releases_browser_tracks');
}

async function classicSuccessfulTurn(page, rows, before, playbackTimes, badge, fixture, operator) {
  const voice = await startClassic(page);
  await until(() => classicTurn(rows(), operator, fixture.reply_fingerprint), 'classic_batch_stt_chat_terminal_tail_tts');
  await until(async () => (await snapshot(page)).ended > before.ended, 'classic_inline_source_ended');
  await closeClassic(voice, page);
  const after = await snapshot(page);
  const turn = classicTurn(rows(), operator, fixture.reply_fingerprint);
  check(turn && after.capture - before.capture === 1, 'classic_one_batch_turn_one_mic_open');
  const firstStart = playbackTimes[before.playback];
  const player = playerReceipt(before, after, firstStart, turn.tts.reply);
  return { ...sampleReceipt(rows(), badge, turn.stt, turn.send, turn.accepted, turn.terminal, turn.tts, player, firstStart),
    classic_stream_probe_disabled: true, classic_terminal_tail_tts: true };
}

async function classicStreaming(page, rows, fixture, operator, playbackTimes) {
  const voice = await startClassic(page);
  const turn = await until(() => streamingTurn(rows(), operator, fixture.reply_fingerprint, fixture.case), 'classic_stream_recognition_and_playback');
  await until(async () => (await snapshot(page)).ended === turn.synthesis.length, 'classic_stream_playback_ended');
  await closeClassic(voice, page);
  check(playbackTimes.length === turn.synthesis.length, 'classic_stream_playback_count');
  if (fixture.case === 'stream-healthy') check(playbackTimes[0] < turn.terminal.at, 'classic_first_playback_before_run_complete');
  const [streamStart] = requests(rows(), 'voice.stream.start');
  const streamId = replies(rows(), streamStart)[0]?.streamId;
  const sentFrames = requests(rows(), 'voice.stream.audio')
    .filter((frame) => frame.streamId === streamId);
  check(streamId && sentFrames.length > 0 &&
    sentFrames.some((frame) => replies(rows(), frame)[0]?.audioAdmission === true),
  'classic_real_relay_admission');
  if (turn.stt) {
    const order = {
      frames: sentFrames.map((frame) => ({
        at: frame.at,
        replies: replies(rows(), frame).map((reply) => ({ at: reply.at, admitted: reply.audioAdmission })),
      })),
      transcribe_at: turn.stt.at,
    };
    check(sentFrames.at(-1).at <= turn.stt.at,
      `classic_fallback_after_last_stream_frame ${JSON.stringify(order)}`);
  }
  return { batch_uploads: Number(Boolean(turn.stt)), preview: Boolean(turn.preview), chat_sends: 1,
    admitted_frames: sentFrames.filter((frame) => replies(rows(), frame)[0]?.audioAdmission === true).length,
    synthesis_calls: turn.synthesis.length, first_playback_relative_to_terminal_ms: playbackTimes[0] - turn.terminal.at,
    last_frame_to_batch_ms: turn.stt ? turn.stt.at - sentFrames.at(-1).at : null };
}

async function classicNegative(page, rows, fixture, operator) {
  const voice = await startClassic(page);
  let send, accepted;
  if (['close-playback', 'barge-in'].includes(fixture.case)) {
    await until(async () => (await snapshot(page)).playback === 1, 'classic_playback_active');
    await closeClassic(voice, page);
    await until(async () => (await snapshot(page)).stoppedEarly === 1, 'classic_active_source_interrupted');
    check(requests(rows(), 'voice.synthesize').length === 1 && !requests(rows(), 'chat.abort').length, 'classic_playback_only_cancel');
    if (fixture.case === 'barge-in') {
      const successor = await startClassic(page);
      await until(async () => (await snapshot(page)).capture === 2, 'classic_barge_in_rearms_successor_capture');
      await closeClassic(successor, page);
      check((await snapshot(page)).stoppedEarly === 1 && requests(rows(), 'chat.send').length === 1,
        'classic_barge_in_does_not_replay_or_duplicate_old_turn');
      return { tracks_released: true, playback_interrupted: true, successor_capture: true };
    }
    return { tracks_released: true, playback_interrupted: true };
  }
  if (fixture.case === 'permission') {
    await until(async () => (await snapshot(page)).denied === 1, 'classic_microphone_denied');
  } else if (fixture.case === 'close-capture') {
    await until(async () => (await snapshot(page)).capture === 1, 'classic_capture_promise_pending');
  } else if (fixture.case === 'batch-empty') {
    await until(() => replies(rows(), requests(rows(), 'voice.transcribe')[0]).length, 'classic_empty_batch_reply');
  } else {
    await until(() => fs.access(fixture.provider_inflight).then(() => true, () => false), 'classic_provider_in_flight');
    if (['cancel-chat', 'close-send', 'disconnect-send'].includes(fixture.case)) {
      accepted = await until(() => {
        [send] = requests(rows(), 'chat.send');
        return replies(rows(), send).find((reply) => reply.run && !reply.failed);
      }, 'classic_accepted_chat');
      check(send.voice && send.runOnly && send.socket === operator.socket &&
        !runEvents(rows(), 'stream.run_complete', accepted).length, 'classic_pending_exact_chat');
    }
  }
  if (fixture.case === 'disconnect-send') {
    check(await page.evaluate(() => window.__voiceQa.close()), 'classic_close_original_socket');
    await voice.waitFor({ state: 'hidden' });
    await until(async () => (await snapshot(page)).live === 0, 'classic_disconnect_releases_tracks');
    await page.evaluate(() => window.__voiceQa.release());
    const replacement = await until(() => operatorConnect(rows().filter((row) => row.socket !== operator.socket)),
      'classic_replacement_connection');
    check(replacement.socket !== operator.socket && requests(rows(), 'chat.send').length === 1 &&
      !requests(rows(), 'chat.abort').length && !requests(rows(), 'voice.synthesize').length, 'classic_disconnect_does_not_retry');
    return { tracks_released: true, replacement_observed: true };
  }
  await closeClassic(voice, page);
  if (fixture.case === 'close-capture') {
    await page.evaluate(() => window.__voiceQa.release());
    await until(async () => {
      const state = await snapshot(page);
      return state.captureResolved === 1 && state.live === 0 && state.captureGraph === 0;
    }, 'classic_late_capture_released_without_graph');
  }
  if (fixture.case === 'close-send') {
    check(!requests(rows(), 'chat.abort').length, 'classic_no_abort_before_receipt');
    await page.evaluate(() => window.__voiceQa.release());
  }
  if (fixture.case === 'cancel-stt') {
    const [transcribe] = requests(rows(), 'voice.transcribe');
    await until(() => replies(rows(), transcribe).length, 'classic_late_stt_reply');
  }
  if (accepted) {
    const request = await until(() => requests(rows(), 'chat.abort')[0], 'classic_exact_abort');
    const receipt = await until(() => replies(rows(), request)[0], 'classic_exact_abort_reply');
    check(ownsChatAbort(request, receipt, accepted), 'classic_abort_only_owned_run');
    await until(() => cancelledChatTerminal(rows(), accepted, request), 'classic_exact_abort_terminal');
  } else {
    check(!requests(rows(), 'chat.send').length && !requests(rows(), 'chat.abort').length, 'classic_revoked_before_send');
  }
  check(requests(rows(), 'chat.send').length === Number(Boolean(accepted)), 'classic_chat_send_count');
  check(!requests(rows(), 'voice.synthesize').length, 'classic_no_audio_after_revocation');
  return { tracks_released: true, run_cancelled: Boolean(accepted), permission_denied: fixture.case === 'permission' };
}

async function negative(page, kind, rows, wire, fixture, before, handshake) {
  const inFlight = () => until(() => fs.access(fixture.provider_inflight).then(() => true, () => false), 'provider_http_request_in_flight');
  let evidence;
  if (kind === 'permission') {
    await until(async () => (await snapshot(page)).denied === 1, 'permission_rejected_by_chromium');
    evidence = { browser_denial: 'NotAllowedError' };
  } else {
    const stt = await until(() => requests(rows(), TRANSCRIBE)[0], 'negative_transcribe_request');
    check(sttRequest(stt) && stt.socket === handshake.socket, 'negative_paired_canonical_audio_envelope');
    if (kind === 'config-only') {
      const receipt = await until(() => replies(rows(), stt)[0], 'config_only_rpc_reply');
      check(receipt.errorKind === 'not_configured', 'web_never_uses_offline_fallback');
      evidence = { stt_error: 'not_configured' };
    } else if (kind === 'cancel-stt') {
      await inFlight();
      check(replies(rows(), stt).length === 0, 'stt_is_in_flight');
      await stopVoice(page);
      const request = await until(() => requests(rows(), CANCEL)[0], 'voice_cancel_request');
      const receipt = await until(() => replies(rows(), request)[0], 'voice_cancel_receipt');
      const interrupted = await until(() => replies(rows(), stt)[0], 'interrupted_stt_rpc_reply');
      check(request.turn === stt.turn && request.socket === stt.socket && receipt.cancelled === true &&
        interrupted.errorKind === 'cancelled', 'exact_stt_cancel');
      evidence = { exact_turn_cancelled: true, stt_error: 'cancelled' };
    } else if (kind === 'cancel-chat') {
      const { send, reply } = await until(() => chatReceipt(rows(), handshake), 'chat_cancel_send_receipt');
      check(send.voice && send.runOnly && send.idempotencyKey === stt.turn, 'chat_cancel_exact_opt_in');
      await inFlight();
      const beforeStop = rows();
      check(!runEvents(beforeStop, 'stream.run_complete', reply).length &&
        !runEvents(beforeStop, 'stream.run_error', reply).length, 'chat_is_in_flight');
      await stopVoice(page);
      const request = await until(() => requests(rows(), 'chat.abort')[0], 'chat_abort_request');
      const receipt = await until(() => replies(rows(), request)[0], 'chat_abort_receipt');
      check(ownsChatAbort(request, receipt, reply), 'owned_run_aborted');
      await until(() => cancelledChatTerminal(rows(), reply, request), 'observed_cancelled_run_terminal');
      evidence = { exact_run_aborted: true, abort_dropped: receipt.dropped, ownedRun: reply, ownedAbort: request };
    } else {
      await inFlight();
      check(replies(rows(), stt).length === 0, 'disconnect_while_stt_in_flight');
      check(await page.evaluate(() => window.__voiceQa.close()), 'close_real_web_v1_socket');
      await until(() => wire.closures.includes(stt.socket), 'observed_real_socket_close');
      const reconnect = page.getByText('Reconnect', { exact: true });
      const newConnection = () => hello(wire.frames.filter((f) => f.socket !== stt.socket));
      await until(async () => newConnection() || await reconnect.isVisible(), 'reconnect_available');
      if (!newConnection()) await reconnect.click();
      await until(newConnection, 'new_connection_live_events');
      check(new URL(page.url()).origin === fixture.origin, 'reconnect_stayed_paired_loopback');
      evidence = { closed_owned_websocket: true, new_live_connection: true };
    }
  }
  // Cover the synthetic provider's full bounded hold, not just the button callback.
  await sleep(['cancel-stt', 'cancel-chat', 'disconnect'].includes(kind) ? 9000 : 750);
  const after = await snapshot(page);
  check(after.live === 0 && after.capture === before.capture + 1 && after.playback === before.playback &&
    after.recognition === 0, 'negative_no_live_capture_rearm_or_playback');
  const allowed = { [TRANSCRIBE]: kind === 'permission' ? 0 : 1, [SYNTHESIZE]: 0, [CANCEL]: kind === 'cancel-stt' ? 1 : 0,
    'chat.send': kind === 'cancel-chat' ? 1 : 0, 'chat.abort': kind === 'cancel-chat' ? 1 : 0 };
  // In chat cancellation an idempotent turn cancel may accompany chat.abort; the abort is the authority.
  if (kind === 'cancel-chat') {
    const calls = requests(rows(), CANCEL);
    check(calls.length <= 1 && (!calls.length || (calls[0].socket === handshake.socket &&
      calls[0].turn === requests(rows(), TRANSCRIBE)[0].turn)), 'bounded_same_turn_cancel');
    delete allowed[CANCEL];
  }
  check(Object.entries(allowed).every(([method, count]) => requests(rows(), method).length === count), 'negative_not_retried');
  const { ownedRun, ownedAbort, ...projected } = evidence;
  if (ownedRun) {
    const terminals = cancelledChatTerminal(rows(), ownedRun, ownedAbort);
    check(terminals, 'aborted_chat_has_one_cancelled_terminal');
    projected.matching_terminals = terminals;
  }
  return { ...projected, wire: traffic(rows()) };
}

async function ordinaryChat(page, rows, handshake) {
  await clickOpen(page, 'Chat');
  const composer = page.getByRole('textbox', { name: 'Message', exact: true });
  await composer.fill('Fixture ordinary Chat.');
  await composer.press('Enter');
  await until(() => completedChat(rows(), handshake), 'paired_ordinary_chat_terminal');
  await page.getByText('PHASE9_RUNTIME_PROOF', { exact: true }).first().waitFor();
  const chat = completedChat(rows(), handshake);
  check(chat && !chat.send.voice && !chat.send.runOnly && chat.send.idempotencyKey &&
    requests(rows(), SYNTHESIZE).length === 0 && requests(rows(), TRANSCRIBE).length === 0, 'ordinary_chat_unchanged');
  return traffic(rows());
}

async function run(fixturePath) {
  const fixture = JSON.parse(await fs.readFile(fixturePath, 'utf8'));
  const origin = new URL(fixture.origin);
  const classic = fixture.mount === 'classic';
  check(origin.protocol === 'https:' && origin.hostname === '127.0.0.1' && origin.port && origin.port !== '18790' &&
    fixture.origin === origin.origin, 'isolated_origin');
  const report = { passed: false, phase: 'browser_launch', samples: [] };
  let browser, wire, page;
  try {
    const { chromium } = require('playwright');
    const publicKey = new X509Certificate(fixture.certificate).publicKey;
    const spki = createHash('sha256').update(publicKey.export({ type: 'spki', format: 'der' })).digest('base64');
    browser = await chromium.launch({
      executablePath: fixture.chrome ?? undefined,
      args: [`--ignore-certificate-errors-spki-list=${spki}`, '--no-proxy-server', '--disable-background-networking',
        // Deliver one fixture utterance per capture, not a repeated input during Classic playback.
        '--use-fake-device-for-media-stream', `--use-file-for-fake-audio-capture=${fixture.clip}%noloop`],
    });
    report.engine = { name: 'Chromium', version: browser.version() };
    const context = await browser.newContext({ viewport: { width: 1100, height: 760 }, locale: fixture.locale, serviceWorkers: 'block' });
    await context.route('**/*', (route) => new URL(route.request().url()).origin === fixture.origin ? route.continue() : route.abort());
    await context.grantPermissions(fixture.case === 'permission' ? [] : ['microphone'], { origin: fixture.origin });
    page = await context.newPage();
    page.setDefaultTimeout(12000);
    const playbackTimes = [];
    await page.exposeFunction('__voiceQaStarted', () => { playbackTimes.push(performance.now()); });
    await page.addInitScript(instrument, {
      socketPath: classic ? '/ws' : '/v1/ws', bypassSyntheticSignalProcessing: classic || fixture.case === 'limit',
      delayFrameAcks: fixture.case === 'stream-late-ack', holdCapture: fixture.case === 'close-capture',
      holdChatReceipt: ['close-send', 'disconnect-send'].includes(fixture.case),
    });
    wire = observe(page, fixture.origin, classic);
    if (classic) {
      check(fixture.ticket === null, 'classic_without_pairing_credentials');
      report.phase = 'classic_connect';
      const response = await page.goto(`${fixture.origin}/`);
      check(response?.ok() && page.url() === `${fixture.origin}/`, 'classic_control_plane_root');
      report.locale = await page.evaluate(() => navigator.language);
      const operator = await until(() => operatorConnect(wire.frames), 'classic_credentialless_loopback_operator');
      report.authorization = 'credentialless_loopback_operator';
      if (fixture.case === 'success') {
        for (let i = 0; i < 1 + fixture.warm; i++) {
          report.phase = 'classic_voice_success';
          const before = await snapshot(page);
          check(before.live === 0 && before.capture === i, 'classic_mic_idle_before_voice');
          const mark = wire.frames.length;
          const rows = () => wire.frames.slice(mark);
          report.samples.push(await classicSuccessfulTurn(page, rows, before, playbackTimes, i === 0 ? 'cold' : 'warm', fixture, operator));
        }
      } else {
        report.phase = `classic_voice_${fixture.case}`;
        const mark = wire.frames.length;
        const rows = () => wire.frames.slice(mark);
        if (fixture.case.startsWith('stream-')) {
          report.streaming = await classicStreaming(page, rows, fixture, operator, playbackTimes);
        } else {
          report.negative = await classicNegative(page, rows, fixture, operator);
        }
      }
    } else {
      report.phase = 'pair';
      await page.goto(`${fixture.origin}/v1/panel/`);
      await page.getByText('Pair this browser with Aleph').waitFor();
      await page.getByPlaceholder('Pairing ticket or device token').fill(fixture.ticket);
      await page.getByText('Pair with ticket').click();
      const handshake = await until(() => hello(wire.frames), 'paired_hello_live_events');
      report.authorization = 'paired_web_v1_live_events';
      report.locale = await page.evaluate(() => navigator.language);
      report.phase = 'open_voice';
      await clickOpen(page, 'Voice');
      const start = step(page, /^(start|start listening|start voice conversation)$/i);
      await start.waitFor();
      check((await snapshot(page)).capture === 0, 'voice_does_not_open_microphone_on_mount');
      const n = fixture.case === 'success' ? 1 + fixture.warm : 1;
      for (let i = 0; i < n; i++) {
        report.phase = `voice_${fixture.case}`;
        const before = await snapshot(page);
        const mark = wire.frames.length;
        const rows = () => wire.frames.slice(mark);
        await start.click();
        if (['success', 'limit'].includes(fixture.case)) {
          const sample = await successfulTurn(page, rows, before, playbackTimes, i === 0 ? 'cold' : 'warm', fixture.reply_fingerprint, handshake);
          if (fixture.case === 'limit') {
            check(sample.input_pcm_bytes === MAX_PCM, 'duration_limit_uploads_exact_bounded_clip');
          }
          report.samples.push(sample);
        } else {
          report.negative = await negative(page, fixture.case, rows, wire, fixture, before, handshake);
        }
      }
      if (fixture.case === 'success') {
        report.phase = 'ordinary_chat';
        const mark = wire.frames.length;
        report.ordinary_chat = await ordinaryChat(page, () => wire.frames.slice(mark), handshake);
      }
    }
    report.phase = 'check_wire';
    check(wire.valid(), 'bounded_expected_wire');
    if (classic) {
      const sockets = new Set(requests(wire.frames, 'connect').map((row) => row.socket));
      const expected = fixture.case === 'disconnect-send' ? 2 : 1;
      check(sockets.size === expected && [...sockets].every((socket) =>
        operatorConnect(wire.frames.filter((row) => row.socket === socket))), 'classic_credentialless_connect_epochs');
    } else {
      check(!wire.frames.some((row) => FORBIDDEN.test(row.topic ?? '')), 'bounded_web_v1_no_legacy_voice_frames');
    }
    report.strata = Object.fromEntries(['cold', 'warm'].map((temperature) => {
      const samples = report.samples.filter((s) => s.temperature === temperature);
      const metrics = ['transcribe_frame_to_reply', 'stt_reply_to_chat_send_receipt', 'stt_reply_to_run_accepted_frame', 'final_frame_to_player_start_hook',
        'transcribe_frame_to_player_start_hook'];
      return [temperature, { n: samples.length, timings_ms: Object.fromEntries(metrics.map((m) => [m, stats(samples.map((s) => s.timings_ms[m]))])) }];
    }));
    report.passed = true;
  } catch (error) {
    // Do not store Playwright exceptions or console text; they can contain arguments and DOM content.
    report.failure = error instanceof Check ? error.message : error.name;
    if (page) report.browser_audio = await snapshot(page).catch(() => null);
  } finally {
    if (wire) report.all_wire = traffic(wire.frames);
    try {
      await browser?.close();
    } finally {
      await fs.writeFile(fixture.observation, JSON.stringify(report, null, 2));
    }
  }
  return report.passed;
}

module.exports = { hello, stopVoice, ownsChatAbort, cancelledChatTerminal, completedChat, completedVoiceTurn };
if (require.main === module) {
  run(process.argv[2]).then((passed) => { process.exitCode = passed ? 0 : 1; }, () => { process.exitCode = 1; });
}
