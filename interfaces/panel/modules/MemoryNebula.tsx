import { cleanup, List, signal, Text, View } from "nsh";
import { Button, Input } from "nsh/base";
import { NebulaModel } from "aleph/panel";
import type { PanelSdk } from "../shell/sdk";
import { NebulaCanvas } from "./NebulaCanvas";
import type { Scene } from "./nebulaViewport";

type NoteRow = { id: string; agent_id?: string; name: string; category: string };
type NoteAddress = { agent_id: string; node_id: string };
type GraphWindow = { nodes: NoteRow[]; total?: number };
type Detail = {
  node: NoteRow;
  content: string;
  backlinks: string[];
  outgoing?: { to: string; status: string; label?: string; relation?: string }[];
};
type Selected = { note: NoteRow; inGraph: boolean };

function address(note: NoteRow): NoteAddress | null {
  return note.agent_id && note.id ? { agent_id: note.agent_id, node_id: note.id } : null;
}

function noteKey(note: NoteRow) {
  return JSON.stringify([note.agent_id ?? null, note.id]);
}

export function MemoryNebula(props: { sdk: PanelSdk }) {
  const graph = signal<(GraphWindow & { scene: Scene | null }) | null>(null);
  const graphError = signal<string | null>(null);
  const query = signal("");
  const hits = signal<NoteRow[] | null>(null);
  const searching = signal(false);
  const searchError = signal<string | null>(null);
  const selected = signal<Selected | null>(null);
  const detail = signal<Detail | null>(null);
  const detailError = signal<string | null>(null);
  let graphRequest = 0;
  let searchRequest = 0;
  let detailRequest = 0;
  let disposed = false;
  cleanup(() => { disposed = true; });
  const current = () => !disposed && props.sdk.active() && props.sdk.connection().state === "connected";

  function resetSearch() {
    searchRequest += 1;
    hits.set(null);
    searchError.set(null);
    searching.set(false);
  }

  function clearDetail() {
    detailRequest += 1;
    selected.set(null);
    detail.set(null);
    detailError.set(null);
  }

  async function refresh() {
    const request = ++graphRequest;
    graph.set(null);
    graphError.set(null);
    resetSearch();
    clearDetail();
    try {
      const response = await props.sdk.invoke("graph.query", { limit: 100 }) as GraphWindow;
      if (current() && request === graphRequest) {
        graph.set({ ...response, scene: NebulaModel.scene(response) as Scene | null });
      }
    } catch (error) {
      if (current() && request === graphRequest) graphError.set((error as Error).message);
    }
  }

  async function search() {
    const term = query().trim();
    resetSearch();
    if (!term) return;
    const request = searchRequest;
    searching.set(true);
    try {
      const response = await props.sdk.invoke("graph.search", { query: term }) as { results: NoteRow[] };
      if (current() && request === searchRequest) hits.set(response.results);
    } catch (error) {
      if (current() && request === searchRequest) searchError.set((error as Error).message);
    } finally {
      if (current() && request === searchRequest) searching.set(false);
    }
  }

  async function select(note: NoteRow) {
    const target = address(note);
    if (!target) return;
    clearDetail();
    const request = detailRequest;
    selected.set({ note, inGraph: graph()?.nodes.some((node) => noteKey(node) === noteKey(note)) ?? false });
    try {
      const response = await props.sdk.invoke("graph.node_detail", target) as Detail;
      if (current() && request === detailRequest) detail.set(response);
    } catch (error) {
      if (current() && request === detailRequest) detailError.set((error as Error).message);
    }
  }

  void refresh();

  return (
    <View class="flex-1 gap-3 p-4">
      <View class="flex-row items-center gap-2">
        <Input
          value={query()}
          onChange={({ value }) => { query.set(value); resetSearch(); }}
          placeholder="Search notes"
          class="flex-1 rounded-(--radius) border border-input px-3 py-2 text-foreground"
        />
        <Button onPress={() => void search()} disabled={!query().trim()} class="px-3 py-2">
          <Text class="text-foreground">Search</Text>
        </Button>
        <Button onPress={() => void refresh()} class="px-3 py-2">
          <Text class="text-foreground">Refresh graph</Text>
        </Button>
      </View>
      <If when={graphError()}><Text class="text-destructive">{graphError()}</Text></If>
      <If when={searchError()}><Text class="text-destructive">{searchError()}</Text></If>
      <If when={searching()}><Text class="text-muted-foreground">Searching…</Text></If>
      <If when={!graph() && !graphError()}><Text class="text-muted-foreground">Loading graph…</Text></If>
      <If when={graph()}>
        <Text class="text-muted-foreground">{`${graph()?.nodes.length ?? 0} of ${graph()?.total ?? graph()?.nodes.length ?? 0} graph notes`}</Text>
      </If>
      <If when={graph()?.scene}>
        {(scene) => <NebulaCanvas
          scene={scene()}
          selected={() => {
            const note = selected()?.note;
            return note ? graph()?.nodes.findIndex((row) => noteKey(row) === noteKey(note)) ?? -1 : -1;
          }}
          onSelect={(index) => {
            const note = graph()?.nodes[index];
            if (note) void select(note);
          }}
        />}
      </If>
      <List
        each={hits() ?? graph()?.nodes ?? []}
        keyExtractor={noteKey}
        empty={<Text class="text-muted-foreground">{hits() ? "No matching notes" : graph() ? "No graph notes" : ""}</Text>}
        class="flex-1"
      >
        {(row) => (
          <If when={address(row())} else={<Text class="py-2 text-muted-foreground">{row().name}</Text>}>
            <Button onPress={() => void select(row())} class="items-start px-2 py-2">
              <Text class="text-foreground">{row().name}</Text>
              <Text class="text-muted-foreground">{`${row().agent_id} · ${row().category}`}</Text>
            </Button>
          </If>
        )}
      </List>
      <If when={selected()}>
        <View class="flex-1 gap-1 border-t border-border pt-2">
          <Text class="text-foreground">{selected()?.note.name}</Text>
          <If when={selected()?.inGraph === false}>
            <Text class="text-muted-foreground">Not in the current graph</Text>
          </If>
          <If when={detail()} else={<Text class={detailError() ? "text-destructive" : "text-muted-foreground"}>{detailError() ?? "Loading note…"}</Text>}>
            <List
              class="flex-1"
              each={[
                ...(detail()?.backlinks ?? []).map((from) => `← ${from}`),
                ...(detail()?.outgoing ?? []).map((link) =>
                  `→ ${link.label ?? link.to}${link.relation ? ` · ${link.relation}` : ""}${link.status === "active" ? "" : ` · ${link.status}`}`),
              ]}
              empty={<Text class="text-muted-foreground">No links</Text>}
              header={<Text class="text-foreground">{detail()?.content}</Text>}
            >
              {(link) => <Text class="text-muted-foreground">{link()}</Text>}
            </List>
          </If>
        </View>
      </If>
    </View>
  );
}
