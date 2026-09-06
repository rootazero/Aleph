// Capture the raw CDP `result` for every method aleph_cdp::methods::* wraps, from one engine,
// into crates/aleph-cdp/tests/fixtures/. usage: node t0-capture.mjs <chrome|obscura>
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { launchEngine, newPage, locate, tryCall, parentUrl, tinyUrl, serveStatic, PROBE_DIR,
         PARENT_PORT, CDP_FIXTURES, ENGINE_FIXTURES, COMPUTED_STYLES, writeFixture, sleep, emit } from "./t0-lib.mjs";

const engine = process.argv[2] ?? "chrome";
const tag = engine === "chrome" || engine === "chromium" ? "chrome" : "obscura";
const server = await serveStatic(PROBE_DIR, PARENT_PORT);
const e = await launchEngine(engine);
const { c, sessionId } = await newPage(e.wsUrl, parentUrl());

// `cdp.mjs:11-12` routes id-bearing frames into `pending` and rejects with the message only, so a
// protocol ERROR loses its numeric `code`. The two error fixtures Task 4 replays need {code,
// message} verbatim, so register the id in `pending` (to keep the shared client consistent) AND a
// raw listener that sees the frame first. Do not fork cdp.mjs: two clients with two error shapes
// is 判据 §1.
const rawCall = (method, params = {}, sid = sessionId, timeoutMs = 60000) => new Promise((resolve) => {
  const id = ++c.id;
  let settled = false;
  const finish = (v) => { if (settled) return; settled = true; clearTimeout(timer); off(); c.pending.delete(id); resolve(v); };
  const off = c.on(() => {});
  const listener = (m) => {
    if (m.id !== id) return;
    if (m.error) finish({ ok: false, error: { code: m.error.code ?? null, message: m.error.message ?? String(m.error) } });
    else finish({ ok: true, result: m.result ?? {} });
  };
  c.ws.addEventListener("message", (evt) => {
    let m; try { m = JSON.parse(evt.data); } catch { return; }
    listener(m);
  });
  const timer = setTimeout(() => finish({ ok: false, error: { code: null, message: `timeout ${timeoutMs}ms` } }), timeoutMs);
  // Park a pending entry so cdp.mjs's own handler does not treat the reply as an unknown frame.
  c.pending.set(id, { method, resolve: () => {}, reject: () => {} });
  const msg = { id, method, params };
  if (sid) msg.sessionId = sid;
  c.ws.send(JSON.stringify(msg));
});

const results = {};
const voids = {};
const matrix = {};
// `name` is the LABEL, and the label — not the CDP method — is the matrix key. Three calls share
// the method `DOM.getBoxModel` (visible / hidden / bad node) and two share `DOM.querySelector` and
// `Runtime.evaluate`; keying by method would let the last write win, so the matrix could never
// hold "ok" for a method whose refusal cases are also captured, and Task 16's capability table
// would be derived from a refusal that was deliberately provoked. One key per measurement.
// Every matrix entry is `{ protocol, effect }`:
//   protocol: "ok" | "<the peer's own refusal text>"  — what the wire said
//   effect:   true | false | null                     — what the PAGE observed, null = not measured
// The two are separate because obscura answers `Input.dispatchTouchEvent` with `Ok({})` and does
// nothing (`domains/input.rs:413` in the source survey) — the report-success no-op (判据 §11). A
// capability row judged on `protocol` alone would call that verb supported. `effect: false` is
// only ever written after a flag was actually read back; if the read itself failed, effect stays
// null, because "we could not look" is not "nothing happened" (判据 §8).
const setEffect = (label, value) => {
  if (!matrix[label]) matrix[label] = { protocol: "not attempted", effect: null };
  matrix[label].effect = value;
};
// `fixture` decides whether a FILE is written; the matrix entry is always written. R38: a
// capture with no named reader is measurement, not a fixture, and measurement belongs in the
// matrix and in t0-results.md. Call sites pass `fixture: true` (or `fixture: CHROME` for the
// Chrome-only ones) exactly where a Task-4 test names the file.
const CHROME = tag === "chrome";
const record = async (
  name, method, params,
  { void: isVoid = false, file = null, fixture = false, sid = sessionId } = {},
) => {
  const r = await rawCall(method, params, sid);
  matrix[name] = { protocol: r.ok ? "ok" : (r.error.message ?? "error"), effect: null };
  if (r.ok) {
    if (isVoid) { if (CHROME) voids[method] = r.result; }
    else if (fixture) results[file ?? `${tag}-${method}.json`] = r.result;
  } else if (fixture && file) {
    results[file] = { error: r.error };
  }
  console.error(`[t0] ${tag} ${name} -> ${r.ok ? "ok" : "ERR " + r.error.message}`);
  return r;
};

// ---- Browser / Target -------------------------------------------------------------------------
await record("Browser.getVersion", "Browser.getVersion", {}, { fixture: true, sid: null });
await record("Target.getTargets", "Target.getTargets", {}, { fixture: true, sid: null });
const created = await record("Target.createTarget", "Target.createTarget", { url: "about:blank" }, { fixture: CHROME, sid: null });
if (created.ok) {
  const spare = created.result.targetId;
  const att = await rawCall("Target.attachToTarget", { targetId: spare, flatten: true }, null);
  // Matrix only. No Task-4 test names a Target.attachToTarget fixture — Task 3 drives `attach`
  // with scripted session ids because each of its cases needs a different one (R38).
  matrix["Target.attachToTarget"] = { protocol: att.ok ? "ok" : att.error.message, effect: null };
  if (att.ok) {
    const det = await rawCall("Target.detachFromTarget", { sessionId: att.result.sessionId }, null);
    matrix["Target.detachFromTarget"] = { protocol: det.ok ? "ok" : det.error.message, effect: null };
    if (det.ok) voids["Target.detachFromTarget"] = det.result;
  }
  await record("Target.activateTarget", "Target.activateTarget", { targetId: spare }, { void: true, sid: null });
  await record("Target.closeTarget", "Target.closeTarget", { targetId: spare }, { fixture: CHROME, sid: null });
}
await record("Target.setDiscoverTargets", "Target.setDiscoverTargets", { discover: true }, { void: true, sid: null });

// ---- Page -------------------------------------------------------------------------------------
await record("Page.navigate", "Page.navigate", { url: parentUrl() }, { fixture: true });
await sleep(700);
await record("Page.getFrameTree", "Page.getFrameTree", {}, { fixture: CHROME });
await record("Page.getLayoutMetrics", "Page.getLayoutMetrics", {}, { fixture: CHROME });
await record("Page.getNavigationHistory", "Page.getNavigationHistory", {}, { fixture: CHROME });
await record("Page.enable", "Page.enable", {}, { void: true });
await record("Runtime.enable", "Runtime.enable", {}, { void: true });
await record("Network.enable", "Network.enable", {}, { void: true });
// NOTE: DOM.enable is deliberately NOT recorded into the void map. Task 4's census drives every
// entry of that map through its wrapper and panics on an entry with no wrapper, and there is no
// `methods::dom::enable` — DOM.getDocument enables the domain implicitly, so a wrapper for it
// would have zero consumers (R10). Capturing it would turn that correct absence into a red test.
await record("Page.bringToFront", "Page.bringToFront", {}, { void: true });
await record("Page.reload", "Page.reload", { ignoreCache: false }, { void: true });
await sleep(900);
{
  const hist = await rawCall("Page.getNavigationHistory", {});
  if (hist.ok && (hist.result.entries ?? []).length > 0) {
    const first = hist.result.entries[0];
    await record("Page.navigateToHistoryEntry", "Page.navigateToHistoryEntry", { entryId: first.id }, { void: true });
    await sleep(600);
    await rawCall("Page.navigate", { url: parentUrl() });
    await sleep(700);
  } else {
    matrix["Page.navigateToHistoryEntry"] = { protocol: "not reachable: no history entries", effect: null };
  }
}

// ---- DOM --------------------------------------------------------------------------------------
//
// Context (docRoot + each element's backendNodeId) is captured HERE, from the SAME
// DOM.getDocument response this fixture uses — deliberately NOT before the Page section above.
// Chrome tears down and rebuilds the whole node tree across Page.navigate / Page.reload /
// Page.navigateToHistoryEntry (all three just ran): a nodeId or backendNodeId captured before
// that churn can reference a node in the discarded document afterward. Measured on this run:
// DOM.describeNode on such a stale backendNodeId still answered — correctly, with `id=go` — which
// made the staleness invisible right up until DOM.getBoxModel / DOM.resolveNode /
// DOM.scrollIntoViewIfNeeded / DOM.focus on that SAME id all failed ("Could not compute box
// model", "Node with given id does not belong to the document", "Node does not have a layout
// object", "Element is not focusable"). A capture built on that ordering would have shipped four
// fixtures missing outright, a fifth (`DOM.querySelector.miss`) recording the WRONG finding — an
// engine refusal standing in for a genuine zero-result query — and two false "unsupported"
// entries in t0-support-matrix.json for verbs Chrome supports fine on a live node. Capturing
// context fresh here, after the churn, is the fix; it changes nothing about what is measured.
const tmpFile = path.join(fs.mkdtempSync(path.join(os.tmpdir(), "t0-up-")), "upload.txt");
fs.writeFileSync(tmpFile, "t0 upload fixture\n");

const docCapture = await record("DOM.getDocument", "DOM.getDocument", { depth: -1, pierce: true }, { fixture: true });
const docRoot = docCapture.ok ? (docCapture.result?.root?.nodeId ?? 1) : 1;

// backendNodeId for `selector`, resolved against the fresh `docRoot` above via a raw querySelector
// + describeNode pair — not `locate()`, whose own internal DOM.getDocument would invalidate
// `docRoot` again the moment it ran.
const backendNodeIdFor = async (selector) => {
  const q = await rawCall("DOM.querySelector", { nodeId: docRoot, selector });
  if (!q.ok || !q.result?.nodeId) return null;
  const d = await rawCall("DOM.describeNode", { nodeId: q.result.nodeId });
  return d.ok ? (d.result?.node?.backendNodeId ?? null) : null;
};
const goBackendNodeId = await backendNodeIdFor("#go");
const hiddenBackendNodeId = await backendNodeIdFor("#hidden-none");
const fileBackendNodeId = await backendNodeIdFor("#file");

await record("DOM.querySelector", "DOM.querySelector", { nodeId: docRoot, selector: "#go" }, { fixture: CHROME });
await record("DOM.querySelector.miss", "DOM.querySelector", { nodeId: docRoot, selector: "#nope-not-here" },
             { fixture: CHROME, file: `${tag}-DOM.querySelector.miss.json` });
await record("DOM.querySelectorAll", "DOM.querySelectorAll", { nodeId: docRoot, selector: "a" }, { fixture: CHROME });
// Everything below addresses nodes by `backendNodeId`, because that is what the production
// wrappers send (`methods::dom`). A refusal Chrome gives for a bogus `nodeId` is a different
// lookup from the one for a bogus `backendNodeId` and need not be worded the same — and the whole
// value of the two error fixtures is that they are the refusals Chrome sends to the call Aleph
// actually makes.
let objectId = null;
if (goBackendNodeId) {
  await record("DOM.describeNode", "DOM.describeNode", { backendNodeId: goBackendNodeId }, { fixture: CHROME });
  await record("DOM.getBoxModel", "DOM.getBoxModel", { backendNodeId: goBackendNodeId }, { fixture: true });
  await record("DOM.scrollIntoViewIfNeeded", "DOM.scrollIntoViewIfNeeded", { backendNodeId: goBackendNodeId }, { void: true });
  await record("DOM.focus", "DOM.focus", { backendNodeId: goBackendNodeId }, { void: true });
  const rn = await record("DOM.resolveNode", "DOM.resolveNode", { backendNodeId: goBackendNodeId }, { fixture: CHROME });
  objectId = rn.ok ? (rn.result?.object?.objectId ?? null) : null;
} else {
  matrix["DOM.getBoxModel"] = { protocol: "not reachable: no backendNodeId for #go", effect: null };
}
if (hiddenBackendNodeId) {
  await record("DOM.getBoxModel.hidden", "DOM.getBoxModel", { backendNodeId: hiddenBackendNodeId },
               { fixture: CHROME, file: `${tag}-DOM.getBoxModel.hidden.json` });
} else {
  matrix["DOM.getBoxModel.hidden"] = { protocol: "not reachable: no backendNodeId for #hidden-none", effect: null };
}
await record("DOM.getBoxModel.badnode", "DOM.getBoxModel", { backendNodeId: 99999999 },
             { fixture: CHROME, file: `${tag}-DOM.getBoxModel.badnode.json` });
if (fileBackendNodeId) {
  await record("DOM.setFileInputFiles", "DOM.setFileInputFiles",
               { backendNodeId: fileBackendNodeId, files: [tmpFile] }, { void: true });
}

// ---- Runtime ----------------------------------------------------------------------------------
await record("Runtime.evaluate", "Runtime.evaluate",
             { expression: "1 + 1", returnByValue: true, awaitPromise: true }, { fixture: true });
await record("Runtime.evaluate.throws", "Runtime.evaluate",
             { expression: "throw new Error('t0 boom')", returnByValue: true, awaitPromise: true },
             { fixture: CHROME, file: `${tag}-Runtime.evaluate.throws.json` });
if (objectId) {
  await record("Runtime.callFunctionOn", "Runtime.callFunctionOn",
    { objectId, functionDeclaration: "function(){ return this.id; }",
      returnByValue: true, awaitPromise: true }, { fixture: CHROME });
} else {
  matrix["Runtime.callFunctionOn"] = { protocol: "not reachable: DOM.resolveNode gave no objectId", effect: null };
}

// ---- Input ------------------------------------------------------------------------------------
await record("Input.dispatchMouseEvent", "Input.dispatchMouseEvent",
  { type: "mouseMoved", x: 10, y: 10, button: "none", clickCount: 0, modifiers: 0 }, { void: true });
await record("Input.dispatchKeyEvent", "Input.dispatchKeyEvent",
  { type: "keyDown", key: "a", code: "KeyA", text: "a", windowsVirtualKeyCode: 65, modifiers: 0 }, { void: true });
await record("Input.insertText", "Input.insertText", { text: "t0" }, { void: true });

// ---- Network ----------------------------------------------------------------------------------
await record("Network.setCookies", "Network.setCookies",
  { cookies: [{ name: "t0_fixture", value: "yes", url: parentUrl() }] }, { void: true });
await record("Network.getAllCookies", "Network.getAllCookies", {}, { fixture: true });
await record("Network.getCookies", "Network.getCookies", { urls: [parentUrl()] }, { fixture: CHROME });
await record("Network.deleteCookies", "Network.deleteCookies",
  { name: "t0_fixture", domain: "127.0.0.1", path: "/" }, { void: true });

// ---- Emulation --------------------------------------------------------------------------------
await record("Emulation.setDeviceMetricsOverride", "Emulation.setDeviceMetricsOverride",
  { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }, { void: true });
await record("Emulation.setUserAgentOverride", "Emulation.setUserAgentOverride",
  { userAgent: "Aleph-T0/1.0" }, { void: true });
await record("Emulation.setGeolocationOverride", "Emulation.setGeolocationOverride",
  { latitude: 1.5, longitude: 2.5, accuracy: 10 }, { void: true });
await record("Emulation.clearDeviceMetricsOverride", "Emulation.clearDeviceMetricsOverride", {}, { void: true });
// The five Part 4 (Tasks 12/13) also calls. Captured here so they reach production having been
// parsed against a real reply like every other wrapper, and so t0-support-matrix.json records
// whether obscura answers them — which is exactly what the `EmulateOptions` capability rows need.
await record("Emulation.setEmulatedMedia", "Emulation.setEmulatedMedia",
  { features: [{ name: "prefers-color-scheme", value: "dark" }] }, { void: true });
await record("Emulation.setCPUThrottlingRate", "Emulation.setCPUThrottlingRate", { rate: 1 }, { void: true });
await record("Network.emulateNetworkConditions", "Network.emulateNetworkConditions",
  { offline: false, latency: 0, downloadThroughput: -1, uploadThroughput: -1 }, { void: true });
await record("Network.setExtraHTTPHeaders", "Network.setExtraHTTPHeaders", { headers: {} }, { void: true });
await record("Network.clearBrowserCookies", "Network.clearBrowserCookies", {}, { void: true });

// ---- DOMSnapshot ------------------------------------------------------------------------------
await record("DOMSnapshot.captureSnapshot", "DOMSnapshot.captureSnapshot",
  { computedStyles: COMPUTED_STYLES, includeDOMRects: true, includePaintOrder: true }, { fixture: CHROME });

// ---- screenshot + pdf on the tiny page, so the base64 stays diff-readable ----------------------
//
// NOTE: `newPage` opens its OWN websocket, so these calls go through `tiny.c` — a session id from
// one connection is not routable on another, and `rawCall` (which closes over the main `c`) would
// address a session that connection has never seen. `rawCall` exists only to keep the numeric
// error code for the two error fixtures, and neither of those is captured here.
{
  const tiny = await newPage(e.wsUrl, tinyUrl(), { width: 100, height: 50, tag: "tiny" });
  const shot = await tryCall(tiny.c, "Page.captureScreenshot",
    { format: "png", captureBeyondViewport: false }, tiny.sessionId, 60000);
  matrix["Page.captureScreenshot"] = { protocol: shot.ok ? "ok" : shot.error.message, effect: null };
  if (shot.ok && CHROME) results[`${tag}-Page.captureScreenshot.json`] = shot.result;
  const pdf = await tryCall(tiny.c, "Page.printToPDF", {}, tiny.sessionId, 60000);
  matrix["Page.printToPDF"] = { protocol: pdf.ok ? "ok" : pdf.error.message, effect: null };
  if (pdf.ok && CHROME) results[`${tag}-Page.printToPDF.json`] = pdf.result;
  tiny.c.close();
}

// ---- javascript dialog: the answer is evidence for Task 16's capability table ------------------
{
  const dlg = new Promise((res) => {
    const off = c.on((m) => { if (m.method === "Page.javascriptDialogOpening") { off(); res(m.params); } });
    setTimeout(() => { off(); res(null); }, 4000);
  });
  c.call("Runtime.evaluate", { expression: "setTimeout(function(){ alert('t0'); }, 0)", returnByValue: true },
         sessionId, 5000).catch(() => {});
  const opened = await dlg;
  matrix["Page.javascriptDialogOpening"] = { protocol: opened ? "ok" : "no event within 4s", effect: opened ? true : false };
  if (opened) await record("Page.handleJavaScriptDialog", "Page.handleJavaScriptDialog", { accept: true }, { void: true });
  else matrix["Page.handleJavaScriptDialog"] = { protocol: "not reachable: no dialog event", effect: null };
}

// ---- effect probes: the verbs where "reported success" and "did something" can differ ----------
//
// Run LAST and on a FRESH page, because two of them mutate the DOM and every fixture above was
// captured from the untouched one. The source survey lists `Input.dispatchTouchEvent`
// (`domains/input.rs:413`), `DOM.setAttributeValue` and `DOM.removeNode` as obscura no-ops that
// report success; `Input.dispatchDragEvent` is measured because it plausibly is one too. Each
// records BOTH what the wire said and what the page saw.
{
  const fresh = await newPage(e.wsUrl, parentUrl(), { tag: "effects" });
  const fc = fresh.c;
  const fs_ = fresh.sessionId;
  // Every call below goes through `fc`, the fresh page's OWN connection: `rawCall` closes over
  // the main `c`, and a session id from one websocket is not routable on another.
  const call = (method, params) => tryCall(fc, method, params, fs_, 30000);
  const evalIn = async (expr) => {
    const r = await call("Runtime.evaluate", { expression: expr, returnByValue: true });
    return r.ok ? { ok: true, value: r.result?.result?.value } : { ok: false, error: r.error.message };
  };
  const at = async (selector) => {
    const l = await locate(fc, fs_, selector);
    return l;
  };

  // --- Input.dispatchTouchEvent ---
  {
    const target = await at("#go");
    const pt = target.clientRect
      ? { x: target.clientRect.x + Math.round(target.clientRect.w / 2),
          y: target.clientRect.y + Math.round(target.clientRect.h / 2) }
      : null;
    const start = pt
      ? await call("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x: pt.x, y: pt.y }] })
      : { ok: false, error: { message: "no rect for #go" } };
    if (pt) await call("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
    matrix["Input.dispatchTouchEvent"] = {
      protocol: start.ok ? "ok" : (start.error.message ?? "error"),
      effect: null,
    };
    const flag = await evalIn("Boolean(window.__aleph_touch)");
    // Only a successful read may set `effect`; a failed read leaves null.
    if (flag.ok) setEffect("Input.dispatchTouchEvent", flag.value === true);
    console.error(`[t0] ${tag} Input.dispatchTouchEvent -> protocol=`
      + `${matrix["Input.dispatchTouchEvent"].protocol} effect=${matrix["Input.dispatchTouchEvent"].effect}`);
  }

  // --- Input.dispatchDragEvent (may not exist at all; the refusal is the finding) ---
  {
    const target = await at("#go");
    const pt = target.clientRect ? { x: target.clientRect.x + 2, y: target.clientRect.y + 2 } : { x: 5, y: 5 };
    const r = await call("Input.dispatchDragEvent", {
      type: "dragEnter", x: pt.x, y: pt.y,
      data: { items: [], dragOperationsMask: 1 },
    });
    matrix["Input.dispatchDragEvent"] = { protocol: r.ok ? "ok" : (r.error.message ?? "error"), effect: null };
    const flag = await evalIn("Boolean(window.__aleph_drag)");
    if (flag.ok) setEffect("Input.dispatchDragEvent", flag.value === true);
  }

  // --- DOM.setAttributeValue (nodeId, not backendNodeId — CDP's own signature) ---
  {
    const target = await at("#go");
    const r = target.nodeId
      ? await call("DOM.setAttributeValue", { nodeId: target.nodeId, name: "data-t0", value: "set" })
      : { ok: false, error: { message: "no nodeId for #go" } };
    matrix["DOM.setAttributeValue"] = { protocol: r.ok ? "ok" : (r.error.message ?? "error"), effect: null };
    const flag = await evalIn(
      "(document.querySelector('#go') && document.querySelector('#go').getAttribute('data-t0')) || ''");
    if (flag.ok) setEffect("DOM.setAttributeValue", flag.value === "set");
  }

  // --- DOM.removeNode: #zero-size is the sacrificial node, asserted on by nothing else ---
  {
    const target = await at("#zero-size");
    const r = target.nodeId
      ? await call("DOM.removeNode", { nodeId: target.nodeId })
      : { ok: false, error: { message: "no nodeId for #zero-size" } };
    matrix["DOM.removeNode"] = { protocol: r.ok ? "ok" : (r.error.message ?? "error"), effect: null };
    const flag = await evalIn("document.querySelector('#zero-size') === null");
    if (flag.ok) setEffect("DOM.removeNode", flag.value === true);
  }

  fc.close();
}

// ---- write everything -------------------------------------------------------------------------
const report = { engine: tag, wsUrl: e.wsUrl, files: [] };
for (const [name, value] of Object.entries(results)) {
  report.files.push(writeFixture(CDP_FIXTURES, name, value));
}
// Only Chrome's void map has a reader (`every_void_method_the_probe_captured_is_accepted_by_its
// _wrapper`). obscura's void answers are in t0-support-matrix.json, which does have one, so no
// obscura-void.json is written (R38).
if (tag === "chrome") report.files.push(writeFixture(CDP_FIXTURES, `${tag}-void.json`, voids));
// merge into the shared support matrix rather than overwriting the other engine's half.
// R48 (controller ruling, 2026-09-06): this file lives at src/browser/engine/fixtures/, beside
// its one reader (Task 16's capability-table test), NOT under crates/aleph-cdp/tests/fixtures/.
const mpath = path.join(ENGINE_FIXTURES, "t0-support-matrix.json");
const existing = fs.existsSync(mpath) ? JSON.parse(fs.readFileSync(mpath, "utf8")) : {};
existing[tag] = matrix;
report.files.push(writeFixture(ENGINE_FIXTURES, "t0-support-matrix.json", existing));
report.totalBytes = report.files.reduce((a, f) => a + f.bytes, 0);
report.oversize = report.files.filter((f) => f.bytes > 4 * 1024 * 1024).map((f) => f.path);
report.unsupported = Object.entries(matrix).filter(([, v]) => v.protocol !== "ok");
report.reportsSuccessButDoesNothing = Object.entries(matrix)
  .filter(([, v]) => v.protocol === "ok" && v.effect === false)
  .map(([k]) => k);
report.verdict = `${tag}: ${report.files.length} fixtures, ${report.totalBytes} bytes total, `
  + `${report.unsupported.length} method(s) not answered: `
  + JSON.stringify(Object.fromEntries(report.unsupported))
  + (report.oversize.length ? ` — OVERSIZE: ${report.oversize.join(", ")}` : "");
emit(report);
c.close(); e.kill(); server.close();
