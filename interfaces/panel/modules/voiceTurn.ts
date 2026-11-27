import { matchesChatTerminal, readChatTerminalRun, type ChatRunReceipt, type ChatTerminalRun, type OwnedChatSend, type PanelEvent, type PanelSdk } from "../shell/sdk";
import { newIdempotencyKey } from "./chatIdempotency";

export type VoiceCaptureResult =
  | { capture_id: string; limit_reached: boolean }
  | { audio_base64: string; limit_reached: boolean };
export type VoiceSynthesis = { audio_base64: string; mime_type: string };

/** Audio handles belong to the adapter, not the tab-scoped SDK. */
export type VoiceAudio = {
  capture(id: string, onReady: () => void): Promise<VoiceCaptureResult>;
  finish(id: string): Promise<void>;
  /** Release this id's device/clip, including eventual success of a pending open. */
  cancel(id: string): Promise<void>;
  /** Resolve only after actual playback finishes and releases its handle; reject on failure. */
  play(id: string, result: VoiceSynthesis): Promise<void>;
  /** Stop this id even if its audio is still opening/decoding. */
  stop(id: string): Promise<void>;
};

type VoiceTurnError = "audio" | "transcription" | "send" | "reply" | "synthesis" | "playback" | "connection";
export type VoiceTurnStatus = { readonly limitReached: boolean } & (
  | { readonly phase: "idle" | "opening" | "listening" | "finishing" | "thinking" | "playing" | "stopping" }
  | { readonly phase: "error"; readonly reason: VoiceTurnError }
);

export type VoiceTurn = {
  status(): VoiceTurnStatus;
  subscribe(listener: (status: VoiceTurnStatus) => void): () => void;
  start(): void;
  finish(): void;
  cancel(): void;
  interrupt(): void;
  dispose(): void;
};

const MAX_EARLY_TERMINALS = 8;
const MAX_FINAL_UTF8_BYTES = 64 * 1024;
const TERMINAL_WAIT_MS = 120_000;

type Terminal = ChatTerminalRun & { text: string | null };
type Epoch = {
  id: string;
  captureOwned: boolean;
  capturing: boolean;
  providerPending: boolean;
  playing: boolean;
  limitReached: boolean;
  send?: OwnedChatSend;
  run?: ChatRunReceipt;
  early: Terminal[];
  earlyBytes: number;
  deadline?: ReturnType<typeof setTimeout>;
};

function utf8BytesUpTo(text: string, limit: number): number {
  // Count UTF-8 without copying text into a byte buffer or needing Web APIs.
  let bytes = 0;
  for (const char of text) {
    const cp = char.codePointAt(0)!;
    bytes += cp < 0x80 ? 1 : cp < 0x800 ? 2 : cp < 0x10000 ? 3 : 4;
    if (bytes > limit) break;
  }
  return bytes;
}

export function createVoiceTurn(sdk: PanelSdk, audio: VoiceAudio): VoiceTurn {
  const idle: VoiceTurnStatus = { phase: "idle", limitReached: false };
  let status: VoiceTurnStatus = idle;
  let active: Epoch | undefined;
  let sessionKey: string | undefined;
  let disposed = false;
  let draining: Promise<boolean> | undefined;
  let rearmOnStop = false;
  let unsafeAudio = false;
  const listeners = new Set<(status: VoiceTurnStatus) => void>();
  const releases: (() => void)[] = [];
  const owns = (turn: Epoch) => active === turn && !disposed && sdk.voiceActive();

  function publish(next: VoiceTurnStatus) {
    status = next;
    for (const listener of listeners) {
      if (status !== next) return;
      listener(next);
    }
  }

  function error(reason: VoiceTurnError, limitReached = false) {
    publish({ phase: "error", reason, limitReached });
  }

  function forgetTerminal(turn: Epoch) {
    if (turn.deadline !== undefined) clearTimeout(turn.deadline);
    turn.deadline = undefined;
    turn.early = [];
    turn.earlyBytes = 0;
    turn.run = undefined;
  }

  function abandon(turn: Epoch) {
    active = undefined;
    forgetTerminal(turn);
    turn.send?.abort();
    turn.send = undefined;
    if (turn.providerPending && sdk.voiceActive()) {
      void sdk.invokeVoice("voice.turn.cancel", { turn_id: turn.id }).catch(() => {});
    }
    if (!turn.playing && !turn.captureOwned) return;
    const cleanup = (async () => {
      try {
        if (turn.playing) await audio.stop(turn.id);
        else await audio.cancel(turn.id);
        return true;
      } catch {
        return false;
      }
    })();
    draining = cleanup;
    void cleanup.then((ok) => {
      draining = undefined;
      // Do not open another device if the adapter cannot confirm teardown.
      if (!ok) unsafeAudio = true;
    });
    return cleanup;
  }

  function fail(turn: Epoch, reason: VoiceTurnError) {
    if (!owns(turn)) return;
    abandon(turn);
    error(reason, turn.limitReached);
  }

  function stopTurn(turn: Epoch, rearm: boolean) {
    const stopped = abandon(turn);
    if (!stopped) {
      publish(idle);
      return;
    }
    rearmOnStop = rearm;
    const stopping: VoiceTurnStatus = { phase: "stopping", limitReached: turn.limitReached };
    publish(stopping);
    void stopped.then((ok) => {
      if (disposed || status !== stopping) return;
      if (!ok) error(turn.playing ? "playback" : "audio", turn.limitReached);
      else if (rearmOnStop) start();
      else publish(idle);
    });
  }

  async function synthesize(turn: Epoch, text: string) {
    let result: VoiceSynthesis;
    try {
      turn.providerPending = true;
      const reply = (await sdk.invokeVoice("voice.turn.synthesize", { turn_id: turn.id, text })) as Partial<VoiceSynthesis> | null;
      if (typeof reply?.audio_base64 !== "string" || !reply.audio_base64 || typeof reply.mime_type !== "string" || !reply.mime_type) {
        throw new Error("Invalid voice synthesis response");
      }
      result = { audio_base64: reply.audio_base64, mime_type: reply.mime_type };
    } catch {
      turn.providerPending = false;
      fail(turn, "synthesis");
      return;
    }
    turn.providerPending = false;
    if (!owns(turn)) return;
    try {
      turn.playing = true;
      publish({ phase: "playing", limitReached: turn.limitReached });
      if (!owns(turn)) return;
      await audio.play(turn.id, result);
    } catch {
      fail(turn, "playback");
      return;
    }
    if (!owns(turn)) return;
    active = undefined;
    start();
  }

  function acceptTerminal(turn: Epoch, text: string | null) {
    forgetTerminal(turn);
    turn.send?.release();
    turn.send = undefined;
    if (text === null || !text.trim()) fail(turn, "reply");
    else void synthesize(turn, text);
  }

  function onTerminal(event: PanelEvent) {
    const turn = active;
    if (!turn?.send?.dispatched() || !owns(turn)) return;
    const identity = readChatTerminalRun(event);
    if (!identity) return;
    if (!turn.run && turn.early.some((entry) => entry.run_id === identity.run_id && entry.session_key === identity.session_key)) return;
    const data = event.data as { summary?: { final_response?: unknown } };
    const final = event.topic === "stream.run_complete" ? data.summary?.final_response : null;
    const terminal = { ...identity, text: typeof final === "string" ? final : null };
    if (turn.run && !matchesChatTerminal(terminal, turn.run)) return;
    const budget = MAX_FINAL_UTF8_BYTES - turn.earlyBytes;
    const bytes = terminal.text === null ? 0 : utf8BytesUpTo(terminal.text, budget);
    if (turn.run) {
      acceptTerminal(turn, bytes <= budget ? terminal.text : null);
    } else if (turn.early.length === MAX_EARLY_TERMINALS || bytes > budget) {
      fail(turn, "reply");
    } else {
      turn.early.push(terminal);
      turn.earlyBytes += bytes;
    }
  }

  async function send(turn: Epoch, text: string) {
    const requestedSession = sessionKey;
    let run: ChatRunReceipt;
    try {
      turn.send = sdk.sendOwnedChat({
        message: text,
        channel: "gui:chat",
        stream: true,
        voice_input: true,
        idempotency_key: turn.id,
        ...(requestedSession === undefined ? {} : { session_key: requestedSession }),
      });
      run = await turn.send.receipt;
    } catch {
      fail(turn, "send");
      return;
    }
    if (!owns(turn)) return;
    if (requestedSession !== undefined && run.session_key !== requestedSession) {
      fail(turn, "send");
      return;
    }
    sessionKey = run.session_key;
    turn.run = run;
    const terminal = turn.early.find((entry) => matchesChatTerminal(entry, run));
    turn.early = [];
    turn.earlyBytes = 0;
    if (terminal) acceptTerminal(turn, terminal.text);
    else turn.deadline = setTimeout(() => fail(turn, "reply"), TERMINAL_WAIT_MS);
  }

  async function captureAndTranscribe(turn: Epoch) {
    let clip: VoiceCaptureResult;
    try {
      turn.capturing = turn.captureOwned = true;
      clip = await audio.capture(turn.id, () => {
        if (owns(turn) && status.phase === "opening") publish({ phase: "listening", limitReached: false });
      });
    } catch {
      fail(turn, "audio");
      return;
    }
    if (!owns(turn)) return;
    turn.capturing = false;
    turn.limitReached = clip.limit_reached;
    publish({ phase: "thinking", limitReached: turn.limitReached });
    if (!owns(turn)) return;
    let text: string;
    try {
      turn.providerPending = true;
      const input = "capture_id" in clip ? { capture_id: clip.capture_id } : { audio_base64: clip.audio_base64 };
      const result = (await sdk.invokeVoice("voice.turn.transcribe", { turn_id: turn.id, ...input })) as { text?: unknown } | null;
      if (typeof result?.text !== "string" || !result.text.trim()) throw new Error("No voice transcription");
      text = result.text;
    } catch {
      turn.providerPending = false;
      fail(turn, "transcription");
      return;
    }
    turn.providerPending = false;
    turn.captureOwned = false;
    if (owns(turn)) void send(turn, text);
  }

  function start() {
    if (active || disposed || draining || unsafeAudio || !sdk.voiceActive()) return;
    const turn: Epoch = {
      id: newIdempotencyKey(), capturing: false, captureOwned: false, providerPending: false,
      limitReached: false, playing: false, early: [], earlyBytes: 0,
    };
    active = turn;
    publish({ phase: "opening", limitReached: false });
    if (owns(turn)) void captureAndTranscribe(turn);
  }

  function dispose() {
    if (disposed) return;
    disposed = true;
    if (active) abandon(active);
    for (const release of releases) release();
    listeners.clear();
    sessionKey = undefined;
    status = idle;
  }

  if (sdk.voiceActive()) {
    releases.push(sdk.on("stream.", onTerminal));
    releases.push(sdk.on("connection.", (event) => {
      if (event.topic !== "connection.warning" && event.topic !== "connection.stale") return;
      if (active) fail(active, "connection");
      else if (status.phase === "stopping") error("connection", status.limitReached);
    }));
    releases.push(sdk.onInvalidate(dispose));
  } else dispose();

  return {
    status: () => status,
    subscribe(listener) {
      if (!disposed) listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    start,
    finish() {
      const turn = active;
      if (!turn || !owns(turn) || status.phase !== "listening") return;
      publish({ phase: "finishing", limitReached: false });
      if (!owns(turn)) return;
      void (async () => {
        try { await audio.finish(turn.id); }
        catch { if (turn.capturing) fail(turn, "audio"); }
      })();
    },
    cancel() {
      if (disposed) return;
      if (active) stopTurn(active, false);
      else rearmOnStop = false;
    },
    interrupt() {
      const turn = active;
      if (turn?.playing && owns(turn)) stopTurn(turn, true);
    },
    dispose,
  };
}
