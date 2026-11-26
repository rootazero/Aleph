// The SDK's generation and dispose rules, against a stand-in shell.
import { expect, test } from "nsh/test";
import { connectionChanged, createPanelRpc, type PanelRpcEnvelope, readConnection } from "./panelRpc";
import { type CommandSpec, type ConnectionState, createSdk, type PanelEvent, type ShellAccess } from "./sdk";

const BINDING = "real-socket-1";
const CONNECTED: ConnectionState = { state: "connected", endpoint: "https://aleph.test", subject: null, pairing: false, voice_binding: BINDING };

function standIn() {
  let generation = 1;
  let connection = CONNECTED;
  const calls: { topic: string; envelope: PanelRpcEnvelope }[] = [];
  const listeners = new Set<(state: ConnectionState) => void>();
  const events = new Set<(event: PanelEvent) => void>();
  const pending: { resolve(value: unknown): void; reject(error: Error): void }[] = [];
  let abortBinding: string | undefined;
  const commands = new Map<string, CommandSpec>();
  const shell: ShellAccess = {
    rpc: createPanelRpc((topic, envelope) => {
      calls.push({ topic, envelope });
      const { method, voice_binding } = envelope;
      if (voice_binding === undefined || (method !== "chat.abort" && method !== "chat.send")) {
        return new Promise((resolve, reject) => pending.push({ resolve, reject }));
      }
      if (method === "chat.abort" && abortBinding === voice_binding) {
        return Promise.resolve({ error: { code: -32090, message: "Physical abort slot is occupied" } });
      }
      if (method === "chat.abort") abortBinding = voice_binding;
      const clearAbortSlot = () => {
        if (method === "chat.abort" && abortBinding === voice_binding) abortBinding = undefined;
      };
      return new Promise((resolve, reject) => pending.push({
        resolve(value) { clearAbortSlot(); resolve(value); },
        reject(error) { clearAbortSlot(); reject(error); },
      }));
    }),
    generation: () => generation,
    connection: () => connection,
    navigate: () => {},
    intent: () => null,
    onConnection(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    onEvent(listener) {
      events.add(listener);
      return () => events.delete(listener);
    },
    registerCommand(spec) {
      commands.set(spec.id, spec);
      return () => commands.delete(spec.id);
    },
  };
  return {
    shell,
    calls,
    commands,
    listeners,
    events,
    push: (event: PanelEvent) => {
      for (const listener of [...events]) listener(event);
    },
    answer: (value: unknown) => pending.shift()?.resolve({ result: value }),
    rejectByHost: () => pending.shift()?.resolve({ error: { code: -32090, message: "Binding no longer live" } }),
    changeBinding(value: unknown, notify = true) {
      connection = readConnection({ ...connection, voice_binding: value })!;
      if (notify) for (const listener of [...listeners]) listener(connection);
    },
    fail: (message = "request failed") => pending.shift()?.reject(new Error(message)),
    reconnect: () => {
      generation += 1;
      for (const listener of [...listeners]) listener(connection);
    },
    disconnect: () => {
      generation += 1;
      connection = { ...CONNECTED, state: "disconnected" };
      for (const listener of [...listeners]) listener(connection);
    },
  };
}

test("invoke after dispose rejects", async () => {
  const { shell } = standIn();
  const sdk = createSdk("aleph:overview", 1, shell);
  sdk.dispose();
  await expect(sdk.invoke("health")).rejects.toThrow("disposed");
});

test("a late result after dispose is dropped", async () => {
  const fake = standIn();
  const sdk = createSdk("aleph:sessions", 1, fake.shell);
  const call = sdk.invoke("sessions.list");
  sdk.dispose();
  fake.answer({ sessions: [] });
  await expect(call).rejects.toThrow("disposed");
});

test("a result from an older generation is dropped", async () => {
  const fake = standIn();
  const sdk = createSdk("aleph:sessions", 1, fake.shell);
  const call = sdk.invoke("sessions.list");
  fake.reconnect();
  fake.answer({ sessions: [] });
  await expect(call).rejects.toThrow("disposed");
});

test("a rejection from an older generation is dropped", async () => {
  const fake = standIn();
  const sdk = createSdk("aleph:memory-nebula", 1, fake.shell);
  const call = sdk.invoke("graph.node_detail");
  fake.reconnect();
  fake.fail();
  await expect(call).rejects.toThrow("disposed");
});

test("dispose releases every subscription", () => {
  const fake = standIn();
  const sdk = createSdk("aleph:overview", 1, fake.shell);
  const heard: ConnectionState[] = [];
  sdk.subscribe((state) => heard.push(state));
  sdk.subscribe((state) => heard.push(state));
  expect(fake.listeners.size).toBe(2);
  sdk.dispose();
  expect(fake.listeners.size).toBe(0);
  expect(() => sdk.subscribe(() => {})).toThrow("disposed");
});

test("live events reach a module by topic prefix", () => {
  const fake = standIn();
  const sdk = createSdk("aleph:chat", 1, fake.shell);
  const heard: string[] = [];
  sdk.on("stream.", (event) => heard.push(event.topic));
  fake.push({ topic: "stream.response_chunk", data: {} });
  fake.push({ topic: "connection.warning", data: {} });
  expect(heard).toEqual(["stream.response_chunk"]);
});

test("live events from an older generation or after dispose are dropped", () => {
  const fake = standIn();
  const sdk = createSdk("aleph:chat", 1, fake.shell);
  const heard: string[] = [];
  const release = sdk.on("", (event) => heard.push(event.topic));
  fake.reconnect();
  fake.push({ topic: "stream.run_complete", data: {} });
  expect(heard).toEqual([]);
  release();
  expect(fake.events.size).toBe(0);
  sdk.on("", () => {});
  sdk.dispose();
  expect(fake.events.size).toBe(0);
  expect(() => sdk.on("", () => {})).toThrow("disposed");
});

test("module command ids carry the tab key", () => {
  const fake = standIn();
  const sdk = createSdk("aleph:chat", 1, fake.shell);
  sdk.commands.register({ id: "stop", label: "Stop reply", run: () => {} });
  expect([...fake.commands.keys()]).toEqual(["aleph:chat/stop"]);
});

test("module commands unregister on dispose", () => {
  const fake = standIn();
  const sdk = createSdk("aleph:chat", 1, fake.shell);
  sdk.commands.register({ id: "stop", label: "Stop reply", run: () => {} });
  sdk.dispose();
  expect(fake.commands.size).toBe(0);
  expect(() => sdk.commands.register({ id: "stop", label: "Stop reply", run: () => {} })).toThrow("disposed");
});

test("module commands from an older generation do not run", () => {
  const fake = standIn();
  const sdk = createSdk("aleph:chat", 1, fake.shell);
  let runs = 0;
  sdk.commands.register({ id: "stop", label: "Stop reply", run: () => (runs += 1) });
  fake.commands.get("aleph:chat/stop")?.run();
  fake.reconnect();
  fake.commands.get("aleph:chat/stop")?.run();
  expect(runs).toBe(1);
});

test("sendOwnedChat aborts only its valid receipt after same-generation disposal", async () => {
  const fake = standIn();
  const sdk = createSdk("aleph:voice", 1, fake.shell);
  const send = sdk.sendOwnedChat({ message: "voice" });
  send.abort();
  sdk.dispose();
  const run = { run_id: "owned", session_key: "agent:main:main@1" };
  fake.answer({ ...run, extra: "no" });
  await expect(send.receipt).rejects.toThrow("disposed");
  send.abort();
  expect(fake.calls).toEqual([
    { topic: "aleph.rpc", envelope: { method: "chat.send", params: { message: "voice", run_only: true }, voice_binding: BINDING } },
    { topic: "aleph.rpc", envelope: { method: "chat.abort", params: { run_id: run.run_id, run_only: true }, voice_binding: BINDING } },
  ]);
});

test("sendOwnedChat cannot abort malformed receipts or cross generations", async () => {
  for (const loseConnection of [false, true]) {
    const fake = standIn();
    const sdk = createSdk("aleph:voice", 1, fake.shell);
    const send = sdk.sendOwnedChat({ message: "voice" });
    send.abort();
    if (loseConnection) fake.disconnect();
    fake.answer({ run_id: "owned", ...(loseConnection ? { session_key: "agent:main:main@1" } : {}) });
    await expect(send.receipt).rejects.toThrow(loseConnection ? "disposed" : "Invalid chat.send receipt");
    expect(fake.calls.map(({ envelope }) => envelope.method)).toEqual(["chat.send"]);
    sdk.dispose();
  }
});

test("sendOwnedChat shares one waiting slot across tab remounts and drops a disposed waiter", async () => {
  const fake = standIn();
  const first = createSdk("aleph:voice", 1, fake.shell);
  const run = { run_id: "A", session_key: "shared-session" };
  const a = first.sendOwnedChat({ message: "A" });
  fake.answer(run);
  await a.receipt;
  first.dispose();

  const second = createSdk("aleph:voice", 1, fake.shell);
  const b = second.sendOwnedChat({ message: "B" });
  expect(b.dispatched()).toBe(false);
  const third = createSdk("aleph:voice", 1, fake.shell);
  expect(() => third.sendOwnedChat({ message: "overflow" })).toThrow("already has a waiting send");
  second.dispose();
  await expect(b.receipt).rejects.toThrow("cancelled before dispatch");
  const c = third.sendOwnedChat({ message: "C" });
  expect(c.dispatched()).toBe(false);
  fake.answer({ run_id: "A", aborted: true, dropped: 0 });
  await Promise.resolve();
  await Promise.resolve();
  expect(c.dispatched()).toBe(true);
  expect(fake.calls.map(({ envelope }) => envelope.params)).toEqual([
    { message: "A", run_only: true }, { run_id: "A", run_only: true }, { message: "C", run_only: true },
  ]);
  fake.answer({ ...run, run_id: "C" });
  await c.receipt;
  c.release();
  third.dispose();
  expect(fake.listeners.size + fake.events.size).toBe(0);
});

test("sendOwnedChat keeps ambiguous send and abort results fenced through the live binding", async () => {
  for (const failure of ["send", "abort", "wrong ack", "negative ack"]) {
    const fake = standIn();
    const first = createSdk("aleph:voice", 1, fake.shell);
    const a = first.sendOwnedChat({ message: "A" });
    if (failure === "send") {
      fake.fail();
      await expect(a.receipt).rejects.toThrow("request failed");
      first.dispose();
    } else {
      fake.answer({ run_id: "A", session_key: "shared-session" });
      await a.receipt;
      first.dispose();
      if (failure === "abort") fake.fail();
      else fake.answer({ run_id: failure === "wrong ack" ? "B" : "A", aborted: failure !== "negative ack", dropped: 0 });
    }
    const second = createSdk("aleph:voice", 1, fake.shell);
    const b = second.sendOwnedChat({ message: "B" });
    for (let i = 0; i < 8; i++) await Promise.resolve();
    expect(b.dispatched()).toBe(false);
    expect(fake.calls.filter(({ envelope }) => envelope.method === "chat.send").length).toBe(1);

    fake.changeBinding("replacement");
    await expect(b.receipt).rejects.toThrow("disposed");
    const replacement = createSdk("aleph:voice", 1, fake.shell);
    const c = replacement.sendOwnedChat({ message: "C" });
    expect(c.dispatched()).toBe(true);
    expect(fake.calls.filter(({ envelope }) => envelope.method === "chat.send")
      .map(({ envelope }) => envelope.params.message)).toEqual(["A", "C"]);
    fake.answer({ run_id: "C", session_key: "shared-session" });
    await c.receipt;
    c.release();
    second.dispose();
    replacement.dispose();
  }
});

async function abandonedAtAbortSlot() {
  const fake = standIn();
  const sdk = createSdk("aleph:voice", 1, fake.shell);
  const a = sdk.sendOwnedChat({ message: "A" });
  a.abort();
  const b = sdk.sendOwnedChat({ message: "B" });
  const run = { run_id: "A", session_key: "shared-session" };
  fake.push({ topic: "stream.run_complete", data: { ...run, session_key: "wrong-session" } });
  fake.answer(run);
  await a.receipt;
  const methods = () => fake.calls.map(({ envelope }) => envelope.method);
  expect(methods()).toEqual(["chat.send", "chat.abort"]);
  const terminal = () => fake.push({ topic: "stream.run_error", data: run });
  async function finishB() {
    expect(methods()).toEqual(["chat.send", "chat.abort", "chat.send"]);
    fake.answer({ ...run, run_id: "B" });
    expect((await b.receipt).run_id).toBe("B");
    b.abort();
    expect(methods()).toEqual(["chat.send", "chat.abort", "chat.send", "chat.abort"]);
    expect(fake.calls.at(-1)?.envelope.params).toEqual({ run_id: "B", run_only: true });
    fake.answer({ run_id: "B", aborted: true, dropped: 0 });
    await flushRpc();
    b.release();
    sdk.dispose();
  }
  return { fake, a, b, sdk, run, methods, terminal, finishB };
}

async function flushRpc() {
  for (let i = 0; i < 4; i++) await Promise.resolve();
}

test("sendOwnedChat keeps B off the occupied physical abort slot even after A's terminal", async () => {
  const f = await abandonedAtAbortSlot();
  f.terminal();
  expect(f.methods()).toEqual(["chat.send", "chat.abort"]);
  f.a.release();
  expect(f.methods()).toEqual(["chat.send", "chat.abort"]);
  f.fake.answer({ run_id: "A", aborted: true, dropped: 0 });
  await flushRpc();
  await f.finishB();
});

for (const terminalFirst of [true, false]) {
  test(`sendOwnedChat requires both a matching terminal and false abort ACK (${terminalFirst ? "terminal first" : "ACK first"})`, async () => {
    const f = await abandonedAtAbortSlot();
    const answer = () => f.fake.answer({ run_id: "A", aborted: false, dropped: 0 });
    if (terminalFirst) f.terminal();
    else answer();
    await flushRpc();
    f.fake.push({ topic: "stream.run_complete", data: { ...f.run, run_id: "wrong-run" } });
    f.fake.push({ topic: "stream.run_complete", data: { ...f.run, session_key: "wrong-session" } });
    expect(f.methods()).toEqual(["chat.send", "chat.abort"]);
    if (terminalFirst) answer();
    else f.terminal();
    await flushRpc();
    await f.finishB();
  });
}

for (const [failure, ack] of [
  ["wrong run id", { run_id: "B", aborted: true, dropped: 0 }],
  ["nonzero dropped", { run_id: "A", aborted: true, dropped: 1 }],
  ["missing dropped", { run_id: "A", aborted: true }],
  ["nonboolean aborted", { run_id: "A", aborted: "true", dropped: 0 }],
  ["rejection", undefined],
  ["timeout", undefined],
] as const) {
  test(`sendOwnedChat stays fenced on abort ${failure}, even with A's terminal`, async () => {
    const f = await abandonedAtAbortSlot();
    if (ack) f.fake.answer(ack);
    else if (failure === "timeout") f.fake.fail("Host request timed out");
    else f.fake.rejectByHost();
    await flushRpc();
    expect(f.methods()).toEqual(["chat.send", "chat.abort"]);
    f.terminal();
    await flushRpc();
    expect(f.methods()).toEqual(["chat.send", "chat.abort"]);
    f.fake.changeBinding("replacement");
    await expect(f.b.receipt).rejects.toThrow("disposed");
    f.sdk.dispose();
  });
}

test("sendOwnedChat can correlate a detached early terminal after the late receipt", async () => {
  const fake = standIn();
  const sdk = createSdk("aleph:voice", 1, fake.shell);
  const a = sdk.sendOwnedChat({ message: "A" });
  a.abort();
  const b = sdk.sendOwnedChat({ message: "B" });
  const run = { run_id: "A", session_key: "shared-session" };
  fake.push({ topic: "stream.run_complete", data: { run_id: "A" } });
  expect(b.dispatched()).toBe(false);
  fake.answer(run);
  await a.receipt;
  expect(b.dispatched()).toBe(true);
  expect(fake.calls.map(({ envelope }) => envelope.method)).toEqual(["chat.send", "chat.send"]);
  fake.answer({ ...run, run_id: "B" });
  await b.receipt;
  b.release();
  sdk.dispose();
});

test("onInvalidate delivers generation loss once without lifting subscribe isolation", () => {
  const fake = standIn();
  const sdk = createSdk("aleph:voice", 1, fake.shell);
  const heard: string[] = [];
  sdk.subscribe(() => heard.push("unexpected"));
  sdk.onInvalidate(() => heard.push("invalidated"));
  fake.reconnect();
  fake.reconnect();
  sdk.dispose();
  expect(heard).toEqual(["invalidated"]);
  expect(fake.listeners.size).toBe(0);
});

test("physical connection identity changes with the host binding, even for the same account and endpoint", () => {
  const replacement = readConnection({ ...CONNECTED, voice_binding: "new socket" })!;
  expect(connectionChanged(CONNECTED, replacement)).toBe(true);
  expect(connectionChanged(CONNECTED, readConnection({ ...CONNECTED, pairing: true })!)).toBe(false);
  expect(readConnection({ ...CONNECTED, state: "disconnected" })?.voice_binding).toBe(undefined);
});

test("invokeVoice uses only the pinned host envelope and ordinary invoke stays unbound", async () => {
  const fake = standIn();
  const sdk = createSdk("aleph:voice", 1, fake.shell);
  const audio = sdk.invokeVoice("voice.turn.capture", { capture_id: "clip" });
  fake.answer({ capture_id: "clip" });
  await audio;
  const chat = sdk.invoke("chat.send", { message: "typed chat" });
  fake.answer({});
  await chat;
  expect(fake.calls).toEqual([
    { topic: "aleph.rpc", envelope: { method: "voice.turn.capture", params: { capture_id: "clip" }, voice_binding: BINDING } },
    { topic: "aleph.rpc", envelope: { method: "chat.send", params: { message: "typed chat" } } },
  ]);
});

test("missing or malformed host binding cannot dispatch or be acquired by an existing SDK", async () => {
  for (const binding of [undefined, null, "", "   ", 42]) {
    const fake = standIn();
    fake.changeBinding(binding);
    const sdk = createSdk("aleph:voice", 1, fake.shell);
    expect(sdk.voiceActive()).toBe(false);
    await expect(sdk.invokeVoice("voice.turn.cancel", { turn_id: "turn" })).rejects.toThrow("Voice connection unavailable");
    expect(() => sdk.sendOwnedChat({ message: "voice" })).toThrow("Voice connection unavailable");
    fake.changeBinding(BINDING);
    expect(sdk.voiceActive()).toBe(false);
    expect(fake.calls).toEqual([]);
  }
});

test("binding mismatch invalidates voice and drops a delayed command and detached send receipt", async () => {
  const fake = standIn();
  const sdk = createSdk("aleph:voice", 1, fake.shell);
  const heard: string[] = [];
  sdk.onInvalidate(() => heard.push("binding changed"));
  const voice = sdk.invokeVoice("voice.turn.synthesize", { turn_id: "turn", text: "final" });
  const send = sdk.sendOwnedChat({ message: "voice" });
  send.abort();
  fake.changeBinding("replacement-same-endpoint-and-subject");
  sdk.dispose();
  const voiceRejected = expect(voice).rejects.toThrow("Voice connection unavailable");
  const receiptRejected = expect(send.receipt).rejects.toThrow("disposed");
  fake.answer({ audio_base64: "d2F2", mime_type: "audio/wav" });
  fake.answer({ run_id: "owned", session_key: "agent:main:main@1" });
  await voiceRejected;
  await receiptRejected;
  expect(heard).toEqual(["binding changed"]);
  expect(fake.calls.map(({ envelope }) => [envelope.method, envelope.voice_binding])).toEqual([
    ["voice.turn.synthesize", BINDING], ["chat.send", BINDING],
  ]);
});

test("a changed binding suppresses abort even if a shell notification was missed", async () => {
  const fake = standIn();
  const sdk = createSdk("aleph:voice", 1, fake.shell);
  const send = sdk.sendOwnedChat({ message: "voice" });
  fake.answer({ run_id: "owned", session_key: "agent:main:main@1" });
  await send.receipt;
  fake.changeBinding("replacement", false);
  sdk.dispose();
  send.abort();
  expect(fake.calls.map(({ envelope }) => envelope.method)).toEqual(["chat.send"]);
});

test("a host rejection while Shell still sees the old socket never retries or grants abort", async () => {
  const fake = standIn();
  const sdk = createSdk("aleph:voice", 1, fake.shell);
  const send = sdk.sendOwnedChat({ message: "voice" });
  // The host has a replacement socket; its notification has not reached TSX.
  fake.rejectByHost();
  await expect(send.receipt).rejects.toThrow("Binding no longer live");
  sdk.dispose();
  send.abort();
  expect(fake.calls).toEqual([
    { topic: "aleph.rpc", envelope: { method: "chat.send", params: { message: "voice", run_only: true }, voice_binding: BINDING } },
  ]);
});
