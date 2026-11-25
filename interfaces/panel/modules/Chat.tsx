// Core owns committed history. Rust folds authorized live presentation into
// the same list, while TSX owns RPC lifetimes and composer interaction.
import { cleanup, signal, type Signal, Text, View } from "nsh";
import { Button, Input } from "nsh/base";
import { ChatModel } from "aleph/panel";
import { type PanelEvent, type PanelSdk, RpcError } from "../shell/sdk";
import { TranscriptList } from "./chat/TranscriptList";
import { ContextSheet } from "./chat/ContextSheet";
import type { Change } from "./chat/Diff";
import type { Diagram } from "./chat/MessageBody";

const HOST_UNAVAILABLE = -32090;
const HISTORY_LIMIT = 200;
const DETAIL_RUN_LIMIT = 12;

// The native runtime need not ship Web Crypto for this cache-scoped key.
function newIdempotencyKey(): string {
  const hex = (digits: number) =>
    Array.from({ length: digits }, () => Math.floor(Math.random() * 16).toString(16)).join("");
  const variant = (8 + Math.floor(Math.random() * 4)).toString(16);
  return `${hex(8)}-${hex(4)}-4${hex(3)}-${variant}${hex(3)}-${hex(12)}`;
}

type History = {
  session_key: string;
  messages?: { run_id?: string }[];
  active_run?: string | null;
};
type SendResult = { run_id: string; session_key: string };

type TextRow = { kind: "text"; key: string; role: string; text: string; streaming: boolean };
export type ToolRow = {
  kind: "tool";
  key: string;
  run_id: string;
  tool_call_id: string;
  display_name: string;
  args_text: string;
  status: "pending" | "running" | "ok" | "error";
  duration_ms: number | null;
  has_diff: boolean;
  presentation_omitted: boolean;
};
export type ChatRow = TextRow | ToolRow;
type RunState = { activeRun: string | null; missingPrefix: boolean };
type Patch = RunState & {
  upsert: ChatRow[];
  remove: string[];
  appendOrder: string[];
  reconcile: boolean;
  error: string | null;
};
type Snapshot = RunState & { rows: ChatRow[]; limited: boolean };

export function Chat(props: { sdk: PanelSdk; setReaderMode: (active: boolean) => void }) {
  const { sdk } = props;
  const fullscreen = signal<string | null>(null);
  const showContext = signal(false);
  const setFullscreen = (key: string | null) => {
    if (key) showContext.set(false);
    fullscreen.set(key);
    props.setReaderMode(key !== null);
  };
  cleanup(() => props.setReaderMode(false));
  const model = new ChatModel();
  const sessionKey = signal<string | null>((sdk.intent()?.session_key as string | undefined) ?? null);
  model.reset(sessionKey() ?? "");
  const keys = signal<string[]>([]);
  const rows = new Map<string, Signal<ChatRow>>();
  const loading = signal(sessionKey() !== null);
  const activeRun = signal<string | null>(null);
  const missingPrefix = signal(false);
  const limited = signal(false);
  const notice = signal<string | null>(null);
  const draft = signal("");
  const sending = signal(false);
  let composing = false;
  let historyRequest = 0;
  let hasHistory = false;

  const upsert = (row: ChatRow) => {
    const current = rows.get(row.key);
    if (current) current.set(row);
    else rows.set(row.key, signal(row));
  };
  const runState = (state: RunState) => {
    activeRun.set(state.activeRun);
    missingPrefix.set(state.missingPrefix);
  };
  const replace = (snapshot: Snapshot | null) => {
    if (!snapshot) return;
    const next = snapshot.rows.map((row) => {
      upsert(row);
      return row.key;
    });
    const retained = new Set(next);
    for (const key of rows.keys()) if (!retained.has(key)) rows.delete(key);
    keys.set(next);
    runState(snapshot);
    limited.set(snapshot.limited);
  };
  const report = (failure: unknown) => {
    const error = failure as Error;
    if (error.message !== "disposed") notice.set(error.message);
  };

  const load = async () => {
    const key = sessionKey();
    if (!key) return;
    const request = ++historyRequest;
    const current = () => request === historyRequest && key === sessionKey();
    try {
      const history = (await sdk.invoke("chat.history", { session_key: key, limit: HISTORY_LIMIT })) as History;
      if (!current()) return;
      // Attach immediately so new chunks can land while the optional details
      // read runs. Later reconciliations replace a completed run in one pass.
      if (!hasHistory && !activeRun()) {
        replace(model.replaceHistory(history, null) as Snapshot | null);
        loading.set(false);
      }
      hasHistory = true;
      const runIds = [...new Set((history.messages ?? []).slice().reverse()
        .map((message) => message.run_id).filter((id): id is string => Boolean(id)))];
      let details: unknown = null;
      if (runIds.length > 0) {
        try {
          details = await sdk.invoke("chat.transcript_details", {
            session_key: key, run_ids: runIds.slice(0, DETAIL_RUN_LIMIT).reverse(),
          });
        } catch {
          // Older hosts and failed detail reads still provide committed prose.
          details = { runs: [], limited: true };
        }
      }
      if (!current()) return;
      const snapshot = model.replaceHistory(history, details) as Snapshot | null;
      if (snapshot && runIds.length > DETAIL_RUN_LIMIT) snapshot.limited = true;
      replace(snapshot);
    } catch (failure) {
      if (current()) report(failure);
    } finally {
      if (current()) loading.set(false);
    }
  };

  const apply = (patch: Patch) => {
    for (const row of patch.upsert) upsert(row);
    for (const key of patch.remove) rows.delete(key);
    if (patch.remove.length || patch.appendOrder.length) {
      const removed = new Set(patch.remove);
      keys.update((prior) => [...prior.filter((key) => !removed.has(key)), ...patch.appendOrder]);
    }
    runState(patch);
    if (patch.error) notice.set(`The reply failed: ${patch.error}`);
    if (patch.reconcile) void load();
  };

  const onStream = (event: PanelEvent) => {
    apply(model.accept({ ...event, nowMs: Date.now() }) as Patch);
  };
  sdk.on("stream.", onStream);
  sdk.on("connection.", (event) => {
    if (event.topic === "connection.warning" || event.topic === "connection.stale") {
      notice.set("Live updates were interrupted; reloaded from Aleph.");
      apply(model.interrupt() as Patch);
      void load();
    }
  });
  void load();

  const send = async () => {
    const text = draft().trim();
    if (!text || composing || sending() || activeRun()) return;
    const key = sessionKey();
    const params: Record<string, unknown> = {
      message: text, channel: "gui:chat", stream: true, idempotency_key: newIdempotencyKey(),
    };
    if (key) params.session_key = key;
    ++historyRequest;
    loading.set(false);
    sending.set(true);
    notice.set(null);
    apply(model.beginSend(text, Date.now()) as Patch);
    draft.set("");
    try {
      const result = (await sdk.invoke("chat.send", params)) as SendResult;
      ++historyRequest;
      sessionKey.set(result.session_key);
      apply(model.resolveSend(result.session_key, result.run_id, Date.now()) as Patch);
    } catch (failure) {
      apply(model.resolveSend(key ?? "", null, Date.now()) as Patch);
      if (failure instanceof RpcError && failure.code === HOST_UNAVAILABLE) {
        notice.set("Connection lost while sending: the message may or may not have reached Aleph. Check the transcript before sending again.");
        void load();
      } else if (failure instanceof RpcError) {
        draft.set(text);
        notice.set(failure.message);
      } else {
        report(failure);
      }
    } finally {
      sending.set(false);
    }
  };

  const stop = async () => {
    const runId = activeRun();
    if (!runId) return;
    try {
      await sdk.invoke("chat.abort", { run_id: runId, session_key: sessionKey() });
    } catch (failure) {
      report(failure);
    }
  };

  return (
    <View class="flex-1 gap-2 p-4">
      <If when={!fullscreen()}>
        <View class="flex-row items-center justify-between">
          <Text class="text-muted-foreground">{sessionKey() ?? "New conversation"}</Text>
          <If when={sessionKey() && !showContext()}>
            <Button class="rounded px-2 py-1 hover:bg-muted focus-visible:bg-muted" onPress={() => showContext.set(true)}>
              <Text>Context</Text>
            </Button>
          </If>
        </View>
      </If>
      <If when={showContext() && sessionKey()}>
        <ContextSheet session={sessionKey} sdk={sdk} project={(raw) => model.contextView(raw)} close={() => showContext.set(false)} />
      </If>
      <If when={!loading()} else={<Text class="text-muted-foreground">Loading conversation…</Text>}>
        <TranscriptList keys={keys} row={(key) => rows.get(key)!} sdk={sdk} session={sessionKey}
          fullscreen={fullscreen} setFullscreen={setFullscreen}
          diagram={(source) => model.mermaidView(source) as Diagram}
          diff={(key, expanded) => model.diffView(key, expanded) as Change[]} />
      </If>
      <If when={!fullscreen() && activeRun()}><Text class="text-muted-foreground">Aleph · replying</Text></If>
      <If when={missingPrefix()}><Text class="text-muted-foreground">Earlier live updates may be missing.</Text></If>
      <If when={limited()}><Text class="text-muted-foreground">Some earlier tool details are unavailable.</Text></If>
      <If when={notice()}><Text class="text-destructive">{notice()}</Text></If>
      <View class={fullscreen() ? "hidden" : "flex-row items-end gap-2"}>
        <Input
          value={draft()}
          onChange={({ value }) => draft.set(value)}
          onCompositionChange={({ composing: active }) => { composing = active; }}
          onSubmit={() => void send()}
          multiline
          submitBehavior="submit"
          placeholder="Message Aleph"
          accessibilityLabel="Message"
          autoFocus
          class="flex-1 rounded-(--radius) border border-input px-3 py-2 text-foreground"
        />
        <If when={activeRun()} else={
          <Button disabled={sending() || !draft().trim()} onPress={() => void send()}
            class="rounded-(--radius) bg-primary px-4 py-2 disabled:opacity-40">
            <Text class="text-primary-foreground">Send</Text>
          </Button>
        }>
          <Button onPress={() => void stop()} class="rounded-(--radius) border border-input px-4 py-2">
            <Text class="text-foreground">Stop</Text>
          </Button>
        </If>
      </View>
    </View>
  );
}
