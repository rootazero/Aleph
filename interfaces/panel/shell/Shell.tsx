// The Panel shell: connection, the capability snapshot, the user's layout and
// the one mounted module. Layout and projection rules live in Rust
// (`aleph/panel`, shared with the Leptos Panel); this file only wires them.
import { batch, cleanup, computed, host, signal, Storage, Text, View } from "nsh";
import { Button } from "nsh/base";
import { PanelModel } from "aleph/panel";
import { Capabilities } from "../modules/Capabilities";
import { Chat } from "../modules/Chat";
import { MemoryNebula } from "../modules/MemoryNebula";
import { Overview } from "../modules/Overview";
import { Sessions } from "../modules/Sessions";
import { LayoutEditor } from "./LayoutEditor";
import { PairingForm } from "./PairingForm";
import { type ConnectionState, createSdk, type Intent, type PanelEvent, type PanelSdk, RpcError, type ShellAccess } from "./sdk";
import type { PanelTab, PanelView, Slot } from "./view";

const INITIAL: ConnectionState = { state: "connecting", endpoint: "", subject: null, pairing: false };

type RpcAnswer = { result?: unknown; error?: { code: number; message: string } };

async function rpc(method: string, params: Record<string, unknown>): Promise<unknown> {
  const answer = (await host.call("aleph.rpc", { method, params })) as RpcAnswer;
  if (answer.error) throw new RpcError(answer.error.code, answer.error.message);
  return answer.result ?? null;
}

export function Shell() {
  const connection = signal<ConnectionState>(INITIAL);
  const generation = signal(0);
  const view = signal<PanelView | null>(null);
  const notice = signal<string | null>(null);
  const editing = signal(false);
  const listeners = new Set<(state: ConnectionState) => void>();
  const eventListeners = new Set<(event: PanelEvent) => void>();
  // What a module handed the one it navigated to (by tab key), for this run
  // of the shell only: a reload starts every module without one.
  const intents = new Map<string, Intent>();
  // The snapshot and storage key the current view was made from.
  let snapshot: unknown = null;
  let storageKey: string | null = null;

  const load = async (loading: number) => {
    let next: unknown = null;
    let failure: string | null = null;
    try {
      next = await rpc("panel.capabilities.snapshot", {});
    } catch (error) {
      failure = `Capabilities unavailable: ${(error as Error).message}`;
    }
    if (loading !== generation()) return;
    const state = connection();
    const key = PanelModel.layoutKey(state.endpoint, state.subject);
    const stored = await Storage.getItem(key);
    if (loading !== generation()) return;
    const made = PanelModel.view(next, stored) as PanelView;
    snapshot = next;
    storageKey = key;
    batch(() => {
      view.set(made);
      notice.set(failure ?? (made.projectionError ? `Capabilities rejected: ${made.projectionError}` : null));
    });
    if (made.stored !== stored) await Storage.setItem(key, made.stored);
  };

  const applyConnection = (next: ConnectionState) => {
    const previous = connection();
    const changed = next.state !== previous.state || next.endpoint !== previous.endpoint || next.subject !== previous.subject;
    batch(() => {
      connection.set(next);
      if (changed) {
        generation.update((value) => value + 1);
        snapshot = null;
        storageKey = null;
        view.set(null);
        notice.set(null);
        editing.set(false);
      }
    });
    for (const listener of [...listeners]) listener(next);
    if (changed && next.state === "connected") void load(generation());
  };

  const edit = (change: (stored: string) => string | null | undefined) => {
    const current = view();
    if (!current || !storageKey) return;
    const stored = change(current.stored);
    if (!stored) return;
    view.set(PanelModel.view(snapshot, stored) as PanelView);
    void Storage.setItem(storageKey, stored);
  };

  const access: ShellAccess = {
    rpc,
    generation: () => generation(),
    connection: () => connection(),
    navigate(key, intent) {
      if (intent) intents.set(key, intent);
      edit((stored) => PanelModel.select(stored, key));
    },
    intent: (key) => intents.get(key) ?? null,
    onConnection(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    onEvent(listener) {
      eventListeners.add(listener);
      return () => eventListeners.delete(listener);
    },
  };

  const applyEvent = (event: PanelEvent) => {
    for (const listener of [...eventListeners]) listener(event);
  };

  const applyInitialConnection = (state: ConnectionState) => {
    // A real transition may arrive before the initial query returns.
    if (generation() === 0) applyConnection(state);
  };
  if (host.available()) {
    cleanup(host.on<ConnectionState>("aleph.connection", applyConnection));
    cleanup(host.on<PanelEvent>("aleph.event", applyEvent));
    void host.call("aleph.connection", null).then(
      (state) => applyInitialConnection(state as ConnectionState),
      () => applyInitialConnection({ ...INITIAL, state: "unavailable" }),
    );
  } else {
    connection.set({ ...INITIAL, state: "unavailable" });
  }

  const tabOf = (key: string): PanelTab | undefined => view()?.tabs.find((tab) => tab.key === key);
  // The active slot, keyed by tab and generation: a switch or a new socket
  // unmounts the old module (its SDK disposes) before the next one mounts.
  const mounted = computed(() => {
    const current = view();
    const active = current?.layout.active;
    const slot = active ? current?.slots.find((candidate) => candidate.key === active) : undefined;
    return slot ? [{ slot, generation: generation() }] : [];
  });

  const readerMode = signal(false);
  return (
    <View class="flex-1 bg-background">
      <If when={connection().state === "connected"} else={<Offline connection={connection} />}>
        <If when={!readerMode()}>
        <View class="flex-row flex-wrap items-center gap-1 border-b border-border px-3 py-2">
          <Each of={view()?.slots ?? []} key={(slot) => `${slot.key}#${slot.state}`}>
            {(slot) => (
              <If
                when={slot().state === "live"}
                else={<Text class="px-3 py-1 text-muted-foreground">{slot().key} · unavailable</Text>}
              >
                <Button
                  onPress={() => access.navigate(slot().key)}
                  accessibilityLabel={`Open ${tabOf(slot().key)?.label ?? slot().key}`}
                  class="rounded-(--radius) px-3 py-1 active:bg-muted"
                >
                  <Text class={slot().key === view()?.layout.active ? "text-foreground" : "text-muted-foreground"}>
                    {tabOf(slot().key)?.label ?? slot().key}
                  </Text>
                </Button>
              </If>
            )}
          </Each>
          <Button onPress={() => editing.update((open) => !open)} class="px-3 py-1 active:bg-muted">
            <Text class="text-muted-foreground">{editing() ? "Done" : "Customize"}</Text>
          </Button>
        </View>
        <If when={notice()}>
          <Text class="px-3 py-2 text-destructive">{notice()}</Text>
        </If>
        <If when={editing() ? view() : null}>
          {(current) => (
            <LayoutEditor
              view={current()}
              onMove={(key, forward) => edit((stored) => PanelModel.moveTab(stored, key, forward))}
              onHidden={(key, hidden) => edit((stored) => PanelModel.setHidden(stored, key, hidden))}
            />
          )}
        </If>
        </If>
        <View class="flex-1">
          <Each of={mounted()} key={(item) => `${item.slot.key}#${item.slot.renderer}#${item.generation}`}>
            {(item) => {
              const sdk = createSdk(item().slot.key, item().generation, access);
              cleanup(() => sdk.dispose());
              return <ModuleSlot slot={item().slot} tab={tabOf(item().slot.key)} tabs={view()?.tabs ?? []} sdk={sdk}
                setReaderMode={(active) => readerMode.set(active)} />;
            }}
          </Each>
        </View>
      </If>
    </View>
  );
}

// Only live slots mount (`PanelModel` never makes an unavailable tab active);
// a slot's key and renderer are fixed for its row.
function ModuleSlot(props: {
  slot: Slot; tab: PanelTab | undefined; tabs: PanelTab[]; sdk: PanelSdk;
  setReaderMode: (active: boolean) => void;
}) {
  const { slot, tab, sdk } = props;
  if (slot.renderer === "dom") {
    return <Unavailable title={tab?.label ?? slot.key} detail="This tab needs the classic Panel; it does not run in this renderer yet." />;
  }
  switch (slot.key) {
    case "aleph:overview":
      return <Overview sdk={sdk} />;
    case "aleph:sessions":
      return <Sessions sdk={sdk} />;
    case "aleph:chat":
      return <Chat sdk={sdk} setReaderMode={props.setReaderMode} />;
    case "aleph:capabilities":
      return <Capabilities tabs={props.tabs} />;
    case "aleph:memory-nebula":
      return <MemoryNebula sdk={sdk} />;
    default:
      return <Unavailable title={slot.key} detail="This renderer has no module for this tab." />;
  }
}

function Unavailable(props: { title: string; detail: string }) {
  return (
    <View class="flex-1 items-center justify-center gap-2 p-6">
      <Text class="text-foreground">{props.title} is unavailable</Text>
      <Text class="text-muted-foreground">{props.detail}</Text>
    </View>
  );
}

const OFFLINE_LABELS: Record<ConnectionState["state"], string> = {
  connecting: "Connecting to Aleph…",
  connected: "Connected",
  disconnected: "Disconnected from Aleph",
  unavailable: "Aleph is not reachable from here",
};

function Offline(props: { connection: () => ConnectionState }) {
  const { connection } = props;
  return (
    <If when={!connection().pairing} else={<PairingForm />}>
      <View class="flex-1 items-center justify-center gap-3 p-6">
        <Text class="text-foreground">{OFFLINE_LABELS[connection().state]}</Text>
        <If when={connection().state === "disconnected" && host.available()}>
          <Button onPress={() => void host.call("aleph.connect", null)} class="rounded-(--radius) bg-primary px-4 py-2">
            <Text class="text-primary-foreground">Reconnect</Text>
          </Button>
        </If>
      </View>
    </If>
  );
}
