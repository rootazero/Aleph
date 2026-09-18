// T17b fix-r1: enumerate, BY MEASUREMENT, every way a Chromium node can end up with
// `computed: None` while still having laid-out descendants.
//
// `cascade_opacity` walks parent edges and needs the chain to survive a node it has no reading
// for. The class it must survive is NOT "display: contents" — that is one member. The class is
// **`computed_from` returns None while the subtree is still laid out**, and `computed_from`
// returns None for TWO different reasons:
//
//   (a) the node has no entry in `layout.nodeIndex` at all — it generates no box;
//   (b) it has an entry whose `styles` row is shorter than the request (`computed_from`'s own
//       length check) — a reading that is not four values is an UNKNOWN.
//
// This probe does not read a list of guesses. It walks EVERY node of the document and reports
// the ones that actually have the shape, so the enumeration is a census rather than a memory
// test (判据 §6 — the count you say out loud first is the real size of the class).
//
// usage: node t17b-boxless.mjs
import { launchEngine, newPage, serveStatic, emit, sleep,
         PROBE_DIR, PARENT_PORT, COMPUTED_STYLES } from "./t0-lib.mjs";

const pageUrl = `http://127.0.0.1:${PARENT_PORT}/t17b-boxless.html`;
const server = await serveStatic(PROBE_DIR, PARENT_PORT);
const e = await launchEngine("chrome");
const { c, sessionId } = await newPage(e.wsUrl, pageUrl);
await sleep(400);

const version = await c.call("Browser.getVersion", {}, undefined, 30000).catch(() => null);
const snap = await c.call("DOMSnapshot.captureSnapshot",
  { computedStyles: COMPUTED_STYLES, includeDOMRects: true, includePaintOrder: true },
  sessionId, 60000);

const out = { probe: "T17b-boxless", chrome: version?.product ?? null, url: pageUrl, documents: [] };

for (const [di, doc] of snap.documents.entries()) {
  const S = snap.strings;
  const str = (i) => (Number.isInteger(i) && i >= 0 && i < S.length ? S[i] : null);
  const pi = doc.nodes.parentIndex;

  // node -> FIRST layout slot. NOT what `layout_slots` does: production prefers the first
  // non-text-run entry. Immaterial to this probe's predicate — it asks only whether a node has a
  // usable reading at all, and a multi-entry node has one either way (4 such nodes exist in the
  // whole corpus, all `::marker`) — but the difference is stated because an earlier version of
  // this comment claimed the two were the same, and a probe that overstates its fidelity is
  // evidence nobody can size (判据 §18).
  const slotOf = new Map();
  doc.layout.nodeIndex.forEach((n, slot) => { if (!slotOf.has(n)) slotOf.set(n, slot); });

  // `computed_from`: a row shorter than the request is None, NOT four defaults.
  const computedOf = (node) => {
    const slot = slotOf.get(node);
    if (slot === undefined) return { computed: null, why: "no layout entry (generates no box)" };
    const row = doc.layout.styles[slot];
    if (!row || row.length < COMPUTED_STYLES.length) {
      return { computed: null, why: `layout entry present, styles row has ${row ? row.length : 0} values` };
    }
    const o = {};
    COMPUTED_STYLES.forEach((name, i) => { o[name] = str(row[i]); });
    return { computed: o, why: null };
  };

  const ident = (n) => {
    const pairs = doc.nodes.attributes[n] ?? [];
    for (let k = 0; k + 1 < pairs.length; k += 2) {
      if (str(pairs[k]) === "id") return "#" + str(pairs[k + 1]);
    }
    return str(doc.nodes.nodeName[n]) ?? `node${n}`;
  };

  // Children, and the transitive "does any descendant have a layout entry".
  const kids = new Map();
  pi.forEach((p, n) => { if (p !== -1) { if (!kids.has(p)) kids.set(p, []); kids.get(p).push(n); } });
  const anyLaidOutDescendant = (n) => {
    const stack = [...(kids.get(n) ?? [])];
    while (stack.length) {
      const k = stack.pop();
      if (slotOf.has(k)) return k;
      stack.push(...(kids.get(k) ?? []));
    }
    return null;
  };

  const breaks = [];
  for (let n = 0; n < pi.length; n++) {
    const { computed, why } = computedOf(n);
    if (computed !== null) continue;              // has a reading — no break here
    const witness = anyLaidOutDescendant(n);
    if (witness === null) continue;               // no reading AND no laid-out subtree — harmless
    breaks.push({
      node: n, name: str(doc.nodes.nodeName[n]), id: ident(n), nodeType: doc.nodes.nodeType[n],
      reason: why,
      firstLaidOutDescendant: { node: witness, id: ident(witness),
                                display: computedOf(witness).computed?.display ?? null },
    });
  }

  // The control: nodes with no reading whose subtree is ALSO absent. These are the ones the
  // old `continue` was right about, and the count says how many of them there are.
  let harmless = 0;
  for (let n = 0; n < pi.length; n++) {
    if (computedOf(n).computed !== null) continue;
    if (anyLaidOutDescendant(n) === null) harmless++;
  }

  out.documents.push({
    index: di, nodes: pi.length, layoutEntries: doc.layout.nodeIndex.length,
    chainBreaksWithLaidOutDescendants: breaks,
    boxlessNodesWithNoLaidOutDescendants: harmless,
  });
}

out.summary = out.documents.map((d) =>
  `doc[${d.index}]: ${d.chainBreaksWithLaidOutDescendants.length} chain break(s) with laid-out `
  + `descendants, ${d.boxlessNodesWithNoLaidOutDescendants} boxless node(s) whose subtree is also absent`);

// ---- does `opacity` on a display:contents element hide its subtree? -----------------------------
//
// It cannot be READ — such a node has no styles row — so if it DID hide the subtree, that would be
// a residual the fix cannot reach and the residual list would have to name it. CSS says a
// display:contents element generates no box and therefore composites no group, so the declaration
// has no effect. Settled by PIXELS rather than by citing the spec: three tiny pages, screenshots
// compared as raw base64. Equal bytes = identical rendering, no image decoder needed.
const shot = async (body) => {
  const p = await newPage(e.wsUrl, "about:blank", { width: 120, height: 40, tag: "px" });
  await p.c.call("Page.navigate",
    { url: "data:text/html," + encodeURIComponent(
        `<body style="margin:0;background:#fff">${body}</body>`) }, p.sessionId, 30000);
  await sleep(350);
  const r = await p.c.call("Page.captureScreenshot",
    { format: "png", captureBeyondViewport: false }, p.sessionId, 60000);
  p.c.close();
  return r.data;
};
const BTN = '<button style="width:80px;height:20px;background:#f00;border:0">X</button>';
const [contentsZero, plainOpaque, plainZero] = await Promise.all([
  shot(`<div style="display:contents;opacity:0">${BTN}</div>`),
  shot(`<div>${BTN}</div>`),
  shot(`<div style="opacity:0">${BTN}</div>`),
]);
out.opacityOnDisplayContents = {
  matchesOpaqueRendering: contentsZero === plainOpaque,
  matchesTransparentRendering: contentsZero === plainZero,
  verdict: contentsZero === plainOpaque && contentsZero !== plainZero
    ? "NO EFFECT — `opacity` on a display:contents element paints identically to no opacity at "
      + "all, so the unreadable declaration hides nothing and is not a residual."
    : contentsZero === plainZero
      ? "IT HIDES — a display:contents element's `opacity` DOES suppress its subtree, and since "
        + "such a node has no styles row the fetcher cannot read it. That is a residual."
      : "INCONCLUSIVE — the three renderings are pairwise different; do not draw a conclusion.",
};
emit(out);

c.close(); e.kill(); server.close();
