// The SDK's generation and dispose rules, against a stand-in shell.
import { expect, test } from "nsh/test";
import { type ConnectionState, createSdk, type PanelEvent, type ShellAccess } from "./sdk";

const CONNECTED: ConnectionState = { state: "connected", endpoint: "https://aleph.test", subject: null, pairing: false };

function standIn() {
  let generation = 1;
  const listeners = new Set<(state: ConnectionState) => void>();
  const events = new Set<(event: PanelEvent) => void>();
  const pending: { resolve(value: unknown): void; reject(error: Error): void }[] = [];
  const shell: ShellAccess = {
    rpc: () => new Promise((resolve, reject) => pending.push({ resolve, reject })),
    generation: () => generation,
    connection: () => CONNECTED,
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
  };
  return {
    shell,
    listeners,
    events,
    push: (event: PanelEvent) => {
      for (const listener of [...events]) listener(event);
    },
    answer: (value: unknown) => pending.shift()?.resolve(value),
    fail: () => pending.shift()?.reject(new Error("request failed")),
    reconnect: () => {
      generation += 1;
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
