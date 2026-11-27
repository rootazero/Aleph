import { cleanup, List, signal, Text, View } from "nsh";
import { Button, Input } from "nsh/base";
import { NebulaModel } from "aleph/panel";
import { RpcError, type PanelSdk } from "../shell/sdk";
import { newIdempotencyKey } from "./chatIdempotency";
import { NebulaCanvas } from "./NebulaCanvas";
import type { Scene } from "./nebulaViewport";

type NoteAddress = { agent_id: string; node_id: string };
type NoteRow = { id: string; agent_id?: string; name: string; category: string };
type GraphWindow = { nodes: NoteRow[]; total?: number; focus?: NoteAddress; truncated?: boolean };
type GraphView = GraphWindow & { scene: Scene; labels: string[]; index: Map<string, number>; windowKey: string };
type Detail = {
  node: NoteRow;
  content: string;
  revision?: string;
  backlinks: string[];
  backlink_targets?: NoteAddress[];
  outgoing?: { to: string; status: string; label?: string; relation?: string; target?: NoteAddress }[];
};
type Selected = { address: NoteAddress; name: string };
type NoteDraft = {
  action: "update" | "rename" | "delete";
  address: NoteAddress;
  revision: string;
  value: string;
  key: string;
};

function address(note: NoteRow): NoteAddress | null {
  return note.agent_id && note.id ? { agent_id: note.agent_id, node_id: note.id } : null;
}

function addressKey(target: NoteAddress): string {
  return JSON.stringify([target.agent_id, target.node_id]);
}

function noteKey(note: NoteRow): string {
  return JSON.stringify([note.agent_id ?? null, note.id]);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function MemoryNebula(props: { sdk: PanelSdk; canMutate: () => boolean }) {
  const graph = signal<GraphView | null>(null);
  const graphLoading = signal(false);
  const graphError = signal<string | null>(null);
  const query = signal("");
  const hits = signal<NoteRow[] | null>(null);
  const searching = signal(false);
  const searchError = signal<string | null>(null);
  const selected = signal<Selected | null>(null);
  const detail = signal<Detail | null>(null);
  const detailError = signal<string | null>(null);
  const draft = signal<NoteDraft | null>(null);
  const saving = signal(false);
  const mutationError = signal<string | null>(null);
  const affected = signal<{ address: NoteAddress; failedSource?: NoteAddress } | null>(null);
  const focusRequest = signal<{ index: number; revision: number } | null>(null);
  let graphRequest = 0;
  let searchRequest = 0;
  let detailRequest = 0;
  let cameraRevision = 0;
  let pendingGraphTarget: NoteAddress | undefined;
  let disposed = false;
  cleanup(() => { disposed = true; });
  const connected = () => props.sdk.connection().state === "connected";
  const current = () => !disposed && props.sdk.active() && connected();
  const selectedIndex = () => {
    const note = selected();
    return note ? graph()?.index.get(addressKey(note.address)) ?? -1 : -1;
  };

  function resetSearch() {
    searchRequest += 1;
    hits.set(null);
    searchError.set(null);
    searching.set(false);
  }

  function focusIndex(index: number) {
    focusRequest.set({ index, revision: ++cameraRevision });
  }

  async function loadGraph(target?: NoteAddress, center = false) {
    if (!current()) return;
    const request = ++graphRequest;
    pendingGraphTarget = target;
    focusRequest.set(null);
    graphLoading.set(true);
    graphError.set(null);
    try {
      const response = await props.sdk.invoke("graph.query", target ? { limit: 500, focus: target } : { limit: 100 }) as GraphWindow;
      if (!current() || request !== graphRequest) return;
      if (target && (!response.focus || addressKey(response.focus) !== addressKey(target))) {
        throw new Error("The gateway cannot open this note’s neighborhood.");
      }
      const scene = NebulaModel.scene(response) as Scene | null;
      if (!scene || scene.points.length !== response.nodes.length) {
        throw new Error("The gateway returned an invalid graph.");
      }
      const index = new Map(response.nodes.map((note, position) => [noteKey(note), position]));
      const windowKey = target ? addressKey(target) : "overview";
      graph.set({ ...response, scene, labels: response.nodes.map((note) => note.name), index, windowKey });
      if (center && target) {
        const position = index.get(addressKey(target));
        const note = selected();
        if (position !== undefined && note && addressKey(note.address) === addressKey(target)) {
          focusIndex(position);
        }
      }
    } catch (error) {
      if (current() && request === graphRequest) graphError.set(errorMessage(error));
    } finally {
      if (current() && request === graphRequest) graphLoading.set(false);
    }
  }

  async function search() {
    const term = query().trim();
    resetSearch();
    if (!term || !current()) return;
    const request = searchRequest;
    searching.set(true);
    try {
      const response = await props.sdk.invoke("graph.search", { query: term }) as { results: NoteRow[] };
      if (current() && request === searchRequest) hits.set(response.results);
    } catch (error) {
      if (current() && request === searchRequest) searchError.set(errorMessage(error));
    } finally {
      if (current() && request === searchRequest) searching.set(false);
    }
  }

  async function select(target: NoteAddress, name: string) {
    if (!current()) return;
    // A neighborhood request belongs to the selection that opened it.
    if (graphLoading() && pendingGraphTarget && addressKey(pendingGraphTarget) !== addressKey(target)) {
      graphRequest += 1;
      graphLoading.set(false);
      graphError.set(null);
    }
    const request = ++detailRequest;
    selected.set({ address: target, name });
    detail.set(null);
    detailError.set(null);
    try {
      const response = await props.sdk.invoke("graph.node_detail", target) as Detail;
      if (!current() || request !== detailRequest) return;
      const reportedAddress = address(response.node);
      if (reportedAddress && addressKey(reportedAddress) !== addressKey(target)) {
        throw new Error("The gateway returned a different note address.");
      }
      selected.set({ address: target, name: response.node.name });
      detail.set(response);
    } catch (error) {
      if (current() && request === detailRequest) detailError.set(errorMessage(error));
    }
  }

  function selectRow(note: NoteRow) {
    const target = address(note);
    if (target && !draft()) void select(target, note.name);
  }

  function startDraft(action: NoteDraft["action"]) {
    const note = detail();
    const target = note && address(note.node);
    if (!current() || !props.canMutate() || !note?.revision || !target) return;
    mutationError.set(null);
    affected.set(null);
    const value = action === "update" ? note.content : action === "rename" ? note.node.name : "";
    draft.set({ action, address: target, revision: note.revision, value, key: newIdempotencyKey() });
  }

  function changeDraft(value: string) {
    const pending = draft();
    if (pending && !saving() && !affected() && value !== pending.value) {
      draft.set({ ...pending, value, key: newIdempotencyKey() });
    }
  }

  function draftMatchesNote() {
    const pending = draft();
    const note = selected();
    return !!pending && !!note && addressKey(note.address) === addressKey(pending.address);
  }

  function useCurrentRevision() {
    const pending = draft();
    const revision = detail()?.revision;
    if (pending && revision && draftMatchesNote()) {
      draft.set({ ...pending, revision, key: newIdempotencyKey() });
      mutationError.set(null);
    }
  }

  function cancelDraft() {
    draft.set(null);
    affected.set(null);
    mutationError.set(null);
  }

  function canSubmitDraft() {
    const pending = draft();
    if (!pending || !current() || !props.canMutate() || saving()) return false;
    if (affected()) return pending.action === "rename";
    return draftMatchesNote() && pending.revision === detail()?.revision &&
      (pending.action !== "rename" || !!pending.value.trim());
  }

  async function submitDraft() {
    const pending = draft();
    if (!pending || !canSubmitDraft()) return;
    saving.set(true);
    mutationError.set(null);
    try {
      const action = pending.action === "update" ? { type: "update", content: pending.value } :
        pending.action === "rename" ? { type: "rename", title: pending.value } : { type: "delete" };
      const result = await props.sdk.invoke("graph.mutate_note", {
        address: pending.address,
        revision: pending.revision,
        action,
        idempotency_key: pending.key,
      }) as { address: NoteAddress; status: "updated" | "renamed" | "deleted" };
      if (!current()) return;
      if (result.address.agent_id !== pending.address.agent_id) {
        throw new Error("The gateway returned a different note partition.");
      }
      cancelDraft();
      resetSearch();
      const deleted = result.status === "deleted";
      let focus = graph()?.focus;
      if (focus && addressKey(focus) === addressKey(pending.address)) {
        focus = deleted ? undefined : result.address;
      }
      if (deleted) {
        detailRequest += 1;
        selected.set(null);
        detail.set(null);
      } else {
        void select(result.address, selected()?.name ?? result.address.node_id);
      }
      void loadGraph(focus);
    } catch (error) {
      if (disposed || !props.sdk.active()) return;
      mutationError.set(errorMessage(error));
      const data = error instanceof RpcError
        ? error.data as {
          reconciliation_required?: boolean;
          address?: NoteAddress;
          failed_source?: NoteAddress;
        } | undefined : undefined;
      const reported = data?.reconciliation_required ? data.address : undefined;
      if (reported?.agent_id === pending.address.agent_id && reported.node_id) {
        const failedSource = data?.failed_source;
        affected.set({ address: reported, failedSource: failedSource?.agent_id === reported.agent_id && failedSource.node_id ? failedSource : undefined });
        resetSearch();
        void loadGraph();
      }
    } finally {
      if (!disposed && props.sdk.active()) saving.set(false);
    }
  }

  function centerSelected() {
    const position = selectedIndex();
    if (position >= 0) focusIndex(position);
  }

  function relations() {
    const note = detail();
    if (!note) return [];
    const backlinks = new Map(note.backlink_targets?.map((target) => [target.node_id, target]));
    return [
      ...note.backlinks.map((from) => ({ label: `← ${from}`, target: backlinks.get(from) })),
      ...(note.outgoing ?? []).map((link) => ({
        label: `→ ${link.label ?? link.to}${link.relation ? ` · ${link.relation}` : ""}${link.status === "active" ? "" : ` · ${link.status}`}`,
        target: link.status === "active" ? link.target : undefined,
      })),
    ];
  }

  void loadGraph();

  return (
    <View class="flex-1 min-h-0 gap-3 p-4">
      <View class="flex-row items-center gap-2">
        <Input
          value={query()}
          onChange={({ value }) => { query.set(value); resetSearch(); }}
          onSubmit={() => void search()}
          placeholder="Search notes"
          class="flex-1 rounded-(--radius) border border-input px-3 py-2 text-foreground"
        />
        <Button onPress={() => void search()} disabled={!connected() || !query().trim() || searching()} class="px-3 py-2">
          <Text class="text-foreground">Search</Text>
        </Button>
        <Button onPress={() => void loadGraph(graph()?.focus)} disabled={!connected() || graphLoading()} class="px-3 py-2">
          <Text class="text-foreground">Refresh graph</Text>
        </Button>
        <If when={graph()?.focus}>
          <Button onPress={() => void loadGraph()} disabled={!connected() || graphLoading()} class="px-3 py-2">
            <Text class="text-foreground">Back to overview</Text>
          </Button>
        </If>
      </View>
      <If when={!connected()}><Text class="text-muted-foreground">Memory connection unavailable</Text></If>
      <If when={graphError()}>
        <View class="flex-row items-center gap-2">
          <Text class="flex-1 text-destructive">{graphError()}</Text>
          <Button onPress={() => void loadGraph(pendingGraphTarget, !!pendingGraphTarget)} disabled={!connected() || graphLoading()}>
            <Text>Retry graph</Text>
          </Button>
        </View>
      </If>
      <If when={searchError()}><Text class="text-destructive">{searchError()}</Text></If>
      <If when={searching()}><Text class="text-muted-foreground">Searching…</Text></If>
      <If when={connected() && graphLoading()}><Text class="text-muted-foreground">Loading graph…</Text></If>
      <If when={graph()}>
        {(view) => (
          <View class="flex-1 min-h-0 gap-1">
            <Text class="text-muted-foreground">{view().focus
              ? `${view().nodes.length} notes in this neighborhood${view().truncated ? " (limited)" : ""}`
              : `${view().nodes.length} of ${view().total ?? view().nodes.length} graph notes`}</Text>
            <If when={view().nodes.length > 0}>
              <NebulaCanvas
                scene={view().scene}
                windowKey={view().windowKey}
                selected={selectedIndex}
                labels={() => view().labels}
                focusRequest={focusRequest}
                onSelect={(index) => {
                  const note = view().nodes[index];
                  if (note) selectRow(note);
                }}
              />
            </If>
          </View>
        )}
      </If>
      <View class="flex-1 min-h-0 flex-row gap-3">
        <List
          each={hits() ?? graph()?.nodes ?? []}
          keyExtractor={noteKey}
          empty={<Text class="text-muted-foreground">{hits() ? "No matching notes" : graph() ? "No graph notes" : ""}</Text>}
          class="flex-1"
        >
          {(row) => (
            <If when={address(row())} else={<Text class="py-2 text-muted-foreground">{row().name}</Text>}>
              <Button onPress={() => selectRow(row())} disabled={!connected() || !!draft()} class="items-start px-2 py-2">
                <Text class="text-foreground">{row().name}</Text>
                <Text class="text-muted-foreground">{`${row().agent_id} · ${row().category}`}</Text>
              </Button>
            </If>
          )}
        </List>
        <If when={selected()}>
          {(note) => (
            <View class="flex-1 min-h-0 gap-1 border-l border-border pl-3">
              <View class="flex-row items-center gap-2">
                <Text class="flex-1 text-foreground">{note().name}</Text>
                <If when={selectedIndex() >= 0}>
                  <Button onPress={centerSelected} disabled={!connected() || graphLoading()}><Text>Center note</Text></Button>
                </If>
                <If when={graph()?.focus?.agent_id !== note().address.agent_id || graph()?.focus?.node_id !== note().address.node_id}>
                  <Button onPress={() => void loadGraph(note().address, true)} disabled={!connected() || graphLoading()}>
                    <Text>Show neighborhood</Text>
                  </Button>
                </If>
              </View>
              <If when={selectedIndex() < 0}>
                <Text class="text-muted-foreground">Not in the current graph</Text>
              </If>
              <If when={detailError()}>
                <View class="flex-row items-center gap-2">
                  <Text class="flex-1 text-destructive">{detailError()}</Text>
                  <Button onPress={() => void select(note().address, note().name)} disabled={!connected() || saving()}>
                    <Text>Retry note</Text>
                  </Button>
                </View>
              </If>
              <If when={props.canMutate() && !!detail()?.revision && !draft()}>
                <View class="flex-row flex-wrap gap-2">
                  <Button onPress={() => startDraft("update")} disabled={!connected()}><Text>Edit Markdown</Text></Button>
                  <Button onPress={() => startDraft("rename")} disabled={!connected()}><Text>Rename</Text></Button>
                  <Button onPress={() => startDraft("delete")} disabled={!connected()}><Text>Delete</Text></Button>
                </View>
              </If>
              <If when={draft()}>
                {(pending) => (
                  <View class="gap-2">
                    <If when={pending().action === "delete"} else={
                      <Input
                        value={pending().value}
                        onChange={({ value }) => changeDraft(value)}
                        multiline={pending().action === "update"}
                        accessibilityLabel={pending().action === "update" ? "Note Markdown" : "New note title"}
                        readonly={saving() || !!affected()}
                        class="rounded-(--radius) border border-input px-3 py-2 text-foreground"
                      />
                    }>
                      <Text class="text-foreground">Delete {note().name}?</Text>
                    </If>
                    <If when={mutationError()}>
                      <Text class="text-destructive">{mutationError()}</Text>
                      <Button onPress={() => void select(affected()?.address ?? pending().address, note().name)} disabled={!connected() || saving()}>
                        <Text>{affected() ? "Reload affected note" : "Reload note"}</Text>
                      </Button>
                      <If when={affected()?.failedSource}>
                        {(source) => <Button onPress={() => void select(source(), source().node_id)} disabled={!connected() || saving()}>
                          <Text>Inspect backlink source</Text>
                        </Button>}
                      </If>
                    </If>
                    <If when={!affected() && draftMatchesNote() && !!detail()?.revision && pending().revision !== detail()?.revision}>
                      <Button onPress={useCurrentRevision} disabled={saving()}><Text>Use latest revision</Text></Button>
                    </If>
                    <View class="flex-row gap-2">
                      <Button onPress={() => void submitDraft()} disabled={!canSubmitDraft()}>
                        <Text>{saving() ? "Saving…" : affected() && pending().action === "rename" ? "Resume rename" : pending().action === "delete" ? "Confirm delete" : "Save"}</Text>
                      </Button>
                      <Button onPress={cancelDraft} disabled={saving()}><Text>Cancel</Text></Button>
                    </View>
                  </View>
                )}
              </If>
              <If when={detail()} else={<If when={!detailError() && connected()}><Text class="text-muted-foreground">Loading note…</Text></If>}>
                <List
                  class="flex-1"
                  each={relations()}
                  empty={<Text class="text-muted-foreground">No links</Text>}
                  header={<Text class="text-foreground">{detail()?.content}</Text>}
                >
                  {(link) => (
                    <If when={link().target} else={<Text class="text-muted-foreground">{link().label}</Text>}>
                      {(target) => (
                        <Button onPress={() => void select(target(), target().node_id)} disabled={!connected() || !!draft()} class="items-start">
                          <Text class="text-foreground">{link().label}</Text>
                        </Button>
                      )}
                    </If>
                  )}
                </List>
              </If>
            </View>
          )}
        </If>
      </View>
    </View>
  );
}
