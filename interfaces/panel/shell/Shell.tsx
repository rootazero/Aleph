// The Panel shell: a fixed two-column frame (menu on the left, the mounted
// module or Settings on the right), connection, the capability snapshot, the
// user's layout and personal preferences. Layout, projection and preference
// rules live in Rust (`aleph/panel`, shared with the Leptos Panel); this file
// only wires them.
import { batch, cleanup, colorScheme, computed, effect, horizontalSizeClass, host, platform, signal, Storage, Text, View, type ViewRef } from "nsh";
import { Button } from "nsh/base";
import { PanelModel } from "aleph/panel";
import { Capabilities } from "../modules/Capabilities";
import { Chat } from "../modules/Chat";
import { MemoryNebula } from "../modules/MemoryNebula";
import { Overview } from "../modules/Overview";
import { Sessions } from "../modules/Sessions";
import { CommandPalette } from "./CommandPalette";
import { bindCommands, type Command, type KeyedCommand, menuCommands, resolveKeys, shellCommands } from "./commands";
import { PairingForm } from "./PairingForm";
import { applyPrefs, type Binding, loadStored, persistPrefs, type Prefs, readPrefs } from "./prefs";
import { type CommandSpec, type ConnectionState, createSdk, type Intent, type PanelEvent, type PanelSdk, RpcError, type ShellAccess } from "./sdk";
import { Settings } from "./Settings";
import { Sidebar } from "./Sidebar";
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
  const settingsOpen = signal(false);
  const prefs = signal<Prefs | null>(null);
  const fontScaleSupported = signal(true);
  const prefsSaveFailed = signal(false);
  const paletteOpen = signal(false);
  const readerMode = signal(false);
  // Commands the mounted module registered (already prefixed with its key).
  const moduleCommands = signal<CommandSpec[]>([]);
  // Every module command seen this session, kept after its module unmounts
  // so Settings (which unmounts the module) can still list and rebind it.
  // Only names and default keys are kept, never the callbacks.
  const seenModuleCommands = signal<Map<string, KeyedCommand>>(new Map());
  // Keys the platform refused to bind, by command id.
  const keyFailures = signal<Map<string, string>>(new Map());
  const listeners = new Set<(state: ConnectionState) => void>();
  const eventListeners = new Set<(event: PanelEvent) => void>();
  // Keep only the last confirmed identity, never a cache of previous identities.
  const intents = new Map<string, Intent>();
  let identity: { endpoint: string; subject: string } | null = null;
  const sendUnknown = signal(false);
  let pendingSend: ((unknown: boolean) => void) | null = null;
  const clearContinuity = () => {
    intents.clear();
    sendUnknown.set(false);
    identity = null;
  };
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
      if (changed) {
        pendingSend?.(true);
        if (!identity || (next.endpoint && next.endpoint !== identity.endpoint)) clearContinuity();
        if (next.state === "connected") {
          if (next.subject !== identity?.subject || next.endpoint !== identity?.endpoint) clearContinuity();
          identity = next.subject && next.endpoint ? { endpoint: next.endpoint, subject: next.subject } : null;
        }
        generation.update((value) => value + 1);
        snapshot = null;
        storageKey = null;
        view.set(null);
        notice.set(null);
        settingsOpen.set(false);
      }
      connection.set(next);
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
      const current = view();
      if (!current || !storageKey || connection().state !== "connected") return false;
      const stored = PanelModel.select(current.stored, key);
      if (!stored) return false;
      const next = PanelModel.view(snapshot, stored) as PanelView;
      if (next.layout.active !== key) return false;
      if (intent && !next.slots.some((slot) => slot.key === key && slot.state === "live")) return false;
      if (intent) intents.set(key, intent);
      view.set(next);
      void Storage.setItem(storageKey, stored);
      return true;
    },
    intent: (key) => connection().state === "connected" ? intents.get(key) ?? null : null,
    rememberChat(sessionKey) {
      if (connection().state !== "connected") return false;
      if (sessionKey === null) intents.delete("aleph:chat");
      else intents.set("aleph:chat", { session_key: sessionKey });
      return true;
    },
    beginChatSend() {
      if (connection().state !== "connected" || pendingSend) return null;
      const finish = (unknown: boolean) => {
        if (pendingSend !== finish) return;
        pendingSend = null;
        if (unknown) sendUnknown.set(true);
      };
      pendingSend = finish;
      return finish;
    },
    chatSendUnknown: () => sendUnknown(),
    dismissChatSendUnknown: () => sendUnknown.set(false),
    onConnection(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    onEvent(listener) {
      eventListeners.add(listener);
      return () => eventListeners.delete(listener);
    },
    registerCommand(spec) {
      moduleCommands.update((list) => [...list.filter((command) => command.id !== spec.id), spec]);
      seenModuleCommands.update((seen) =>
        new Map(seen).set(spec.id, { id: spec.id, label: spec.label, defaultKey: spec.defaultKey }),
      );
      return () => moduleCommands.update((list) => list.filter((command) => command !== spec));
    },
  };

  const applyEvent = (event: PanelEvent) => {
    for (const listener of [...eventListeners]) listener(event);
  };

  const showPrefs = async (next: Prefs) => {
    prefs.set(next);
    const applied = await applyPrefs(next);
    fontScaleSupported.set(applied.fontScale === "applied");
  };

  // An edit shows at once and is then stored; the next edit starts from it,
  // not from what storage holds yet.
  const keepPrefs = (stored: string) => {
    void showPrefs(readPrefs(stored));
    void persistPrefs(stored).then((saved) => prefsSaveFailed.set(!saved));
  };

  const changeAppearance = (axis: string, value: string) => {
    const current = prefs();
    if (!current) return;
    const stored = PanelModel.setAppearance(current.stored, axis, value);
    if (stored) keepPrefs(stored);
  };

  // A stored form that needed repair is written back, but the Panel shows
  // either way: storage that refuses the write must not leave it blank.
  void loadStored().then((stored) => {
    const loaded = readPrefs(stored);
    void showPrefs(loaded);
    if (stored !== null && loaded.stored !== stored) keepPrefs(loaded.stored);
  });

  const openModule = (key: string) => {
    settingsOpen.set(false);
    access.navigate(key);
  };

  const changeBinding = (id: string, binding: Binding | null) => {
    const current = prefs();
    if (!current) return;
    const stored = PanelModel.setBinding(current.stored, id, binding);
    if (stored) keepPrefs(stored);
  };

  const commands = computed<Command[]>(() => [
    ...shellCommands(
      {
        palette: () => paletteOpen.update((open) => !open),
        settings: () => settingsOpen.update((open) => !open),
        toggleScheme: () => void changeAppearance("mode", colorScheme() === "dark" ? "light" : "dark"),
        reconnect: () => {
          if (host.available()) void host.call("aleph.connect", null);
        },
        openSlot: (index) => {
          const live = (view()?.slots ?? []).filter((slot) => slot.state === "live");
          const slot = live[index];
          if (slot) openModule(slot.key);
        },
      },
      platform.os === "web",
    ),
    ...menuCommands(view(), openModule),
    ...moduleCommands().map((command) => ({ ...command, group: "module" as const })),
  ]);
  // Keys resolve over the live commands and the module commands seen
  // earlier, once, so Settings and the bound keys agree on every conflict.
  const keyedCommands = computed<KeyedCommand[]>(() => {
    const live = commands();
    const liveIds = new Set(live.map((command) => command.id));
    const away = [...seenModuleCommands().values()].filter((command) => !liveIds.has(command.id));
    return [...live, ...away];
  });
  const keys = computed(() => {
    const current = prefs();
    return current ? resolveKeys(keyedCommands(), current.stored) : [];
  });
  const labelOf = (id: string) => keyedCommands().find((command) => command.id === id)?.label ?? id;
  const keyOf = (id: string) => {
    const entry = keys().find((candidate) => candidate.id === id);
    return entry?.key && !entry.problem && !keyFailures().has(id) ? entry.key : null;
  };
  // Rebind whenever the commands or the user's keys change.
  effect(() => {
    const bound = bindCommands(commands(), keys());
    keyFailures.set(bound.failures);
    return () => bound.unbind();
  });

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

  // The Panel's shortcuts only hear keys while the keyboard focus is in the
  // Panel. On Desktop it is embedded in the shell's window, whose own root
  // has the focus at launch, so the Panel takes it once it is laid out in
  // that window (its first render comes earlier, when a `focus()` does
  // nothing). A focused field that goes away hands it back to the Panel's
  // root.
  let root: ViewRef | undefined;
  let focusTaken = false;
  const takeFocus = (bounds: { size: { width: number; height: number } }) => {
    if (focusTaken || !root || bounds.size.width === 0 || bounds.size.height === 0) return;
    focusTaken = true;
    root.focus();
  };

  // Nothing until preferences are read, so the first frame has the user's
  // colours and density.
  return (
    <If when={prefs()} else={<View class="flex-1 bg-background" />}>
      {(current) => (
        <View
          ref={(node) => (root = node)}
          focusable
          onLayout={({ bounds }) => takeFocus(bounds)}
          class="flex-1 flex-row bg-background"
        >
          <If when={!readerMode()}>
            <Sidebar
              view={view}
              connection={connection}
              settingsOpen={settingsOpen}
              compact={() => horizontalSizeClass() === "compact"}
              density={() => current().density}
              onOpen={openModule}
              onSettings={() => settingsOpen.update((open) => !open)}
            />
          </If>
          <View class="flex-1">
            <If when={!readerMode() && notice()}>
              <Text class="px-3 py-2 text-destructive">{notice()}</Text>
            </If>
            <If
              when={settingsOpen()}
              else={
                <If when={connection().state === "connected"} else={<Offline connection={connection} />}>
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
              }
            >
              <Settings
                prefs={current}
                fontScaleSupported={fontScaleSupported}
                saveFailed={prefsSaveFailed}
                onAppearance={changeAppearance}
                view={view}
                onMove={(key, forward) => edit((stored) => PanelModel.moveTab(stored, key, forward))}
                onHidden={(key, hidden) => edit((stored) => PanelModel.setHidden(stored, key, hidden))}
                shortcuts={() =>
                  keys().map((resolved) => ({
                    id: resolved.id,
                    label: labelOf(resolved.id),
                    resolved,
                    failure: keyFailures().get(resolved.id) ?? null,
                  }))
                }
                labelOf={labelOf}
                onBinding={changeBinding}
              />
            </If>
          </View>
          <If when={paletteOpen()}>
            <CommandPalette commands={commands} keyOf={keyOf} onClose={() => paletteOpen.set(false)} />
          </If>
        </View>
      )}
    </If>
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
