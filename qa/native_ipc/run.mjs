#!/usr/bin/env node
// Native IPC QA harness (Windows, one visible WebView2 window).
//
// Proves — against a REAL debug lite shell built with `--features qa-owned-root` —
// that the Tauri IPC ACL admits `panel_credential_load/save/delete` from the
// configured Gateway origin and that the shell stays inside an owned temp root.
// It is NOT a full Panel/WASM test: the "Gateway" is a throw-away HTTP server on
// 127.0.0.1:<random> serving one static page that invokes the three commands via
// the real `window.__TAURI_INTERNALS__.invoke` and POSTs the outcomes back.
//
// Safety model (see README.md): this script owns creation AND cleanup of the QA
// root; the shell never deletes. Cleanup happens only after the containment proof
// (`proveOwned`) and only if the tree holds no symlink/junction. Any ambiguity
// leaves the directory in place and exits 3. Never sets HOME/USERPROFILE, never
// touches ~/.aleph, HKCU, the OS keyring or port 18790. Node stdlib only.
//
// Usage: node qa/native_ipc/run.mjs --exe <abs path to qa shell .exe> [--timeout-ms 60000]

import { spawn, spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";

const ROOT_PREFIX = "aleph-shell-qa-";
const NONCE_FILE = ".qa-owned";
const QA_BUILD_MARKER = "aleph-qa-owned-root-build-v1"; // keep == desktop/shell/src/qa_root.rs
const FORBIDDEN_PORT = 18790;
const DEFAULT_TIMEOUT_MS = 60_000;
const MAX_TIMEOUT_MS = 180_000;
const MAX_BODY = 64 * 1024;
const MAX_TREE_ENTRIES = 20_000;

const log = (...a) => console.error("[native_ipc]", ...a);
class Refuse extends Error {}

// ---------------------------------------------------------------- arguments
function parseArgs(argv) {
  const out = { exe: null, timeoutMs: DEFAULT_TIMEOUT_MS };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--exe") out.exe = argv[++i];
    else if (a === "--timeout-ms") out.timeoutMs = Number(argv[++i]);
    else throw new Refuse(`unknown argument: ${a}`);
  }
  if (!out.exe) throw new Refuse("--exe <absolute path to the qa-owned-root shell .exe> is required");
  if (!Number.isInteger(out.timeoutMs) || out.timeoutMs < 5_000 || out.timeoutMs > MAX_TIMEOUT_MS) {
    throw new Refuse(`--timeout-ms must be an integer in 5000..${MAX_TIMEOUT_MS}`);
  }
  return out;
}

const sameFsPath = (a, b) =>
  path.resolve(a).toLowerCase() === path.resolve(b).toLowerCase(); // win32 only (checked in main)
const hasAlephComponent = (p) => p.split(/[\\/]+/).some((c) => c.toLowerCase() === ".aleph");

function checkExe(exe) {
  if (!path.isAbsolute(exe)) throw new Refuse("--exe must be an absolute path");
  if (!exe.toLowerCase().endsWith(".exe")) throw new Refuse("--exe must be a .exe");
  if (hasAlephComponent(exe)) throw new Refuse("--exe must not live under a .aleph directory");
  const st = fs.lstatSync(exe);
  if (!st.isFile() || st.isSymbolicLink()) throw new Refuse("--exe is not a regular file");
  // A non-QA shell would IGNORE --qa-root and write to the real ~/.aleph and the
  // real OS keyring. Prove statically that this binary carries the QA build
  // marker (only compiled in with `--features qa-owned-root`).
  const bytes = fs.readFileSync(exe);
  if (!bytes.includes(Buffer.from(QA_BUILD_MARKER, "utf8"))) {
    throw new Refuse(
      `--exe does not contain "${QA_BUILD_MARKER}": not a qa-owned-root build; refusing to launch it`,
    );
  }
}

// ------------------------------------------------------- owned root lifecycle
function createOwnedRoot() {
  const base = fs.realpathSync(os.tmpdir());
  const nonce = randomBytes(16).toString("hex");
  const root = path.join(base, ROOT_PREFIX + randomBytes(8).toString("hex"));
  // NON-recursive: fails if the path exists (no adoption of a foreign dir) or if
  // the parent is missing.
  fs.mkdirSync(root, { recursive: false });
  // 'wx': exclusive create of the ownership marker.
  fs.writeFileSync(path.join(root, NONCE_FILE), nonce, { flag: "wx" });
  fs.mkdirSync(path.join(root, ".aleph"), { recursive: false });
  return { base, root, nonce };
}

/** Containment proof: throws unless `root` is exactly the dir this run created. */
function proveOwned({ base, root, nonce }) {
  const st = fs.lstatSync(root);
  if (!st.isDirectory() || st.isSymbolicLink()) throw new Error("root is not a plain directory");
  const real = fs.realpathSync.native(root);
  if (!sameFsPath(real, root)) throw new Error(`root is aliased (${real} != ${root})`);
  if (!sameFsPath(path.dirname(real), base) || !path.basename(real).startsWith(ROOT_PREFIX)) {
    throw new Error("root is not <tmp>/aleph-shell-qa-*");
  }
  if (hasAlephComponent(real)) throw new Error("root has a .aleph component");
  const marker = path.join(real, NONCE_FILE);
  const mst = fs.lstatSync(marker);
  if (!mst.isFile() || mst.isSymbolicLink() || mst.size > 256) throw new Error("bad ownership file");
  if (fs.readFileSync(marker, "utf8").trim() !== nonce) throw new Error("ownership nonce mismatch");
}

/** Relative file listing; throws on any symlink/junction or oversize tree. */
function inventory(root) {
  const files = [];
  const stack = [""];
  let seen = 0;
  while (stack.length) {
    const rel = stack.pop();
    for (const ent of fs.readdirSync(path.join(root, rel), { withFileTypes: true })) {
      if (++seen > MAX_TREE_ENTRIES) throw new Error("tree too large to audit");
      const childRel = path.join(rel, ent.name);
      const st = fs.lstatSync(path.join(root, childRel));
      if (st.isSymbolicLink()) throw new Error(`link/junction found: ${childRel}`);
      if (st.isDirectory()) stack.push(childRel);
      else files.push(childRel);
    }
  }
  return files.sort();
}

function cleanup(owned, procDead) {
  if (!owned) return "no-root";
  try {
    if (!procDead) throw new Error("shell process tree not confirmed dead");
    proveOwned(owned);
    inventory(owned.root); // refuse on links / oversize
    fs.rmSync(owned.root, { recursive: true, force: false, maxRetries: 10, retryDelay: 500 });
    if (fs.existsSync(owned.root)) throw new Error("root still exists after rm");
    return "removed";
  } catch (e) {
    log(`CLEANUP AMBIGUOUS — leaving ${owned.root} in place: ${e.message}`);
    return "LEFT_IN_PLACE";
  }
}

// ------------------------------------------------------------- fake Gateway
const PAGE = (runToken, port) => `<!doctype html><meta charset="utf-8"><title>Aleph native IPC QA</title>
<body style="font:14px sans-serif"><h3>Aleph native IPC QA</h3><pre id="out">running…</pre>
<script>
(async () => {
  const RUN = ${JSON.stringify(runToken)};
  const steps = [];
  const out = document.getElementById("out");
  const post = (path, body) => fetch(path, { method: "POST", headers: { "content-type": "application/json", "x-qa-run": RUN }, body: JSON.stringify(body) });
  const rec = (name, ok, info) => { steps.push({ name, ok, info }); out.textContent = JSON.stringify(steps, null, 1); };
  const errOf = (e) => (typeof e === "string" ? e : JSON.stringify(e)).slice(0, 200);
  const inv = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);
  const hex = (n) => Array.from(crypto.getRandomValues(new Uint8Array(n)), (b) => b.toString(16).padStart(2, "0")).join("");
  try {
    if (!window.__TAURI_INTERNALS__ || typeof window.__TAURI_INTERNALS__.invoke !== "function") {
      rec("tauri_internals_present", false, "window.__TAURI_INTERNALS__.invoke missing");
      throw new Error("no ipc");
    }
    rec("tauri_internals_present", true, location.origin);
    const cred = { device_id: "qa-device-" + hex(4), device_token: "qa-token-" + hex(16) }; // dummy, QA-only, in-memory backend
    const wantUrl = "ws://127.0.0.1:${port}/ws";

    let l = await inv("panel_credential_load");
    rec("load_initial", Number.isInteger(l.lease_id) && l.lease_id > 0 && l.gateway_url === wantUrl && l.credential === null,
      { gateway_url: l.gateway_url, lease_positive_int: Number.isInteger(l.lease_id) && l.lease_id > 0, credential_null: l.credential === null });

    await inv("panel_credential_save", { lease_id: l.lease_id, credential: cred });
    rec("save", true, "resolved");

    l = await inv("panel_credential_load");
    rec("load_after_save", !!l.credential && l.credential.device_id === cred.device_id && l.credential.device_token === cred.device_token,
      { device_id_matches: !!l.credential && l.credential.device_id === cred.device_id, token_matches: !!l.credential && l.credential.device_token === cred.device_token });

    const existed = await inv("panel_credential_delete", { lease_id: l.lease_id });
    rec("delete", existed === true, { existed });

    l = await inv("panel_credential_load");
    rec("load_after_delete", l.credential === null, { credential_null: l.credential === null });

    // Negative controls. The load above left a lease active, so a bogus id must hit
    // BridgeError::StaleLease (desktop/shell/src/keyring_bridge.rs, \`leased\`). That unit variant
    // serializes (serde rename_all = "snake_case") to the JSON string "stale_lease", and Tauri
    // rejects invoke() with exactly that string. Any other rejection (denied, ACL, transport,
    // argument/deserialization error, ...) is a FAIL: the control must prove the native lease check.
    const expectStale = async (name, call) => {
      try { await call(); rec(name, false, "unexpectedly resolved"); }
      catch (e) { rec(name, e === "stale_lease", { rejection: errOf(e) }); }
    };
    await expectStale("save_bogus_lease_rejected", () => inv("panel_credential_save", { lease_id: 999999999, credential: cred }));
    await expectStale("delete_bogus_lease_rejected", () => inv("panel_credential_delete", { lease_id: 999999999 }));
  } catch (e) {
    rec("aborted", false, errOf(e));
  }
  try { await post("/result", { origin: location.origin, steps }); } catch (_) { /* harness times out */ }
})();
</script></body>`;

function startGateway(runToken, onResult) {
  const server = http.createServer((req, res) => {
    const url = new URL(req.url, "http://x");
    const send = (code, type, body) => {
      res.writeHead(code, { "content-type": type, "cache-control": "no-store", connection: "close" });
      res.end(body);
    };
    if (req.method === "GET" && url.pathname === "/ready") return send(200, "application/json", '{"status":"ready"}');
    if (req.method === "GET" && (url.pathname === "/" || url.pathname === "/index.html")) {
      const port = server.address().port;
      return send(200, "text/html; charset=utf-8", PAGE(runToken, port));
    }
    if (req.method === "POST" && url.pathname === "/result" && req.headers["x-qa-run"] === runToken) {
      let size = 0;
      const chunks = [];
      req.on("data", (c) => {
        size += c.length;
        if (size <= MAX_BODY) chunks.push(c);
      });
      req.on("end", () => {
        if (size > MAX_BODY) return send(413, "text/plain", "too large");
        try {
          onResult(JSON.parse(Buffer.concat(chunks).toString("utf8")));
          send(200, "text/plain", "ok");
        } catch {
          send(400, "text/plain", "bad json");
        }
      });
      return;
    }
    send(404, "text/plain", "not found");
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port, address } = server.address();
      if (address !== "127.0.0.1") reject(new Error(`bound to ${address}, expected 127.0.0.1`));
      else if (port === FORBIDDEN_PORT) reject(new Error("bound the forbidden port 18790"));
      else resolve({ server, port });
    });
  });
}

const REQUIRED_STEPS = [
  "tauri_internals_present",
  "load_initial",
  "save",
  "load_after_save",
  "delete",
  "load_after_delete",
  "save_bogus_lease_rejected",
  "delete_bogus_lease_rejected",
];

function verdict(result, port) {
  if (!result) return { pass: false, why: "no result posted before timeout/exit" };
  const byName = Object.fromEntries((result.steps ?? []).map((s) => [s.name, s]));
  const missing = REQUIRED_STEPS.filter((n) => !byName[n]);
  const failed = REQUIRED_STEPS.filter((n) => byName[n] && !byName[n].ok);
  const originOk = result.origin === `http://127.0.0.1:${port}`;
  return {
    pass: missing.length === 0 && failed.length === 0 && originOk,
    why: { missing, failed, originOk },
  };
}

// ---------------------------------------------------------------------- main
async function killTree(child) {
  if (!child || child.pid == null) return true;
  // PID-scoped tree kill (never by image name). Then poll (bounded) until the
  // root process is gone; "not confirmed dead" makes cleanup fail closed.
  spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore", windowsHide: true });
  for (let i = 0; i < 20; i++) {
    if (child.exitCode !== null || child.signalCode !== null || !pidAlive(child.pid)) return true;
    await new Promise((r) => setTimeout(r, 250));
  }
  return false;
}
function pidAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

async function main() {
  if (process.platform !== "win32") throw new Refuse("Windows/WebView2 only");
  const args = parseArgs(process.argv.slice(2));
  checkExe(args.exe);

  const before = { HOME: process.env.HOME, USERPROFILE: process.env.USERPROFILE };
  let owned = null;
  let gateway = null;
  let child = null;
  let childDead = true;
  let result = null;
  let exitInfo = null;
  let stderrTail = "";
  let report;

  try {
    owned = createOwnedRoot();
    log(`owned root: ${owned.root}`);
    const runToken = randomBytes(16).toString("hex");
    let resolveResult;
    const resultP = new Promise((r) => (resolveResult = r));
    gateway = await startGateway(runToken, (body) => {
      result = body;
      resolveResult();
    });
    log(`fake Gateway on 127.0.0.1:${gateway.port}`);

    // The shell reads this marker at startup (mints the keyring capability for
    // exactly this origin, then navigates there). Same form `to_persisted` writes.
    const marker = path.join(owned.root, ".aleph", ".desktop-shell-panel-target");
    fs.writeFileSync(marker, `http://127.0.0.1:${gateway.port}`, { flag: "wx" });

    proveOwned(owned);
    child = spawn(args.exe, ["--qa-root", owned.root, "--qa-nonce", owned.nonce], {
      cwd: owned.root,
      env: process.env, // inherited unchanged; HOME/USERPROFILE are never set
      stdio: ["ignore", "ignore", "pipe"],
      windowsHide: false, // the window is deliberately visible
    });
    childDead = false;
    child.stderr.on("data", (c) => (stderrTail = (stderrTail + c.toString("utf8")).slice(-4000)));
    const exited = new Promise((r) => child.once("exit", (code, sig) => ((exitInfo = { code, sig }), r())));
    child.once("error", (e) => ((exitInfo = { spawnError: e.message }), resolveResult()));

    const timer = new Promise((r) => setTimeout(r, args.timeoutMs, "timeout"));
    const winner = await Promise.race([resultP.then(() => "result"), exited.then(() => "exited"), timer]);
    log(`wait ended: ${winner}`);
    if (winner === "exited" && !result) await new Promise((r) => setTimeout(r, 500)); // late POST
    if (winner === "result") await new Promise((r) => setTimeout(r, 1_000)); // let the shell flush

    report = { winner, exitInfo, stderrTail: stderrTail.trim() || undefined, ...verdict(result, gateway.port) };
    report.steps = result?.steps;
  } catch (e) {
    report = { pass: false, error: String(e?.message ?? e) };
  } finally {
    childDead = await killTree(child);
    if (gateway) gateway.server.close();
    // Post-run audit of what the shell wrote (inventory also refuses on links).
    if (owned) {
      try {
        report.rootInventory = inventory(owned.root);
      } catch (e) {
        report.rootInventory = `unavailable: ${e.message}`;
      }
    }
    const envOk = process.env.HOME === before.HOME && process.env.USERPROFILE === before.USERPROFILE;
    report.envUntouched = envOk;
    report.cleanup = cleanup(owned, childDead);
    report.rootPath = owned?.root;
  }
  console.log(JSON.stringify(report, null, 2));
  if (report.cleanup === "LEFT_IN_PLACE") process.exit(3);
  process.exit(report.pass ? 0 : 1);
}

main().catch((e) => {
  console.error(`[native_ipc] refused: ${e.message}`);
  process.exit(e instanceof Refuse ? 2 : 1);
});
