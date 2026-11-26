import { cleanup, effect, Text, untracked, View, type ViewRef } from "nsh";
import { Button } from "nsh/base";
import type { ToolRow } from "../Chat";
import { Diff, type Change } from "./Diff";
import type { outputReader } from "./output";

const status: Record<ToolRow["status"], string> = {
  pending: "Status unknown",
  running: "Running",
  ok: "Done",
  error: "Failed",
};

type Reader = ReturnType<typeof outputReader>;

export function ToolCard(props: {
  row: () => ToolRow;
  reader: () => Reader | undefined;
  diff: (expanded: boolean) => Change[];
  focusedReader: () => boolean;
  onToggle: () => void;
  onFullscreen: () => void;
  bindFullscreen: (ref?: ViewRef) => void;
}) {
  const row = props.row;
  const open = () => props.reader() !== undefined;
  const title = () => row().display_name + (row().args_text ? ` · ${row().args_text}` : "");
  const settled = () => row().status === "ok" || row().status === "error";
  const output = () => props.reader()?.state();
  const load = () => void props.reader()?.load(row());
  const expectsDiff = () => row().display_name === "Edit" || row().display_name === "Write";
  let toggleRef: ViewRef | undefined;
  const toggle = () => {
    const closingReader = props.focusedReader();
    props.onToggle();
    if (closingReader) requestAnimationFrame(() => toggleRef?.focus());
  };
  effect(() => {
    const reader = props.reader();
    if (!reader || !settled()) return;
    untracked(() => {
      const state = reader.state();
      if (state.total === null && !state.error) void reader.load(row());
    });
  });
  cleanup(() => props.bindFullscreen());

  return (
    <View class="gap-1 py-1">
      <Button ref={(ref) => { toggleRef = ref; }} onPress={toggle}
        accessibilityLabel={`${open() ? "Collapse" : "Expand"} ${title()}`}
        class={`items-stretch rounded-(--radius) border px-3 py-2 hover:bg-muted focus:border-primary ${props.focusedReader() ? "border-primary bg-muted" : "border-border"}`}>
        <View class="flex-row flex-wrap items-center justify-between gap-1">
          <Text class="text-foreground">{title()}</Text>
          <View class="flex-row gap-2">
            <Text class={row().status === "error" ? "text-destructive" : "text-muted-foreground"}>{status[row().status]}</Text>
            <If when={row().duration_ms !== null}><Text class="text-muted-foreground">{row().duration_ms} ms</Text></If>
          </View>
        </View>
      </Button>
      <If when={open()}>
        <View class="gap-2 border-l border-border pl-3 py-2">
          <Button ref={(ref) => props.bindFullscreen(ref)} onPress={props.onFullscreen}
            class="self-start rounded px-2 py-1 hover:bg-muted focus-visible:bg-muted">
            <Text>{props.focusedReader() ? "Exit full screen" : "Open full screen"}</Text>
          </Button>
          <If when={row().has_diff}><Diff read={props.diff} /></If>
          <If when={row().presentation_omitted}><Text class="text-muted-foreground">Recorded changes were omitted.</Text></If>
          <If when={settled() && expectsDiff() && !row().has_diff && !row().presentation_omitted}>
            <Text class="text-muted-foreground">No recorded diff.</Text>
          </If>
          <If when={!settled()}><Text class="text-muted-foreground">Output is not available until the call finishes.</Text></If>
          <If when={output()?.text}>
            <View selectable class="rounded bg-muted p-2"><Text class="font-mono text-sm">{output()?.text}</Text></View>
          </If>
          <If when={output()?.total === 0 && !output()?.expired}><Text class="text-muted-foreground">No output.</Text></If>
          <If when={output()?.expired}><Text class="text-muted-foreground">Tool output expired.</Text></If>
          <If when={output()?.limited}><Text class="text-muted-foreground">Output preview limited to 256 KiB.</Text></If>
          <If when={output()?.loading}><Text class="text-muted-foreground">Loading tool output…</Text></If>
          <If when={output()?.error}>
            <Text class="text-destructive">{output()?.error}</Text>
            <Button onPress={load} class="self-start px-2 py-1 hover:bg-muted"><Text>Retry output</Text></Button>
          </If>
          <If when={!output()?.error && !output()?.loading && !output()?.limited && (output()?.next ?? 0) < (output()?.total ?? 0)}>
            <Button onPress={load} class="self-start px-2 py-1 hover:bg-muted"><Text>Load more output</Text></Button>
          </If>
        </View>
      </If>
    </View>
  );
}
