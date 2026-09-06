// M10: screenshots. usage: node m10-shots.mjs <wsUrl> <tag> <url...>
import fs from "node:fs";
import { Cdp, sleep, now, errStr } from "./cdp.mjs";
const [wsUrl, tag, ...urls] = process.argv.slice(2);
for (const url of urls) {
  const c = new Cdp(wsUrl, tag); await c.connect();
  const rec = { url };
  try {
    const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
    const { sessionId: s } = await c.call("Target.attachToTarget", { targetId, flatten: true });
    await c.call("Page.enable", {}, s); await c.call("Runtime.enable", {}, s).catch(()=>{});
    await c.call("Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }, s).catch(()=>{});
    const lp = c.waitEvent("Page.loadEventFired", s, 45000).catch(e => errStr(e));
    const t0 = now();
    try { await c.call("Page.navigate", { url }, s, 45000); } catch (e) { rec.navErr = errStr(e); }
    await lp; rec.navMs = Math.round(now() - t0);
    await sleep(4000);
    try { const r = await c.call("Runtime.evaluate", { expression: "JSON.stringify({t:document.title,u:location.href,len:(document.body&&document.body.innerText||'').length,h:document.documentElement.scrollHeight})", returnByValue: true }, s, 120000); rec.page = r.result?.value; } catch (e) { rec.pageErr = errStr(e); }
    const slug = url.replace(/^https?:\/\//, "").replace(/[^a-z0-9]+/gi, "_").slice(0, 30);
    try { const sh = await c.call("Page.captureScreenshot", { format: "png" }, s, 90000); const b = Buffer.from(sh.data, "base64"); fs.writeFileSync(`m10-${tag}-${slug}.png`, b); rec.png = `m10-${tag}-${slug}.png`; rec.pngBytes = b.length; }
    catch (e) { rec.shotErr = errStr(e); }
    await c.call("Target.closeTarget", { targetId }).catch(()=>{});
  } catch (e) { rec.error = errStr(e); }
  c.close();
  console.log(JSON.stringify(rec));
}
