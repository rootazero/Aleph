// T0 shared helpers: engine launch + endpoint discovery, a static server on both loopback
// families, fixture writing. Throwaway probe support — never imported by Rust, never enters qa/.
import { spawn, execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Cdp, sleep } from "./cdp.mjs";

export { sleep, Cdp };
export const PROBE_DIR = path.dirname(fileURLToPath(import.meta.url));
export const EVIDENCE_DIR = path.resolve(PROBE_DIR, "..");
export const REPO = path.resolve(PROBE_DIR, "../../../../..");
if (!fs.existsSync(path.join(REPO, "Cargo.toml"))) {
  throw new Error(`t0: repo root not found (resolved ${REPO} from ${PROBE_DIR})`);
}
export const CDP_FIXTURES = path.join(REPO, "crates/aleph-cdp/tests/fixtures");
export const PAGE_FIXTURES = path.join(REPO, "src/browser/page_state/fixtures");
// R48 (controller ruling 2026-09-06): t0-support-matrix.json lives beside its one reader —
// Task 16's capability-table test in src/browser/engine/capability.rs, which reads it as a
// same-directory `include_str!("fixtures/t0-support-matrix.json")`. It is NOT under
// crates/aleph-cdp/tests/fixtures/ even though it is written by the same capture script that
// writes the aleph-cdp fixtures.
export const ENGINE_FIXTURES = path.join(REPO, "src/browser/engine/fixtures");
// These files are COMMITTED, so neither default may be a session-scoped scratch path: a dead
// absolute path in the tree reads like a usable one. The obscura default is the ledger's install
// location for the pinned tag (spec §6.1), which is where a machine that ran `runtime_manage
// {install}` actually has it.
export const OBSCURA = process.env.OBSCURA_BIN
  ?? path.join(os.homedir(), ".aleph/runtimes/obscura/v0.2.2/obscura");
export const CHROME = process.env.T0_CHROME
  ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
export const PARENT_PORT = Number(process.env.T0_PARENT_PORT ?? 18999);
export const CHILD_PORT = Number(process.env.T0_CHILD_PORT ?? 19001);
export const parentUrl = (qs = "") => `http://127.0.0.1:${PARENT_PORT}/t0-page.html${qs}`;
export const childUrl = () => `http://localhost:${CHILD_PORT}/t0-frame.html`;
export const tinyUrl = () => `http://127.0.0.1:${PARENT_PORT}/t0-tiny.html`;

// The five computed styles the spec's fetchers ask for (§3.3 / §4.1 `Computed`).
export const COMPUTED_STYLES = ["display", "visibility", "opacity", "cursor", "overflow"];

export function writeFixture(dir, name, value) {
  fs.mkdirSync(dir, { recursive: true });
  const p = path.join(dir, name);
  fs.writeFileSync(p, JSON.stringify(value, null, 2) + "\n");
  const bytes = fs.statSync(p).size;
  console.error(`[t0] wrote ${path.relative(REPO, p)} (${bytes} bytes)`);
  return { path: p, bytes };
}


export function freePort() {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.on("error", reject);
    s.listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => resolve(p)); });
  });
}

export function listenerPids(port) {
  try {
    return execFileSync("lsof", ["-nP", `-iTCP:${port}`, "-sTCP:LISTEN", "-t"], { encoding: "utf8" })
      .split("\n").map((s) => Number(s.trim())).filter((n) => Number.isInteger(n) && n > 0);
  } catch { return []; }
}

// Serve `dir` on BOTH loopback families at `port`, so `http://localhost:<port>` reaches it no
// matter whether the resolver answers 127.0.0.1 or ::1 first. Reports which families bound: a
// silent single-family bind is how the cross-site iframe would fail to load and U3/U4 would read
// "no child frame" when the truth is "the child was never served".
export async function serveStatic(dir, port) {
  const mime = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8",
                 ".css": "text/css; charset=utf-8", ".json": "application/json" };
  const handler = (req, res) => {
    const rel = decodeURIComponent(new URL(req.url, "http://h").pathname).replace(/^\/+/, "");
    const file = path.join(dir, rel === "" ? "t0-page.html" : rel);
    if (!file.startsWith(dir)) { res.writeHead(403).end("no"); return; }
    fs.readFile(file, (e, buf) => {
      if (e) { res.writeHead(404).end("not found"); return; }
      res.writeHead(200, { "content-type": mime[path.extname(file)] ?? "application/octet-stream",
                           "cache-control": "no-store" });
      res.end(buf);
    });
  };
  const listen = (host) => new Promise((resolve) => {
    const s = http.createServer(handler);
    s.on("error", (e) => { console.error(`[t0] static ${host}:${port} not listening (${e.code})`); resolve(null); });
    s.listen(port, host, () => resolve({ host, server: s }));
  });
  const bound = (await Promise.all([listen("127.0.0.1"), listen("::1")])).filter(Boolean);
  if (bound.length === 0) throw new Error(`t0: could not bind ${port} on any loopback family`);
  return {
    families: bound.map((b) => b.host),
    close: () => bound.forEach((b) => { try { b.server.close(); } catch {} }),
  };
}

export async function jsonVersion(port, timeoutMs = 3000) {
  try {
    const r = await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(timeoutMs) });
    return { status: r.status, body: await r.json() };
  } catch (e) { return { error: String(e?.message ?? e) }; }
}

// `--allow-private-network` is passed ONLY here: the probe pages live on 127.0.0.1 and obscura's
// own SSRF floor blocks private ranges by default. Production passes it only when the profile's
// network policy allows private ranges (spec §6.2). `--allow-file-access` is never passed.
export async function launchObscura({ port, storageDir, extraArgs = [], waitMs = 20000 } = {}) {
  const dir = storageDir ?? fs.mkdtempSync(path.join(os.tmpdir(), "t0-obs-"));
  const p = port ?? await freePort();
  const args = ["serve", "--port", String(p), "--storage-dir", dir, "--allow-private-network", ...extraArgs];
  const child = spawn(OBSCURA, args, { stdio: ["ignore", "pipe", "pipe"] });
  const buf = { out: "", err: "", exit: null };
  child.stdout.on("data", (d) => { buf.out += d; });
  child.stderr.on("data", (d) => { buf.err += d; });
  child.on("exit", (code, signal) => { buf.exit = { code, signal }; });

  // R14: the stdout banner is printed BEFORE the socket is bound, and it reports
  // `ws://127.0.0.1:0` verbatim when `--port 0` was passed. So the banner is EVIDENCE (U1 records
  // it) and never an endpoint. Readiness is a 200 from `/json/version` on the port we passed —
  // without this poll every obscura capture races the bind, and a refused connection would be
  // written into t0-support-matrix.json as "obscura does not answer this method" (判据 §8).
  const banner = () => {
    const m = (buf.out + buf.err).match(/ws:\/\/127\.0\.0\.1:(\d+)\/devtools\/browser\S*/);
    return m ? { url: m[0], port: Number(m[1]) } : null;
  };
  const deadline = Date.now() + waitMs;
  let wsUrl = null;
  let version = null;
  while (Date.now() < deadline) {
    if (buf.exit) break;
    const v = await jsonVersion(p, 1000);
    if (v.status === 200) {
      version = v;
      // The authority comes from the port WE passed; only the PATH is taken from the body,
      // because R14 measured a body that advertises `:0`. Trusting the body's authority is how a
      // probe ends up connecting to a port nothing is listening on.
      let wsPath = "/devtools/browser";
      const advertised = v.body?.webSocketDebuggerUrl;
      if (typeof advertised === "string") {
        const at = advertised.indexOf("/devtools");
        if (at > 0) wsPath = advertised.slice(at);
      }
      wsUrl = `ws://127.0.0.1:${p}${wsPath}`;
      break;
    }
    await sleep(100);
  }
  return {
    engine: "obscura", child, pid: child.pid, args, argv: [OBSCURA, ...args], dir, port: p, wsUrl,
    // U1's evidence: what the banner said, whether the port it named is the one we asked for, and
    // whether the listening socket on our port is ours.
    banner: banner(),
    jsonVersion: version,
    listenerPids: wsUrl ? listenerPids(p) : [],
    get ownedByLaunchedPid() { return this.listenerPids.includes(child.pid); },
    get stdout() { return buf.out; }, get stderr() { return buf.err; }, get exit() { return buf.exit; },
    kill() { try { child.kill("SIGKILL"); } catch {} },
  };
}

// argv order mirrors src/browser/chromium_launch.rs:90-99; endpoint discovery mirrors
// chromium_launch.rs:129-149 (a 0 in the port file is never a listening port; the path must be
// absolute — both of those are the "not yet" state, not a failure).
export async function launchChrome({ extraArgs = [], waitMs = 30000 } = {}) {
  const udd = fs.mkdtempSync(path.join(os.tmpdir(), "t0-chrome-"));
  const args = ["--use-mock-keychain", "--headless=new", `--user-data-dir=${udd}`,
                "--remote-debugging-port=0", ...extraArgs, "about:blank"];
  const child = spawn(CHROME, args, { stdio: ["ignore", "pipe", "pipe"] });
  const buf = { err: "", exit: null };
  child.stderr.on("data", (d) => { buf.err += d; });
  child.on("exit", (code, signal) => { buf.exit = { code, signal }; });
  const portFile = path.join(udd, "DevToolsActivePort");
  const deadline = Date.now() + waitMs;
  let wsUrl = null, port = null;
  while (Date.now() < deadline) {
    if (fs.existsSync(portFile)) {
      const [rawPort, rawPath] = fs.readFileSync(portFile, "utf8").split("\n");
      const n = Number((rawPort ?? "").trim());
      const pth = (rawPath ?? "").trim();
      if (Number.isInteger(n) && n > 0 && pth.startsWith("/")) {
        port = n; wsUrl = `ws://127.0.0.1:${n}${pth}`; break;
      }
    }
    if (buf.exit) break;
    await sleep(100);
  }
  return {
    engine: "chrome", child, pid: child.pid, args, argv: [CHROME, ...args], udd, port, wsUrl,
    get stderr() { return buf.err; }, get exit() { return buf.exit; },
    kill() { try { child.kill("SIGKILL"); } catch {} },
  };
}

export async function launchEngine(engine, opts = {}) {
  const e = engine === "chrome" || engine === "chromium"
    ? await launchChrome(opts)
    : await launchObscura(opts);
  if (!e.wsUrl) {
    const detail = e.engine === "chrome"
      ? `exit=${JSON.stringify(e.exit)} stderr=${e.stderr.slice(0, 400)}`
      : `exit=${JSON.stringify(e.exit)} banner=${JSON.stringify(e.banner)} `
        + `exit=${JSON.stringify(e.exit)} stdout=${e.stdout.slice(0, 400)} stderr=${e.stderr.slice(0, 400)}`;
    e.kill();
    // Both launchers now return `wsUrl` only after something answered: Chrome's port file, or
    // obscura's `/json/version`. Reaching here means "no engine to measure", which must stop the
    // probe rather than be captured as an engine's refusal (R14, 判据 §8).
    throw new Error(`t0: ${engine} published no reachable CDP endpoint (${detail})`);
  }
  return e;
}

// Open a target, attach flat, enable the four domains, set the viewport, navigate, settle.
export async function newPage(wsUrl, url, { width = 1280, height = 800, dpr = 1, tag = "t0", settleMs = 900 } = {}) {
  const c = new Cdp(wsUrl, tag);
  await c.connect();
  const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await c.call("Target.attachToTarget", { targetId, flatten: true });
  await c.call("Page.enable", {}, sessionId).catch(() => {});
  await c.call("Runtime.enable", {}, sessionId).catch(() => {});
  await c.call("DOM.enable", {}, sessionId).catch(() => {});
  await c.call("Network.enable", {}, sessionId).catch(() => {});
  await c.call("Emulation.setDeviceMetricsOverride",
               { width, height, deviceScaleFactor: dpr, mobile: false }, sessionId).catch(() => {});
  if (url) {
    const load = c.waitEvent("Page.loadEventFired", sessionId, 45000).catch((e) => String(e));
    await c.call("Page.navigate", { url }, sessionId, 45000);
    await load;
    await sleep(settleMs);
  }
  return { c, targetId, sessionId };
}

// One element's nodeId + backendNodeId + its JS getBoundingClientRect, for cross-checking geometry.
export async function locate(c, sessionId, selector) {
  const { root } = await c.call("DOM.getDocument", { depth: 1 }, sessionId, 30000);
  const { nodeId } = await c.call("DOM.querySelector", { nodeId: root.nodeId, selector }, sessionId, 30000);
  let backendNodeId = null;
  if (nodeId) {
    const d = await c.call("DOM.describeNode", { nodeId }, sessionId, 30000).catch(() => null);
    backendNodeId = d?.node?.backendNodeId ?? null;
  }
  const expr = `(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)return null;
    const r=e.getBoundingClientRect();
    return JSON.stringify({x:Math.round(r.x),y:Math.round(r.y),w:Math.round(r.width),h:Math.round(r.height),
                           scrollX:Math.round(window.scrollX),scrollY:Math.round(window.scrollY)});})()`;
  const ev = await c.call("Runtime.evaluate", { expression: expr, returnByValue: true }, sessionId, 30000)
    .catch(() => null);
  const raw = ev?.result?.value;
  return { selector, nodeId: nodeId ?? 0, backendNodeId, clientRect: raw ? JSON.parse(raw) : null };
}

// Call a method and return {ok, result} or {ok:false, error:{code,message}} — never throw, so a
// probe records what an engine refused instead of dying on the first refusal.
export async function tryCall(c, method, params, sessionId, timeoutMs = 45000) {
  try { return { ok: true, result: await c.call(method, params, sessionId, timeoutMs) }; }
  catch (e) {
    const msg = String(e?.message ?? e);
    const m = msg.match(/^(.*?):\s*(.*)$/s);
    return { ok: false, error: { code: null, message: m ? m[2] : msg } };
  }
}

export function emit(out) { process.stdout.write(JSON.stringify(out, null, 2) + "\n"); }
