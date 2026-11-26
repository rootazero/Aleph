import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { webcrypto } from "node:crypto";
import vm from "node:vm";
import test from "node:test";

const source = readFileSync(new URL("../src/panel_bridge.js", import.meta.url), "utf8");
function host({ element = true, origin = "http://127.0.0.1:18790", subframe = false } = {}) {
  const listeners = new Map();
  const timers = new Map();
  const sent = [];
  let timer = 0;
  const window = {
    addEventListener(name, callback) { if (!listeners.has(name)) listeners.set(name, []); listeners.get(name).push(callback); },
    nsh: { postMessage(raw) { assert.ok(raw.startsWith("prefix:")); sent.push(JSON.parse(raw.slice(7))); } },
  };
  window.top = subframe ? {} : window;
  const context = {
    window, location: { origin }, crypto: webcrypto,
    document: { documentElement: element ? { setAttribute() {} } : null, addEventListener: window.addEventListener },
    setTimeout(fn) { timers.set(++timer, fn); return timer; },
    clearTimeout(id) { timers.delete(id); },
  };
  vm.runInNewContext(source + '\ninstallAlephShellBridge("http://127.0.0.1:18790", "prefix:");', context);
  const emit = (name, event) => (listeners.get(name) ?? []).forEach(callback => callback(event));
  const bridge = window.__alephShellBridge;
  const receive = (message, options = {}) => emit("message", { source: null, origin: "", data: JSON.stringify(message), ...options });
  return { window, bridge, sent, timers, emit, receive, ready() { receive({ kind: "credentials-ready", document: bridge.document }); } };
}
const tick = async () => { await Promise.resolve(); await Promise.resolve(); };

test("preload works before documentElement and marks native immediately", () => {
  const h = host({ element: false });
  assert.ok(h.bridge);
  assert.equal(h.window.__ALEPH_PANEL_KEYRING__, true);
  assert.equal(h.window.__TAURI_INTERNALS__, undefined);
  assert.ok(Object.isFrozen(h.bridge));
});

test("credential load waits for document probe and returns a typed lease", async () => {
  const h = host();
  const pending = h.bridge.invoke("panel_credential_load", null);
  await tick();
  assert.equal(h.sent.length, 0);
  h.ready(); await tick();
  assert.equal(h.sent.length, 1);
  assert.deepEqual(h.sent[0].call, { operation: "load" });
  const lease = { gateway_url: "ws://127.0.0.1:18790/ws", lease_id: 1, credential: null };
  h.receive({ kind: "credential-reply", document: h.bridge.document, request: h.sent[0].request, reply: { outcome: "loaded", lease } });
  assert.deepEqual(JSON.parse(JSON.stringify(await pending)), lease);
  assert.equal(h.timers.size, 0);
});

test("saved reply resolves to null rather than becoming an absent value", async () => {
  const h = host(); h.ready();
  const pending = h.bridge.invoke("panel_credential_save", { lease_id: 1, credential: { device_id: "d", device_token: "t" } });
  await tick();
  h.receive({ kind: "credential-reply", document: h.bridge.document, request: h.sent[0].request, reply: { outcome: "saved" } });
  assert.equal(await pending, null);
});

test("fixed commands reject unknown fields and cannot name another gateway", async () => {
  const h = host(); h.ready();
  for (const [command, args] of [
    ["exec", null], ["panel_credential_load", { gateway_url: "wss://other/ws" }],
    ["panel_credential_delete", { lease_id: 1, role: "owner" }],
    ["panel_credential_save", { lease_id: 1, credential: { device_id: "d", device_token: "t", role: "owner" } }],
  ]) await assert.rejects(h.bridge.invoke(command, args), /credential/);
  assert.equal(h.sent.length, 0);
});

test("one pending operation bounds admission and missing reply times out", async () => {
  const h = host(); h.ready();
  const first = h.bridge.invoke("panel_credential_load", null);
  await assert.rejects(h.bridge.invoke("panel_credential_load", null), /credential/);
  await tick(); assert.equal(h.sent.length, 1);
  const rejected = assert.rejects(first, /credential/);
  for (const fn of [...h.timers.values()]) fn();
  await rejected;
  assert.equal(h.timers.size, 0);
});

test("wrong document or page-forged native reply cannot settle a request", async () => {
  const h = host(); h.ready();
  let settled = false;
  const pending = h.bridge.invoke("panel_credential_delete", { lease_id: 1 });
  pending.then(() => settled = true, () => settled = true);
  await tick();
  const message = { kind: "credential-reply", document: h.bridge.document, request: h.sent[0].request, reply: { outcome: "deleted", existed: true } };
  h.receive({ ...message, document: "0".repeat(32) });
  h.receive(message, { source: h.window });
  h.receive(message, { origin: "https://other" });
  await tick(); assert.equal(settled, false);
  h.receive(message); assert.equal(await pending, true);
});

test("pagehide revokes pending and future calls", async () => {
  const h = host();
  const pending = h.bridge.invoke("panel_credential_load", null);
  const rejected = assert.rejects(pending, /credential/);
  h.emit("pagehide", {});
  await rejected;
  h.ready(); await tick(); assert.equal(h.sent.length, 0);
  await assert.rejects(h.bridge.invoke("panel_credential_load", null), /credential/);
  assert.equal(h.timers.size, 0);
});

test("status read waits for probe and preserves requires approval, without credential contention", async () => {
  const h = host();
  const credential = h.bridge.invoke("panel_credential_load", null);
  const status = h.bridge.autostartStatus();
  await tick(); assert.equal(h.sent.length, 0);
  h.ready(); await tick();
  assert.equal(h.sent.length, 2);
  const request = h.sent.find(message => message.kind === "autostart-request");
  assert.deepEqual(Object.keys(request).sort(), ["document", "kind", "request"]);
  h.receive({ kind: "autostart-reply", document: h.bridge.document, request: request.request, status: { status: "requires_approval" } });
  assert.deepEqual(JSON.parse(JSON.stringify(await status)), { status: "requires_approval" });
  const load = h.sent.find(message => message.kind === "credential-request");
  h.receive({ kind: "credential-reply", document: h.bridge.document, request: load.request, reply: { outcome: "loaded", lease: { credential: null } } });
  await credential;
  assert.equal(h.timers.size, 0);
});

test("status has bounded admission, timeout and pagehide revocation", async () => {
  for (const stop of ["timeout", "pagehide"]) {
    const h = host(); h.ready();
    const pending = h.bridge.autostartStatus();
    const rejected = assert.rejects(pending, /status/);
    await assert.rejects(h.bridge.autostartStatus(), /status/);
    assert.equal(h.sent.length, 1);
    if (stop === "timeout") for (const fn of [...h.timers.values()]) fn();
    else h.emit("pagehide", {});
    await rejected; assert.equal(h.timers.size, 0);
    if (stop === "pagehide") await assert.rejects(h.bridge.autostartStatus(), /status/);
  }
});

test("status replies require matching native document, request and a closed status shape", async () => {
  const h = host(); h.ready();
  const pending = h.bridge.autostartStatus();
  let settled = false;
  pending.then(() => settled = true, () => settled = true);
  const message = { kind: "autostart-reply", document: h.bridge.document, request: h.sent[0].request, status: { status: "unavailable", reason: "not_found" } };
  h.receive({ ...message, document: "0".repeat(32) });
  h.receive({ ...message, request: message.request + 1 });
  h.receive(message, { source: h.window });
  h.receive(message, { origin: "https://other" });
  await tick(); assert.equal(settled, false);
  h.receive(message); assert.deepEqual(JSON.parse(JSON.stringify(await pending)), message.status);
  for (const status of [false, { status: "unknown" }, { status: "disabled", extra: true }, { status: "unavailable", reason: "unknown" }]) {
    const p = h.bridge.autostartStatus(); const rejected = assert.rejects(p, /status/);
    h.receive({ ...message, request: h.sent.at(-1).request, status });
    await rejected;
  }
});

test("untrusted origin and iframe have no bridge", () => {
  assert.equal(host({ origin: "https://other" }).bridge, undefined);
  assert.equal(host({ subframe: true }).bridge, undefined);
});

test("autostartSet sends the wanted state and resolves with the re-read status", async () => {
  const h = host(); h.ready();
  const pending = h.bridge.autostartSet(true);
  await tick();
  assert.equal(h.sent.length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(h.sent[0])),
    { kind: "autostart-set", request: h.sent[0].request, document: h.bridge.document, enabled: true });
  h.receive({ kind: "autostart-reply", document: h.bridge.document, request: h.sent[0].request, status: { status: "enabled" } });
  assert.deepEqual(JSON.parse(JSON.stringify(await pending)), { status: "enabled" });
  assert.equal(h.timers.size, 0);
});

test("autostartSet rejects a non-boolean without sending", async () => {
  const h = host(); h.ready();
  for (const value of ["true", 1, null, undefined]) {
    await assert.rejects(h.bridge.autostartSet(value));
  }
  await assert.rejects(h.bridge.autostartSet(true, "extra"));
  assert.equal(h.sent.length, 0);
});
