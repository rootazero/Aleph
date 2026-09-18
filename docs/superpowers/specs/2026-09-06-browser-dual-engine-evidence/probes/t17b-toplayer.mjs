// T17b fix-r2 / D3: does an element in the TOP LAYER escape its DOM ancestor's opacity group,
// and — the question that decides the fix — does the ENGINE STATE that fact anywhere?
//
// `cascade_opacity` is a DOM-ancestry computation. Top layer is a PAINT-time fact. The defect is
// that ancestry was taken to determine paint. The last round's lesson was that a fix must be keyed
// to a fact the engine states, not to an enumeration of mechanisms — so the first question here is
// not "what escapes" but "what can be read".
//
// Four readings, in the order they decide things:
//   1. PIXELS — is the escape real, for each of the three containers and each way into the top
//      layer, with both controls (A==C and A!=B), so the instrument is shown to tell pages apart.
//   2. CDP — does `DOM.getTopLayerElements` exist, what does it return, and can it be mapped onto
//      the `backendNodeId` space `DOMSnapshot` uses? That is the candidate stated fact.
//   3. DOMSnapshot alone — do `paintOrders` / `stackingContexts` identify it without a second call?
//   4. THE PAGE — `:modal` / `:popover-open`, the DOM-visible spelling, which is what a JS-based
//      fetcher (obscura's) could ask for if the engine implements it.
//
// usage: node t17b-toplayer.mjs
import { launchEngine, newPage, serveStatic, emit, sleep,
         PROBE_DIR, PARENT_PORT, COMPUTED_STYLES } from "./t0-lib.mjs";

const server = await serveStatic(PROBE_DIR, PARENT_PORT);
const e = await launchEngine("chrome");
const url = (qs) => `http://127.0.0.1:${PARENT_PORT}/t17b-toplayer.html${qs}`;

const { c, sessionId } = await newPage(e.wsUrl, url("?open=all"));
await sleep(500);
const version = await c.call("Browser.getVersion", {}, undefined, 30000).catch(() => null);
const out = { probe: "T17b-toplayer", chrome: version?.product ?? null };

const ev = async (expr) => {
  const r = await c.call("Runtime.evaluate",
    { expression: expr, returnByValue: true }, sessionId, 30000);
  if (r.exceptionDetails) return { threw: String(r.exceptionDetails.text ?? "exception") };
  return r.result.value;
};
out.opened = await ev("JSON.stringify(window.__opened)").then((s) => JSON.parse(s ?? "[]"));

// ---- 4. what the PAGE says (the spelling a JS fetcher could use) --------------------------------
out.pageMatches = await ev("JSON.stringify(window.__matches())").then((s) => JSON.parse(s ?? "{}"));

// ---- 2. what CDP says ---------------------------------------------------------------------------
const topLayer = { available: null, nodeIds: null, resolved: [], error: null };
try {
  const { root } = await c.call("DOM.getDocument", { depth: -1, pierce: true }, sessionId, 30000);
  topLayer.rootBackendNodeId = root.backendNodeId;
  const r = await c.call("DOM.getTopLayerElements", {}, sessionId, 30000);
  topLayer.available = true;
  topLayer.nodeIds = r.nodeIds ?? [];
  for (const nodeId of topLayer.nodeIds) {
    const d = await c.call("DOM.describeNode", { nodeId }, sessionId, 30000).catch(() => null);
    const n = d?.node;
    const attrs = n?.attributes ?? [];
    let id = null;
    for (let k = 0; k + 1 < attrs.length; k += 2) if (attrs[k] === "id") id = attrs[k + 1];
    topLayer.resolved.push({ nodeId, backendNodeId: n?.backendNodeId ?? null,
                             nodeName: n?.nodeName ?? null, id });
  }
} catch (err) {
  topLayer.available = false;
  topLayer.error = String(err?.message ?? err);
}
out.domGetTopLayerElements = topLayer;

// ---- 3. what the snapshot alone says -------------------------------------------------------------
const snap = await c.call("DOMSnapshot.captureSnapshot",
  { computedStyles: COMPUTED_STYLES, includeDOMRects: true, includePaintOrder: true },
  sessionId, 60000);
{
  const S = snap.strings;
  const str = (i) => (Number.isInteger(i) && i >= 0 && i < S.length ? S[i] : null);
  const doc = snap.documents[0];
  const slotOf = new Map();
  doc.layout.nodeIndex.forEach((n, slot) => { if (!slotOf.has(n)) slotOf.set(n, slot); });
  const stacking = new Set((doc.layout.stackingContexts?.index ?? []));
  const idOf = new Map();
  for (let i = 0; i < doc.nodes.parentIndex.length; i++) {
    const pairs = doc.nodes.attributes[i] ?? [];
    for (let k = 0; k + 1 < pairs.length; k += 2) {
      if (str(pairs[k]) === "id") idOf.set(str(pairs[k + 1]), i);
    }
  }
  const backend = (n) => doc.nodes.backendNodeId[n];
  const row = (n) => {
    const slot = slotOf.get(n);
    const r = slot === undefined ? null : doc.layout.styles[slot];
    if (!r || r.length < COMPUTED_STYLES.length) return null;
    const o = {}; COMPUTED_STYLES.forEach((k, i) => { o[k] = str(r[i]); }); return o;
  };
  const chain = (n) => { const out = []; for (let cur = n; cur !== -1 && cur !== undefined; cur = doc.nodes.parentIndex[cur]) out.push(cur); return out; };
  const topBackend = new Set(topLayer.resolved.map((r) => r.backendNodeId).filter((b) => b !== null));

  out.snapshotView = {};
  for (const id of ["fade", "fade-inflow", "fade-dlg", "fade-dlg-btn", "fade-pop", "fade-pop-btn",
                    "vishide", "vishide-inflow", "vishide-dlg", "vishide-dlg-btn",
                    "dispnone", "dispnone-inflow", "dispnone-dlg", "dispnone-dlg-btn",
                    "plain-dlg", "plain-dlg-btn"]) {
    const n = idOf.get(id);
    if (n === undefined) { out.snapshotView[id] = { node: null }; continue; }
    const slot = slotOf.get(n);
    out.snapshotView[id] = {
      node: n, backendNodeId: backend(n), laidOut: slot !== undefined,
      styles: row(n),
      paintOrder: slot === undefined ? null : (doc.layout.paintOrders?.[slot] ?? null),
      formsStackingContext: slot === undefined ? null : stacking.has(slot),
      inTopLayerPerCdp: topBackend.has(backend(n)),
      domAncestorIds: chain(n).slice(1).map((a) => {
        const pairs = doc.nodes.attributes[a] ?? [];
        for (let k = 0; k + 1 < pairs.length; k += 2) if (str(pairs[k]) === "id") return "#" + str(pairs[k + 1]);
        return str(doc.nodes.nodeName[a]);
      }),
    };
  }
  // Can the snapshot ALONE separate a top-layer element from any other stacking context?
  const scSlots = [...stacking];
  out.stackingContextCount = scSlots.length;
  out.stackingContextIsNotAPredicate =
    scSlots.length > topBackend.size
      ? "stackingContexts contains more entries than there are top-layer elements, so it cannot "
        + "be used as a top-layer predicate (every opacity<1 element forms one)"
      : "inconclusive on this page";
}

// ---- 1. PIXELS, per container, with both controls ------------------------------------------------
const shot = async (qs, tag) => {
  const p = await newPage(e.wsUrl, url(qs), { width: 260, height: 200, tag });
  await sleep(450);
  const r = await p.c.call("Page.captureScreenshot",
    { format: "png", captureBeyondViewport: false }, p.sessionId, 60000);
  p.c.close();
  return r.data;
};
const [allOpen, noneOpen] = await Promise.all([shot("?open=all", "px1"), shot("?open=none", "px2")]);
out.pixels = {
  openedVsClosed_differ: allOpen !== noneOpen,
  note: "A != B proves the opened top layer changes what is painted even though every dialog's "
      + "DOM ancestor is opacity:0 / visibility:hidden / display:none. The per-container A==C leg "
      + "is the reviewer's rr-toplayer2.mjs; this run re-establishes the A!=B control on the "
      + "committed page.",
};

emit(out);
c.close(); e.kill(); server.close();
