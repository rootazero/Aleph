// The shell's flows against a stand-in embedding host: `aleph.rpc` answers
// like the Gateway's v1 method set, `aleph.connection` like a host that is
// already connected.
import { expect, open, type OpenOptions, test } from "nsh/test";

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

// The Panel ships on Desktop and Web: open it in a desktop-sized window
// (the runner's default is a phone).
const openPanel = (options: OpenOptions) => open("./App.tsx", { platform: "macos", width: 1024, height: 768, ...options });

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
  const app = await openPanel({ host: { handlers: gateway() } });
  await app.waitFor("Gateway: healthy");
  expect(app.texts()).toContain("Overview");
  expect(app.texts()).toContain("Sessions");
});

test("layout round-trips through storage", async () => {
  const first = await openPanel({ host: { handlers: gateway() } });
  await first.waitFor("Gateway: healthy");
  await first.press("label:Settings");
  await first.press("label:Move Sessions up");
  await first.press("label:Hide Capabilities");
  const saved = (await first.storage())[LAYOUT_KEY];
  expect(JSON.parse(saved).order.slice(0, 2)).toEqual([
    { plugin_id: "aleph", tab_id: "sessions" },
    { plugin_id: "aleph", tab_id: "overview" },
  ]);
  first.dispose();

  const second = await openPanel({ host: { handlers: gateway() }, storage: { [LAYOUT_KEY]: saved } });
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
  const app = await openPanel({ host: { handlers: gateway() }, storage: { [LAYOUT_KEY]: stored } });
  await app.waitFor("gone.plugin:old · unavailable");
  expect(app.count("label:Open gone.plugin:old")).toBe(0);
  await app.waitFor("Gateway: healthy");
});

test("a DOM-ABI community tab shows unavailable in this renderer", async () => {
  const app = await openPanel({ host: { handlers: gateway() } });
  await app.waitFor("label:Open Notes");
  await app.press("label:Open Notes");
  await app.waitFor("Notes is unavailable");
});

test("a hidden module releases its subscriptions", async () => {
  const app = await openPanel({ host: { handlers: gateway() } });
  await app.waitFor("Gateway: healthy");
  const mounted = rpcCalls(app, "health");
  await app.host.send("aleph.connection", CONNECTED);
  await app.waitUntil(() => rpcCalls(app, "health") === mounted + 1);

  await app.press("label:Settings");
  await app.press("label:Hide Overview");
  await app.press("label:Settings");
  await app.waitFor("Appearance", { state: "gone" });
  expect(app.shows("Gateway: healthy")).toBe(false);
  await app.host.send("aleph.connection", CONNECTED);
  await app.wait(200);
  expect(rpcCalls(app, "health")).toBe(mounted + 1);
});

const PREFS_KEY = "aleph.panel.prefs.v1";

test("settings open while disconnected", async () => {
  const handlers = {
    ...gateway(),
    "aleph.connection": () => ({ state: "disconnected", endpoint: ENDPOINT, subject: SUBJECT, pairing: false }),
  };
  const app = await openPanel({ host: { handlers } });
  await app.waitFor("Disconnected from Aleph");
  await app.press("label:Settings");
  await app.press("label:Accent ocean");
  await app.waitFor("Ocean ✓");
  await app.press("label:Corners sharp");
  await app.waitFor("Sharp ✓");
  const stored = JSON.parse((await app.storage())[PREFS_KEY]);
  expect([stored.appearance.accent, stored.appearance.roundness]).toEqual(["ocean", "sharp"]);
  await app.waitFor("Connect to Aleph to arrange the menu.");
});

test("appearance survives a restart", async () => {
  const stored = JSON.stringify({ version: 1, appearance: { mode: "dark", accent: "rose" } });
  const app = await openPanel({ host: { handlers: gateway() }, storage: { [PREFS_KEY]: stored } });
  await app.waitFor("Gateway: healthy");
  await app.press("label:Settings");
  await app.waitFor("Rose ✓");
  await app.waitFor("Dark ✓");
  expect(app.shows("Mauve ✓")).toBe(false);
});

test("corrupt prefs are repaired on open", async () => {
  const app = await openPanel({ host: { handlers: gateway() }, storage: { [PREFS_KEY]: "{" } });
  await app.waitFor("Gateway: healthy");
  const repaired = JSON.parse((await app.storage())[PREFS_KEY]);
  expect(repaired.version).toBe(1);
  expect(repaired.appearance.accent).toBe("mauve");
});

test("font size reaches the host", async () => {
  const handlers = { ...gateway(), "aleph.fontScale": () => null };
  const app = await openPanel({ host: { handlers } });
  await app.waitFor("Gateway: healthy");
  await app.press("label:Settings");
  await app.press("label:Text size large");
  const fontCalls = () => app.host.calls().filter((call) => call.name === "aleph.fontScale");
  await app.waitUntil(() => fontCalls().at(-1)?.data === 1.25);
  expect(app.shows("Not supported here")).toBe(false);
});

test("a host without text size says so", async () => {
  const app = await openPanel({ host: { handlers: gateway() } });
  await app.waitFor("Gateway: healthy");
  await app.press("label:Settings");
  await app.waitFor("Not supported here");
});

test("a compact window keeps a rail of initials", async () => {
  const app = await openPanel({ host: { handlers: gateway() }, width: 390 });
  await app.waitFor("Gateway: healthy");
  await app.waitFor("label:Open Sessions");
  expect(app.texts()).not.toContain("Sessions");
  await app.press("label:Open Sessions");
  await app.waitFor("Release notes");
});

// A late result after its module went away: covered where the rule lives,
// `shell/sdk.test.ts` (a handler held open would stall every app step here).

test("a refused session shows the pairing form and pairs", async () => {
  const handlers = {
    ...gateway(),
    "aleph.connection": () => ({ state: "unavailable", endpoint: ENDPOINT, subject: null, pairing: true }),
  };
  const app = await openPanel({ host: { handlers } });
  await app.waitFor("Pair this browser with Aleph");
  await app.type("text:Pairing ticket or device token", "ticket-123");
  await app.press("Pair with ticket");
  await app.waitUntil(() => app.host.calls().some((call) => call.name === "aleph.pair"));
  const pair = app.host.calls().find((call) => call.name === "aleph.pair");
  expect(pair?.data).toEqual({ bootstrap_ticket: "ticket-123" });
});

// Chat: a stand-in Gateway whose transcript grows once a run completes.
function chatGateway() {
  const transcript = [{ role: "assistant", content: "Earlier answer", timestamp: "t0" }];
  let active: string | null = null;
  const handlers = gateway({
    "chat.history": () => ({ session_key: "s-1", messages: [...transcript], active_run: active }),
    "chat.send": (params) => {
      const { message } = params as { message: string };
      transcript.push({ role: "user", content: message, timestamp: "t1" });
      active = "r-1";
      return { run_id: "r-1", session_key: "s-1", streaming: true };
    },
    "chat.abort": () => ({ aborted: true }),
  });
  const complete = (text: string) => {
    transcript.push({ role: "assistant", content: text, timestamp: "t2" });
    active = null;
  };
  return { handlers, complete };
}

const rpcParams = (app: { host: { calls(): { name: string; data: unknown }[] } }, method: string) =>
  app.host
    .calls()
    .filter((call) => call.name === "aleph.rpc" && (call.data as { method: string }).method === method)
    .map((call) => (call.data as { params: Record<string, unknown> }).params);

const chunk = (delta: string) => ({ topic: "stream.response_chunk", data: { type: "response_chunk", run_id: "r-1", delta } });

async function openChat(handlers: ReturnType<typeof gateway>) {
  const app = await openPanel({ host: { handlers } });
  await app.waitFor("label:Open Sessions");
  await app.press("label:Open Sessions");
  await app.press("label:Open chat Release notes");
  await app.waitFor("Earlier answer");
  return app;
}

test("a session opens in chat and its reply streams in, then history takes over", async () => {
  const { handlers, complete } = chatGateway();
  const app = await openChat(handlers);
  expect(rpcParams(app, "chat.history")[0]).toEqual({ session_key: "s-1", limit: 200 });

  await app.type("label:Message", "你好");
  await app.press("Send");
  await app.waitFor("Stop");
  const [sent] = rpcParams(app, "chat.send");
  expect(sent.message).toBe("你好");
  expect(sent.session_key).toBe("s-1");
  expect(sent.channel).toBe("gui:chat");
  expect(sent.stream).toBe(true);
  expect(String(sent.idempotency_key)).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);

  await app.host.send("aleph.event", chunk("Hel"));
  await app.host.send("aleph.event", { topic: "stream.response_chunk", data: { run_id: "other", delta: "nope" } });
  await app.host.send("aleph.event", chunk("lo"));
  await app.waitFor("Hello");
  expect(app.shows("nope")).toBe(false);

  complete("Hello, world");
  const loads = rpcParams(app, "chat.history").length;
  await app.host.send("aleph.event", { topic: "stream.run_complete", data: { type: "run_complete", run_id: "r-1" } });
  await app.waitFor("Hello, world");
  await app.waitFor("Stop", { state: "gone" });
  expect(rpcParams(app, "chat.history").length).toBe(loads + 1);
});

test("stop aborts the running reply", async () => {
  const { handlers } = chatGateway();
  const app = await openChat(handlers);
  await app.type("label:Message", "long task");
  await app.press("Send");
  await app.press("Stop");
  await app.waitUntil(() => rpcParams(app, "chat.abort").length === 1);
  expect(rpcParams(app, "chat.abort")[0]).toEqual({ run_id: "r-1", session_key: "s-1" });
});

// Return in the middle of a composition is the platform's (the input method
// commits the candidate); the harness's `submit` focuses the input first,
// which commits, so that case is checked on a real Mac. Here: a composition
// that ended must not leave the composer refusing to send.
test("an ended composition does not block sending", async () => {
  const { handlers } = chatGateway();
  const app = await openChat(handlers);
  await app.type("label:Message", "你");
  await app.compose("label:Message", "hao");
  await app.compose("label:Message", "");
  expect(app.value("label:Message")).toBe("你");
  expect(rpcParams(app, "chat.send").length).toBe(0);
  await app.submit("label:Message");
  await app.waitUntil(() => rpcParams(app, "chat.send").length === 1);
  expect(rpcParams(app, "chat.send")[0].message).toBe("你");
});

test("an interrupted live feed refetches the transcript", async () => {
  const { handlers } = chatGateway();
  const app = await openChat(handlers);
  const loads = rpcParams(app, "chat.history").length;
  await app.host.send("aleph.event", { topic: "connection.stale", data: { dropped: 3 } });
  await app.waitFor("Live updates were interrupted; reloaded from Aleph.");
  await app.waitUntil(() => rpcParams(app, "chat.history").length === loads + 1);
});

test("a send whose answer was lost is reported unknown, not retried", async () => {
  const { handlers } = chatGateway();
  const base = handlers["aleph.rpc"];
  const lossy = {
    ...handlers,
    "aleph.rpc": async (call: { method: string; params: unknown }) =>
      call.method === "chat.send" ? { error: { code: -32090, message: "disconnected" } } : base(call),
  };
  const app = await openChat(lossy);
  await app.type("label:Message", "did it arrive?");
  await app.press("Send");
  await app.waitFor("Connection lost while sending: the message may or may not have reached Aleph. Check the transcript before sending again.");
  await app.wait(100);
  expect(app.host.calls().filter((call) => (call.data as { method?: string })?.method === "chat.send").length).toBe(1);
});


test("mod+k opens the palette and opens a menu entry", async () => {
  const app = await openPanel({ host: { handlers: gateway() } });
  await app.waitFor("Gateway: healthy");
  await app.keyboard.press("mod+k");
  await app.waitFor("label:Command search");
  await app.type("label:Command search", "sess");
  await app.keyboard.press("enter");
  await app.waitFor("Release notes");
  await app.waitFor("label:Command search", { state: "gone" });
});

test("a rebound shortcut fires", async () => {
  const app = await openPanel({ host: { handlers: gateway() } });
  await app.waitFor("Gateway: healthy");
  await app.press("label:Settings");
  await app.type("label:Shortcut for Command palette", "mod+j");
  await app.submit("label:Shortcut for Command palette");
  await app.waitFor("⌘J");
  expect(JSON.parse((await app.storage())[PREFS_KEY]).keybindings["shell.palette"]).toEqual({ key: "mod+j" });
  await app.press("label:Settings");
  await app.keyboard.press("mod+k");
  expect(app.shows("label:Command search")).toBe(false);
  await app.keyboard.press("mod+j");
  await app.waitFor("label:Command search");
});

test("a reserved override disables only itself", async () => {
  const stored = JSON.stringify({ version: 1, keybindings: { "shell.settings": { key: "mod+q" } } });
  const app = await openPanel({ host: { handlers: gateway() }, storage: { [PREFS_KEY]: stored } });
  await app.waitFor("Gateway: healthy");
  await app.press("label:Settings");
  await app.waitUntil(() => app.texts().some((text) => text.startsWith("Reserved by the system")));
  await app.press("label:Settings");
  await app.keyboard.press("mod+k");
  await app.waitFor("label:Command search");
});

test("conflicting shortcuts are shown", async () => {
  const app = await openPanel({ host: { handlers: gateway() } });
  await app.waitFor("Gateway: healthy");
  await app.press("label:Settings");
  await app.type("label:Shortcut for Settings", "mod+k");
  await app.submit("label:Shortcut for Settings");
  await app.waitUntil(() => app.texts().some((text) => text.startsWith("Conflicts with")));
});

test("typing in chat does not trigger shortcuts", async () => {
  const { handlers } = chatGateway();
  const app = await openChat(handlers);
  await app.type("label:Message", "k,1");
  expect(app.shows("label:Command search")).toBe(false);
  expect(app.shows("Earlier answer")).toBe(true);
});

test("chat stop is in the palette only while chat is mounted", async () => {
  const { handlers } = chatGateway();
  const app = await openChat(handlers);
  await app.keyboard.press("mod+k");
  await app.type("label:Command search", "stop");
  await app.waitFor("Stop reply");
  await app.keyboard.press("escape");
  await app.waitFor("label:Command search", { state: "gone" });
  await app.press("label:Open Overview");
  await app.waitFor("Gateway: healthy");
  await app.keyboard.press("mod+k");
  await app.type("label:Command search", "stop");
  await app.waitFor("No matching command");
});

test("a module command can be rebound in Settings, which unmounts the module", async () => {
  const { handlers } = chatGateway();
  const app = await openChat(handlers);
  await app.press("label:Settings");
  await app.type("label:Shortcut for Stop reply", "mod+j");
  await app.submit("label:Shortcut for Stop reply");
  await app.waitFor("⌘J");
  expect(JSON.parse((await app.storage())[PREFS_KEY]).keybindings["aleph:chat/stop"]).toEqual({ key: "mod+j" });
  await app.press("label:Open Sessions");
  await app.press("label:Open chat Release notes");
  await app.waitFor("Earlier answer");
  await app.type("label:Message", "long task");
  await app.press("Send");
  await app.keyboard.press("mod+j");
  await app.waitUntil(() => rpcParams(app, "chat.abort").length === 1);
});
