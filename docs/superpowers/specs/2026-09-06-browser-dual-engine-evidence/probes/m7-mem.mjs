// M7: hold N targets open, print a marker, wait for stdin line to advance.
import { Cdp, sleep, errStr } from "./cdp.mjs";
const [wsUrl, engine] = process.argv.slice(2);
const urls = ["https://github.com", "https://news.ycombinator.com/", "https://en.wikipedia.org/wiki/Rust_(programming_language)"];
const c = new Cdp(wsUrl, engine); await c.connect();
const held = [];
for (let i = 0; i < urls.length; i++) {
  const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
  const { sessionId: s } = await c.call("Target.attachToTarget", { targetId, flatten: true });
  await c.call("Page.enable", {}, s);
  await c.call("Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }, s).catch(()=>{});
  const lp = c.waitEvent("Page.loadEventFired", s, 60000).catch(e=>errStr(e));
  try { await c.call("Page.navigate", { url: urls[i] }, s, 60000); } catch (e) { console.error("navErr " + errStr(e)); }
  await lp; await sleep(3000);
  held.push({ targetId, s });
  console.error(`MARK ${i + 1} targets loaded: ${urls.slice(0, i + 1).join(", ")}`);
  console.log(`MARK${i + 1}`);
  await sleep(6000); // hold so the shell can sample RSS
}
await sleep(4000);
for (const h of held) await c.call("Target.closeTarget", { targetId: h.targetId }).catch(()=>{});
c.close();
console.log("DONE");
