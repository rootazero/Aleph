// M1b: map the starvation window. eval "1" every 500ms from loadEventFired to +45s wall.
import { Cdp, sleep, now, errStr } from "./cdp.mjs";
const [wsUrl, url] = process.argv.slice(2);
const c = new Cdp(wsUrl, "w"); await c.connect();
const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
const { sessionId: s } = await c.call("Target.attachToTarget", { targetId, flatten: true });
await c.call("Page.enable", {}, s); await c.call("Runtime.enable", {}, s).catch(()=>{});
const loadP = c.waitEvent("Page.loadEventFired", s, 60000).catch(e=>errStr(e));
try { await c.call("Page.navigate", { url }, s, 60000); } catch(e) { console.error("NAV_ERR "+errStr(e)); }
await loadP;
const t0 = now(); const samples = [];
while (now() - t0 < 45000) {
  const a = now();
  try { await c.call("Runtime.evaluate", { expression: "1", returnByValue: true }, s, 60000); } catch (e) { samples.push({ tSinceLoad: Math.round(a - t0), ms: null, err: errStr(e) }); continue; }
  samples.push({ tSinceLoad: Math.round(a - t0), ms: Math.round(now() - a) });
  await sleep(500);
}
await c.call("Target.closeTarget", { targetId }).catch(()=>{});
c.close();
console.log(JSON.stringify({ url, samples }, null, 1));
console.error(samples.map(x => `${x.tSinceLoad}:${x.ms ?? x.err}`).join("  "));
