// Core owns committed history. Rust folds authorized live presentation into
// the same list, while TSX owns RPC lifetimes and composer interaction.
import { batch, cleanup, signal, type Signal, Text, View } from "nsh";
import { Button, Input } from "nsh/base";
import { ChatModel } from "aleph/panel";
import { HOST_UNAVAILABLE, RESOURCE_NOT_FOUND, type PanelEvent, type PanelSdk, RpcError } from "../shell/sdk";
import { TranscriptList } from "./chat/TranscriptList";
import { ContextSheet } from "./chat/ContextSheet";
import type { Change } from "./chat/Diff";
import type { Diagram } from "./chat/MessageBody";
import { newIdempotencyKey } from "./chatIdempotency";

const HISTORY_LIMIT = 200;
const COMMIT_REREAD_MS = 350;
const DETAIL_RUN_LIMIT = 12;

type History = {
  session_key: string;
  messages?: { run_id?: string }[];
  active_run?: string | null;
};

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
type RunState = { activeRun: string | null; missingPrefix: boolean; unconfirmed: boolean; droppedUnconfirmed: boolean; canResume: boolean };
type Patch = RunState & {
  upsert: ChatRow[];
  remove: string[];
  appendOrder: string[];
  insertAfter?: string;
  insertBefore?: string;
  reconcile: boolean;
  terminalRun?: string;
  error: string | null;
};
export type Scrollback = {
  scrollbackSupported: boolean;
  nextBefore: string | null;
  scrollbackReset: boolean;
  needsLatest: boolean;
  continuationUnavailable: boolean;
};
const NO_SCROLLBACK: Scrollback = {
  scrollbackSupported: false, nextBefore: null, scrollbackReset: false,
  needsLatest: false, continuationUnavailable: false,
};
type Snapshot = RunState & Scrollback & { rows: ChatRow[]; limited: boolean; unconfirmedRuns: string[] };

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
  const historyReady = signal(sessionKey() === null);
  const waitingForAdmission = signal(sessionKey() !== null);
  const historyFailed = signal(false);
  const activeRun = signal<string | null>(null);
  const missingPrefix = signal(false);
  const unconfirmed = signal(false);
  const droppedUnconfirmed = signal(false);
  const canResume = signal(false);
  const limited = signal(false);
  const notice = signal<string | null>(null);
  const draft = signal("");
  const sending = signal(false);
  let composing = false;
  const scrolling = signal(NO_SCROLLBACK);
  const loadingOlder = signal(false);
  const olderFailed = signal(false);
  let historyRequest = 0;
  let historyEpoch = 0;
  let olderRequest = 0;
  let detailRequest = 0;
  let liveRevision = 0;
  const detailOwners = new Map<string, number>();
  const resetWindowRequests = () => {
    ++historyEpoch;
    ++olderRequest;
    detailOwners.clear();
    loadingOlder.set(false);
    olderFailed.set(false);
  };

  const upsert = (row: ChatRow) => {
    const current = rows.get(row.key);
    if (current) current.set(row);
    else rows.set(row.key, signal(row));
  };
  const runState = (state: RunState) => {
    activeRun.set(state.activeRun);
    missingPrefix.set(state.missingPrefix);
    unconfirmed.set(state.unconfirmed);
    droppedUnconfirmed.set(state.droppedUnconfirmed);
    canResume.set(state.canResume);
  };
  const replace = (snapshot: Snapshot) => {
    const { rows: committed, limited: detailLimited, unconfirmedRuns: _, ...state } = snapshot;
    const next = committed.map((row) => {
      upsert(row);
      return row.key;
    });
    const retained = new Set(next);
    for (const key of rows.keys()) if (!retained.has(key)) rows.delete(key);
    batch(() => {
      scrolling.set(state);
      keys.set(next);
      runState(state);
      limited.set(detailLimited);
    });
  };
  const report = (failure: unknown) => {
    const error = failure as Error;
    if (error.message !== "disposed") notice.set(error.message);
  };

  const loadDetails = async (key: string, messages: NonNullable<History["messages"]>, fromTail: boolean) => {
    const runIds = [...new Set([...messages].reverse().map((message) => message.run_id)
      .filter((id): id is string => typeof id === "string" && id.length > 0))].reverse();
    const eligible = runIds.filter((id) => fromTail || !detailOwners.has(id));
    const requested = eligible.slice(-DETAIL_RUN_LIMIT);
    const skipped = eligible.slice(0, -DETAIL_RUN_LIMIT).filter((id) => !detailOwners.has(id));
    if (skipped.length > 0) replace(model.applyHistoryDetails(skipped, null) as Snapshot);
    if (requested.length === 0) return;
    const epoch = historyEpoch;
    const owner = ++detailRequest;
    for (const run of requested) detailOwners.set(run, owner);
    let details: unknown = null;
    try {
      details = await sdk.invoke("chat.transcript_details", { session_key: key, run_ids: requested });
    } catch {
      // Scoped replay can fail or be unavailable on an older Core. Prose is already visible.
    }
    if (!sdk.active() || key !== sessionKey() || epoch !== historyEpoch) return;
    const current = requested.filter((id) => detailOwners.get(id) === owner);
    if (current.length > 0) replace(model.applyHistoryDetails(current, details) as Snapshot);
  };

  const load = async (terminalRun?: string) => {
    const key = sessionKey();
    if (!key) return;
    const request = ++historyRequest;
    const revision = liveRevision;
    const current = () => sdk.active() && request === historyRequest && key === sessionKey();
    try {
      const history = (await sdk.invoke("chat.history", { session_key: key, limit: HISTORY_LIMIT, scrollback: {} })) as History;
      if (!current()) return;
      if (history?.session_key !== key || !Array.isArray(history.messages)) throw new Error("Unable to read conversation history.");
      const snapshot = model.replaceHistory({ ...history, live_status_stale: liveRevision !== revision }, null) as Snapshot | null;
      if (!snapshot) throw new Error("Unable to read conversation history.");
      if (snapshot.scrollbackReset) resetWindowRequests();
      batch(() => {
        historyReady.set(true);
        waitingForAdmission.set(false);
        historyFailed.set(false);
        replace(snapshot);
      });
      void loadDetails(key, history.messages, true);
      // Completion can reach the feed before Core stamps the projected history row.
      if (terminalRun && snapshot.unconfirmedRuns.includes(terminalRun)) {
        setTimeout(() => { if (current()) void load(); }, COMMIT_REREAD_MS);
      }
    } catch (failure) {
      if (current()) {
        if (failure instanceof RpcError && failure.code === RESOURCE_NOT_FOUND) waitingForAdmission.set(true);
        historyFailed.set(true);
        report(failure);
      }
    } finally {
      if (current()) loading.set(false);
    }
  };

  const loadOlder = async () => {
    const key = sessionKey();
    const before = scrolling().nextBefore;
    if (!key || !before || loadingOlder() || waitingForAdmission()) return;
    const request = ++olderRequest;
    const current = () => sdk.active() && request === olderRequest && key === sessionKey();
    loadingOlder.set(true);
    olderFailed.set(false);
    try {
      const history = (await sdk.invoke("chat.history", {
        session_key: key, limit: HISTORY_LIMIT, scrollback: { before },
      })) as History;
      if (!current() || before !== scrolling().nextBefore) return;
      if (history?.session_key !== key || !Array.isArray(history.messages)) throw new Error("Invalid history page");
      const snapshot = model.prependHistory(history, before) as Snapshot | null;
      if (!snapshot) throw new Error("Unmatched history page");
      replace(snapshot);
      if (!snapshot.needsLatest) void loadDetails(key, history.messages, false);
    } catch (failure) {
      if (!current() || before !== scrolling().nextBefore) return;
      olderFailed.set(true);
      if (failure instanceof RpcError && failure.code === RESOURCE_NOT_FOUND) {
        waitingForAdmission.set(true);
        historyFailed.set(true);
        report(failure);
      }
    } finally {
      if (current()) loadingOlder.set(false);
    }
  };

  const apply = (patch: Patch) => {
    for (const row of patch.upsert) upsert(row);
    for (const key of patch.remove) rows.delete(key);
    if (patch.remove.length || patch.appendOrder.length) {
      const removed = new Set(patch.remove);
      keys.update((prior) => {
        const next = prior.filter((key) => !removed.has(key));
        const before = patch.insertBefore === undefined ? -1 : next.indexOf(patch.insertBefore);
        const after = patch.insertAfter === undefined ? -1 : next.indexOf(patch.insertAfter);
        const insertAt = before >= 0 ? before : after >= 0 ? after + 1 : next.length;
        next.splice(insertAt, 0, ...patch.appendOrder);
        return next;
      });
    }
    runState(patch);
    if (patch.error) notice.set(`The reply failed: ${patch.error}`);
    if (patch.reconcile) void load(patch.terminalRun);
  };

  const onStream = (event: PanelEvent) => {
    const data = event.data as { run_id?: string } | undefined;
    const run = activeRun();
    if (run && data?.run_id === run) ++liveRevision;
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

  const startNewConversation = () => {
    if (sending() || !sdk.rememberChat(null)) return;
    ++historyRequest;
    model.reset("");
    batch(() => {
      resetWindowRequests();
      scrolling.set(NO_SCROLLBACK);
      sessionKey.set(null);
      rows.clear();
      keys.set([]);
      loading.set(false);
      historyReady.set(true);
      waitingForAdmission.set(false);
      historyFailed.set(false);
      runState({ activeRun: null, missingPrefix: false, unconfirmed: false, droppedUnconfirmed: false, canResume: false });
      limited.set(false);
      notice.set(null);
      draft.set("");
      showContext.set(false);
      setFullscreen(null);
    });
  };

  const send = async () => {
    const text = draft().trim();
    if (!text || composing || sending() || activeRun() || waitingForAdmission()) return;
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
      const result = await sdk.sendChat(params);
      if (!sdk.rememberChat(result.session_key)) return;
      ++historyRequest;
      sessionKey.set(result.session_key);
      apply(model.resolveSend(result.session_key, result.run_id, Date.now()) as Patch);
    } catch (failure) {
      if (!sdk.active()) return;
      apply(model.resolveSend(key ?? "", null, Date.now()) as Patch);
      if (failure instanceof RpcError && failure.code !== HOST_UNAVAILABLE) {
        if (failure.code === RESOURCE_NOT_FOUND && key) {
          waitingForAdmission.set(true);
          historyFailed.set(true);
        }
        draft.set(text);
        notice.set(failure.message);
      } else if (sdk.chatSendUnknown()) {
        void load();
      } else {
        report(failure);
      }
    } finally {
      if (sdk.active()) sending.set(false);
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
      <If when={!fullscreen()}>
        <View class="flex-row items-center justify-between">
          <Text class="text-muted-foreground">{sessionKey() ?? "New conversation"}</Text>
          <If when={sessionKey()}>
            <View class="flex-row gap-2">
              <Button accessibilityLabel="Start new conversation" disabled={sending()} onPress={startNewConversation}
                class="rounded px-2 py-1 hover:bg-muted focus-visible:bg-muted">
                <Text>New conversation</Text>
              </Button>
              <If when={!waitingForAdmission() && !showContext()}>
                <Button class="rounded px-2 py-1 hover:bg-muted focus-visible:bg-muted" onPress={() => showContext.set(true)}>
                  <Text>Context</Text>
                </Button>
              </If>
            </View>
          </If>
        </View>
      </If>
      <If when={showContext() && sessionKey()}>
        <ContextSheet session={sessionKey} sdk={sdk} project={(raw) => model.contextView(raw)} close={() => showContext.set(false)} />
      </If>
      <If when={loading()}><Text class="text-muted-foreground">Loading conversation…</Text></If>
      <If when={!loading() && historyReady()}>
        <TranscriptList keys={keys} row={(key) => rows.get(key)!} sdk={sdk} session={sessionKey}
          fullscreen={fullscreen} setFullscreen={setFullscreen}
          scrollback={scrolling} loadingOlder={loadingOlder} olderFailed={olderFailed}
          loadOlder={() => void loadOlder()} reloadLatest={() => void load()}
          canLoadOlder={() => !waitingForAdmission()}
          diagram={(source) => model.mermaidView(source) as Diagram}
          diff={(key, expanded) => model.diffView(key, expanded) as Change[]} />
      </If>
      <If when={!fullscreen() && activeRun()}>
        <Text class="text-muted-foreground">{canResume() ? "Aleph reports no active run; the previous outcome is unknown." : "Aleph · replying"}</Text>
        <If when={canResume()} else={
          <If when={missingPrefix()}>
            <Button onPress={() => void load()} class="self-start rounded px-2 py-1 hover:bg-muted focus-visible:bg-muted">
              <Text>Check live status</Text>
            </Button>
          </If>
        }>
          <Button onPress={() => apply(model.resumeComposing() as Patch)}
            class="self-start rounded px-2 py-1 hover:bg-muted focus-visible:bg-muted">
            <Text>Resume composing</Text>
          </Button>
        </If>
      </If>
      <If when={missingPrefix()}><Text class="text-muted-foreground">Earlier live updates may be missing.</Text></If>
      <If when={unconfirmed()}>
        <View class="flex-row items-center gap-2">
          <Text class="text-muted-foreground">Some turns are not yet confirmed in history.</Text>
          <Button onPress={() => void load()} class="rounded px-2 py-1 hover:bg-muted focus-visible:bg-muted">
            <Text>Refresh history</Text>
          </Button>
        </View>
      </If>
      <If when={droppedUnconfirmed()}><Text class="text-muted-foreground">The oldest unconfirmed turn exceeded the preview limit.</Text></If>
      <If when={limited()}><Text class="text-muted-foreground">Some earlier tool details are unavailable.</Text></If>
      <If when={notice()}><Text class="text-destructive">{notice()}</Text></If>
      <If when={!fullscreen() && historyFailed()}>
        <Button onPress={() => { notice.set(null); void load(); }} class="self-start rounded px-2 py-1 hover:bg-muted focus-visible:bg-muted">
          <Text>Retry history</Text>
        </Button>
      </If>
      <If when={!fullscreen() && sdk.chatSendUnknown()}>
        <View class="flex-row items-center gap-2">
          <Text class="text-muted-foreground">A send may have reached Aleph. Check Sessions before sending again.</Text>
          <Button accessibilityLabel="Dismiss send notice" onPress={sdk.dismissChatSendUnknown} class="rounded px-2 py-1 hover:bg-muted focus-visible:bg-muted">
            <Text>Dismiss</Text>
          </Button>
        </View>
      </If>
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
          <Button disabled={sending() || waitingForAdmission() || !draft().trim()} onPress={() => void send()}
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
