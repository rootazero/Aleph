// fetch-probe.mjs — the measurement the `network_interception` capability row's
// NOT_PROBED note promised: does a REAL obscura serve the Fetch handshake Aleph's
// mock routes are built on (Fetch.enable → requestPaused → fulfillRequest)?
//
//   node fetch-probe.mjs
//
// The handshake has to hold in BOTH directions before the row may say
// Supported, so the probe measures each link on its own rather than one
// end-to-end green (判据 §4 — a stage that only asserts the call succeeded is
// a protocol reading, not the effect):
//
//   1. CONTROL, interception off: the fixture page loads AND a subresource
//      `fetch()` completes. A red anywhere below can then only mean the Fetch
//      path, never a broken page/server (判据 §2).
//   2. `Fetch.enable` with the exact catch-all pattern
//      `methods::fetch::enable` sends. A refusal here = the enable path does
//      not exist.
//   3. NAVIGATION pause: a `requestPaused` must arrive for `Page.navigate`,
//      and — the link a pure event-count cannot see — the request must
//      actually be HELD: the page may not reach `complete` with the real body
//      while the pause sits unanswered. An event the engine does not wait on
//      is advisory, and a mock that answers after the real response already
//      landed is a fulfilled-nothing (判据 §11).
//   4. `Fetch.fulfillRequest` on that pause: the PAGE reads back what it
//      received. Only the mock marker proving arrival settles this link.
//   5. SUBRESOURCE pause: a page-issued `fetch()` must also produce a
//      `requestPaused` — `browser_network`'s routes exist mostly for XHR, so
//      an engine that pauses navigations only does not serve the verb.
//
// Every arm is recorded in the emitted JSON whether or not it fails, so the
// capability row's comment can quote the mechanism rather than the verdict.
//
// Lives beside the spec that demanded it (this directory), reusing the
// 2026-09-06 evidence round's launch/server helpers rather than growing a
// second copy — the launch argv (`--allow-private-network`, never
// `--allow-file-access`) is a measured property of those helpers, not
// something to re-derive here.
import path from "node:path";
import { fileURLToPath } from "node:url";
import { launchObscura, serveStatic, freePort, Cdp, sleep, emit }
  from "../../2026-09-06-browser-dual-engine-evidence/probes/t0-lib.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const MOCK_BODY = "<!doctype html><html><head><title>fetch-probe-mock</title></head>"
  + "<body>MOCK-SERVED-7f3a</body></html>";

const report = { engine: "obscura", steps: {} };
const done = (verdict) => ({ verdict, ...report });

const port = await freePort();
const server = await serveStatic(HERE, port);
const pageUrl = `http://127.0.0.1:${port}/fetch-page.html`;

const e = await launchObscura({});
if (!e.wsUrl) { server.close(); throw new Error("obscura published no CDP endpoint"); }
report.wsUrl = e.wsUrl;
report.pid = e.pid;

const c = new Cdp(e.wsUrl, "fetch");
await c.connect();
const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
const { sessionId } = await c.call("Target.attachToTarget", { targetId, flatten: true });
await c.call("Page.enable", {}, sessionId).catch(() => {});
await c.call("Runtime.enable", {}, sessionId).catch(() => {});

const evalJs = async (expr) => {
  const r = await c.call("Runtime.evaluate", { expression: expr, returnByValue: true },
    sessionId, 10000);
  return r?.result?.value ?? null;
};
const bodyText = () => evalJs("document.body ? document.body.textContent.trim() : '<none>'");
const finish = async (out) => { c.close(); e.kill(); server.close(); emit(out); };
const bail = async (verdict) => { await finish(done(verdict)); process.exit(0); };

// ---- step 1: control, interception OFF --------------------------------------
await c.call("Page.navigate", { url: pageUrl }, sessionId, 30000);
await sleep(1200);
report.steps.control_page = await bodyText();
await evalJs("fetch('/fetch-page.html').then(r => r.text())"
  + ".then(t => { window.__fr = t.includes('REAL-SERVED-7f3a') ? 'ok' : t.slice(0, 60); })"
  + ".catch(x => { window.__fr = 'ERR:' + x; })");
await sleep(2000);
report.steps.control_subresource = await evalJs("window.__fr ?? '<pending>'");
if (report.steps.control_page !== "REAL-SERVED-7f3a"
    || report.steps.control_subresource !== "ok") {
  await bail("FIXTURE BROKEN — unintercepted page/subresource did not behave; nothing below can be believed");
}

// ---- step 2: Fetch.enable, the exact shape methods::fetch::enable sends -----
const enable = await c.call("Fetch.enable", { patterns: [{ urlPattern: "*" }] }, sessionId, 15000)
  .then(() => ({ ok: true }))
  .catch((err) => ({ ok: false, error: String(err?.message ?? err) }));
report.steps.fetch_enable = enable;
if (!enable.ok) {
  await bail(`UNSUPPORTED (Fetch.enable refused): ${enable.error}`);
}

// ---- step 3: navigation pause — the event must arrive AND hold --------------
// Subscribe BEFORE the navigate: a subscriber that registers after the event
// fired cannot see it — the same ordering rule the Rust loop obeys.
const navPaused = c.waitEvent("Fetch.requestPaused", sessionId, 15000).catch((x) => String(x));
await c.call("Page.navigate", { url: pageUrl }, sessionId, 30000);
const navEvent = await navPaused;
if (typeof navEvent === "string") {
  report.steps.nav_request_paused = navEvent;
  await bail(`UNSUPPORTED (no requestPaused for the navigation — enable is a no-op): ${navEvent}`);
}
report.steps.nav_request_paused = { requestId: navEvent.requestId, url: navEvent.request?.url };

// Deliberately UNANSWERED for 1.5 s: a held request cannot have completed.
await sleep(1500);
report.steps.nav_while_unanswered = await evalJs(
  "document.readyState + '|' + (document.body ? document.body.textContent.trim() : '<none>')");
const navHeld = report.steps.nav_while_unanswered !== "complete|REAL-SERVED-7f3a";

// ---- step 4: fulfill the navigation pause; the page reads the verdict -------
const navFulfill = await c.call("Fetch.fulfillRequest", {
  requestId: navEvent.requestId,
  responseCode: 200,
  responseHeaders: [{ name: "content-type", value: "text/html; charset=utf-8" }],
  body: Buffer.from(MOCK_BODY, "utf8").toString("base64"),
}, sessionId, 15000)
  .then(() => ({ ok: true }))
  .catch((err) => ({ ok: false, error: String(err?.message ?? err) }));
report.steps.nav_fulfill = navFulfill;
await sleep(1500);
report.steps.nav_page_received = await bodyText();
const navServed = navFulfill.ok && report.steps.nav_page_received === "MOCK-SERVED-7f3a";

// ---- step 5: subresource pause — XHR is what the routes exist for -----------
const subPaused = c.waitEvent("Fetch.requestPaused", sessionId, 6000).catch((x) => String(x));
await evalJs("fetch('/fetch-page.html').then(r => r.text())"
  + ".then(t => { window.__fr2 = t.includes('REAL-SERVED-7f3a') ? 'real' : t.slice(0, 60); })"
  + ".catch(x => { window.__fr2 = 'ERR:' + x; })");
const subEvent = await subPaused;
await sleep(2000);
report.steps.sub_while_unanswered = await evalJs("window.__fr2 ?? '<pending>'");
if (typeof subEvent === "string") {
  report.steps.sub_request_paused = subEvent;
} else {
  report.steps.sub_request_paused = { requestId: subEvent.requestId, url: subEvent.request?.url };
  const subFulfill = await c.call("Fetch.fulfillRequest", {
    requestId: subEvent.requestId,
    responseCode: 200,
    responseHeaders: [{ name: "content-type", value: "text/plain" }],
    body: Buffer.from("MOCK-SUB-9z", "utf8").toString("base64"),
  }, sessionId, 10000)
    .then(() => ({ ok: true }))
    .catch((err) => ({ ok: false, error: String(err?.message ?? err) }));
  report.steps.sub_fulfill = subFulfill;
  await sleep(1500);
  report.steps.sub_received = await evalJs("window.__fr2 ?? '<pending>'");
}
const subServed = typeof subEvent !== "string"
  && report.steps.sub_fulfill?.ok === true
  && (report.steps.sub_received ?? "").includes("MOCK-SUB-9z");

// ---- verdict ----------------------------------------------------------------
const links = { enable: enable.ok, navEvent: true, navHeld, navServed, subServed };
report.links = links;
const verdict = Object.values(links).every(Boolean)
  ? "SUPPORTED — enable armed, both pauses held, both fulfills served the mock body"
  : `UNSUPPORTED — measured broken link(s): ${
      Object.entries(links).filter(([, v]) => !v).map(([k]) => k).join(", ")}`;
await finish(done(verdict));
