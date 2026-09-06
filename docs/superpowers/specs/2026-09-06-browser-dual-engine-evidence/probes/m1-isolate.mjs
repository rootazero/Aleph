// M1: isolate latency. usage: node m1-isolate.mjs <wsUrl> <url> <runs>
import fs from "node:fs";
import { Cdp, sleep, now, errStr, q } from "./cdp.mjs";
const [wsUrl, url, runsS = "3"] = process.argv.slice(2);
const runs = Number(runsS);
const out = { url, wsUrl, runs: [] };
const evalRtt = async (c, s) => {
  const t0 = now();
  let v;
  try { v = (await c.call("Runtime.evaluate", { expression: "1", returnByValue: true }, s, 120000)).result?.value; }
  catch (e) { return { ms: Math.round(now() - t0), err: errStr(e) }; }
  return { ms: Math.round(now() - t0), v };
};
for (let r = 0; r < runs; r++) {
  const c = new Cdp(wsUrl, `r${r}`); await c.connect();
  const rec = { run: r };
  try {
    const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
    const { sessionId: s } = await c.call("Target.attachToTarget", { targetId, flatten: true });
    await c.call("Page.enable", {}, s); await c.call("Runtime.enable", {}, s).catch(() => {});
    await c.call("Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }, s).catch(() => {});
    const tNav = now();
    const loadP = c.waitEvent("Page.loadEventFired", s, 60000).then(() => "load").catch((e) => errStr(e));
    await c.call("Page.navigate", { url }, s, 60000);
    const loadRes = await loadP;
    rec.navToLoadMs = Math.round(now() - tNav); rec.load = loadRes;
    rec.evalAtLoad = await evalRtt(c, s);
    await sleep(8000);
    rec.evalAfter8s = await evalRtt(c, s);
    rec.evalSeries = [];
    for (let i = 0; i < 5; i++) { rec.evalSeries.push(await evalRtt(c, s)); await sleep(1000); }
    await c.call("Target.closeTarget", { targetId }).catch(() => {});
  } catch (e) { rec.error = errStr(e); }
  c.close();
  out.runs.push(rec);
  console.error(`run ${r}: load=${rec.navToLoadMs}ms atLoad=${rec.evalAtLoad?.ms}ms after8s=${rec.evalAfter8s?.ms}ms series=${(rec.evalSeries||[]).map(x=>x.ms).join(",")}`);
}
const all = [];
for (const r of out.runs) { if (r.evalAtLoad?.ms != null) all.push(r.evalAtLoad.ms); }
out.summary = {
  atLoad: q(out.runs.filter(r=>r.evalAtLoad).map(r => r.evalAtLoad.ms)),
  after8s: q(out.runs.filter(r=>r.evalAfter8s).map(r => r.evalAfter8s.ms)),
  series: q(out.runs.flatMap(r => (r.evalSeries||[]).map(x => x.ms))),
  navToLoad: q(out.runs.map(r => r.navToLoadMs).filter(x=>x!=null)),
};
console.log(JSON.stringify(out, null, 1));
