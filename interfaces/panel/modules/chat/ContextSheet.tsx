import { cleanup, Scroll, signal, Text, View } from "nsh";
import { Button } from "nsh/base";
import { RpcError, type PanelSdk } from "../../shell/sdk";

type Counts = {
  session_key: string;
  turn: number;
  system_prompt_bytes: number | null;
  skill_pre_trim_bytes: number;
  tool_schema_bytes: number;
  tool_result_tokens: number | null;
  provider_total: number | null;
  layers: { name: string; zone: string; bytes: number }[];
};

type State = { kind: "loading" } | { kind: "unavailable"; message: string } | { kind: "ready"; counts: Counts };

export function ContextSheet(props: {
  session: () => string | null;
  sdk: PanelSdk;
  project: (data: unknown) => unknown;
  close: () => void;
}) {
  const state = signal<State>({ kind: "loading" });
  let generation = 0;
  cleanup(() => { generation += 1; });
  const load = async () => {
    const key = props.session();
    if (!key) return;
    const request = ++generation;
    const current = () => generation === request && props.session() === key;
    state.set({ kind: "loading" });
    try {
      const result = await props.sdk.invoke("context.breakdown", { session_key: key });
      if (!current()) return;
      const counts = props.project(result) as Counts | null;
      if (!counts || counts.session_key !== key) throw new Error("Unexpected context response");
      state.set({ kind: "ready", counts });
    } catch (err) {
      if (!current()) return;
      const unmeasured = err instanceof RpcError && err.message === "no measured prompt for this session yet";
      state.set({ kind: "unavailable", message: unmeasured ? "No request measured yet." : "Context is unavailable." });
    }
  };
  void load();

  const counts = () => {
    const current = state();
    return current.kind === "ready" ? current.counts : null;
  };
  const message = () => {
    const current = state();
    return current.kind === "unavailable" ? current.message : null;
  };

  return (
    <View class="gap-2 rounded border border-border p-2">
      <View class="flex-row flex-wrap items-center justify-between gap-2">
        <Text class="font-semibold">Context</Text>
        <View class="flex-row gap-2">
          <Button onPress={() => void load()} disabled={state().kind === "loading"}><Text>Refresh Context</Text></Button>
          <Button onPress={props.close}><Text>Close Context</Text></Button>
        </View>
      </View>
      <If when={state().kind === "loading"}><Text>Loading Context…</Text></If>
      <If when={message()}><Text class="text-muted-foreground">{message()}</Text></If>
      <If when={counts()}>
        {(view) => (
          <Scroll class="max-h-56">
            <View class="gap-1">
              <Text class="text-muted-foreground">Last measured request · turn {view().turn}</Text>
              <Text>System Prompt · {view().system_prompt_bytes === null ? "Unknown" : `${view().system_prompt_bytes} bytes sent`}</Text>
              <Text>Skill · {view().skill_pre_trim_bytes} bytes before trimming (part of System Prompt)</Text>
              <Text>Tool schemas · {view().tool_schema_bytes} bytes</Text>
              <Text>Tool results · {view().tool_result_tokens === null ? "Unknown" : `${view().tool_result_tokens} estimated tokens`}</Text>
              <Text>Provider input · {view().provider_total === null ? "Unknown" : `${view().provider_total} tokens`}</Text>
              <If when={view().layers.length > 0}>
                <Text class="mt-2 text-muted-foreground">System Prompt layers before trimming</Text>
                <Each of={view().layers} key={(layer) => `${layer.zone}/${layer.name}`}>
                  {(layer) => <Text>{layer().name} · {layer().bytes} bytes ({layer().zone})</Text>}
                </Each>
              </If>
            </View>
          </Scroll>
        )}
      </If>
    </View>
  );
}
