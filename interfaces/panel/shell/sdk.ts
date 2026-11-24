// The SDK one mounted Panel module gets. It is bound to one tab key and one
// connection generation: after `dispose()` (the module went away) or once the
// shell moved to a newer generation (a new socket), results of earlier calls
// are dropped instead of reaching the module.
//
// No `nsh` import: the shell lends everything through `ShellAccess`, so the
// rules here are tested on their own (`sdk.test.ts`).

export type ConnectionState = {
  state: "connecting" | "connected" | "disconnected" | "unavailable";
  endpoint: string;
  subject: string | null;
  pairing: boolean;
};

/** A JSON-RPC error answer, or a host failure (`code` -32090). */
export class RpcError extends Error {
  readonly code: number;
  constructor(code: number, message: string) {
    super(message);
    this.code = code;
  }
}

/** What the shell lends a module's SDK; read at call time, never cached. */
export type ShellAccess = {
  rpc(method: string, params: Record<string, unknown>): Promise<unknown>;
  generation(): number;
  connection(): ConnectionState;
  navigate(key: string): void;
  onConnection(listener: (state: ConnectionState) => void): () => void;
};

export type PanelSdk = {
  readonly key: string;
  readonly generation: number;
  active(): boolean;
  invoke(method: string, params?: Record<string, unknown>): Promise<unknown>;
  subscribe(listener: (state: ConnectionState) => void): () => void;
  connection(): ConnectionState;
  navigate(key: string): void;
  dispose(): void;
};

export function createSdk(key: string, generation: number, shell: ShellAccess): PanelSdk {
  let disposed = false;
  const releases = new Set<() => void>();
  const current = () => !disposed && shell.generation() === generation;

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
      const stop = shell.onConnection((state) => {
        if (current()) listener(state);
      });
      const release = () => {
        stop();
        releases.delete(release);
      };
      releases.add(release);
      return release;
    },
    connection: () => shell.connection(),
    navigate(target) {
      if (current()) shell.navigate(target);
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      for (const release of [...releases]) release();
    },
  };
}
