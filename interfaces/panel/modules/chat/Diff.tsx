import { signal, Text, View } from "nsh";
import { Button } from "nsh/base";

type Span = { text: string; emphasis: boolean };
type Line = { old: number | null; new: number | null; tag: "add" | "del" | "ctx"; spans: Span[] };
export type Change = {
  path: string;
  label: string;
  unavailable: string | null;
  lines: Line[];
  hidden: number;
};

const marker = { add: "+", del: "−", ctx: " " };
const lineStyle = { add: "bg-green-500/10", del: "bg-red-500/10", ctx: "" };

export function Diff(props: { read: (expanded: boolean) => Change[] }) {
  const full = signal(false);
  const changes = () => props.read(full());
  return (
    <View class="gap-2" selectable>
      <Each of={changes()}>
        {(change) => (
          <View class="gap-1 rounded-(--radius) border border-border p-2">
            <Text class="font-mono text-sm">{change().path}{change().label && ` · ${change().label}`}</Text>
            <If when={change().unavailable}>
              <Text class="text-muted-foreground">{change().unavailable}</Text>
            </If>
            <If when={!change().unavailable && change().lines.length === 0}>
              <Text class="text-muted-foreground">No changed lines.</Text>
            </If>
            <Each of={change().lines}>
              {(line) => (
                <View class={`flex-row gap-2 px-1 ${lineStyle[line().tag]}`}>
                  <Text class="w-8 text-right font-mono text-muted-foreground">{line().old ?? ""}</Text>
                  <Text class="w-8 text-right font-mono text-muted-foreground">{line().new ?? ""}</Text>
                  <Text class="w-3 font-mono">{marker[line().tag]}</Text>
                  <Text class="min-w-0 flex-1 font-mono">{line().spans.map((span) => (
                    <Text class={span.emphasis ? "font-bold underline" : ""}>{span.text}</Text>
                  ))}</Text>
                </View>
              )}
            </Each>
            <If when={change().hidden > 0}><Text class="text-muted-foreground">{change().hidden} more lines</Text></If>
          </View>
        )}
      </Each>
      <If when={full() || changes().some((change) => change.hidden > 0)}>
        <Button onPress={() => full.update((value) => !value)} class="self-start rounded px-2 py-1 hover:bg-muted">
          <Text>{full() ? "Show less diff" : "Show full diff"}</Text>
        </Button>
      </If>
    </View>
  );
}
