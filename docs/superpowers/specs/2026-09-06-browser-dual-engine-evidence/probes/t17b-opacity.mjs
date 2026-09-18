// T17b: what does Chrome report for an ELEMENT descendant of a hidden container?
//
// The question is per FLAG, not per engine. `fetch_chromium` reports each layout node's own
// style array and walks no ancestors, so every flag whose answer for a descendant does NOT
// already account for the ancestor is a gap. Three flags, three readings, one page:
//
//   * `display:none`    — expected: the whole subtree gets NO layout entry, so `computed` is
//                         `None` and `rect: None` carries it. Nothing to cascade.
//   * `visibility:hidden` — expected: Chrome resolves inherited properties before answering, so
//                         a descendant already reads `hidden`. Nothing to cascade, and cascading
//                         would be wrong — `#vis-d2-shown` re-shows itself and Chrome says so.
//   * `opacity:0`       — expected: NOT inherited, so a descendant reads its OWN `1`, while the
//                         subtree still lays out. That is the gap.
//
// Two independent readings of each, because one of them is the mechanism this fetcher actually
// uses: `getComputedStyle` (what CSS says) and `DOMSnapshot.captureSnapshot` (what production
// parses). A claim that held in one and not the other would be the interesting result.
//
// usage: node t17b-opacity.mjs            # measure + write the fixture (ONE document)
//        node t17b-opacity.mjs --no-write # measure only
//        node t17b-opacity.mjs --child --no-write   # + the CROSS-FRAME residual, two documents
//
// `--child` nests a same-origin iframe inside a third `opacity:0` container. It is deliberately
// NOT part of the committed fixture: `parse_nodes` runs per document and `parentIndex` never
// crosses a document boundary, so the child's nodes cannot be reached by an in-document cascade.
// That residual is measured here and NAMED in `cascade_opacity`'s doc rather than fixed.
import { launchEngine, newPage, serveStatic, writeFixture, emit, sleep,
         PROBE_DIR, PAGE_FIXTURES, PARENT_PORT, COMPUTED_STYLES } from "./t0-lib.mjs";

const write = !process.argv.includes("--no-write");
const withChild = process.argv.includes("--child");
const pageUrl = `http://127.0.0.1:${PARENT_PORT}/t17b-page.html`
  + (withChild ? `?child=${encodeURIComponent(`http://127.0.0.1:${PARENT_PORT}/t0-frame.html`)}` : "");

const server = await serveStatic(PROBE_DIR, PARENT_PORT);
const e = await launchEngine("chrome");
const { c, sessionId } = await newPage(e.wsUrl, pageUrl);
await sleep(300);

// ---- reading 1: getComputedStyle, straight out of the page ------------------------------------
//
// `document.querySelector('#opacity-zero > *')` is the one-line falsifier the review named. It is
// spelled out per element here so the answer names which node it is about.
const IDS = ["opacity-zero", "op-d1", "op-d2", "op-d2-opaque",
             "vis-hidden", "vis-d1", "vis-d2", "vis-d2-shown",
             "disp-none", "none-d1", "none-d2", "control"];
const expr = `(() => {
  const out = {};
  for (const id of ${JSON.stringify(IDS)}) {
    const el = document.getElementById(id);
    if (!el) { out[id] = null; continue; }
    const s = getComputedStyle(el);
    const r = el.getBoundingClientRect();
    out[id] = { display: s.display, visibility: s.visibility, opacity: s.opacity,
                rect: { x: Math.round(r.x), y: Math.round(r.y),
                        w: Math.round(r.width), h: Math.round(r.height) } };
  }
  return JSON.stringify(out);
})()`;
const ev = await c.call("Runtime.evaluate", { expression: expr, returnByValue: true }, sessionId);
if (ev.exceptionDetails) throw new Error("t17b: page expression threw: " + JSON.stringify(ev.exceptionDetails));
const css = JSON.parse(ev.result.value);

// ---- reading 2: the capture production actually parses -----------------------------------------
//
// Same request shape as every other fixture in `src/browser/page_state/fixtures/`: the probe's
// COMPUTED_STYLES (a superset of what this build asks for — `fetch_chromium`'s
// `the_request_this_build_sends_is_a_prefix_of_the_list_the_fixtures_were_captured_with` is what
// keeps the two in step) and `includeDOMRects`.
const snap = await c.call("DOMSnapshot.captureSnapshot",
  { computedStyles: COMPUTED_STYLES, includeDOMRects: true, includePaintOrder: true },
  sessionId, 60000);

// Read the capture the way `parse_nodes` does: node index -> layout slot -> styles row.
const doc = snap.documents[0];
const strings = snap.strings;
const str = (i) => (Number.isInteger(i) && i >= 0 && i < strings.length ? strings[i] : null);
const slotOf = new Map();
doc.layout.nodeIndex.forEach((n, slot) => { if (!slotOf.has(n)) slotOf.set(n, slot); });
const styleAt = (node) => {
  const slot = slotOf.get(node);
  if (slot === undefined) return null;
  const row = doc.layout.styles[slot];
  // An EMPTY row is not a row. `#document` carries `[]`, and `[]` is truthy in JS — so the first
  // version of this reader handed the caller a node "reporting" every style as null, and the
  // cross-frame verdict below read "no residual" off it. Same rule `computed_from` applies to a
  // short array: it is an UNKNOWN, not a set of defaults (判据 §8).
  if (!row || row.length < COMPUTED_STYLES.length) return null;
  const o = {};
  COMPUTED_STYLES.forEach((name, i) => { o[name] = str(row[i]); });
  return o;
};
// Which node index is each id? Read the attributes the same way `parse_nodes` does.
const idOf = new Map();
for (let i = 0; i < doc.nodes.parentIndex.length; i++) {
  const pairs = doc.nodes.attributes[i] ?? [];
  for (let k = 0; k + 1 < pairs.length; k += 2) {
    if (str(pairs[k]) === "id") idOf.set(str(pairs[k + 1]), i);
  }
}
const nodeName = (i) => str(doc.nodes.nodeName[i]);
const depthUnder = (node, ancestor) => {
  let d = 0;
  for (let cur = node; cur >= 0; cur = doc.nodes.parentIndex[cur], d++) {
    if (cur === ancestor) return d;
  }
  return null;
};
const snapshotView = {};
for (const id of IDS) {
  const node = idOf.get(id);
  snapshotView[id] = node === undefined
    ? { node: null, note: "no element with this id is in the capture" }
    : { node, nodeName: nodeName(node), laidOut: slotOf.has(node), styles: styleAt(node) };
}
// The TEXT children too — the one arm the existing corpus already covers, kept so the two are
// compared rather than assumed to behave alike.
const textUnder = {};
for (const [label, hostId] of [["op-d1-text", "op-d1"], ["vis-d1-text", "vis-d1"], ["none-d1-text", "none-d1"]]) {
  const host = idOf.get(hostId);
  const kid = host === undefined ? undefined
    : doc.nodes.parentIndex.findIndex((p, i) => p === host && doc.nodes.nodeType[i] === 3);
  textUnder[label] = kid === undefined || kid < 0
    ? { node: null }
    : { node: kid, laidOut: slotOf.has(kid), styles: styleAt(kid) };
}

// ---- verdict -----------------------------------------------------------------------------------
//
// Stated as the three separate questions, because one answer per engine is what put this gap in.
const opEl = css["op-d2"];
const visEl = css["vis-d2"];
const noneEl = snapshotView["none-d2"];
// The instrument names itself. A style reading is only as good as the build that answered, and
// `launchEngine` does not carry one (判据 §18).
const version = await c.call("Browser.getVersion", {}, undefined, 30000).catch(() => null);
// Does `parentIndex` ever point FORWARD? The cascade this probe justifies walks the array once in
// wire order and relies on a parent being finished before its child is reached. Measured, not
// assumed — and the fetcher keeps a `parent >= i` guard regardless (P7).
const forwardEdges = [];
snap.documents.forEach((d, di) => d.nodes.parentIndex.forEach((p, i) => {
  if (p >= i && p !== -1) forwardEdges.push({ doc: di, node: i, parent: p });
}));

// ---- the CROSS-FRAME residual, only under `--child` ---------------------------------------------
//
// The owner `<iframe>` sits in an `opacity:0` container in documents[0]; its content is
// documents[1] with its own node index space. Read what the child's own nodes say about opacity:
// if they say `1`, an in-document cascade cannot reach them and the residual is real.
let crossFrame = null;
if (withChild) {
  const kid = snap.documents[1];
  if (!kid) {
    crossFrame = { note: "no second document — the child frame did not load, so nothing was measured" };
  } else {
    const kSlot = new Map();
    kid.layout.nodeIndex.forEach((n, slot) => { if (!kSlot.has(n)) kSlot.set(n, slot); });
    const kStyles = (node) => {
      const slot = kSlot.get(node);
      const row = slot === undefined ? null : kid.layout.styles[slot];
      if (!row || row.length < COMPUTED_STYLES.length) return null;
      const o = {}; COMPUTED_STYLES.forEach((name, i) => { o[name] = str(row[i]); }); return o;
    };
    const rows = [];
    for (let i = 0; i < kid.nodes.parentIndex.length; i++) {
      const s = kStyles(i);
      if (s) rows.push({ node: i, nodeName: str(kid.nodes.nodeName[i]), opacity: s.opacity });
    }
    const owner = idOf.get("opacity-zero-frame");
    crossFrame = {
      ownerContainerNode: owner ?? null,
      ownerContainerOpacity: owner === undefined ? null : styleAt(owner)?.opacity,
      childDocumentNodes: rows,
      allChildNodesReportOpacityOne: rows.length > 0 && rows.every((r) => r.opacity === "1"),
      verdict: rows.length === 0 ? "the child document has no laid-out node — nothing measured"
        : rows.every((r) => r.opacity === "1")
          ? "RESIDUAL CONFIRMED — every node of the child document reports opacity 1 while its "
            + "owner sits in an opacity:0 container. An in-document cascade cannot reach them."
          : "the child document already carries the owner's opacity — no cross-frame residual",
    };
  }
}

const out = {
  probe: "T17b", chrome: version?.product ?? null, userAgent: version?.userAgent ?? null,
  url: pageUrl, forwardParentEdges: forwardEdges, crossFrame,
  depths: { "op-d2": depthUnder(idOf.get("op-d2"), idOf.get("opacity-zero")),
            "vis-d2": depthUnder(idOf.get("vis-d2"), idOf.get("vis-hidden")),
            "none-d2": depthUnder(idOf.get("none-d2"), idOf.get("disp-none")) },
  getComputedStyle: css,
  captureSnapshot: snapshotView,
  captureSnapshotText: textUnder,
  verdicts: {
    opacity: `#op-d2 (element, depth ${depthUnder(idOf.get("op-d2"), idOf.get("opacity-zero"))} under opacity:0): `
      + `getComputedStyle says opacity=${opEl?.opacity}, capture says opacity=${snapshotView["op-d2"].styles?.opacity}, `
      + `laid out = ${snapshotView["op-d2"].laidOut}. `
      + (opEl?.opacity === "1" ? "GAP CONFIRMED — own value, subtree still lays out."
                               : "GAP REFUTED — Chrome already accounts for the ancestor."),
    visibility: `#vis-d2 says visibility=${visEl?.visibility} (capture ${snapshotView["vis-d2"].styles?.visibility}); `
      + `#vis-d2-shown, which declares visibility:visible, says ${css["vis-d2-shown"]?.visibility} `
      + `(capture ${snapshotView["vis-d2-shown"].styles?.visibility}). `
      + (visEl?.visibility === "hidden" ? "Chrome RESOLVES the inheritance — nothing to cascade."
                                        : "Chrome does NOT resolve it — visibility would need cascading too."),
    display: `#none-d2 is in the capture's node array = ${noneEl.node !== null}, laid out = ${noneEl.laidOut}. `
      + (noneEl.laidOut === false ? "No layout entry, so `computed` is None and `rect: None` carries it."
                                  : "It HAS a layout entry — display would need cascading too."),
  },
};
emit(out);

if (write && withChild) {
  // The committed fixture is the ONE-document capture. Writing the `--child` variant over it
  // would silently change every corpus count that names it, so it is refused rather than
  // overwritten (判据 §8 — a mode flag is not permission).
  throw new Error("t17b: --child is a measurement-only mode; re-run without it to write the fixture");
}
if (write) {
  writeFixture(PAGE_FIXTURES, "local-hidden-containers.domsnapshot.json", snap);
}

c.close(); e.kill(); server.close();
