import test from "node:test";
import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn } from "node:child_process";

const binary = process.env.NATIVE_QA_GATEWAY_BIN ?? "target/debug/aleph-panel-native-qa-gateway.exe";

test("disposable Gateway helper binds away from the daily daemon and shuts down", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "aleph-native-qa-helper-"));
  t.after(async () => rm(root, { recursive: true, force: true }));
  const panel = join(root, "panel");
  await mkdir(panel, { recursive: true });
  await writeFile(join(panel, "index.html"), "<!doctype html><html><body>qa</body></html>");
  const nonce = "56".repeat(16);
  const child = spawn(binary, ["--owned-root", root, "--nonce", nonce, "--panel-root", panel], {
    stdio: ["pipe", "pipe", "pipe"],
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
