import { type ConnectionState, RpcError } from "./sdk";

export type PanelRpcEnvelope = { method: string; params: Record<string, unknown>; voice_binding?: string };
type RpcAnswer = { result?: unknown; error?: { code: number; message: string } };

export function createPanelRpc(call: (topic: "aleph.rpc", envelope: PanelRpcEnvelope) => Promise<unknown>) {
  return async (method: string, params: Record<string, unknown>, voiceBinding?: string): Promise<unknown> => {
    const envelope: PanelRpcEnvelope = { method, params };
    if (voiceBinding !== undefined) envelope.voice_binding = voiceBinding;
    const answer = (await call("aleph.rpc", envelope)) as RpcAnswer;
    if (answer.error) throw new RpcError(answer.error.code, answer.error.message);
    return answer.result ?? null;
  };
}

export function readConnection(value: unknown): ConnectionState | null {
  const raw = value as (Omit<ConnectionState, "voice_binding"> & { voice_binding?: unknown }) | null;
  if (!raw || (raw.state !== "connected" && raw.state !== "connecting" && raw.state !== "disconnected" && raw.state !== "unavailable") ||
      typeof raw.endpoint !== "string" || (raw.subject !== null && typeof raw.subject !== "string") || typeof raw.pairing !== "boolean") return null;
  const { state, endpoint, subject, pairing, voice_binding } = raw;
  const connection = { state, endpoint, subject, pairing };
  return state === "connected" && typeof voice_binding === "string" && voice_binding.trim()
    ? { ...connection, voice_binding }
    : connection;
}

export function connectionChanged(previous: ConnectionState, next: ConnectionState): boolean {
  return next.state !== previous.state || next.endpoint !== previous.endpoint ||
    next.subject !== previous.subject || next.voice_binding !== previous.voice_binding;
}
