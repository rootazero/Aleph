// The SDK one mounted Panel module gets. It is bound to one tab key and one
// connection generation: after `dispose()` (the module went away) or once the
// shell moved to a newer generation (a new socket), results of earlier calls
// and live events are dropped instead of reaching the module.
//
// No `nsh` import: the shell lends everything through `ShellAccess`, so the
// rules here are tested on their own (`sdk.test.ts`).

export type ConnectionState = {
  state: "connecting" | "connected" | "disconnected" | "unavailable";
  endpoint: string;
  subject: string | null;
  pairing: boolean;
  /** Opaque host-issued identity for this actual live connection. */
  voice_binding?: string;
};

export const HOST_UNAVAILABLE = -32090;
export const RESOURCE_NOT_FOUND = -32009;
const CHAT_TAB = "aleph:chat";
type ChatSendAcknowledgement = { run_id: string; session_key: string };

/** A JSON-RPC error answer, or a host failure (`code` -32090). */
export class RpcError extends Error {
  readonly code: number;
  constructor(code: number, message: string, readonly data?: unknown) {
    super(message);
    this.code = code;
  }
}

/**
 * A live frame the host forwarded (`aleph.event`): `stream.<type>` with the
 * stream event as `data`, a Gateway topic such as `connection.warning`, or the
 * desktop host's own `connection.stale` after it dropped frames. Live only:
 * nothing is replayed, so a module refetches when it sees a warning or stale.
 */
export type PanelEvent = { topic: string; data: unknown };

/** What a module that navigated here handed over, e.g. `{ session_key }`. */
export type Intent = Record<string, unknown>;

/**
 * A command in the palette, with an optional default key. Module commands
 * get the tab key as an id prefix (`aleph:chat/stop`), so two modules cannot
 * claim one id.
 */
export type CommandSpec = {
  id: string;
  label: string;
  keywords?: string[];
  defaultKey?: string;
  run(): void;
};

/** What the shell lends a module's SDK; read at call time, never cached. */
export type ShellAccess = {
  rpc(method: string, params: Record<string, unknown>, voiceBinding?: string): Promise<unknown>;
  generation(): number;
  connection(): ConnectionState;
  navigate(key: string, intent?: Intent): boolean;
  /** The intent last handed to the module `key`, if any. */
  intent(key: string): Intent | null;
  rememberChat(sessionKey: string | null): boolean;
  beginChatSend(): ((unknown: boolean) => void) | null;
  chatSendUnknown(): boolean;
  dismissChatSendUnknown(): void;
  onConnection(listener: (state: ConnectionState) => void): () => void;
  onEvent(listener: (event: PanelEvent) => void): () => void;
  /** Add a command to the palette and shortcuts; the answer removes it. */
  registerCommand(spec: CommandSpec): () => void;
};

export type ChatRunReceipt = { run_id: string; session_key: string };
export type ChatTerminalRun = { run_id: string; session_key?: string };

export function readChatTerminalRun(event: PanelEvent): ChatTerminalRun | undefined {
  if (event.topic !== "stream.run_complete" && event.topic !== "stream.run_error") return;
  const data = event.data as Partial<ChatTerminalRun> | null;
  if (typeof data?.run_id !== "string" || !data.run_id) return;
  if (data.session_key !== undefined && (typeof data.session_key !== "string" || !data.session_key)) return;
  return { run_id: data.run_id, session_key: data.session_key };
}

export function matchesChatTerminal(terminal: ChatTerminalRun, run: ChatRunReceipt): boolean {
  // Standard stream terminals have only a unique run id; an explicit session must agree too.
  return terminal.run_id === run.run_id && (terminal.session_key === undefined || terminal.session_key === run.session_key);
}

export type OwnedChatSend = {
  receipt: Promise<ChatRunReceipt>;
  dispatched(): boolean;
  /** Drop a waiting send, or abort once using the receipt and its original live connection. */
  abort(): void;
  /** Report a terminal matching this send's receipt; any dispatched abort must also be acknowledged. */
  release(): void;
};

type OwnedChatJob = { dispatch(): void; retire(): void };
type OwnedChatLane = {
  binding: string;
  owner?: OwnedChatJob | "unknown-run";
  waiting?: OwnedChatJob;
  close(): void;
};

// The shell outlives tab SDKs. Keep only one owner and one waiting send for its physical binding.
const ownedChatLanes = new WeakMap<ShellAccess, OwnedChatLane>();
const MAX_EARLY_CLEANUP_TERMINALS = 8;
const MAX_CLEANUP_TERMINAL_ID_LENGTH = 1024;

function ownedChatLane(shell: ShellAccess, binding: string): OwnedChatLane {
  const previous = ownedChatLanes.get(shell);
  if (previous?.binding === binding) return previous;
  previous?.close();
  const lane: OwnedChatLane = {
    binding,
    close() {
      if (ownedChatLanes.get(shell) !== lane) return;
      stop();
      ownedChatLanes.delete(shell);
      if (lane.owner !== "unknown-run") lane.owner?.retire();
      lane.waiting?.retire();
      lane.owner = lane.waiting = undefined;
    },
  };
  const stop = shell.onConnection(() => {
    const state = shell.connection();
    if (state.state !== "connected" || state.voice_binding !== binding) lane.close();
  });
  ownedChatLanes.set(shell, lane);
  return lane;
}

type VoiceRpcParams = {
  "voice.turn.capture": { capture_id: string };
  "voice.turn.capture_ready": { capture_id: string };
  "voice.turn.capture_finish": { capture_id: string };
  "voice.turn.capture_cancel": { capture_id: string };
  "voice.turn.transcribe": { turn_id: string; language?: string } & (
    { capture_id: string; audio_base64?: never } | { audio_base64: string; capture_id?: never }
  );
  "voice.turn.synthesize": { turn_id: string; text: string };
  "voice.turn.cancel": { turn_id: string };
};

export type PanelSdk = {
  readonly key: string;
  readonly generation: number;
  active(): boolean;
  invoke(method: string, params?: Record<string, unknown>): Promise<unknown>;
  voiceActive(): boolean;
  invokeVoice<M extends keyof VoiceRpcParams>(method: M, params: VoiceRpcParams[M]): Promise<unknown>;
  sendOwnedChat(params: Record<string, unknown>): OwnedChatSend;
  /** One-shot: on voice connection loss or just before this SDK is disposed. */
  onInvalidate(listener: () => void): () => void;
  subscribe(listener: (state: ConnectionState) => void): () => void;
  /** Live events whose topic starts with `topicPrefix` (`"stream."`, `"connection."`). */
  on(topicPrefix: string, listener: (event: PanelEvent) => void): () => void;
  connection(): ConnectionState;
  navigate(key: string, intent?: Intent): boolean;
  /** What the module that navigated here handed over, if any. */
  intent(): Intent | null;
  rememberChat(sessionKey: string | null): boolean;
  sendChat(params: Record<string, unknown>): Promise<ChatSendAcknowledgement>;
  chatSendUnknown(): boolean;
  dismissChatSendUnknown(): void;
  /** Commands this module offers while it is mounted. */
  commands: {
    /** `id` is local to the module; the shell sees `<tab key>/<id>`. */
    register(spec: CommandSpec): () => void;
  };
  dispose(): void;
};

export function createSdk(key: string, generation: number, shell: ShellAccess): PanelSdk {
  let disposed = false;
  let disposing = false;
  const releases = new Set<() => void>();
  const invalidators = new Set<() => void>();
  const current = () => !disposed && shell.generation() === generation;
  const initialConnection = shell.connection();
  const binding = initialConnection.state === "connected" ? initialConnection.voice_binding : undefined;
  const voiceBinding = typeof binding === "string" && binding.trim() ? binding : undefined;
  const bound = () => {
    const state = shell.connection();
    return voiceBinding !== undefined && shell.generation() === generation &&
      state.state === "connected" && state.voice_binding === voiceBinding;
  };
  const voiceActive = () => !disposed && bound();
  const dispatchVoice = (method: string, params: Record<string, unknown>) => {
    if (voiceBinding === undefined || !bound()) throw new Error("Voice connection unavailable");
    return shell.rpc(method, params, voiceBinding);
  };
  const ownsChat = () => key === CHAT_TAB && current();
  // A listener registration the SDK releases on `dispose()`.
  const hold = (stop: () => void) => {
    const release = () => {
      stop();
      releases.delete(release);
    };
    releases.add(release);
    return release;
  };

  const sdk: PanelSdk = {
    key,
    generation,
    active: current,
    async invoke(method, params = {}) {
      if (!current()) throw new Error("disposed");
      try {
        return await shell.rpc(method, params);
      } finally {
        if (!current()) throw new Error("disposed");
      }
    },
    voiceActive,
    async invokeVoice(method, params) {
      if (!voiceActive()) throw new Error("Voice connection unavailable");
      try {
        return await dispatchVoice(method, params);
      } finally {
        if (!voiceActive()) throw new Error("Voice connection unavailable");
      }
    },
    sendOwnedChat(params) {
      if (voiceBinding === undefined || !voiceActive() || disposing) throw new Error("Voice connection unavailable");
      const lane = ownedChatLane(shell, voiceBinding);
      if (lane.waiting) throw new Error("Voice Chat already has a waiting send");
      let sendParams: Record<string, unknown> | undefined = { ...params, run_only: true };
      let run: ChatRunReceipt | undefined;
      let dispatched = false;
      let abandoned = false;
      let terminalSeen = false;
      let abortState: "not-sent" | "unconfirmed" | "confirmed-noop" = "not-sent";
      let stopInvalidation: (() => void) | undefined;
      let stopTerminals: (() => void) | undefined;
      const early: ChatTerminalRun[] = [];
      let resolve!: (run: ChatRunReceipt) => void;
      let reject!: (error: unknown) => void;
      const receipt = new Promise<ChatRunReceipt>((yes, no) => { resolve = yes; reject = no; });
      // A queued send can be cancelled synchronously during disposal, before its caller awaits.
      void receipt.catch(() => {});

      const detach = () => {
        stopInvalidation?.();
        stopTerminals?.();
        stopInvalidation = stopTerminals = undefined;
        early.length = 0;
        sendParams = undefined;
      };
      const release = () => {
        if (lane.owner !== job) return;
        detach();
        const next = lane.waiting;
        lane.waiting = undefined;
        lane.owner = next;
        if (next) next.dispatch();
        else lane.close();
      };
      const recordTerminal = () => {
        if (!run) return;
        terminalSeen = true;
        // A terminal proves the run stopped, not that a dispatched abort freed the host slot.
        if (abortState !== "unconfirmed") release();
      };
      const abortRun = async () => {
        if (!run || !bound() || abortState !== "not-sent" || lane.owner !== job) return;
        abortState = "unconfirmed";
        const { run_id } = run;
        try {
          const ack = (await dispatchVoice("chat.abort", { run_id, run_only: true })) as {
            run_id?: unknown; aborted?: unknown; dropped?: unknown;
          } | null;
          if (ack?.run_id !== run_id || ack.dropped !== 0 || typeof ack.aborted !== "boolean") return;
          if (ack.aborted) release();
          else {
            abortState = "confirmed-noop";
            if (terminalSeen) release();
          }
        } catch {
          // The host slot is unconfirmed even if a terminal arrived. Only retirement can recover.
        }
      };
      const abort = () => {
        if (lane.waiting === job) {
          lane.waiting = undefined;
          detach();
          reject(new Error("Voice Chat send cancelled before dispatch"));
        } else if (lane.owner === job && !abandoned) {
          abandoned = true;
          stopInvalidation?.();
          if (!bound()) return;
          stopTerminals = shell.onEvent((event) => {
            if (lane.owner !== job || !bound()) return;
            const terminal = readChatTerminalRun(event);
            if (!terminal) return;
            if (run) {
              if (matchesChatTerminal(terminal, run)) recordTerminal();
            } else if (early.length < MAX_EARLY_CLEANUP_TERMINALS && terminal.run_id.length <= MAX_CLEANUP_TERMINAL_ID_LENGTH &&
              (terminal.session_key?.length ?? 0) <= MAX_CLEANUP_TERMINAL_ID_LENGTH &&
              !early.some((entry) => entry.run_id === terminal.run_id && entry.session_key === terminal.session_key)) {
              early.push(terminal);
            }
          });
          void abortRun();
        }
      };
      const dispatch = async () => {
        if (!voiceActive() || disposing) {
          reject(new Error("disposed"));
          release();
          return;
        }
        const finish = shell.beginChatSend();
        if (!finish) {
          reject(new Error("A send is already pending"));
          release();
          return;
        }
        let unknown = true;
        try {
          dispatched = true;
          const sending = dispatchVoice("chat.send", sendParams!);
          sendParams = undefined;
          const result = (await sending) as Partial<ChatRunReceipt> | null;
          if (lane.owner !== job) return;
          if (typeof result?.run_id !== "string" || !result.run_id || typeof result.session_key !== "string" || !result.session_key) {
            throw new Error("Invalid chat.send receipt");
          }
          run = { run_id: result.run_id, session_key: result.session_key };
          unknown = false;
          finish(false);
          if (!voiceActive()) abort();
          const received = run;
          if (abandoned) {
            if (bound() && early.some((terminal) => matchesChatTerminal(terminal, received))) recordTerminal();
            else void abortRun();
          }
          if (voiceActive()) resolve({ ...received });
          else reject(new Error("disposed"));
        } catch (error) {
          // A failed send may already have reached Core. Without its receipt only retirement is safe.
          if (error instanceof RpcError && error.code !== HOST_UNAVAILABLE) unknown = false;
          if (lane.owner === job) lane.owner = "unknown-run";
          detach();
          reject(voiceActive() ? error : new Error("disposed"));
        } finally {
          finish(unknown);
          early.length = 0;
        }
      };
      const job: OwnedChatJob = {
        dispatch: () => { void dispatch(); },
        retire() {
          detach();
          reject(new Error("disposed"));
        },
      };
      const waiting = lane.owner !== undefined;
      if (waiting) lane.waiting = job;
      else lane.owner = job;
      stopInvalidation = sdk.onInvalidate(abort);
      if (!waiting) job.dispatch();
      return { receipt, dispatched: () => dispatched, abort, release: recordTerminal };
    },
    onInvalidate(listener) {
      if (!voiceActive() || disposing) {
        listener();
        return () => {};
      }
      let listening = true;
      const invalidate = () => {
        if (!listening) return;
        release();
        listener();
      };
      const stop = shell.onConnection(() => {
        if (!voiceActive()) invalidate();
      });
      const release = hold(() => {
        if (!listening) return;
        listening = false;
        stop();
        invalidators.delete(invalidate);
      });
      invalidators.add(invalidate);
      return release;
    },
    subscribe(listener) {
      if (!current()) throw new Error("disposed");
      return hold(
        shell.onConnection((state) => {
          if (current()) listener(state);
        }),
      );
    },
    on(topicPrefix, listener) {
      if (disposed) throw new Error("disposed");
      return hold(
        shell.onEvent((event) => {
          if (current() && event.topic.startsWith(topicPrefix)) listener(event);
        }),
      );
    },
    connection: () => shell.connection(),
    navigate(target, intent) {
      return current() && shell.navigate(target, intent);
    },
    intent: () => current() ? shell.intent(key) : null,
    rememberChat: (sessionKey) => ownsChat() && shell.rememberChat(sessionKey),
    async sendChat(params) {
      if (!ownsChat()) throw new Error("disposed");
      const finish = shell.beginChatSend();
      if (!finish) throw new Error("A send is already pending");
      let unknown = true;
      try {
        const result = (await shell.rpc("chat.send", params)) as ChatSendAcknowledgement | null;
        if (!current()) throw new Error("disposed");
        if (typeof result?.session_key !== "string" || !result.session_key ||
            typeof result.run_id !== "string" || !result.run_id) {
          throw new Error("Invalid chat.send acknowledgement");
        }
        unknown = false;
        return result;
      } catch (failure) {
        if (!current()) throw new Error("disposed");
        if (failure instanceof RpcError && failure.code !== HOST_UNAVAILABLE) unknown = false;
        throw failure;
      } finally {
        // Keep the shared slot across a tab change until the RPC settles.
        finish(unknown);
      }
    },
    chatSendUnknown: () => ownsChat() && shell.chatSendUnknown(),
    dismissChatSendUnknown() {
      if (ownsChat()) shell.dismissChatSendUnknown();
    },
    commands: {
      register(spec) {
        if (disposed) throw new Error("disposed");
        return hold(
          shell.registerCommand({
            ...spec,
            id: `${key}/${spec.id}`,
            run: () => {
              if (current()) spec.run();
            },
          }),
        );
      },
    },
    dispose() {
      if (disposed || disposing) return;
      disposing = true;
      try {
        for (const invalidate of [...invalidators]) invalidate();
      } finally {
        disposed = true;
        for (const release of [...releases]) release();
      }
    },
  };
  return sdk;
}
