import test from "node:test";
import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile, access } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { validateHelperMessage } from "./native_project_protocol.mjs";
import { createRequire } from "node:module";
const WebSocket = createRequire(import.meta.url)("ws");

const binary = process.env.NATIVE_QA_GATEWAY_BIN ?? "target/debug/aleph-panel-native-qa-gateway.exe";

test("helper refuses an unowned root before creating fixture state", async () => {
  const root = await mkdtemp(join(tmpdir(), "aleph-shell-qa-"));
  try {
    const result = spawnSync(binary, ["--owned-root", root, "--nonce", "56".repeat(16), "--panel-root", root], { encoding: "utf8", timeout: 10_000 });
    assert.notEqual(result.status, 0);
    assert.equal(result.stdout.includes('"type":"ready"'), false);
    await assert.rejects(access(join(root, ".aleph")));
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("disposable Gateway helper authenticates and serves actor-bound project bytes", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "aleph-shell-qa-"));
  let socket;
  const panel = join(root, "panel");
  await mkdir(panel, { recursive: true });
  await writeFile(join(panel, "index.html"), "<!doctype html><html><body>qa</body></html>");
  const nonce = "56".repeat(16);
  await writeFile(join(root, ".qa-owned"), nonce);
  const child = spawn(binary, ["--owned-root", root, "--nonce", nonce, "--panel-root", panel], {
    stdio: ["pipe", "pipe", "pipe"],
  });
  t.after(async () => {
    socket?.terminate();
    if (child.exitCode === null) {
      const closed = new Promise((resolve) => child.once("close", resolve));
      child.kill();
      await Promise.race([closed, new Promise((resolve) => setTimeout(resolve, 2000))]);
    }
    if (child.exitCode !== null || child.signalCode !== null) await rm(root, { recursive: true, force: true });
  });
  const messages = [];
  let stdout = "";
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    stdout += chunk;
    for (;;) {
      const end = stdout.indexOf("\n");
      if (end < 0) break;
      messages.push(JSON.parse(stdout.slice(0, end)));
      stdout = stdout.slice(end + 1);
    }
  });
  const waitFor = async (predicate) => {
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline) {
      const message = messages.find(predicate);
      if (message) return message;
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    throw new Error("helper_timeout");
  };

  const ready = await waitFor((message) => message.type === "ready");
  assert.notEqual(ready.origin, "http://127.0.0.1:18790");
  child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id: 1, command: "issue_pairing", user: "alice", device: "device-a" })}\n`);
  const pairing = await waitFor((message) => message.type === "pairing");
  assert.match(pairing.ticket, /^aleph-bt-/);
  socket = new WebSocket(ready.origin.replace("http:", "ws:") + "/ws");
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { socket.terminate(); reject(new Error("ws_open_timeout")); }, 5000);
    socket.once("open", () => { clearTimeout(timer); resolve(); });
    socket.once("error", () => { clearTimeout(timer); reject(new Error("ws_open_failed")); });
    socket.once("unexpected-response", (_request, response) => {
      clearTimeout(timer);
      response.destroy();
      socket.terminate();
      reject(new Error(`ws_upgrade_status_${response.statusCode}`));
    });
  });
  let rpcId = 0;
  const rpc = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++rpcId;
    const receive = (event) => {
      const message = JSON.parse(event.data);
      if (message.id !== id) return;
      clearTimeout(timer);
      socket.removeEventListener("message", receive);
      if (message.error) reject(new Error("rpc_refused"));
      else resolve(message.result);
    };
    const timer = setTimeout(() => { socket.removeEventListener("message", receive); reject(new Error("rpc_timeout")); }, 5000);
    socket.addEventListener("message", receive);
    socket.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
  });
  const connected = await rpc("connect", { bootstrap_ticket: pairing.ticket, device_id: "smoke-native-device" });
  assert.equal(connected.authorized === true && connected.user_id === "alice", true, "real bootstrap connect must authenticate Alice");
  const token = connected.device_token;
  assert.equal(typeof token === "string" && token.startsWith("aleph-dt-"), true);
  const snapshot = await rpc("panel.capabilities.snapshot");
  assert.deepEqual(snapshot.declarations.map(row => row.descriptor.key.plugin_id), ["community-live"]);
  const projects = (await rpc("projects.list")).projects;
  const a = projects.find(project => project.name === "Native QA A");
  const b = projects.find(project => project.name === "Native QA B");
  assert.equal(Boolean(a && b), true);
  const select = (project) => rpc("panel.project.select", { project_id: project, idempotency_key: `native-smoke-${++rpcId}` });
  await select(a.id);
  const selectedA = await rpc("panel.capabilities.snapshot");
  const declaration = selectedA.declarations.find(row => row.descriptor.key.plugin_id === "project-a");
  assert.equal(Boolean(declaration), true);
  const assetUrl = new URL(declaration.descriptor.entry, `${ready.origin}/`).href;
  const headers = { Authorization: `Bearer ${token}` };
  const asset = await fetch(assetUrl, { headers, signal: AbortSignal.timeout(5000) });
  assert.equal(asset.status, 200);
  const module = await asset.text();
  assert.equal(module.includes("export function mount"), true);
  child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id: 18, command: "receipt_since", after: 0 })}\n`);
  assert.equal((await waitFor(message => message.type === "error")).code, "not_found", "an asset observation cannot establish a pairing binding");
  child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id: 19, command: "confirm_pairing", device: "device-a" })}\n`);
  await waitFor(message => message.type === "ack" && message.id === 19);
  assert.equal((await fetch(assetUrl, { headers, signal: AbortSignal.timeout(5000) })).status, 200);
  const readReceipt = async (after, predicate = () => true) => {
    let cursor = after;
    for (let attempt = 0; attempt < 100; attempt++) {
      const messageCursor = messages.length;
      child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id: 20 + attempt, command: "receipt_since", after: cursor })}\n`);
      const message = await waitFor(message => messages.indexOf(message) >= messageCursor && (message.type === "receipt" || message.type === "error"));
      if (message.type === "receipt") {
        cursor = message.seq;
        if (predicate(message)) return message;
        continue;
      }
      assert.equal(message.code, "not_found");
      await new Promise(resolve => setTimeout(resolve, 20));
    }
    throw Error("receipt_timeout");
  };
  const connectReceipt = await readReceipt(0, message => message.operation === "connect");
  assert.equal(validateHelperMessage(connectReceipt).ok, true);
  assert.equal(connectReceipt.device, "device-a");
  assert.equal(connectReceipt.user, "alice");
  assert.equal(connectReceipt.project, "global");
  assert.equal(connectReceipt.result, "authorized");
  const globalDiscovery = await readReceipt(connectReceipt.seq, message => message.operation === "discovery" && message.project === "global");
  const selectReceipt = await readReceipt(globalDiscovery.seq, message => message.operation === "select" && message.project === "project-a");
  assert.equal(selectReceipt.result, "authorized");
  const projectDiscovery = await readReceipt(selectReceipt.seq, message => message.operation === "discovery" && message.project === "project-a");
  const receipt = await readReceipt(projectDiscovery.seq, message => message.operation === "asset" && message.project === "project-a");
  assert.equal(receipt.device, "device-a");
  assert.equal(receipt.user, "alice");
  assert.equal(receipt.revision, BigInt(selectedA.revision).toString(16).padStart(16, "0"));

  child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id: 30, command: "hold_response", gate: "asset_a" })}\n`);
  await waitFor(message => message.type === "ack" && message.id === 30);
  const heldA = fetch(assetUrl, { headers, signal: AbortSignal.timeout(10_000) });
  // This receipt proves the actual A handler completed before B becomes current.
  const heldReceipt = await readReceipt(receipt.seq, message => message.operation === "asset" && message.project === "project-a");
  assert.equal(heldReceipt.result, "asset_ok");
  assert.equal(heldReceipt.seq, receipt.seq + 1);
  await select(b.id);
  const selectedB = await rpc("panel.capabilities.snapshot");
  assert.equal(selectedB.revision, selectedA.revision);
  const selectBReceipt = await readReceipt(heldReceipt.seq, message => message.operation === "select" && message.project === "project-b");
  const discoveryBReceipt = await readReceipt(selectBReceipt.seq, message => message.operation === "discovery" && message.project === "project-b");
  assert.equal(discoveryBReceipt.result, "snapshot_ok");
  assert.equal((await fetch(assetUrl, { headers, signal: AbortSignal.timeout(5000) })).status, 404);
  child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id: 31, command: "release_response", gate: "asset_a" })}\n`);
  await waitFor(message => message.type === "ack" && message.id === 31);
  assert.equal((await heldA).status, 200, "delayed A is a real old successful response, not a request re-authorized under B");
  await select(null);
  const clearReceipt = await readReceipt(discoveryBReceipt.seq, message => message.operation === "select" && message.project === "global");
  child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id: 22, command: "set_fixture", action: "unbind_workspace" })}\n`);
  await waitFor(message => message.type === "ack" && message.id === 22);
  await assert.rejects(select(a.id), /rpc_refused/);
  const refusedReceipt = await readReceipt(clearReceipt.seq, message => message.operation === "select" && message.project === "project-a" && message.result === "refused");
  assert.equal(refusedReceipt.status, "refused");
  child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id: 21, command: "set_fixture", action: "select_a" })}\n`);

  assert.equal((await waitFor(message => message.type === "error" && message.code === "bad_state")).code, "bad_state");
  socket.close();
  child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id: 2, command: "set_fixture", action: "unbind_workspace" })}\n`);
  assert.equal((await waitFor((message) => message.type === "ack" && message.id === 2)).command, "set_fixture");
  child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id: 3, command: "hold_response", gate: "asset_a" })}\n`);
  assert.equal((await waitFor((message) => message.type === "ack" && message.id === 3)).command, "hold_response");
  child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id: 4, command: "release_response", gate: "asset_a" })}\n`);
  assert.equal((await waitFor((message) => message.type === "ack" && message.id === 4)).command, "release_response");
  child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id: 5, command: "shutdown" })}\n`);
  const stopped = await waitFor((message) => message.type === "stopped");
  assert.equal(stopped.listenerClosed, true);
  assert.equal(stopped.fixtureClosed, true);
  child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id: 40, command: "revoke_exact", device: "device-a" })}\n`);
  assert.equal((await waitFor((message) => (message.type === "ack" || message.type === "error") && message.id === 40)).type, "ack");
  child.stdin.end();
  if (child.exitCode === null) {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("helper_exit_timeout")), 10_000);
      child.once("close", () => {
        clearTimeout(timer);
        resolve();
      });
    });
  }
  assert.equal(child.exitCode, 0);
});
