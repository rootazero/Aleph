// M12e: fixed 500ms schedule, ticks NOT awaited, so a stalled tick cannot eat the sampling window.
import fs from "node:fs";
import { Cdp, sleep, now, errStr } from "./cdp.mjs";
const [wsUrl] = process.argv.slice(2);
const URL_ = "https://en.wikipedia.org/wiki/Rust_(programming_language)";
const c = new Cdp(wsUrl, "D"); await c.connect();
const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
const { sessionId: s } = await c.call("Target.attachToTarget", { targetId, flatten: true });
await c.call("Page.enable", {}, s); await c.call("Runtime.enable", {}, s).catch(()=>{}); await c.call("DOM.enable", {}, s).catch(()=>{});
await c.call("Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }, s).catch(()=>{});

// navigate with retry: this network drops requests intermittently, and a failed nav silently
// leaves about:blank, which would make every RTT look healthy.
let loadP = null, navOk = false;
for (let attempt = 1; attempt <= 5 && !navOk; attempt++) {
  loadP = c.waitEvent("Page.loadEventFired", s, 60000).then(()=>"load").catch(e=>errStr(e));
  try { await c.call("Page.navigate", { url: URL_ }, s, 60000); navOk = true; }
  catch (e) { console.error(`NAV_ATTEMPT_${attempt}_FAILED ` + errStr(e).slice(0,80)); await sleep(1500); }
}
if (!navOk) { console.error("NAV_GAVE_UP"); process.exit(3); }

// resolve <body> BEFORE loadEventFired if possible
let body = null, bodyWhen = null;
{
  const deadline = now() + 25000;
  let loaded = false; loadP.then(()=>{ loaded = true; });
  while (now() < deadline && body == null) {
    try {
      const { root } = await c.call("DOM.getDocument", { depth: 1 }, s, 8000);
      const r = await c.call("DOM.querySelector", { nodeId: root.nodeId, selector: "body" }, s, 8000);
      if (r.nodeId) { body = r.nodeId; bodyWhen = loaded ? "after loadEventFired" : "BEFORE loadEventFired"; break; }
    } catch (e) { /* retry */ }
    if (loaded) break;
    await sleep(150);
  }
}
const loadRes = await loadP;
if (body == null) {
  const { root } = await c.call("DOM.getDocument", { depth: 1 }, s, 60000);
  body = (await c.call("DOM.querySelector", { nodeId: root.nodeId, selector: "body" }, s, 60000)).nodeId;
  bodyWhen = "after loadEventFired (fallback)";
}
const urlCheck = await c.call("Runtime.evaluate", { expression: "location.href + ' | ' + document.title", returnByValue: true }, s, 120000).then(r=>r.result.value).catch(e=>errStr(e));
console.error(`bodyNodeId=${body} resolved ${bodyWhen}; load=${loadRes}; page=${urlCheck}`);
if (!String(urlCheck).includes("wikipedia.org")) { console.error("WRONG_PAGE_ABORT: " + urlCheck); process.exit(4); }

const t0 = now();
const recs = [];
const fire = (tick, name, p) => {
  const a = now();
  recs.push({ tick, name, issued: Math.round(a - t0), ms: null, done: null, err: null });
  const rec = recs[recs.length - 1];
  p.then(() => { rec.ms = Math.round(now() - a); rec.done = Math.round(now() - t0); },
         (e) => { rec.ms = Math.round(now() - a); rec.done = Math.round(now() - t0); rec.err = errStr(e).slice(0, 50); });
};
const TICKS = 80; // 40 s at 500 ms
for (let i = 0; i < TICKS; i++) {
  const target = t0 + i * 500;
  const wait = target - now(); if (wait > 0) await sleep(wait);
  const tick = Math.round(now() - t0);
  fire(tick, "evaluate",      c.call("Runtime.evaluate", { expression: "1", returnByValue: true }, s, 120000));
  fire(tick, "getDocument",   c.call("DOM.getDocument", { depth: 1 }, s, 120000));
  fire(tick, "getBoxModel",   c.call("DOM.getBoxModel", { nodeId: body }, s, 120000));
  fire(tick, "layoutMetrics", c.call("Page.getLayoutMetrics", {}, s, 120000));
  fire(tick, "getTargets",    c.call("Target.getTargets", {}, undefined, 120000));
}
// drain
const drainDeadline = now() + 90000;
while (now() < drainDeadline && recs.some(r => r.ms === null)) await sleep(250);
await c.call("Target.closeTarget", { targetId }).catch(()=>{});
c.close();
fs.writeFileSync("m12e-dense.json", JSON.stringify({ url: URL_, body, bodyWhen, recs }, null, 1));
const NAMES = ["evaluate","getDocument","getBoxModel","layoutMetrics","getTargets"];
const byTick = new Map();
for (const r of recs) { if (!byTick.has(r.tick)) byTick.set(r.tick, {}); byTick.get(r.tick)[r.name] = r; }
console.error("tick\t" + NAMES.join("\t"));
for (const [tick, o] of [...byTick.entries()].sort((a,b)=>a[0]-b[0])) {
  console.error(tick + "\t" + NAMES.map(n => o[n] ? (o[n].err ? "ERR" : o[n].ms) : "-").join("\t"));
}
console.error("--- completion instants (done ms since load) for the first 6 ticks ---");
for (const [tick, o] of [...byTick.entries()].sort((a,b)=>a[0]-b[0]).slice(0,6)) {
  console.error(tick + "\t" + NAMES.map(n => o[n] ? o[n].done : "-").join("\t"));
}
console.log("samples=" + recs.length + " unfinished=" + recs.filter(r=>r.ms===null).length);
