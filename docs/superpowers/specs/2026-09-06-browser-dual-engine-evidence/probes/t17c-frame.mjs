// T17c: the three flags AT A FRAME BOUNDARY, and the pixels that say what the user sees.
//
// T17b answered "does a descendant of a hidden container already know" for `display`,
// `visibility` and `opacity` INSIDE one document. A frame boundary is not a parent edge: the
// child document's root inherits nothing from the `<iframe>` element, and `parse_nodes` runs per
// document with `parentIndex` never crossing the boundary. So all three questions are open again,
// with possibly different answers, and this probe asks them rather than arguing them.
//
// Two readings per arm, because one of them is what production parses and the other is what the
// user gets:
//
//   * `DOMSnapshot.captureSnapshot` — every laid-out node of every document, read the way
//     `parse_nodes` reads it (node index -> layout slot -> styles row).
//   * PIXELS — for each container, the page is screenshotted twice: as-is, and with that frame's
//     child (and grandchild) body emptied. IDENTICAL means the child contributes no pixels, i.e.
//     the user cannot see it. `#plain-box` is the instrument's control: there the two MUST
//     differ, or the method cannot see child content at all and every "identical" above is
//     vacuous (判据 §2).
//
// usage: node t17c-frame.mjs                 # measure + write the fixture
//        node t17c-frame.mjs --no-write      # measure only
//        node t17c-frame.mjs --oopif --no-write   # + the CROSS-RENDERER boundary (separate
//                                                 #   target, `stitch_snapshots`' path)
import { launchChrome, launchEngine, newPage, serveStatic, writeFixture, emit, sleep, tryCall,
         PROBE_DIR, PAGE_FIXTURES, PARENT_PORT, CHILD_PORT, COMPUTED_STYLES } from "./t0-lib.mjs";

const write = !process.argv.includes("--no-write");
const withOopif = process.argv.includes("--oopif");

const CHILD = `http://127.0.0.1:${PARENT_PORT}/t17c-child.html`;
const INNER = `http://127.0.0.1:${PARENT_PORT}/t17c-inner.html`;
const pageUrl = `http://127.0.0.1:${PARENT_PORT}/t17c-page.html`
  + `?child=${encodeURIComponent(CHILD)}&inner=${encodeURIComponent(INNER)}`;

const server = await serveStatic(PROBE_DIR, PARENT_PORT);

// ---------------------------------------------------------------------------------------------
// Reading 1 — the capture, per document.
// ---------------------------------------------------------------------------------------------
const e = await launchEngine("chrome");
const { c, sessionId } = await newPage(e.wsUrl, pageUrl);
await sleep(600);

const version = await c.call("Browser.getVersion", {}, undefined, 30000).catch(() => null);
const snap = await c.call("DOMSnapshot.captureSnapshot",
  { computedStyles: COMPUTED_STYLES, includeDOMRects: true, includePaintOrder: true },
  sessionId, 60000);

const strings = snap.strings;
const str = (i) => (Number.isInteger(i) && i >= 0 && i < strings.length ? strings[i] : null);

// One document, read the way `parse_nodes` reads it.
function viewOf(doc) {
  const slotOf = new Map();
  doc.layout.nodeIndex.forEach((n, slot) => { if (!slotOf.has(n)) slotOf.set(n, slot); });
  const styleAt = (node) => {
    const slot = slotOf.get(node);
    if (slot === undefined) return null;
    const row = doc.layout.styles[slot];
    // An EMPTY or SHORT row is an UNKNOWN, not a set of defaults — the same rule `computed_from`
    // applies, and the reason t17b's first reader got the cross-frame verdict backwards.
    if (!row || row.length < COMPUTED_STYLES.length) return null;
    const o = {};
    COMPUTED_STYLES.forEach((name, i) => { o[name] = str(row[i]); });
    return o;
  };
  const idOf = new Map();
  for (let i = 0; i < doc.nodes.parentIndex.length; i++) {
    const pairs = doc.nodes.attributes[i] ?? [];
    for (let k = 0; k + 1 < pairs.length; k += 2) {
      if (str(pairs[k]) === "id") idOf.set(str(pairs[k + 1]), i);
    }
  }
  const idAt = new Map(Array.from(idOf, ([id, node]) => [node, id]));
  const laidOut = [];
  for (let i = 0; i < doc.nodes.parentIndex.length; i++) {
    const s = styleAt(i);
    if (!s) continue;
    laidOut.push({ node: i, nodeName: str(doc.nodes.nodeName[i]), id: idAt.get(i) ?? null,
                   nodeType: doc.nodes.nodeType[i],
                   display: s.display, visibility: s.visibility, opacity: s.opacity });
  }
  return { slotOf, idOf, idAt, styleAt, laidOut,
           nodeCount: doc.nodes.parentIndex.length,
           url: str(doc.documentURL), frameId: str(doc.frameId) };
}

const views = snap.documents.map(viewOf);

// `owner[child document] = (owning document, node index)` — the SAME derivation `frame_offsets`
// walks, spelled here so the probe can say whether a child ever precedes its owner in
// `documents[]` (the premise behind "this cannot live in `parse_nodes`").
const owner = new Map();
snap.documents.forEach((doc, d) => {
  const rare = doc.nodes.contentDocumentIndex ?? { index: [], value: [] };
  rare.index.forEach((nodeIndex, slot) => {
    const child = rare.value[slot];
    if (Number.isInteger(nodeIndex) && Number.isInteger(child)) owner.set(child, [d, nodeIndex]);
  });
});

const documentMap = snap.documents.map((doc, d) => {
  const o = owner.get(d) ?? null;
  return {
    doc: d, url: views[d].url, frameId: views[d].frameId,
    ownedBy: o ? { doc: o[0], node: o[1], id: views[o[0]].idAt.get(o[1]) ?? null,
                   nodeName: str(snap.documents[o[0]].nodes.nodeName[o[1]]),
                   ownerLaidOut: views[o[0]].slotOf.has(o[1]),
                   ownerStyles: views[o[0]].styleAt(o[1]) } : null,
    laidOutNodes: views[d].laidOut.length,
    nodes: views[d].nodeCount,
  };
});
const childBeforeOwner = documentMap.filter((m) => m.ownedBy && m.ownedBy.doc > m.doc)
  .map((m) => ({ doc: m.doc, ownerDoc: m.ownedBy.doc }));

// Which document does each tagged frame own? Found by the OWNER element's id, never by position.
function docOwnedBy(id) {
  for (const [child, [d, node]] of owner) {
    if (views[d].idAt.get(node) === id) return child;
  }
  return null;
}
const opDoc = docOwnedBy("op-frame");
const visDoc = docOwnedBy("vis-frame");
const noneDoc = docOwnedBy("none-frame");
const plainDoc = docOwnedBy("plain-frame");
const contentsDoc = docOwnedBy("contents-frame");
const selfDoc = docOwnedBy("self-frame");
const innerOfOp = opDoc === null ? null : docOwnedBy("c-inner-op");
const innerOfPlain = plainDoc === null ? null : docOwnedBy("c-inner-plain");

const arm = (label, ownerId, childDoc, grandchildDoc) => {
  const o = childDoc === null ? null : owner.get(childDoc);
  const rows = childDoc === null ? [] : views[childDoc].laidOut;
  const grand = grandchildDoc === null || grandchildDoc === undefined
    ? null : views[grandchildDoc].laidOut;
  return {
    label, ownerId,
    ownerElement: o === undefined || o === null ? null : {
      doc: o[0], node: o[1], laidOut: views[o[0]].slotOf.has(o[1]),
      styles: views[o[0]].styleAt(o[1]),
    },
    childDocument: childDoc,
    childDocumentPresent: childDoc !== null,
    childLaidOutNodes: rows.length,
    childRows: rows,
    childAllOpacityOne: rows.length > 0 && rows.every((r) => r.opacity === "1"),
    childAllVisibilityVisible: rows.length > 0 && rows.every((r) => r.visibility === "visible"),
    childAnyDisplayNone: rows.some((r) => r.display === "none"),
    grandchildDocument: grandchildDoc ?? null,
    grandchildLaidOutNodes: grand === null ? null : grand.length,
    grandchildRows: grand,
    grandchildAllOpacityOne: grand === null ? null : grand.length > 0 && grand.every((r) => r.opacity === "1"),
  };
};

const arms = {
  opacity: arm("opacity:0 container", "op-frame", opDoc, innerOfOp),
  visibility: arm("visibility:hidden container", "vis-frame", visDoc, null),
  display: arm("display:none container", "none-frame", noneDoc, null),
  control: arm("no hiding container (NEGATIVE control)", "plain-frame", plainDoc, innerOfPlain),
  boxlessOwner: arm("opacity:0 container, owner <iframe> is display:contents",
                    "contents-frame", contentsDoc, null),
  selfDeclared: arm("the <iframe> itself declares opacity:0", "self-frame", selfDoc, null),
};

// ---------------------------------------------------------------------------------------------
// Reading 2 — pixels. Does the child's content contribute anything the user can see?
// ---------------------------------------------------------------------------------------------
//
// Per arm, on a FRESH page each time (emptying a body is not undoable): screenshot as-is, then
// screenshot with that frame's child body — and its grandchild's — emptied. Identical bytes mean
// the child painted nothing. The `plain` arm is the control and MUST differ.
async function pixelArm(frameId, url = pageUrl) {
  const page = await newPage(e.wsUrl, url, { tag: `px-${frameId}` });
  await sleep(500);
  const shot = async () => (await page.c.call("Page.captureScreenshot", { format: "png" },
                                              page.sessionId, 45000)).data;
  const before = await shot();
  const blank = `(() => {
    const f = document.getElementById(${JSON.stringify(frameId)});
    if (!f) return "no such frame";
    const d = f.contentDocument;
    if (!d) return "no contentDocument — cross-origin or not loaded";
    for (const inner of d.querySelectorAll('iframe')) {
      if (inner.contentDocument) inner.contentDocument.body.replaceChildren();
    }
    d.body.replaceChildren();
    return "blanked";
  })()`;
  const ev = await page.c.call("Runtime.evaluate", { expression: blank, returnByValue: true },
                               page.sessionId, 30000);
  await sleep(300);
  const after = await shot();
  page.c.close();
  return { frame: frameId, blanked: ev.result?.value ?? null,
           identical: before === after,
           beforeBytes: before.length, afterBytes: after.length };
}

const pixels = {};
for (const [name, id] of [["opacity", "op-frame"], ["visibility", "vis-frame"],
                          ["display", "none-frame"], ["control", "plain-frame"]]) {
  pixels[name] = await pixelArm(id);
}

// The OVER-report direction, which this file's own doc calls the worse one: a `<dialog>` opened
// with `showModal()` INSIDE the child document is in that document's top layer. In-document, the
// top layer leaves the ancestor's paint group and T17d had to exempt it. Does it leave the
// PARENT's container too? If it does, a cross-frame OR would delete a dialog the user is looking
// at; if it does not, the OR is exact in that direction and needs no cross-frame exemption.
const modalUrl = `${pageUrl}&modal=1`;
const escapes = {};
for (const [name, id] of [["opacityWithModal", "op-frame"], ["visibilityWithModal", "vis-frame"],
                          ["controlWithModal", "plain-frame"]]) {
  escapes[name] = await pixelArm(id, modalUrl);
}

// ---------------------------------------------------------------------------------------------
// Reading 3 — the CROSS-RENDERER boundary, only under `--oopif`.
// ---------------------------------------------------------------------------------------------
//
// A third-party widget in a faded modal is cross-ORIGIN, so its document is not in the parent's
// capture at all: it is a separate target that `fetch_chromium` captures separately and
// `stitch_snapshots` joins. That boundary is a different code path from `contentDocumentIndex`
// and this arm measures whether it has the same gap.
let oopif = null;
if (withOopif) {
  const childServer = await serveStatic(PROBE_DIR, CHILD_PORT);
  const iso = await launchChrome({ extraArgs: ["--site-per-process"] });
  // `localhost` vs `127.0.0.1` are different SITES to Chrome's isolation; the port alone is not.
  const crossChild = `http://localhost:${CHILD_PORT}/t17c-child.html`;
  const url = `http://127.0.0.1:${PARENT_PORT}/t17c-page.html?child=${encodeURIComponent(crossChild)}`;
  const p = await newPage(iso.wsUrl, null, { tag: "oopif" });
  const sessions = [];
  const off = p.c.on((m) => {
    if (m.method === "Target.attachedToTarget" && m.params?.targetInfo?.type === "iframe") {
      sessions.push({ sessionId: m.params.sessionId, url: m.params.targetInfo.url });
    }
  });
  await tryCall(p.c, "Target.setAutoAttach",
    { autoAttach: true, waitForDebuggerOnStart: false, flatten: true }, p.sessionId, 10000);
  const load = p.c.waitEvent("Page.loadEventFired", p.sessionId, 45000).catch((x) => String(x));
  await p.c.call("Page.navigate", { url }, p.sessionId, 45000).catch(() => {});
  await load;
  await sleep(1800);
  off();

  const parentSnap = await c0(p, "DOMSnapshot.captureSnapshot", {
    computedStyles: COMPUTED_STYLES, includeDOMRects: true, includePaintOrder: true }, p.sessionId);
  // `targetInfo.url` is EMPTY at attach time — the target exists before it has navigated, so
  // matching on it found nothing and the first run of this arm reported an empty child capture
  // that read exactly like "the child has no laid-out nodes" (判据 §8). Ask each session where
  // it actually is, after the load.
  for (const s of sessions) {
    const loc = await tryCall(p.c, "Runtime.evaluate",
      { expression: "location.href", returnByValue: true }, s.sessionId, 15000);
    s.liveUrl = loc.ok ? loc.result.result?.value ?? "" : `ERR ${loc.error.message}`;
  }
  const opChild = sessions.find((s) => String(s.liveUrl).includes("tag=op"));
  const childSnap = opChild ? await c0(p, "DOMSnapshot.captureSnapshot", {
    computedStyles: COMPUTED_STYLES, includeDOMRects: true, includePaintOrder: true },
    opChild.sessionId) : null;

  const pv = parentSnap ? parentSnap.documents.map((d) => viewOfWith(parentSnap, d)) : [];
  const ownerStyles = pv.length ? pv[0].styleAt(pv[0].idOf.get("op-frame")) : null;
  const childRows = childSnap ? viewOfWith(childSnap, childSnap.documents[0]).laidOut : [];
  oopif = {
    attachedIframeSessions: sessions.map((s) => s.liveUrl),
    opChildSessionFound: Boolean(opChild),
    parentDocumentCount: parentSnap ? parentSnap.documents.length : null,
    parentSeesChildDocument: parentSnap
      ? parentSnap.documents.some((d, i) => i > 0 && (pv[i].url ?? "").includes("localhost"))
      : null,
    ownerIframeStylesInParent: ownerStyles,
    childCaptureLaidOutNodes: childRows.length,
    childRows,
    childAllOpacityOne: childRows.length > 0 && childRows.every((r) => r.opacity === "1"),
  };
  p.c.close();
  iso.kill();
  childServer.close();
}

async function c0(page, method, params, sid) {
  const r = await tryCall(page.c, method, params, sid, 60000);
  return r.ok ? r.result : null;
}
// `viewOf` closes over the page capture's `strings`; the OOPIF arm has two captures with two
// string tables, so it needs the same reader parameterised rather than a second copy (判据 §1).
function viewOfWith(capture, doc) {
  const saveStrings = capture.strings;
  const s = (i) => (Number.isInteger(i) && i >= 0 && i < saveStrings.length ? saveStrings[i] : null);
  const slotOf = new Map();
  doc.layout.nodeIndex.forEach((n, slot) => { if (!slotOf.has(n)) slotOf.set(n, slot); });
  const styleAt = (node) => {
    const slot = slotOf.get(node);
    if (slot === undefined) return null;
    const row = doc.layout.styles[slot];
    if (!row || row.length < COMPUTED_STYLES.length) return null;
    const o = {};
    COMPUTED_STYLES.forEach((name, i) => { o[name] = s(row[i]); });
    return o;
  };
  const idOf = new Map();
  const idAt = new Map();
  for (let i = 0; i < doc.nodes.parentIndex.length; i++) {
    const pairs = doc.nodes.attributes[i] ?? [];
    for (let k = 0; k + 1 < pairs.length; k += 2) {
      if (s(pairs[k]) === "id") { idOf.set(s(pairs[k + 1]), i); idAt.set(i, s(pairs[k + 1])); }
    }
  }
  const laidOut = [];
  for (let i = 0; i < doc.nodes.parentIndex.length; i++) {
    const st = styleAt(i);
    if (!st) continue;
    laidOut.push({ node: i, nodeName: s(doc.nodes.nodeName[i]), id: idAt.get(i) ?? null,
                   display: st.display, visibility: st.visibility, opacity: st.opacity });
  }
  return { slotOf, idOf, idAt, styleAt, laidOut, url: s(doc.documentURL) };
}

// ---------------------------------------------------------------------------------------------
// Verdicts — one per flag, stated as the question rather than as a conclusion.
// ---------------------------------------------------------------------------------------------
const verdicts = {
  opacity: `owner <iframe id=op-frame> reads opacity=${arms.opacity.ownerElement?.styles?.opacity}; `
    + `its child document has ${arms.opacity.childLaidOutNodes} laid-out nodes, all opacity 1 = `
    + `${arms.opacity.childAllOpacityOne}; the GRANDCHILD document has `
    + `${arms.opacity.grandchildLaidOutNodes} laid-out nodes, all opacity 1 = `
    + `${arms.opacity.grandchildAllOpacityOne}. `
    + (arms.opacity.childAllOpacityOne
        ? "CASCADE NEEDED — the child document knows nothing about its owner's transparency."
        : "NO CASCADE NEEDED — the child already carries it."),
  visibility: `owner <iframe id=vis-frame> reads visibility=`
    + `${arms.visibility.ownerElement?.styles?.visibility}; its child document has `
    + `${arms.visibility.childLaidOutNodes} laid-out nodes, all visibility=visible = `
    + `${arms.visibility.childAllVisibilityVisible}. `
    + (arms.visibility.childAllVisibilityVisible
        ? "the child does NOT know — so whether this needs a cross-frame OR is decided by the "
          + "pixels arm, not by this row alone."
        : "the child already reads hidden — nothing to cascade."),
  display: `owner <iframe id=none-frame> laid out = ${arms.display.ownerElement?.laidOut}, styles = `
    + `${JSON.stringify(arms.display.ownerElement?.styles)}; its child document `
    + (arms.display.childDocumentPresent
        ? `IS in documents[] with ${arms.display.childLaidOutNodes} laid-out nodes.`
        : `is NOT in documents[] at all.`)
    + (arms.display.childLaidOutNodes === 0
        ? " Nothing to cascade — no node of it can be offered to the model."
        : " It HAS laid-out nodes, so display would need a cross-frame arm too."),
  boxlessOwner: `<iframe id=contents-frame style="display:contents"> inside an opacity:0 `
    + `container: the owner element is laid out = ${arms.boxlessOwner.ownerElement?.laidOut}, `
    + `styles = ${JSON.stringify(arms.boxlessOwner.ownerElement?.styles)}; its child document `
    + (arms.boxlessOwner.childDocumentPresent
        ? `IS in documents[] with ${arms.boxlessOwner.childLaidOutNodes} laid-out nodes.`
        : `is NOT in documents[].`)
    + ` This is the one shape where "read the owner's CASCADED flag" has nothing to read — `
    + `whether it costs anything depends on that node count.`,
  pixels: `identical-when-blanked: opacity=${pixels.opacity.identical}, `
    + `visibility=${pixels.visibility.identical}, display=${pixels.display.identical}, `
    + `CONTROL plain=${pixels.control.identical} (the control MUST be false, or the method `
    + `cannot see child pixels and every true above is vacuous).`,
  modalEscape: `a showModal() dialog in the CHILD document, blanked: opacity container `
    + `identical=${escapes.opacityWithModal.identical}, visibility container `
    + `identical=${escapes.visibilityWithModal.identical}, CONTROL `
    + `identical=${escapes.controlWithModal.identical}. `
    + (escapes.opacityWithModal.identical && escapes.visibilityWithModal.identical
        ? "the child's top layer does NOT escape the parent's container — no cross-frame "
          + "exemption is needed and the OR is exact in the over-report direction."
        : "the child's top layer DOES escape — a cross-frame OR would delete a dialog the user "
          + "can see, and this arm needs an exemption."),
};

emit({
  probe: "T17c", chrome: version?.product ?? null, userAgent: version?.userAgent ?? null,
  url: pageUrl,
  documentOrder: documentMap,
  aChildDocumentPrecedesItsOwner: childBeforeOwner,
  frameDocuments: { op: opDoc, vis: visDoc, none: noneDoc, plain: plainDoc,
                    innerOfOp, innerOfPlain },
  arms, pixels, escapes, oopif, verdicts,
});

if (write) {
  writeFixture(PAGE_FIXTURES, "local-nested-frames.domsnapshot.json", snap);
}

c.close();
e.kill();
server.close();
