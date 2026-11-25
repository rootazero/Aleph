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
};

export const HOST_UNAVAILABLE = -32090;
export const RESOURCE_NOT_FOUND = -32009;
const CHAT_TAB = "aleph:chat";
type ChatSendAcknowledgement = { run_id: string; session_key: string };

/** A JSON-RPC error answer, or a host failure (`code` -32090). */
export class RpcError extends Error {
  readonly code: number;
  constructor(code: number, message: string) {
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
  rpc(method: string, params: Record<string, unknown>): Promise<unknown>;
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

export type PanelSdk = {
  readonly key: string;
  readonly generation: number;
  active(): boolean;
  invoke(method: string, params?: Record<string, unknown>): Promise<unknown>;
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
  const releases = new Set<() => void>();
  const current = () => !disposed && shell.generation() === generation;
  const ownsChat = () => key === CHAT_TAB && current();
  let finishSend: ((unknown: boolean) => void) | null = null;
  // A listener registration the SDK releases on `dispose()`.
  const hold = (stop: () => void) => {
    const release = () => {
      stop();
      releases.delete(release);
    };
    releases.add(release);
    return release;
  };

  return {
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
      finishSend = finish;
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
        finish(unknown);
        finishSend = null;
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
      if (disposed) return;
      disposed = true;
      finishSend?.(true);
      for (const release of [...releases]) release();
    },
  };
}
