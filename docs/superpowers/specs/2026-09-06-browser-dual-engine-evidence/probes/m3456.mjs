// M3 geometry / M4 AX / M5 multi-statement / M6 connection scoping. usage: node m3456.mjs <wsUrl> <engine> <url>
import fs from "node:fs";
import { Cdp, sleep, now, errStr } from "./cdp.mjs";
const [wsUrl, engine, url = "https://news.ycombinator.com/"] = process.argv.slice(2);
const out = { engine, url, wsUrl };
const log = (k, v) => { out[k] = v; console.error(`## ${k}: ${JSON.stringify(v).slice(0, 700)}`); };
const c = new Cdp(wsUrl, "A"); await c.connect();
const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
const { sessionId: s } = await c.call("Target.attachToTarget", { targetId, flatten: true });
await c.call("Page.enable", {}, s); await c.call("Runtime.enable", {}, s).catch(()=>{});
await c.call("DOM.enable", {}, s).catch(()=>{});
await c.call("Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }, s).catch(()=>{});
const lp = c.waitEvent("Page.loadEventFired", s, 60000).catch(e=>errStr(e));
try { await c.call("Page.navigate", { url }, s, 60000); } catch (e) { log("navErr", errStr(e)); }
await lp; await sleep(3000);

// ---- M3 ----
let t = now(); let doc;
try { doc = await c.call("DOM.getDocument", { depth: -1, pierce: true }, s, 180000); log("M3_getDocument_ms", Math.round(now() - t)); }
catch (e) { log("M3_getDocument_err", errStr(e)); log("M3_getDocument_ms", Math.round(now() - t)); }
const nodeIds = [];
let total = 0;
if (doc) {
  const walk = (n) => { total++; if (n.nodeType === 1) nodeIds.push(n.nodeId); for (const ch of (n.children || [])) walk(ch); if (n.contentDocument) walk(n.contentDocument); for (const sr of (n.shadowRoots||[])) walk(sr); };
  walk(doc.root);
  log("M3_nodes_total", total); log("M3_element_nodes", nodeIds.length);
}
const first200 = nodeIds.slice(0, 200);
t = now(); let bmOk = 0, bmFail = 0; const bmErrs = new Set();
for (const nid of first200) {
  try { const r = await c.call("DOM.getBoxModel", { nodeId: nid }, s, 60000); if (r?.model) bmOk++; else { bmFail++; bmErrs.add("no model"); } }
  catch (e) { bmFail++; bmErrs.add(errStr(e).slice(0, 120)); }
}
log("M3_getBoxModel_200", { totalMs: Math.round(now() - t), ok: bmOk, fail: bmFail, perCallMs: +((now()-t)/Math.max(1,first200.length)).toFixed(2), errs: [...bmErrs].slice(0,4) });
// sample a box model shape
if (first200.length) { try { const r = await c.call("DOM.getBoxModel", { nodeId: first200[Math.min(20,first200.length-1)] }, s); log("M3_boxModel_sample", { keys: Object.keys(r.model), width: r.model.width, height: r.model.height, contentLen: r.model.content?.length }); } catch (e) { log("M3_boxModel_sample_err", errStr(e)); } }
const quads = [];
for (const nid of first200.slice(10, 13)) { try { const r = await c.call("DOM.getContentQuads", { nodeId: nid }, s, 30000); quads.push({ nid, quads: r.quads?.length, first: r.quads?.[0]?.slice(0,4) }); } catch (e) { quads.push({ nid, err: errStr(e).slice(0,120) }); } }
log("M3_getContentQuads_x3", quads);
try { log("M3_getLayoutMetrics", await c.call("Page.getLayoutMetrics", {}, s, 30000)); } catch (e) { log("M3_getLayoutMetrics_err", errStr(e)); }

// ---- M4 ----
t = now();
try {
  const ax = await c.call("Accessibility.getFullAXTree", {}, s, 180000);
  const ms = Math.round(now() - t);
  const links = ax.nodes.filter(n => n.role?.value === "link");
  const named = links.filter(n => n.name?.value && String(n.name.value).trim().length);
  log("M4_getFullAXTree", { ms, nodes: ax.nodes.length, links: links.length, linksNamed: named.length,
    sampleNames: named.slice(0, 3).map(n => String(n.name.value).slice(0, 40)),
    rawLinkKeys: links[1] ? Object.keys(links[1]) : null,
    ignoredCount: ax.nodes.filter(n=>n.ignored).length });
  const one = links[1] || links[0];
  if (one) { try { const p = await c.call("Accessibility.getPartialAXTree", { nodeId: one.backendDOMNodeId ? undefined : undefined, backendNodeId: one.backendDOMNodeId, fetchRelatives: true }, s, 60000); log("M4_getPartialAXTree", { nodes: p.nodes?.length, roles: p.nodes?.slice(0,4).map(n=>n.role?.value), names: p.nodes?.slice(0,4).map(n=>n.name?.value) }); } catch (e) { log("M4_getPartialAXTree_err", errStr(e)); } }
} catch (e) { log("M4_getFullAXTree_err", { ms: Math.round(now() - t), err: errStr(e) }); }

// ---- M5 ----
const evalRaw = async (expr) => { try { const r = await c.call("Runtime.evaluate", { expression: expr, returnByValue: true }, s, 60000); return r.exceptionDetails ? { exception: r.exceptionDetails.text + " / " + (r.exceptionDetails.exception?.description||"").split("\n")[0] } : { value: r.result?.value }; } catch (e) { return { protocolError: errStr(e) }; } };
log("M5_eval_1_semicolon_2", await evalRaw("1; 2"));
log("M5_eval_multiline_var", await evalRaw("var a=1;\nvar b=2;\na+b"));
log("M5_eval_iife", await evalRaw("(()=>{const a=1;const b=2;return a+b;})()"));
try {
  const g = await c.call("Runtime.evaluate", { expression: "window", returnByValue: false }, s);
  const oid = g.result?.objectId;
  const r = await c.call("Runtime.callFunctionOn", { functionDeclaration: "function(){ const a=1; const b=2; let c=0; for(let i=0;i<3;i++){c+=i;} return a+b+c; }", objectId: oid, returnByValue: true }, s, 60000);
  log("M5_callFunctionOn_multistatement", r.exceptionDetails ? { exception: r.exceptionDetails.text } : { value: r.result?.value });
} catch (e) { log("M5_callFunctionOn_err", errStr(e)); }

// ---- M6 ----
const B = new Cdp(wsUrl, "B"); await B.connect();
try { const g = await B.call("Target.getTargets", {}, undefined, 30000); log("M6_connB_getTargets", { n: g.targetInfos.length, infos: g.targetInfos.map(t2 => ({ type: t2.type, url: String(t2.url).slice(0, 40) })) }); } catch (e) { log("M6_connB_getTargets_err", errStr(e)); }
try { const a = await B.call("Target.attachToTarget", { targetId, flatten: true }, undefined, 30000); log("M6_connB_attach", { sessionId: a.sessionId }); } catch (e) { log("M6_connB_attach_err", errStr(e)); }
try { await B.call("Target.setDiscoverTargets", { discover: true }, undefined, 30000); await sleep(500); const g2 = await B.call("Target.getTargets", {}, undefined, 30000); log("M6_connB_getTargets_after_discover", { n: g2.targetInfos.length }); } catch (e) { log("M6_connB_discover_err", errStr(e)); }
B.close();

await c.call("Target.closeTarget", { targetId }).catch(()=>{});
c.close();
console.log(JSON.stringify(out, null, 1));
