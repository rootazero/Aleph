import { expect, test } from "nsh/test";
import { type ConnectionState, createSdk, type PanelEvent, type ShellAccess } from "../shell/sdk";
import { createVoiceTurn, type VoiceAudio, type VoiceCaptureResult, type VoiceSynthesis } from "./voiceTurn";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

async function flush() {
  for (let i = 0; i < 12; i++) await Promise.resolve();
}

const PRIVATE_WORDS = "private spoken words";
const FINAL = "final answer, not a chunk";
const SOUND = { audio_base64: "d2F2", mime_type: "audio/wav" };
const RUN = { run_id: "run-1", session_key: "agent:main:main@1" };
const WAV = { audio_base64: "d2F2", limit_reached: false };
const VOICE_BINDING = "fixture-live-connection";

function fixture({ delayAbort = false } = {}) {
  let generation = 1;
  let state: ConnectionState = {
    state: "connected", endpoint: "https://aleph.test", subject: "owner", pairing: false, voice_binding: VOICE_BINDING,
  };
  const connections = new Set<(state: ConnectionState) => void>();
  const events = new Set<(event: PanelEvent) => void>();
  const calls: { method: string; params: Record<string, unknown>; reply: ReturnType<typeof deferred<unknown>> }[] = [];
  const shell: ShellAccess = {
    rpc(method, params, voiceBinding) {
      expect(voiceBinding).toBe(VOICE_BINDING);
      const reply = deferred<unknown>();
      calls.push({ method, params, reply });
      if (method === "voice.turn.cancel") reply.resolve({});
      if (method === "chat.abort" && !delayAbort) reply.resolve({ run_id: params.run_id, aborted: true, dropped: 0 });
      return reply.promise;
    },
    generation: () => generation,
    connection: () => state,
    navigate: () => true,
    intent: () => null,
    rememberChat: () => false,
    beginChatSend: () => () => {},
    chatSendUnknown: () => false,
    dismissChatSendUnknown: () => {},
    onConnection(listener) { connections.add(listener); return () => { connections.delete(listener); }; },
    onEvent(listener) { events.add(listener); return () => { events.delete(listener); }; },
    registerCommand: () => () => {},
  };
  const emit = (topic: string, data: unknown) => {
    for (const listener of [...events]) listener({ topic, data });
  };
  const complete = (run: string, text: unknown, sessionKey?: string) => emit("stream.run_complete", {
    type: "run_complete", run_id: run, seq: 42, summary: { final_response: text }, total_duration_ms: 123,
    ...(sessionKey === undefined ? {} : { session_key: sessionKey }),
  });
  const captureRequests: {
    id: string; open(): void; result: ReturnType<typeof deferred<VoiceCaptureResult>>;
  }[] = [];
  const playbacks: { id: string; sound: VoiceSynthesis; result: ReturnType<typeof deferred<void>> }[] = [];
  const microphones = new Set<string>();
  const cancelled = new Set<string>();
  const speakers = new Set<string>();
  const finishes: string[] = [];
  const stopGate = deferred<void>();
  let delayStop = false;
  const audio: VoiceAudio = {
    capture(id, onReady) {
      const result = deferred<VoiceCaptureResult>();
      captureRequests.push({ id, open() {
        if (!cancelled.has(id)) microphones.add(id);
        onReady();
      }, result });
      return result.promise.finally(() => { microphones.delete(id); });
    },
    async finish(id) { finishes.push(id); },
    async cancel(id) { cancelled.add(id); microphones.delete(id); },
    play(id, sound) {
      const result = deferred<void>();
      speakers.add(id);
      playbacks.push({ id, sound, result });
      return result.promise.finally(() => { speakers.delete(id); });
    },
    async stop(id) { speakers.delete(id); if (delayStop) await stopGate.promise; },
  };
  const sdk = createSdk("aleph:voice", generation, shell);
  const turn = createVoiceTurn(sdk, audio);
  const statusHistory = [turn.status()];
  turn.subscribe((status) => statusHistory.push(status));
  const matching = (method: string) => calls.filter((call) => call.method === method);
  async function throughTranscription(clip: VoiceCaptureResult = WAV) {
    turn.start();
    const capture = captureRequests[captureRequests.length - 1]!;
    capture.open();
    capture.result.resolve(clip);
    await flush();
    const transcribe = matching("voice.turn.transcribe").at(-1)!;
    transcribe.reply.resolve({ text: PRIVATE_WORDS });
    await flush();
    return { capture, transcribe };
  }
  async function throughSend(clip: VoiceCaptureResult = WAV) {
    return { ...await throughTranscription(clip), send: matching("chat.send").at(-1)! };
  }
  async function throughPlayback() {
    const { send } = await throughSend();
    send.reply.resolve(RUN);
    await flush();
    complete(RUN.run_id, FINAL);
    await flush();
    matching("voice.turn.synthesize")[0].reply.resolve(SOUND);
    await flush();
  }
  return {
    sdk, turn, statusHistory, calls, matching, emit, complete, throughTranscription, throughSend, throughPlayback, captureRequests,
    playbacks, microphones, speakers, finishes, cancelled, connections, events,
    delayStop: () => { delayStop = true; return stopGate; },
    drop() {
      generation++;
      state = { ...state, state: "disconnected", subject: null };
      for (const listener of [...connections]) listener(state);
    },
  };
}

test("whole_turn_uses_final_summary_without_rendering_spoken_words", async () => {
  const f = fixture();
  expect(f.turn.status().phase).toBe("idle");
  expect(f.calls.length).toBe(0);
  expect(f.captureRequests.length).toBe(0);
  f.turn.start();
  f.turn.start();
  expect(f.turn.status().phase).toBe("opening");
  const capture = f.captureRequests[0];
  capture.open();
  expect(f.turn.status().phase).toBe("listening");
  f.turn.finish();
  f.turn.finish();
  expect(f.finishes).toEqual([capture.id]);
  capture.result.resolve({ ...WAV, limit_reached: true });
  await flush();
  expect(f.calls[0].params).toEqual({ turn_id: capture.id, audio_base64: WAV.audio_base64 });
  expect(f.turn.status().limitReached).toBe(true);
  f.calls[0].reply.resolve({ text: PRIVATE_WORDS });
  await flush();
  expect(f.calls[1].params).toEqual({
    message: PRIVATE_WORDS, channel: "gui:chat", stream: true, voice_input: true, idempotency_key: capture.id,
    run_only: true,
  });
  expect(capture.id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  f.emit("stream.response_chunk", { run_id: RUN.run_id, content: "never spoken", delta: "never spoken" });
  f.calls[1].reply.resolve(RUN);
  await flush();
  f.complete(RUN.run_id, FINAL);
  f.complete(RUN.run_id, "duplicate answer");
  await flush();
  expect(f.calls.map(({ method }) => method)).toEqual(["voice.turn.transcribe", "chat.send", "voice.turn.synthesize"]);
  expect(f.calls[2].params).toEqual({ turn_id: capture.id, text: FINAL });
  f.calls[2].reply.resolve(SOUND);
  await flush();
  expect(f.playbacks.length).toBe(1);
  expect(f.playbacks[0].sound).toEqual(SOUND);
  expect(f.microphones.size).toBe(0);
  expect(f.captureRequests.length).toBe(1);
  expect(JSON.stringify(f.statusHistory)).not.toContain(PRIVATE_WORDS);
  expect(JSON.stringify(f.statusHistory)).not.toContain(FINAL);
  f.playbacks[0].result.resolve();
  await flush();
  expect(f.speakers.size).toBe(0);
  expect(f.turn.status().phase).toBe("opening");
  const next = f.captureRequests[1];
  expect(next.id).not.toBe(capture.id);
  expect(f.turn.status().limitReached).toBe(false);
  next.open();
  next.result.resolve(WAV);
  await flush();
  f.matching("voice.turn.transcribe")[1].reply.resolve({ text: "next" });
  await flush();
  expect(f.matching("chat.send")[1].params.session_key).toBe(RUN.session_key);
  f.turn.dispose();
});

test("early_or_late_terminal_is_bound_to_current_run_and_receipt_session", async () => {
  const f = fixture();
  const { send, capture, transcribe } = await f.throughSend({ capture_id: "sealed-native-clip", limit_reached: true });
  expect(transcribe.params).toEqual({ turn_id: capture.id, capture_id: "sealed-native-clip" });
  f.complete("other-run", "other", RUN.session_key);
  f.complete(RUN.run_id, "wrong session", "foreign-session");
  f.complete(RUN.run_id, FINAL, RUN.session_key);
  for (let i = 0; i < 10; i++) f.complete(RUN.run_id, FINAL, RUN.session_key);
  expect(f.matching("voice.turn.synthesize").length).toBe(0);
  send.reply.resolve(RUN);
  await flush();
  expect(f.matching("voice.turn.synthesize").map(({ params }) => params.text)).toEqual([FINAL]);
  f.turn.dispose();
});

test("known_run_ignores_foreign_terminal_and_accepts_standard_keyless_gateway_frame", async () => {
  const f = fixture();
  const { send } = await f.throughSend();
  send.reply.resolve(RUN);
  await flush();
  f.complete(RUN.run_id, "wrong session", "foreign-session");
  f.complete("other-run", "wrong run", RUN.session_key);
  expect(f.matching("voice.turn.synthesize").length).toBe(0);
  f.complete(RUN.run_id, FINAL);
  expect(f.matching("voice.turn.synthesize").map(({ params }) => params.text)).toEqual([FINAL]);
  f.turn.dispose();
});

test("early_terminal_budget_is_eight_entries_and_64_KiB_utf8_including_keyless_frames", async () => {
  const full = "😀".repeat(16_384);
  const exact = fixture();
  const { send } = await exact.throughSend();
  for (let i = 0; i < 7; i++) exact.complete(`other-${i}`, undefined);
  exact.complete(RUN.run_id, full);
  expect(exact.turn.status().phase).toBe("thinking");
  send.reply.resolve(RUN);
  await flush();
  expect(exact.matching("voice.turn.synthesize")[0].params.text === full).toBe(true);
  exact.turn.dispose();

  for (const overflow of ["entries", "text"]) {
    const f = fixture();
    const { send: pending } = await f.throughSend();
    if (overflow === "entries") {
      for (let i = 0; i < 9; i++) f.complete(`other-${i}`, "early");
    } else {
      f.complete("other-run", full);
      f.complete(RUN.run_id, "x");
    }
    expect(f.turn.status().phase).toBe("error");
    pending.reply.resolve(RUN);
    await flush();
    expect(f.matching("chat.abort").map(({ params }) => params)).toEqual([{ run_id: RUN.run_id, run_only: true }]);
    expect(f.matching("voice.turn.synthesize").length).toBe(0);
    f.turn.dispose();
  }
});

test("failed_missing_or_oversize_final_never_synthesizes", async () => {
  for (const final of ["run-error", undefined, "\n  ", "界".repeat(21_846)]) {
    const f = fixture();
    const { send } = await f.throughSend();
    send.reply.resolve(RUN);
    await flush();
    if (final === "run-error") f.emit("stream.run_error", { ...RUN, error: PRIVATE_WORDS });
    else f.complete(RUN.run_id, final);
    await flush();
    expect(f.turn.status().phase).toBe("error");
    expect(f.matching("voice.turn.synthesize").length).toBe(0);
    expect(JSON.stringify(f.statusHistory)).not.toContain(PRIVATE_WORDS);
    expect(f.matching("chat.abort").length).toBe(0);
    f.turn.dispose();
  }
});

test("cancel_before_send_receipt_aborts_only_returned_owned_run", async () => {
  const f = fixture();
  const { send } = await f.throughSend();
  f.complete("unowned", "ignore me", RUN.session_key);
  f.turn.cancel();
  await flush();
  expect(f.matching("chat.abort").length).toBe(0);
  send.reply.resolve(RUN);
  await flush();
  f.complete(RUN.run_id, FINAL);
  expect(f.matching("chat.abort").map(({ params }) => params)).toEqual([{ run_id: RUN.run_id, run_only: true }]);
  expect(f.matching("voice.turn.synthesize").length).toBe(0);
  expect(f.captureRequests.length).toBe(1);
  f.turn.dispose();
});

test("next_voice_turn_transcribes_but_waits_for_abandoned_run_receipt_and_abort_ack", async () => {
  const f = fixture({ delayAbort: true });
  const { capture: a, send } = await f.throughSend();
  f.turn.cancel();
  const { capture: b, transcribe } = await f.throughTranscription();
  expect(b.id === a.id).toBe(false);
  expect(transcribe.params.turn_id).toBe(b.id);
  expect(f.matching("chat.send").map(({ params }) => params.idempotency_key)).toEqual([a.id]);
  expect(f.matching("chat.abort").length).toBe(0);
  for (let i = 0; i < 9; i++) f.complete(`unrelated-${i}`, FINAL);
  expect(f.turn.status().phase).toBe("thinking");

  send.reply.resolve(RUN);
  await flush();
  const [abort] = f.matching("chat.abort");
  expect(abort.params).toEqual({ run_id: RUN.run_id, run_only: true });
  expect(f.matching("chat.send").map(({ params }) => params.idempotency_key)).toEqual([a.id]);
  abort.reply.resolve({ run_id: RUN.run_id, aborted: true, dropped: 0 });
  await flush();
  expect(f.matching("chat.send").map(({ params }) => params.idempotency_key)).toEqual([a.id, b.id]);

  const bRun = { ...RUN, run_id: "run-2" };
  f.matching("chat.send")[1].reply.resolve(bRun);
  await flush();
  f.complete(RUN.run_id, "abandoned reply", RUN.session_key);
  expect(f.matching("voice.turn.synthesize").length).toBe(0);
  f.complete(bRun.run_id, FINAL, bRun.session_key);
  expect(f.matching("voice.turn.synthesize").map(({ params }) => params.text)).toEqual([FINAL]);
  f.turn.dispose();
});

test("cancel_next_voice_turn_while_waiting_never_dispatches_its_chat", async () => {
  const f = fixture({ delayAbort: true });
  const { capture: a, send } = await f.throughSend();
  send.reply.resolve(RUN);
  await flush();
  f.turn.cancel();
  const { capture: b } = await f.throughTranscription();
  expect(f.matching("chat.send").map(({ params }) => params.idempotency_key)).toEqual([a.id]);
  f.turn.cancel();
  expect(f.turn.status().phase).toBe("idle");
  f.matching("chat.abort")[0].reply.resolve({ run_id: RUN.run_id, aborted: true, dropped: 0 });
  await flush();
  expect(f.matching("chat.send").map(({ params }) => params.idempotency_key)).toEqual([a.id]);
  const { capture: c } = await f.throughSend();
  expect(c.id === b.id).toBe(false);
  expect(f.matching("chat.send").map(({ params }) => params.idempotency_key)).toEqual([a.id, c.id]);
  f.turn.dispose();
});

test("sdk_unmount_cleans_up_pending_capture_and_no_late_ready_reopens_it", async () => {
  const f = fixture();
  f.turn.start();
  const capture = f.captureRequests[0];
  f.sdk.dispose();
  capture.open();
  capture.result.resolve(WAV);
  await flush();
  expect(f.cancelled.has(capture.id)).toBe(true);
  expect(f.microphones.size).toBe(0);
  expect(f.calls.length).toBe(0);
  expect(f.events.size + f.connections.size).toBe(0);
  expect(f.statusHistory.map(({ phase }) => phase)).toEqual(["idle", "opening"]);
});

test("warning_and_generation_loss_cancel_without_replaying", async () => {
  const warning = fixture();
  const { send } = await warning.throughSend();
  send.reply.resolve(RUN);
  await flush();
  warning.emit("connection.stale", {});
  warning.complete(RUN.run_id, FINAL);
  expect(warning.matching("chat.abort").map(({ params }) => params)).toEqual([{ run_id: RUN.run_id, run_only: true }]);
  expect(warning.matching("voice.turn.synthesize").length).toBe(0);
  warning.turn.dispose();

  const dropped = fixture();
  const { send: staleReceipt } = await dropped.throughSend();
  dropped.drop();
  staleReceipt.reply.resolve(RUN);
  await flush();
  expect(dropped.matching("chat.abort").length).toBe(0);
  expect(dropped.matching("chat.send").length).toBe(1);
  expect(dropped.matching("voice.turn.synthesize").length).toBe(0);
  dropped.turn.start();
  expect(dropped.captureRequests.length).toBe(1);
  dropped.turn.dispose();
});

test("dispose_cancels_provider_work_before_sdk_revocation_and_discards_late_result", async () => {
  const f = fixture();
  f.turn.start();
  const capture = f.captureRequests[0];
  capture.open();
  capture.result.resolve(WAV);
  await flush();
  f.sdk.dispose();
  expect(f.matching("voice.turn.cancel").map(({ params }) => params)).toEqual([{ turn_id: capture.id }]);
  expect(f.cancelled.has(capture.id)).toBe(true);
  f.matching("voice.turn.transcribe")[0].reply.resolve({ text: PRIVATE_WORDS });
  await flush();
  expect(f.matching("chat.send").length).toBe(0);
});

test("interrupt_waits_for_stop_and_old_playback_cannot_rearm", async () => {
  const f = fixture();
  await f.throughPlayback();
  expect(f.speakers.size).toBe(1);
  const stopped = f.delayStop();
  f.turn.interrupt();
  expect(f.speakers.size).toBe(0);
  expect(f.captureRequests.length).toBe(1);
  f.playbacks[0].result.resolve();
  await flush();
  expect(f.captureRequests.length).toBe(1);
  stopped.resolve();
  await flush();
  expect(f.captureRequests.length).toBe(2);
  f.turn.dispose();
});

test("only_genuine_playback_success_or_explicit_interrupt_rearms", async () => {
  const failed = fixture();
  await failed.throughPlayback();
  failed.playbacks[0].result.reject(new Error(PRIVATE_WORDS));
  await flush();
  expect(failed.turn.status().phase).toBe("error");
  expect(failed.speakers.size).toBe(0);
  expect(failed.captureRequests.length).toBe(1);
  expect(JSON.stringify(failed.statusHistory)).not.toContain(PRIVATE_WORDS);
  failed.turn.dispose();

  const dropped = fixture();
  await dropped.throughPlayback();
  dropped.drop();
  expect(dropped.speakers.size).toBe(0);
  dropped.playbacks[0].result.resolve();
  await flush();
  expect(dropped.captureRequests.length).toBe(1);
  dropped.turn.dispose();
});

test("lost_send_is_not_retried_and_a_known_run_times_out_without_history_guess", async () => {
  const f = fixture();
  const { send } = await f.throughSend();
  send.reply.reject(new Error(PRIVATE_WORDS));
  await flush();
  expect(f.turn.status().phase).toBe("error");
  expect(f.matching("chat.send").length).toBe(1);
  expect(f.matching("chat.abort").length).toBe(0);
  expect(JSON.stringify(f.statusHistory)).not.toContain(PRIVATE_WORDS);
  f.turn.dispose();

  const timed = fixture();
  const { send: known } = await timed.throughSend();
  const schedule = globalThis.setTimeout;
  let deadline: (() => void) | undefined;
  let delay: number | undefined;
  try {
    globalThis.setTimeout = ((callback: () => void, ms: number) => {
      deadline = callback;
      delay = ms;
      return schedule(() => {}, ms);
    }) as typeof setTimeout;
    known.reply.resolve(RUN);
    await flush();
  } finally {
    globalThis.setTimeout = schedule;
  }
  expect(typeof deadline).toBe("function");
  expect(delay! > 0 && delay! <= 180_000).toBe(true);
  deadline!();
  timed.complete(RUN.run_id, FINAL);
  expect(timed.turn.status().phase).toBe("error");
  expect(timed.calls.map(({ method }) => method)).toEqual(["voice.turn.transcribe", "chat.send", "chat.abort"]);
  timed.turn.dispose();
});
