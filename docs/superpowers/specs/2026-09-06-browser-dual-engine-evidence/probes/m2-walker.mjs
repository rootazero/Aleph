// M2: JS walker. usage: node m2-walker.mjs <wsUrl> <engineName> <url...>
import fs from "node:fs";
import { Cdp, sleep, now, errStr } from "./cdp.mjs";
const [wsUrl, engine, ...urls] = process.argv.slice(2);
const walker = fs.readFileSync(new URL("./walker.js", import.meta.url), "utf8").trim();
const res = { engine, wsUrl, sites: [] };
for (const url of urls) {
  const c = new Cdp(wsUrl, engine); await c.connect();
  const rec = { url };
  try {
    const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
    const { sessionId: s } = await c.call("Target.attachToTarget", { targetId, flatten: true });
    await c.call("Page.enable", {}, s); await c.call("Runtime.enable", {}, s).catch(()=>{});
    await c.call("Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }, s).catch(()=>{});
    const lp = c.waitEvent("Page.loadEventFired", s, 60000).catch(e=>errStr(e));
    const t0 = now();
    try { await c.call("Page.navigate", { url }, s, 60000); } catch (e) { rec.navErr = errStr(e); }
    await lp; rec.navMs = Math.round(now() - t0);
    await sleep(2500); // let the post-load storm settle so we measure the walker, not the stall
    const t1 = now();
    let r;
    try { r = await c.call("Runtime.evaluate", { expression: walker, returnByValue: true }, s, 180000); }
    catch (e) { rec.walkErr = errStr(e); rec.walkMs = Math.round(now() - t1); }
    if (r) {
      rec.walkMs = Math.round(now() - t1);
      const v = r.result?.value;
      if (!v) { rec.walkErr = "no value: " + JSON.stringify(r).slice(0, 400); }
      else {
        rec.count = v.n; rec.interactive = v.els.filter(e=>e.interactive).length;
        rec.viewport = v.viewport; rec.scroll = [v.scrollX, v.scrollY]; rec.docHeight = v.docHeight;
        const json = JSON.stringify(v.els); rec.bytes = Buffer.byteLength(json);
        const slug = url.replace(/[^a-z0-9]+/gi, "_").slice(0, 40);
        fs.writeFileSync(`walk-${engine}-${slug}.json`, JSON.stringify(v, null, 0));
        rec.sample = v.els.filter(e=>e.interactive && e.name).slice(0, 3).map(e=>({tag:e.tag,name:e.name.replace(/\n/g," ").slice(0,40),rect:e.rect}));
        rec.file = `walk-${engine}-${slug}.json`;
      }
    }
    // screenshot for the spot-check
    try {
      const sh = await c.call("Page.captureScreenshot", { format: "png" }, s, 60000);
      const slug = url.replace(/[^a-z0-9]+/gi, "_").slice(0, 40);
      fs.writeFileSync(`shot-${engine}-${slug}.png`, Buffer.from(sh.data, "base64"));
      rec.shot = `shot-${engine}-${slug}.png`; rec.shotBytes = Buffer.from(sh.data, "base64").length;
    } catch (e) { rec.shotErr = errStr(e); }
    await c.call("Target.closeTarget", { targetId }).catch(()=>{});
  } catch (e) { rec.error = errStr(e); }
  c.close();
  res.sites.push(rec);
  console.error(`[${engine}] ${url} nav=${rec.navMs}ms walk=${rec.walkMs}ms n=${rec.count} interactive=${rec.interactive} bytes=${rec.bytes} ${rec.walkErr||rec.error||""}`);
}
console.log(JSON.stringify(res, null, 1));
