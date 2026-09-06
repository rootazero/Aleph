// M11: four geometry sources for the same 5 elements. usage: node m11-geom.mjs <wsUrl> <tag>
import fs from "node:fs";
import { Cdp, sleep, now, errStr } from "./cdp.mjs";
const [wsUrl, tag = "obs"] = process.argv.slice(2);
const SEL = {
  "hn_title_link":   "a[href=\"news\"]",
  "first_story":     "span.titleline a",
  "first_comments":  "span.subline > a:last-of-type",
  "login_link":      "span.pagetop a[href^='login']",
  "footer_search":   "input[name=q]",
};
const c = new Cdp(wsUrl, tag); await c.connect();
const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
const { sessionId: s } = await c.call("Target.attachToTarget", { targetId, flatten: true });
await c.call("Page.enable", {}, s); await c.call("Runtime.enable", {}, s).catch(()=>{}); await c.call("DOM.enable", {}, s).catch(()=>{});
await c.call("Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }, s).catch(()=>{});
const lp = c.waitEvent("Page.loadEventFired", s, 60000).catch(e=>errStr(e));
await c.call("Page.navigate", { url: "https://news.ycombinator.com/" }, s, 60000);
await lp; await sleep(2500);

const out = { tag, elements: {} };

// (a) getBoundingClientRect via Runtime.evaluate, one call for all five
const expr = `(()=>{const S=${JSON.stringify(SEL)};const o={};for(const k in S){const e=document.querySelector(S[k]);if(!e){o[k]=null;continue;}const r=e.getBoundingClientRect();o[k]={rect:[Math.round(r.x),Math.round(r.y),Math.round(r.width),Math.round(r.height)],text:(e.value!==undefined&&e.tagName==='INPUT')?('[input]'):((e.textContent||'').trim().slice(0,30))};}return JSON.stringify(o);})()`;
const rA = await c.call("Runtime.evaluate", { expression: expr, returnByValue: true }, s, 120000);
const A = JSON.parse(rA.result.value);
for (const k in SEL) out.elements[k] = { selector: SEL[k], a_getBoundingClientRect: A[k]?.rect ?? null, text: A[k]?.text ?? null };

// (b) DOM.getBoxModel via DOM.querySelector
const { root } = await c.call("DOM.getDocument", { depth: 1 }, s, 60000);
for (const k in SEL) {
  try {
    const { nodeId } = await c.call("DOM.querySelector", { nodeId: root.nodeId, selector: SEL[k] }, s, 60000);
    out.elements[k].nodeId = nodeId;
    if (!nodeId) { out.elements[k].b_getBoxModel = "querySelector returned nodeId 0"; continue; }
    const d = await c.call("DOM.describeNode", { nodeId }, s, 60000).catch(()=>null);
    out.elements[k].backendNodeId = d?.node?.backendNodeId ?? null;
    const bm = await c.call("DOM.getBoxModel", { nodeId }, s, 60000);
    const q = bm.model.content;
    out.elements[k].b_getBoxModel = { quad: q, rect: [Math.round(q[0]), Math.round(q[1]), Math.round(q[2]-q[0]), Math.round(q[5]-q[1])], w: bm.model.width, h: bm.model.height };
  } catch (e) { out.elements[k].b_getBoxModel = errStr(e); }
}

// (c) DOMSnapshot.captureSnapshot
try {
  const t0 = now();
  const snap = await c.call("DOMSnapshot.captureSnapshot", { computedStyles: [] }, s, 180000);
  out.c_snapshotMs = Math.round(now() - t0);
  fs.writeFileSync(`m11-${tag}-domsnapshot.json`, JSON.stringify(snap));
  const doc = snap.documents?.[0];
  out.c_docCount = snap.documents?.length;
  if (doc) {
    out.c_layoutNodes = doc.layout?.nodeIndex?.length;
    out.c_nodeCount = doc.nodes?.backendNodeId?.length;
    const backend = doc.nodes.backendNodeId;
    const idxByBackend = new Map(); backend.forEach((b, i) => { if (!idxByBackend.has(b)) idxByBackend.set(b, i); });
    const li = new Map(); doc.layout.nodeIndex.forEach((ni, i) => { if (!li.has(ni)) li.set(ni, i); });
    for (const k in SEL) {
      const bn = out.elements[k].backendNodeId;
      if (bn == null) { out.elements[k].c_domSnapshot = "no backendNodeId"; continue; }
      const ni = idxByBackend.get(bn);
      if (ni === undefined) { out.elements[k].c_domSnapshot = `backendNodeId ${bn} absent from snapshot nodes`; continue; }
      const l = li.get(ni);
      if (l === undefined) { out.elements[k].c_domSnapshot = `node index ${ni} has no layout entry`; continue; }
      const b = doc.layout.bounds[l];
      out.elements[k].c_domSnapshot = { layoutIdx: l, nodeIdx: ni, bounds: b, rect: b ? b.map(x=>Math.round(x)) : null };
    }
    // is the whole layout a synthesized 1280x18 stack?
    const bs = doc.layout.bounds || [];
    const widths = new Set(bs.slice(0, 200).map(b=>Math.round(b[2])));
    const heights = new Set(bs.slice(0, 200).map(b=>Math.round(b[3])));
    out.c_first200_distinctWidths = [...widths].slice(0, 12);
    out.c_first200_distinctHeights = [...heights].slice(0, 12);
    out.c_first10_bounds = bs.slice(0, 10).map(b=>b.map(x=>Math.round(x)));
  }
} catch (e) { out.c_error = errStr(e); }

// (d) screenshot
try { const sh = await c.call("Page.captureScreenshot", { format: "png" }, s, 90000); fs.writeFileSync(`m11-${tag}-shot.png`, Buffer.from(sh.data, "base64")); out.d_png = `m11-${tag}-shot.png`; } catch (e) { out.d_error = errStr(e); }

// control: is getBoxModel returning the documented constant fallback quad anywhere?
out.constantFallbackQuad = [8,8,108,8,108,28,8,28];
await c.call("Target.closeTarget", { targetId }).catch(()=>{});
c.close();
console.log(JSON.stringify(out, null, 1));
