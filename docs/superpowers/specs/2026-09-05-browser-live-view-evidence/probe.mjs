// THROWAWAY spike probe against `obscura serve` (Node 22+, native WebSocket).
// usage: node probe.mjs <mode: block|full> <cdpPort> <localBase>
import fs from "node:fs";
const [mode = "full", port = "9333", localBase = "http://127.0.0.1:18999"] = process.argv.slice(2);
const WS = `ws://127.0.0.1:${port}/devtools/browser`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const now = () => performance.now();

class Cdp {
  constructor(name) { this.name = name; this.id = 0; this.pending = new Map(); this.listeners = []; this.events = []; }
  async connect() {
    this.ws = new WebSocket(WS);
    await new Promise((res, rej) => { this.ws.addEventListener("open", res); this.ws.addEventListener("error", (e) => rej(new Error(`${this.name}: ws error`))); });
    this.ws.addEventListener("message", (ev) => {
      const m = JSON.parse(ev.data);
      if (m.id && this.pending.has(m.id)) { const p = this.pending.get(m.id); this.pending.delete(m.id); m.error ? p.reject(new Error(`${this.name} ${p.method}: ${m.error.message}`)) : p.resolve(m.result); }
      else if (m.method) { this.events.push({ t: now(), method: m.method, sessionId: m.sessionId }); for (const l of this.listeners) l(m); }
    });
  }
  call(method, params = {}, sessionId, timeoutMs = 20000) {
    return new Promise((resolve, reject) => {
      const id = ++this.id; this.pending.set(id, { resolve, reject, method });
      const t = setTimeout(() => { if (this.pending.has(id)) { this.pending.delete(id); reject(new Error(`${this.name} ${method}: timeout ${timeoutMs}ms`)); } }, timeoutMs);
      const msg = { id, method, params }; if (sessionId) msg.sessionId = sessionId;
      this.ws.send(JSON.stringify(msg));
      const p = this.pending.get(id); const r0 = p.resolve, j0 = p.reject;
      p.resolve = (v) => { clearTimeout(t); r0(v); }; p.reject = (e) => { clearTimeout(t); j0(e); };
    });
  }
  on(fn) { this.listeners.push(fn); return () => { this.listeners = this.listeners.filter((l) => l !== fn); }; }
  waitEvent(method, sessionId, timeoutMs = 15000) {
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => { off(); reject(new Error(`${this.name}: timeout waiting ${method}`)); }, timeoutMs);
      const off = this.on((m) => { if (m.method === method && (!sessionId || m.sessionId === sessionId)) { clearTimeout(t); off(); resolve(m.params); } });
    });
  }
  close() { try { this.ws.close(); } catch {} }
}

const out = { mode, port, obscuraEvents: {} };
const log = (k, v) => { out[k] = v; console.log(`## ${k}\n${JSON.stringify(v, null, 1)}`); };
const errStr = (e) => String(e?.message ?? e);

async function navigate(c, s, url, settleMs = 800) {
  const t0 = now();
  const loadP = c.waitEvent("Page.loadEventFired", s, 25000).catch((e) => ({ loadWaitError: errStr(e) }));
  let nav;
  try { nav = await c.call("Page.navigate", { url }, s, 30000); } catch (e) { return { url, navigateError: errStr(e), ms: Math.round(now() - t0) }; }
  const load = await loadP;
  await sleep(settleMs);
  return { url, nav, load, ms: Math.round(now() - t0) };
}

function frameCollector(c, s, label) {
  const frames = [];
  const off = c.on((m) => {
    if (m.method === "Page.screencastFrame" && m.sessionId === s) {
      const p = m.params;
      frames.push({ t: now(), bytes: Math.floor((p.data?.length ?? 0) * 3 / 4), w: p.metadata?.deviceWidth, h: p.metadata?.deviceHeight });
      c.call("Page.screencastFrameAck", { sessionId: p.sessionId }, s).catch((e) => frames.push({ ackError: errStr(e) }));
    }
  });
  return { frames, off, label, since: (t) => frames.filter((f) => f.t > t && f.bytes) };
}
const stats = (fs_) => ({ n: fs_.length, bytesAvg: fs_.length ? Math.round(fs_.reduce((a, f) => a + f.bytes, 0) / fs_.length) : 0, w: fs_[0]?.w, h: fs_[0]?.h });

// ---------- Phase 0: driver A ----------
const A = new Cdp("A"); await A.connect();
const ver = await A.call("Browser.getVersion").catch((e) => ({ error: errStr(e) }));
log("browserVersion", ver);
const { targetId } = await A.call("Target.createTarget", { url: "about:blank" });
let sA = `${targetId}-session`;
try { await A.call("Page.enable", {}, sA); log("sessionA", { targetId, sA, via: "managed-id" }); }
catch (e) { const at = await A.call("Target.attachToTarget", { targetId, flatten: true }); sA = at.sessionId; await A.call("Page.enable", {}, sA); log("sessionA", { targetId, sA, via: "attach", firstErr: errStr(e) }); }
log("viewport", await A.call("Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }, sA).then(() => "ok").catch(errStr));

if (mode === "block") {
  // ---------- Phase 4: private-network floor, as seen over CDP ----------
  log("blockNavigateLocal", await navigate(A, sA, `${localBase}/probe.html`));
  log("blockEvaluateAfter", await A.call("Runtime.evaluate", { expression: "location.href + ' | ' + document.title", returnByValue: true }, sA).catch(errStr));
  log("blockEventsSeen", A.events.map((e) => e.method).filter((m, i, a) => a.indexOf(m) === i));
  fs.writeFileSync(`results-${mode}.json`, JSON.stringify(out, null, 2)); A.close(); process.exit(0);
}

// ---------- Phase 1: observer B, dual-session screencast ----------
const B = new Cdp("B"); await B.connect();
const targets = await B.call("Target.getTargets").catch((e) => ({ error: errStr(e) }));
log("targetsSeenByB", targets?.targetInfos?.map((t) => ({ type: t.type, id: t.targetId, url: t.url })) ?? targets);
let sB, attachErr;
try { const at = await B.call("Target.attachToTarget", { targetId, flatten: true }); sB = at.sessionId; await B.call("Page.enable", {}, sB); } catch (e) { attachErr = errStr(e); }
log("sessionB", { sB, attachErr });
const colB = frameCollector(B, sB, "B");
log("startScreencastB", await B.call("Page.startScreencast", { format: "jpeg", quality: 60, maxWidth: 1280, maxHeight: 800, everyNthFrame: 1 }, sB).then(() => "ok").catch(errStr));

let t = now();
log("navLocalByA", await navigate(A, sA, `${localBase}/probe.html`));
await sleep(3000);
log("B_frames_while_A_idle_animation_3s", stats(colB.since(t)));

// A drives: click, scroll, type — does B keep receiving?
t = now();
const clickT0 = now();
await A.call("Input.dispatchMouseEvent", { type: "mousePressed", x: 150, y: 100, button: "left", clickCount: 1 }, sA).catch((e) => log("clickErr", errStr(e)));
await A.call("Input.dispatchMouseEvent", { type: "mouseReleased", x: 150, y: 100, button: "left", clickCount: 1 }, sA).catch(() => {});
let firstAfterClick = null; const deadline = now() + 3000;
while (!firstAfterClick && now() < deadline) { const f = colB.since(clickT0); if (f.length) firstAfterClick = Math.round(f[0].t - clickT0); else await sleep(10); }
log("clickToNextFrameOnB_ms", firstAfterClick);
log("boxCountAfterClick", await A.call("Runtime.evaluate", { expression: "document.getElementById('box').textContent", returnByValue: true }, sA).then((r) => r.result?.value).catch(errStr));
await A.call("Input.dispatchMouseEvent", { type: "mouseWheel", x: 400, y: 400, deltaX: 0, deltaY: 600 }, sA).catch((e) => log("wheelErr", errStr(e)));
await sleep(300);
log("scrollYAfterWheel", await A.call("Runtime.evaluate", { expression: "window.scrollY", returnByValue: true }, sA).then((r) => r.result?.value).catch(errStr));
await sleep(2000);
log("B_frames_while_A_drives_3s", stats(colB.since(t)));

// Now A also screencasts: do both receive?
const colA = frameCollector(A, sA, "A");
log("startScreencastA", await A.call("Page.startScreencast", { format: "jpeg", quality: 60, maxWidth: 1280, maxHeight: 800 }, sA).then(() => "ok").catch(errStr));
t = now(); await sleep(3000);
log("dual_3s", { A: stats(colA.since(t)), B: stats(colB.since(t)) });
await A.call("Page.stopScreencast", {}, sA).catch(() => {});

// ---------- Phase 3: IME-style text insertion ----------
await A.call("Runtime.evaluate", { expression: "window.scrollTo(0,0); document.getElementById('name').value=''; document.getElementById('name').focus(); document.activeElement.id", returnByValue: true }, sA)
  .then((r) => log("focusedBeforeInsert", r.result?.value)).catch((e) => log("focusErr", errStr(e)));
log("insertText", await A.call("Input.insertText", { text: "中文测试 こんにちは 🙂" }, sA).then(() => "ok").catch(errStr));
for (const ch of "ab") await A.call("Input.dispatchKeyEvent", { type: "char", text: ch, key: ch, unmodifiedText: ch }, sA).catch((e) => log("charErr", errStr(e)));
log("inputValueAfter", await A.call("Runtime.evaluate", { expression: "document.getElementById('name').value", returnByValue: true }, sA).then((r) => r.result?.value).catch(errStr));
log("inputEventFired", await A.call("Runtime.evaluate", { expression: "(()=>{const i=document.getElementById('name');let n=0;i.addEventListener('input',()=>n++);return 'listener-armed'})()", returnByValue: true }, sA).then((r) => r.result?.value).catch(errStr));

// ---------- Phase 2: AX tree + screenshots on real pages (observer B still attached) ----------
const pages = ["https://news.ycombinator.com/", "https://github.com/h4ckf0r0day/obscura", "https://en.wikipedia.org/wiki/Web_scraping"];
const ax = [];
for (const [i, url] of pages.entries()) {
  const t0 = now();
  const nav = await navigate(A, sA, url, 1500);
  const framesB = stats(colB.since(t0));
  let tree, axErr, axMs = 0;
  try { const a0 = now(); tree = await A.call("Accessibility.getFullAXTree", {}, sA, 30000); axMs = Math.round(now() - a0); } catch (e) { axErr = errStr(e); }
  let summary = { axErr };
  if (tree?.nodes) {
    const nodes = tree.nodes; const roles = {};
    for (const n of nodes) { const r = n.role?.value ?? "?"; roles[r] = (roles[r] ?? 0) + 1; }
    const byId = new Map(nodes.map((n) => [n.nodeId, n]));
    const depth = (n) => { let d = 0, p = n; while (p?.parentId && byId.has(p.parentId) && d < 200) { p = byId.get(p.parentId); d++; } return d; };
    summary = {
      nodes: nodes.length, withName: nodes.filter((n) => n.name?.value).length, withBackendDOMNodeId: nodes.filter((n) => n.backendDOMNodeId != null).length,
      ignored: nodes.filter((n) => n.ignored).length, maxDepth: Math.max(...nodes.map(depth)),
      topRoles: Object.entries(roles).sort((a, b) => b[1] - a[1]).slice(0, 10),
      sampleInteractive: nodes.filter((n) => ["link", "button", "textbox", "combobox", "checkbox"].includes(n.role?.value)).slice(0, 6).map((n) => ({ role: n.role?.value, name: (n.name?.value ?? "").slice(0, 40), be: n.backendDOMNodeId })),
      axMs,
    };
  }
  let shot;
  try { const s = await A.call("Page.captureScreenshot", { format: "png" }, sA, 30000); fs.writeFileSync(`shot-${i}.png`, Buffer.from(s.data, "base64")); shot = `shot-${i}.png (${Math.round(s.data.length * 3 / 4 / 1024)}KB)`; } catch (e) { shot = errStr(e); }
  const dom = await A.call("DOM.getDocument", { depth: 0 }, sA).then((d) => `root=${d.root?.nodeId}`).catch(errStr);
  const title = await A.call("Runtime.evaluate", { expression: "document.title", returnByValue: true }, sA).then((r) => r.result?.value).catch(errStr);
  ax.push({ url, title, navMs: nav.ms, navErr: nav.navigateError ?? nav.load?.loadWaitError, framesBDuringNav: framesB, ax: summary, shot, dom });
  console.log(`## page ${i}\n${JSON.stringify(ax[i], null, 1)}`);
}
out.pages = ax;
log("B_sessionAliveAfterNavigations", await B.call("Runtime.evaluate", { expression: "location.href", returnByValue: true }, sB).then((r) => r.result?.value).catch(errStr));
await B.call("Page.stopScreencast", {}, sB).catch(() => {});
log("eventMethodsSeen", { A: [...new Set(A.events.map((e) => e.method))], B: [...new Set(B.events.map((e) => e.method))] });
fs.writeFileSync(`results-${mode}.json`, JSON.stringify(out, null, 2));
A.close(); B.close(); process.exit(0);
