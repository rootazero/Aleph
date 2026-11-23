// The SDK's generation and dispose rules, against a stand-in shell.
import { expect, test } from "nsh/test";
import { type ConnectionState, createSdk, type ShellAccess } from "./sdk";

const CONNECTED: ConnectionState = { state: "connected", endpoint: "https://aleph.test", subject: null, pairing: false };

function standIn() {
  let generation = 1;
  const listeners = new Set<(state: ConnectionState) => void>();
  const pending: ((value: unknown) => void)[] = [];
  const shell: ShellAccess = {
    rpc: () => new Promise((resolve) => pending.push(resolve)),
    generation: () => generation,
    connection: () => CONNECTED,
    navigate: () => {},
    onConnection(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
  return {
    shell,
    listeners,
    answer: (value: unknown) => pending.shift()?.(value),
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
