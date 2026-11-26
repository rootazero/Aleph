import { cleanup, effect, keymap, List, signal, Text, untracked, View, type ListRef, type Signal, type ViewRef } from "nsh";
import { Button } from "nsh/base";
import type { ChatRow, Scrollback, ToolRow } from "../Chat";
import type { PanelSdk } from "../../shell/sdk";
import type { Change } from "./Diff";
import { MessageBody, type Diagram } from "./MessageBody";
import { outputReader } from "./output";
import { ToolCard } from "./ToolCard";

const TAIL_DISTANCE = 72;
type Reader = ReturnType<typeof outputReader>;

function TranscriptItem(props: {
  row: () => ChatRow | undefined;
  reader: (key: string) => Reader | undefined;
  diagram: (source: string) => Diagram;
  diff: (key: string, expanded: boolean) => Change[];
  fullscreen: () => string | null;
  bindLauncher: (key: string, ref?: ViewRef) => void;
  onToggle: (key: string) => void;
  onFullscreen: (key: string) => void;
}) {
  const body = () => {
    const row = props.row();
    return row?.kind === "text" ? row : null;
  };
  const tool = () => props.row() as ToolRow;
  const launcherRef = signal<ViewRef | undefined>(undefined);
  effect(() => {
    const row = props.row();
    const ref = launcherRef();
    if (row?.kind !== "tool" || !ref) return;
    untracked(() => props.bindLauncher(row.key, ref));
    cleanup(() => props.bindLauncher(row.key));
  });
  return (
    <If when={props.row()?.kind === "tool"} else={
      <If when={body()?.role === "notice"} else={
        <View class="gap-1 py-2">
          <Text class="text-muted-foreground">{body()?.role === "user" ? "You" : "Aleph"}</Text>
          <If when={body()?.role === "assistant"} else={<Text class="text-foreground">{body()?.text}</Text>}>
            <MessageBody text={body()?.text ?? ""} streaming={body()?.streaming ?? false} layout={props.diagram} />
          </If>
        </View>
      }>
        <Text class="border-t border-border py-2 text-muted-foreground">{body()?.text}</Text>
      </If>
    }>
      <ToolCard row={tool} reader={() => props.reader(tool().key)}
        diff={(expanded) => props.diff(tool().key, expanded)}
        focusedReader={() => props.fullscreen() === tool().key}
        bindFullscreen={(ref) => launcherRef.set(ref)}
        onToggle={() => props.onToggle(tool().key)} onFullscreen={() => props.onFullscreen(tool().key)} />
    </If>
  );
}

export function TranscriptList(props: {
  keys: () => string[];
  row: (key: string) => Signal<ChatRow> | undefined;
  session: () => string | null;
  sdk: PanelSdk;
  fullscreen: () => string | null;
  setFullscreen: (key: string | null) => void;
  scrollback: () => Scrollback;
  loadingOlder: () => boolean;
  olderFailed: () => boolean;
  loadOlder: () => void;
  reloadLatest: () => void;
  canLoadOlder: () => boolean;
  diagram: (source: string) => Diagram;
  diff: (key: string, expanded: boolean) => Change[];
}) {
  const open = signal(new Map<string, Reader>());
  const following = signal(true);
  const launchers = new Map<string, ViewRef>();
  let list: ListRef | undefined;
  let restoreFocus: string | null = null;

  const close = (key: string) => {
    const next = new Map(open());
    next.get(key)?.reset();
    next.delete(key);
    launchers.delete(key);
    open.set(next);
  };
  const toggle = (key: string) => {
    if (open().has(key)) {
      if (props.fullscreen() === key) props.setFullscreen(null);
      close(key);
    } else {
      open.set(new Map(open()).set(key, outputReader(props.sdk, props.session, () => open().has(key))));
    }
  };
  const position = (key: string) => {
    const index = props.keys().indexOf(key);
    if (index >= 0) list?.scrollToIndex({ index, animated: false, viewPosition: 0 });
  };
  const leaveFullscreen = () => {
    const key = props.fullscreen();
    if (!key) return;
    props.setFullscreen(null);
    const launcher = launchers.get(key);
    if (launcher) requestAnimationFrame(() => { if (launchers.get(key) === launcher && !props.fullscreen()) launcher.focus(); });
    else if (open().has(key) && props.keys().includes(key)) {
      restoreFocus = key;
      position(key);
    }
  };
  const toggleFullscreen = (key: string) => {
    if (props.fullscreen() === key) {
      leaveFullscreen();
    } else {
      restoreFocus = null;
      props.setFullscreen(key);
    }
  };
  const bindLauncher = (key: string, ref?: ViewRef) => {
    if (ref) launchers.set(key, ref);
    else launchers.delete(key);
    if (ref && restoreFocus === key) {
      restoreFocus = null;
      requestAnimationFrame(() => { if (!props.fullscreen() && launchers.get(key) === ref) ref.focus(); });
    }
  };
  effect(() => {
    const keys = new Set(props.keys());
    untracked(() => {
      for (const key of open().keys()) if (!keys.has(key)) close(key);
      const fullscreen = props.fullscreen();
      if (fullscreen && !keys.has(fullscreen)) leaveFullscreen();
    });
  });
  keymap.bind("escape", leaveFullscreen, { when: "always", passThrough: true });
  cleanup(() => {
    for (const reader of open().values()) reader.reset();
  });

  return (
    <View class="flex-1 gap-2">
      <If when={props.fullscreen()}>
        <Button onPress={leaveFullscreen} class="self-start rounded border border-border px-2 py-1 hover:bg-muted focus-visible:bg-muted">
          <Text>Close reader</Text>
        </Button>
      </If>
      <If when={!props.fullscreen() && props.scrollback().scrollbackSupported}>
        <View class="h-10 flex-row items-center gap-2" testID="chat-scrollback">
          <If when={props.scrollback().needsLatest} else={
            <If when={props.scrollback().nextBefore} else={
              <Text class="text-muted-foreground">{props.scrollback().continuationUnavailable
                ? "More history exists, but Aleph cannot provide the next page."
                : "No earlier messages"}</Text>
            }>
              <Button onPress={props.loadOlder} disabled={props.loadingOlder() || !props.canLoadOlder()}
                class="rounded border border-border px-2 py-1 hover:bg-muted focus-visible:bg-muted disabled:opacity-40">
                <Text>{props.loadingOlder() ? "Loading older messages…" : props.olderFailed() ? "Retry older messages" : "Load older messages"}</Text>
              </Button>
              <If when={props.olderFailed()}><Text class="text-destructive">Older messages could not be loaded.</Text></If>
            </If>
          }>
            <Text class="text-muted-foreground">History changed. Reload the latest messages to continue.</Text>
            <Button onPress={props.reloadLatest} class="rounded border border-border px-2 py-1 hover:bg-muted focus-visible:bg-muted">
              <Text>Reload latest messages</Text>
            </Button>
          </If>
          <If when={props.scrollback().scrollbackReset && !props.scrollback().needsLatest}>
            <Text class="text-muted-foreground">History changed. Showing the latest messages.</Text>
          </If>
        </View>
      </If>
      {/* NSH only pins the oldest visible row at offset zero when start-loading
          is enabled. Keep network reads explicit: only the button fetches. */}
      <List testID="chat-transcript" ref={(ref) => { list = ref; }} each={props.keys()} keyExtractor={(key) => key}
        empty={<Text class="text-muted-foreground">No messages yet</Text>}
        onLoadMoreAtStart={() => undefined}
        initialScrollAtEnd maintainVisibleContentPosition={{ autoscrollToBottomThreshold: TAIL_DISTANCE }}
        onScroll={({ offset, contentSize, size }) => following.set(contentSize.height - size.height - offset.y <= TAIL_DISTANCE)}
        class="flex-1">
        {(item) => <TranscriptItem row={() => props.row(item())?.()}
          reader={(key) => open().get(key)} diff={props.diff} diagram={props.diagram} fullscreen={props.fullscreen}
          bindLauncher={bindLauncher} onToggle={toggle} onFullscreen={toggleFullscreen} />}
      </List>
      <If when={!following()}>
        <Button onPress={() => list?.scrollToEnd({ animated: false })}
          class="self-end rounded border border-border px-2 py-1 hover:bg-muted">
          <Text>Jump to bottom</Text>
        </Button>
      </If>
    </View>
  );
}
