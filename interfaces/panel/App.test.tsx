// The shell's flows against a stand-in embedding host: `aleph.rpc` answers
// like the Gateway's v1 method set, `aleph.connection` like a host that is
// already connected.
import { expect, open, test } from "nsh/test";

const ENDPOINT = "https://aleph.test";
const SUBJECT = "0123456789abcdef0123456789abcdef";
const LAYOUT_KEY = `aleph.panel.layout.v1:${ENDPOINT}:${SUBJECT}`;
const CONNECTED = { state: "connected", endpoint: ENDPOINT, subject: SUBJECT, pairing: false };

const COMMUNITY_TAB = {
  descriptor: {
    key: { plugin_id: "community.fixture", tab_id: "notes" },
    label: "Notes",
    icon: "note",
    order: 5,
    entry: "./tabs/community.fixture/1.0.0/notes.js",
    api_major: 1,
    trust: "trusted",
    permissions: [],
    status: "enabled",
    status_detail: null,
  },
  origin: "community",
  plugin_version: "1.0.0",
  asset_integrity: `sha256-${"A".repeat(43)}=`,
  public_base: "./tabs/",
  source_revision: 4,
};

type Answers = Record<string, (params: unknown) => unknown>;

function gateway(answers: Answers = {}) {
  const methods: Answers = {
    health: () => ({ status: "healthy" }),
    "sessions.list": () => ({ sessions: [{ key: "s-1", label: "Release notes", message_count: 3 }], count: 1 }),
    "panel.capabilities.snapshot": () => ({
      revision: 4,
      declarations: [COMMUNITY_TAB],
      project_context: { project_id: null, generation: 0 },
    }),
    ...answers,
  };
  return {
    "aleph.connection": () => CONNECTED,
    "aleph.connect": () => null,
    "aleph.pair": () => ({ result: null }),
    "aleph.rpc": async (call: { method: string; params: unknown }) => {
      const answer = methods[call.method];
      if (!answer) return { error: { code: -32601, message: "method unavailable on Web v1" } };
      return { result: await answer(call.params) };
    },
  };
}

const rpcCalls = (app: { host: { calls(): { name: string; data: unknown }[] } }, method: string) =>
  app.host.calls().filter((call) => call.name === "aleph.rpc" && (call.data as { method: string }).method === method).length;

test("opens the overview with the gateway's health", async () => {
  const app = await open("./App.tsx", { host: { handlers: gateway() } });
  await app.waitFor("Gateway: healthy");
  expect(app.texts()).toContain("Overview");
  expect(app.texts()).toContain("Sessions");
});

test("layout round-trips through storage", async () => {
  const first = await open("./App.tsx", { host: { handlers: gateway() } });
  await first.waitFor("Gateway: healthy");
  await first.press("Customize");
  await first.press("label:Move Sessions up");
  await first.press("label:Hide Capabilities");
  const saved = (await first.storage())[LAYOUT_KEY];
  expect(JSON.parse(saved).order.slice(0, 2)).toEqual([
    { plugin_id: "aleph", tab_id: "sessions" },
    { plugin_id: "aleph", tab_id: "overview" },
  ]);
  first.dispose();

  const second = await open("./App.tsx", { host: { handlers: gateway() }, storage: { [LAYOUT_KEY]: saved } });
  await second.waitFor("label:Open Sessions");
  expect(second.count("label:Open Capabilities")).toBe(0);
  expect((await second.storage())[LAYOUT_KEY]).toBe(saved);
});

test("a stored tab the gateway no longer offers renders unavailable", async () => {
  const stored = JSON.stringify({
    version: 1,
    order: [{ plugin_id: "gone.plugin", tab_id: "old" }],
    hidden: [],
    active: null,
  });
  const app = await open("./App.tsx", { host: { handlers: gateway() }, storage: { [LAYOUT_KEY]: stored } });
  await app.waitFor("gone.plugin:old · unavailable");
  expect(app.count("label:Open gone.plugin:old")).toBe(0);
  await app.waitFor("Gateway: healthy");
});

test("a DOM-ABI community tab shows unavailable in this renderer", async () => {
  const app = await open("./App.tsx", { host: { handlers: gateway() } });
  await app.waitFor("label:Open Notes");
  await app.press("label:Open Notes");
  await app.waitFor("Notes is unavailable");
});

test("a hidden module releases its subscriptions", async () => {
  const app = await open("./App.tsx", { host: { handlers: gateway() } });
  await app.waitFor("Gateway: healthy");
  const mounted = rpcCalls(app, "health");
  await app.host.send("aleph.connection", CONNECTED);
  await app.waitUntil(() => rpcCalls(app, "health") === mounted + 1);

  await app.press("Customize");
  await app.press("label:Hide Overview");
  await app.waitFor("Gateway: healthy", { state: "gone" });
  await app.host.send("aleph.connection", CONNECTED);
  await app.wait(200);
  expect(rpcCalls(app, "health")).toBe(mounted + 1);
});

// A late result after its module went away: covered where the rule lives,
// `shell/sdk.test.ts` (a handler held open would stall every app step here).

test("a refused session shows the pairing form and pairs", async () => {
  const handlers = {
    ...gateway(),
    "aleph.connection": () => ({ state: "unavailable", endpoint: ENDPOINT, subject: null, pairing: true }),
  };
  const app = await open("./App.tsx", { host: { handlers } });
  await app.waitFor("Pair this browser with Aleph");
  await app.type("text:Pairing ticket or device token", "ticket-123");
  await app.press("Pair with ticket");
  await app.waitUntil(() => app.host.calls().some((call) => call.name === "aleph.pair"));
  const pair = app.host.calls().find((call) => call.name === "aleph.pair");
  expect(pair?.data).toEqual({ bootstrap_ticket: "ticket-123" });
});

const NEBULA_TAB = {
  ...COMMUNITY_TAB,
  descriptor: {
    ...COMMUNITY_TAB.descriptor,
    key: { plugin_id: "aleph", tab_id: "memory-nebula" },
    label: "Memory Nebula",
    entry: "shell/memory-nebula",
    trust: "verified",
  },
  origin: "builtin",
  asset_integrity: null,
  public_base: "shell/",
};

function nebulaSnapshot(declarations: unknown[]) {
  return { revision: 4, declarations, project_context: { project_id: null, generation: 0 } };
}

test("Nebula opens exact search addresses without assuming graph membership", async () => {
  const inGraph = { id: "concept/same", path: "concept/same", tags: [], link_count: 0, agent_id: "org-a", name: "In graph", category: "concept" };
  const outside = { ...inGraph, agent_id: "org-b", name: "Search only" };
  const paramsSeen: unknown[] = [];
  const app = await open("./App.tsx", { host: { handlers: gateway({
    "panel.capabilities.snapshot": () => nebulaSnapshot([NEBULA_TAB]),
    "graph.query": () => ({ nodes: [inGraph], edges: [] }),
    "graph.search": () => ({ results: [outside] }),
    "graph.node_detail": (params) => {
      paramsSeen.push(params);
      return { node: outside, content: "Body from org-b", backlinks: [], outgoing: [] };
    },
  }) } });
  await app.waitFor("label:Open Memory Nebula");
  await app.press("label:Open Memory Nebula");
  await app.waitFor("In graph");
  await app.type("text:Search notes", "same");
  await app.press("Search");
  await app.waitFor("Search only");
  await app.press("Search only");
  await app.waitFor("Body from org-b");
  expect(app.texts()).toContain("Not in the current graph");
  expect(paramsSeen).toEqual([{ agent_id: "org-b", node_id: "concept/same" }]);
});

test("Nebula discovery is not carried across connection identities", async () => {
  let declarations: unknown[] = [NEBULA_TAB];
  const note = { id: "concept/private", path: "concept/private", tags: [], link_count: 0, agent_id: "org-a", name: "Original note", category: "concept" };
  const app = await open("./App.tsx", { host: { handlers: gateway({
    "panel.capabilities.snapshot": () => nebulaSnapshot(declarations),
    "graph.query": () => ({ nodes: [note], edges: [] }),
  }) } });
  await app.waitFor("label:Open Memory Nebula");
  await app.press("label:Open Memory Nebula");
  await app.waitFor("Original note");
  declarations = [];
  await app.host.send("aleph.connection", { ...CONNECTED, subject: "fedcba9876543210fedcba9876543210" });
  await app.waitFor("label:Open Memory Nebula", { state: "gone" });
  await app.waitFor("Gateway: healthy");
  expect(rpcCalls(app, "graph.query")).toBe(1);
});
