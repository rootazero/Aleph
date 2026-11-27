import { expect, open, test } from "nsh/test";

const CONNECTED = {
  state: "connected", endpoint: "https://aleph.test", subject: "0123456789abcdef0123456789abcdef", pairing: false,
  voice_binding: "01234567-89ab-4def-8123-456789abcdef",
};
const VOICE_TAB = {
  descriptor: {
    key: { plugin_id: "aleph", tab_id: "voice" },
    label: "Voice", icon: "mic", order: 10, entry: "shell/voice", api_major: 1,
    trust: "verified", permissions: [], status: "enabled", status_detail: null,
  },
  origin: "builtin", plugin_version: "1.0.0", asset_integrity: null, public_base: "shell/", source_revision: 1,
};
const START = "label:Start voice conversation";
const STOP = "label:Stop voice conversation";
const TRANSCRIPT = "Fixture transcript never rendered";
const CHUNK = "Fixture chunk never rendered";
const REPLY = "Fixture final answer never rendered";
const SYNTHESIS = { audio_base64: "UklGRg==", mime_type: "audio/wav" };

type Params = Record<string, unknown>;
function fixture({ declared = true, available = true } = {}) {
  const captured: unknown[] = [];
  const transcribed: Params[] = [];
  const sent: Params[] = [];
  const activeRuns = new Set<unknown>();
  const synthesized: Params[] = [];
  const played: Params[] = [];
  const methods: Record<string, (params: Params) => unknown> = {
    health: () => ({ status: "healthy" }),
    "panel.capabilities.snapshot": () => ({
      revision: 1, declarations: declared ? [VOICE_TAB] : [], project_context: { project_id: null, generation: 0 },
    }),
    "voice.turn.capture": ({ capture_id }) => {
      captured.push(capture_id);
      return { capture_id, limit_reached: false };
    },
    "voice.turn.capture_ready": () => ({ ready: true }),
    "voice.turn.transcribe": (params) => {
      transcribed.push(params);
      return { text: TRANSCRIPT };
    },
    "chat.send": (params) => {
      sent.push(params);
      const run_id = `voice-run-${sent.length}`;
      activeRuns.add(run_id);
      return { run_id, session_key: "voice-session" };
    },
    "chat.abort": ({ run_id, run_only, ...other }) => {
      expect(run_only).toBe(true);
      expect(other).toEqual({});
      return { run_id, aborted: activeRuns.delete(run_id), dropped: 0 };
    },
    "voice.turn.synthesize": (params) => {
      synthesized.push(params);
      return SYNTHESIS;
    },
  };
  const handlers = {
    "aleph.connection": () => CONNECTED,
    "aleph.voice.availability": () => ({ capture: available, playback: true, capture_transport: "desktop" }),
    "aleph.voice.play": (data: Params) => {
      played.push(data);
      throw new Error("Fixture device did not start playback");
    },
    "aleph.voice.stop": () => null,
    "aleph.rpc": async ({ method, params, voice_binding }: { method: string; params: Params; voice_binding?: string }) => {
      if ((method.startsWith("voice.turn.") || method === "chat.abort") && voice_binding !== CONNECTED.voice_binding) {
        return { error: { code: -32600, message: "Fixture requires a bound voice request" } };
      }
      return methods[method] ? { result: await methods[method](params) ?? null }
        : { error: { code: -32601, message: "Fixture method unavailable" } };
    },
  };
  const openPanel = () => open("../App.tsx", { platform: "macos", width: 1024, height: 768, host: { handlers } });
  return { openPanel, captured, transcribed, sent, activeRuns, synthesized, played };
}

async function openVoice(host: ReturnType<typeof fixture>) {
  const app = await host.openPanel();
  await app.waitFor("label:Open Voice");
  await app.press("label:Open Voice");
  expect(host.captured).toEqual([]);
  return app;
}

const terminal = (run_id: string) => ({
  topic: "stream.run_complete", data: { run_id, summary: { final_response: REPLY } },
});

test("Voice is mounted only from a Core declaration; host unavailability does not open the mic", async () => {
  const absent = fixture({ declared: false });
  const overview = await absent.openPanel();
  await overview.waitFor("Gateway: healthy");
  expect(overview.count("label:Open Voice")).toBe(0);
  overview.dispose();

  const unavailable = fixture({ available: false });
  const app = await openVoice(unavailable);
  await app.waitFor("Microphone and playback are not available in this host");
  expect(app.count(START)).toBe(0);
  expect(unavailable.captured).toEqual([]);
});

test("whole_turn_uses_final_summary_without_rendering_spoken_words", async () => {
  const host = fixture();
  const app = await openVoice(host);
  await app.waitFor("Ready to start");
  expect(host.sent).toEqual([]);
  await app.press(START);
  await app.waitFor("Waiting for reply");
  await app.waitUntil(() => host.sent.length === 1);
  expect(host.captured.length).toBe(1);
  const [id] = host.captured;
  expect(host.transcribed).toEqual([{ capture_id: id, turn_id: id }]);
  expect(host.sent[0]).toMatchObject({ message: TRANSCRIPT, voice_input: true, idempotency_key: id, run_only: true });
  expect(app.count(STOP)).toBe(1);

  await app.host.send("aleph.event", { topic: "stream.response_chunk", data: { run_id: "voice-run-1", delta: CHUNK } });
  expect(host.synthesized).toEqual([]);
  await app.host.send("aleph.event", terminal("voice-run-1"));
  await app.waitFor("Reply playback failed");
  expect(host.synthesized).toEqual([{ text: REPLY, turn_id: id }]);
  expect(host.played).toEqual([{ playback_id: id, ...SYNTHESIS }]);
  expect(app.host.calls().filter((call) => call.name === "aleph.rpc" && (
    (call.data as { method: string }).method.startsWith("voice.stream.") ||
    (call.data as { method: string }).method === "voice.format"
  ))).toEqual([]);
  for (const text of [TRANSCRIPT, CHUNK, REPLY]) {
    expect(app.shows(text)).toBe(false);
    expect(app.count(`label:${text}`)).toBe(0);
  }
  expect(host.captured.length).toBe(1);
});

test("Voice hide and module removal stop their owned run, without auto-start on show", async () => {
  const host = fixture();
  const app = await openVoice(host);
  await app.waitFor(START);
  await app.press(START);
  await app.waitUntil(() => host.activeRuns.has("voice-run-1"));
  await app.host.send("aleph.voice.visibility", { visible: false });
  await app.waitUntil(() => host.activeRuns.size === 0);
  expect(app.count(START)).toBe(0);
  await app.host.send("aleph.event", terminal("voice-run-1"));
  await app.host.send("aleph.voice.visibility", { visible: true });
  await app.waitFor(START);
  expect(host.captured.length).toBe(1);

  await app.press(START);
  await app.waitUntil(() => host.activeRuns.has("voice-run-2"));
  await app.press("label:Open Overview");
  await app.waitFor("Gateway: healthy");
  await app.waitUntil(() => host.activeRuns.size === 0);
  await app.host.send("aleph.event", terminal("voice-run-2"));
  expect(host.synthesized).toEqual([]);
  expect(host.played).toEqual([]);
  expect(app.count(START)).toBe(0);
});
