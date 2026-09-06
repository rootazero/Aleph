// M12: which CDP methods stall during the isolate block. usage: node m12-lock.mjs <wsUrl>
import fs from "node:fs";
import { Cdp, sleep, now, errStr } from "./cdp.mjs";
const [wsUrl] = process.argv.slice(2);
const URL_ = "https://en.wikipedia.org/wiki/Rust_(programming_language)";
const c = new Cdp(wsUrl, "L"); await c.connect();
const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
const { sessionId: s } = await c.call("Target.attachToTarget", { targetId, flatten: true });
await c.call("Page.enable", {}, s); await c.call("Runtime.enable", {}, s).catch(()=>{}); await c.call("DOM.enable", {}, s).catch(()=>{});
await c.call("Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }, s).catch(()=>{});
const lp = c.waitEvent("Page.loadEventFired", s, 60000).catch(e=>errStr(e));
try { await c.call("Page.navigate", { url: URL_ }, s, 60000); } catch (e) { console.error("NAV_ERR " + errStr(e)); }
await lp;
const t0 = now();
// resolve <body> nodeId ONCE, before the stall
let bodyNodeId = null, resolveMs = null;
{ const a = now();
  try { const { root } = await c.call("DOM.getDocument", { depth: 1 }, s, 120000);
        const r = await c.call("DOM.querySelector", { nodeId: root.nodeId, selector: "body" }, s, 120000);
        bodyNodeId = r.nodeId; } catch (e) { console.error("resolve err " + errStr(e)); }
  resolveMs = Math.round(now() - a); }
console.error(`bodyNodeId=${bodyNodeId} resolvedIn=${resolveMs}ms atTSinceLoad=${Math.round(now()-t0)}`);
const timed = async (fn) => { const a = now(); try { await fn(); return Math.round(now() - a); } catch (e) { return { ms: Math.round(now() - a), err: errStr(e).slice(0, 60) }; } };
const rows = [];
while (now() - t0 < 40000) {
  const tick = Math.round(now() - t0);
  // fire ALL FIVE concurrently so no call can absorb the stall on behalf of the others
  const [evaluate, getDocument, getBoxModel, layoutMetrics, getTargets] = await Promise.all([
    timed(() => c.call("Runtime.evaluate", { expression: "1", returnByValue: true }, s, 120000)),
    timed(() => c.call("DOM.getDocument", { depth: 1 }, s, 120000)),
    bodyNodeId ? timed(() => c.call("DOM.getBoxModel", { nodeId: bodyNodeId }, s, 120000)) : Promise.resolve(null),
    timed(() => c.call("Page.getLayoutMetrics", {}, s, 120000)),
    timed(() => c.call("Target.getTargets", {}, undefined, 120000)),
  ]);
  rows.push({ tick, evaluate, getDocument, getBoxModel, layoutMetrics, getTargets });
  console.error(`${tick}\t${JSON.stringify(evaluate)}\t${JSON.stringify(getDocument)}\t${JSON.stringify(getBoxModel)}\t${JSON.stringify(layoutMetrics)}\t${JSON.stringify(getTargets)}`);
  await sleep(500);
}
await c.call("Target.closeTarget", { targetId }).catch(()=>{}); c.close();
fs.writeFileSync("m12-lock.json", JSON.stringify({ url: URL_, bodyNodeId, resolveMs, rows }, null, 1));
console.log("rows=" + rows.length);
