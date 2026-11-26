// One conversation: the transcript (`chat.history`), a composer (`chat.send`),
// the reply streaming in (`stream.*` live events) and Stop (`chat.abort`).
//
// The transcript is always Core's: live chunks only preview the reply being
// written, and every end of a run, every lost or interrupted live feed and
// every new connection (the shell remounts this module) refetches history.
// Nothing is replayed, and a send whose answer never came back is reported as
// unknown, never retried on its own.
import { List, signal, Text, View } from "nsh";
import { Button, Input } from "nsh/base";
import { type PanelEvent, type PanelSdk, RpcError } from "../shell/sdk";
import { newIdempotencyKey } from "./chatIdempotency";

// `aleph_protocol::panel::PANEL_HOST_UNAVAILABLE`: the host lost the socket,
// so whether a write reached Core is unknown.
const HOST_UNAVAILABLE = -32090;
const HISTORY_LIMIT = 200;

type Message = { role: string; content: string; timestamp?: string; run_id?: string };
type History = { session_key: string; messages?: Message[]; active_run?: string | null };
type SendResult = { run_id: string; session_key: string };
type Draft = { text: string; done: boolean; error: string | null };
type StreamData = { run_id?: string; delta?: string; content?: string; error?: string };

export function Chat(props: { sdk: PanelSdk }) {
  const { sdk } = props;
  const sessionKey = signal<string | null>((sdk.intent()?.session_key as string | undefined) ?? null);
  const messages = signal<Message[]>([]);
  const loading = signal(sessionKey() !== null);
  const activeRun = signal<string | null>(null);
  const reply = signal("");
  const notice = signal<string | null>(null);
  const draft = signal("");
  const sending = signal(false);
  let composing = false;
  // Stream frames for runs this module does not know yet: on Desktop IPC a
  // run's first chunks can arrive before `chat.send` answers with its id.
  // Collected only while a send is waiting.
  const early = new Map<string, Draft>();

  const report = (failure: unknown) => {
    const error = failure as Error;
    if (error.message !== "disposed") notice.set(error.message);
  };

  const load = async () => {
    const key = sessionKey();
    if (!key) return;
    try {
      const history = (await sdk.invoke("chat.history", { session_key: key, limit: HISTORY_LIMIT })) as History;
      messages.set(history.messages ?? []);
      // A run Core reports still going: its earlier chunks are gone, its next
      // ones and its end still arrive.
      if (history.active_run !== activeRun()) reply.set("");
      activeRun.set(history.active_run ?? null);
    } catch (failure) {
      report(failure);
    } finally {
      loading.set(false);
    }
  };

  const finish = (error: string | null) => {
    activeRun.set(null);
    reply.set("");
    if (error) notice.set(`The reply failed: ${error}`);
    void load();
  };

  const onStream = (event: PanelEvent) => {
    const data = (event.data ?? {}) as StreamData;
    const runId = data.run_id;
    if (!runId) return;
    const chunk = event.topic === "stream.response_chunk" ? (data.delta ?? data.content ?? "") : "";
    const ended = event.topic === "stream.run_complete" || event.topic === "stream.run_error";
    const error = event.topic === "stream.run_error" ? (data.error ?? "unknown error") : null;
    if (runId === activeRun()) {
      if (chunk) reply.update((text) => text + chunk);
      if (ended) finish(error);
      return;
    }
    if (!sending()) return;
    const held = early.get(runId) ?? { text: "", done: false, error: null };
    early.set(runId, { text: held.text + chunk, done: held.done || ended, error: held.error ?? error });
  };

  // The live feed lost frames (the Gateway's overflow warning, or the desktop
  // host's own count): what is on screen may be missing text, so reload it.
  const onInterrupted = () => {
    notice.set("Live updates were interrupted; reloaded from Aleph.");
    reply.set("");
    void load();
  };

  sdk.on("stream.", onStream);
  sdk.on("connection.", (event) => {
    if (event.topic === "connection.warning" || event.topic === "connection.stale") onInterrupted();
  });
  void load();

  const send = async () => {
    const text = draft().trim();
    if (!text || composing || sending() || activeRun()) return;
    const key = sessionKey();
    const params: Record<string, unknown> = {
      message: text,
      channel: "gui:chat",
      stream: true,
      // One key per attempt: a retry of this exact send is answered from
      // the Gateway's cache instead of starting a second run.
      idempotency_key: newIdempotencyKey(),
    };
    if (key) params.session_key = key;
    sending.set(true);
    notice.set(null);
    early.clear();
    messages.update((list) => [...list, { role: "user", content: text }]);
    draft.set("");
    try {
      const result = (await sdk.invoke("chat.send", params)) as SendResult;
      if (!key) sessionKey.set(result.session_key);
      const held = early.get(result.run_id);
      activeRun.set(result.run_id);
      reply.set(held?.text ?? "");
      if (held?.done) finish(held.error);
    } catch (failure) {
      if (failure instanceof RpcError && failure.code === HOST_UNAVAILABLE) {
        notice.set("Connection lost while sending: the message may or may not have reached Aleph. Check the transcript before sending again.");
        void load();
      } else if (failure instanceof RpcError) {
        // Core refused it: nothing was sent, so give the text back.
        messages.update((list) => list.slice(0, -1));
        draft.set(text);
        notice.set(failure.message);
      } else {
        report(failure);
      }
    } finally {
      early.clear();
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
  sdk.commands.register({ id: "stop", label: "Stop reply", keywords: ["abort", "cancel"], run: () => void stop() });

  return (
    <View class="flex-1 gap-2 p-4">
      <Text class="text-muted-foreground">{sessionKey() ?? "New conversation"}</Text>
      <If when={!loading()} else={<Text class="text-muted-foreground">Loading conversation…</Text>}>
        <List
          each={messages()}
          keyExtractor={(message, index) => `${index}:${message.timestamp ?? ""}`}
          empty={<Text class="text-muted-foreground">No messages yet</Text>}
          initialScrollAtEnd
          class="flex-1"
        >
          {(message) => (
            <View class="gap-1 py-2">
              <Text class="text-muted-foreground">{message().role === "user" ? "You" : "Aleph"}</Text>
              <Text class="text-foreground">{message().content}</Text>
            </View>
          )}
        </List>
      </If>
      <If when={activeRun()}>
        <View class="gap-1 py-2">
          <Text class="text-muted-foreground">Aleph · replying</Text>
          <Text class="text-foreground">{reply() || "…"}</Text>
        </View>
      </If>
      <If when={notice()}>
        <Text class="text-destructive">{notice()}</Text>
      </If>
      <View class="flex-row items-end gap-2">
        <Input
          value={draft()}
          onChange={({ value }) => draft.set(value)}
          onCompositionChange={({ composing: active }) => {
            composing = active;
          }}
          onSubmit={() => void send()}
          multiline
          submitBehavior="submit"
          placeholder="Message Aleph"
          accessibilityLabel="Message"
          autoFocus
          class="flex-1 rounded-(--radius) border border-input px-3 py-2 text-foreground"
        />
        <If
          when={activeRun()}
          else={
            <Button
              disabled={sending() || !draft().trim()}
              onPress={() => void send()}
              class="rounded-(--radius) bg-primary px-4 py-2 disabled:opacity-40"
            >
              <Text class="text-primary-foreground">Send</Text>
            </Button>
          }
        >
          <Button onPress={() => void stop()} class="rounded-(--radius) border border-input px-4 py-2">
            <Text class="text-foreground">Stop</Text>
          </Button>
        </If>
      </View>
    </View>
  );
}
