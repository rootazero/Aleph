// T17d: the readings the top-layer arm of `cascade_opacity` is built on, and the fixture it is
// falsified against.
//
// Everything about WHAT escapes was settled by `t17b-escape.mjs` (pixels, both controls). This
// probe answers the four questions that remained, in the order they decide code:
//
//   A. THE HANDSHAKE, exactly as production calls it. `fetch_chromium` never sends `DOM.enable`,
//      so the reading has to be taken on a session that has not had it either — otherwise the
//      measurement is of a setup production does not use. Three arms on three FRESH sessions:
//      no `DOM.getDocument` at all, `depth: 0`, `depth: -1`. The one that matters is arm 1: if it
//      answers `[]` rather than an error, a forgotten handshake is a no-op that reports success.
//   B. Does `DOM.describeNode { nodeId }` resolve a top-layer nodeId after a `depth: 0` handshake?
//      `depth: 0` returns zero children, so "the node is in the session's map" is a different
//      question from "the tree was walked", and the cheap handshake is only usable if the answer
//      is yes.
//   C. ALL THREE escapers in ONE list. `t17b-escape.mjs` rendered one candidate per page, so it
//      established `getTopLayerElements == escaping set` one candidate at a time. The brief asks
//      for the un-assumed version: modal + popover + fullscreen open together, all three named.
//   D. THE FIXTURE. A capture of the nested shape, plus the companion top-layer list Chrome
//      returned for that same render — two files, written from one run, so they cannot disagree
//      about which page they describe.
//
// usage: node t17d-toplayer.mjs             # measure + write the fixture
//        node t17d-toplayer.mjs --no-write  # measure only
import fs from "node:fs";
import path from "node:path";
import { launchEngine, serveStatic, writeFixture, emit, sleep, Cdp, CDP_FIXTURES,
         PROBE_DIR, PAGE_FIXTURES, PARENT_PORT, COMPUTED_STYLES } from "./t0-lib.mjs";

const write = !process.argv.includes("--no-write");
const server = await serveStatic(PROBE_DIR, PARENT_PORT);
const e = await launchEngine("chrome");
const url = (qs = "") => `http://127.0.0.1:${PARENT_PORT}/t17d-page.html${qs}`;

// `t0-lib`'s `newPage` sends `DOM.enable`; production's `fetch_chromium` never does. Arm A is a
// statement about the session production actually has, so this opener is the production shape:
// attach flat, enable Page only (for the load event), navigate. Nothing from the DOM domain.
async function barePage(wsUrl, pageUrl, tag) {
  const c = new Cdp(wsUrl, tag);
  await c.connect();
  const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await c.call("Target.attachToTarget", { targetId, flatten: true });
  await c.call("Page.enable", {}, sessionId).catch(() => {});
  await c.call("Emulation.setDeviceMetricsOverride",
               { width: 1000, height: 800, deviceScaleFactor: 1, mobile: false }, sessionId)
    .catch(() => {});
  const load = c.waitEvent("Page.loadEventFired", sessionId, 45000).catch((x) => String(x));
  await c.call("Page.navigate", { url: pageUrl }, sessionId, 45000);
  await load;
  await sleep(600);
  return { c, sessionId };
}

const call = async (c, sessionId, method, params) => {
  try { return { ok: true, result: await c.call(method, params, sessionId, 30000) }; }
  catch (err) { return { ok: false, error: String(err?.message ?? err) }; }
};

const out = { probe: "T17d", chrome: null };
{
  const v = await (async () => {
    const p = await barePage(e.wsUrl, "about:blank", "ver");
    const r = await call(p.c, undefined, "Browser.getVersion", {});
    p.c.close();
    return r.ok ? r.result.product : null;
  })();
  out.chrome = v;
}

// Resolve a list of nodeIds to {backendNodeId, nodeName, id} through describeNode-BY-NODEID, which
// is the direction `fetch_chromium` needs: `getTopLayerElements` answers in nodeId space and
// `DOMSnapshot` is entirely in backendNodeId space.
async function resolve(c, sessionId, nodeIds) {
  const rows = [];
  for (const nodeId of nodeIds) {
    const d = await call(c, sessionId, "DOM.describeNode", { nodeId });
    if (!d.ok) { rows.push({ nodeId, error: d.error }); continue; }
    const n = d.result.node ?? {};
    const attrs = n.attributes ?? [];
    let id = null;
    for (let k = 0; k + 1 < attrs.length; k += 2) if (attrs[k] === "id") id = attrs[k + 1];
    rows.push({ nodeId, backendNodeId: n.backendNodeId ?? null, nodeName: n.nodeName ?? null, id });
  }
  return rows;
}

// ---- A + B: the handshake, on THREE fresh sessions ---------------------------------------------
//
// Fresh sessions, not three calls on one: `DOM.getDocument` is sticky per session, so arm 1's
// answer is only about "no handshake" if nothing has handshaken on that session.
out.handshake = {};
for (const [label, depth] of [["none", null], ["depth0", 0], ["depthMinus1", -1]]) {
  const p = await barePage(e.wsUrl, url("?open=all"), `hs-${label}`);
  const got = { sentGetDocument: depth !== null, depth };
  if (depth !== null) {
    const doc = await call(p.c, p.sessionId, "DOM.getDocument", { depth, pierce: false });
    got.getDocument = doc.ok
      ? { ok: true, rootBackendNodeId: doc.result.root?.backendNodeId ?? null,
          childrenReturned: (doc.result.root?.children ?? []).length }
      : { ok: false, error: doc.error };
  }
  const tl = await call(p.c, p.sessionId, "DOM.getTopLayerElements", {});
  got.getTopLayerElements = tl.ok ? { ok: true, nodeIds: tl.result.nodeIds ?? [] }
                                  : { ok: false, error: tl.error };
  if (tl.ok) got.resolved = await resolve(p.c, p.sessionId, tl.result.nodeIds ?? []);
  out.handshake[label] = got;
  p.c.close();
}
out.handshakeVerdict = {
  withoutHandshake:
    out.handshake.none.getTopLayerElements.ok
      ? `ANSWERED ${JSON.stringify(out.handshake.none.getTopLayerElements.nodeIds)} — an EMPTY `
        + "list is also the honest answer for nearly every page, so a forgotten handshake is a "
        + "no-op that reports success"
      : `REFUSED: ${out.handshake.none.getTopLayerElements.error} — a forgotten handshake would `
        + "be loud, and no census would be needed",
  depth0Enough:
    out.handshake.depth0.getTopLayerElements.ok
    && (out.handshake.depth0.getTopLayerElements.nodeIds ?? []).length > 0
    && (out.handshake.depth0.resolved ?? []).every((r) => Number.isInteger(r.backendNodeId)),
  depth0ChildrenReturned: out.handshake.depth0.getDocument?.childrenReturned ?? null,
  domEnableNeverSent: "this probe's opener sends Page.enable and Emulation only — no DOM.enable, "
    + "which is the session shape fetch_chromium has",
};

// ---- A2: WHO makes the refusal go quiet ---------------------------------------------------------
//
// Arm A says the refusal is loud on a session that never touched the DOM domain. That is only half
// the question, because a production session is long-lived and shared: `browser_click` sends
// `DOM.getBoxModel`, `browser_snapshot` sends `DOM.getFrameOwner`, and a second CDP client on the
// same browser may send `DOM.enable` outright. If ANY of those leaves the agent enabled, deleting
// the handshake stops being a loud error and becomes an empty list — the silent no-op.
//
// Each arm is a FRESH session that does the named thing and then calls `getTopLayerElements` with
// NO `DOM.getDocument` of its own.
out.agentEnabled = {};
for (const [label, prime] of [
  ["domEnable", async (c, s) => call(c, s, "DOM.enable", {})],
  ["getBoxModelOnABadId", async (c, s) => call(c, s, "DOM.getBoxModel", { backendNodeId: 99999 })],
  ["getFrameOwnerOnABadFrame", async (c, s) => call(c, s, "DOM.getFrameOwner", { frameId: "nope" })],
  // A SUCCESSFUL `DOM.getBoxModel` — what `browser_click` sends on every click. The id comes out
  // of a `DOMSnapshot` capture, which needs no DOM agent, so this arm sends nothing from the DOM
  // domain except the call under test.
  ["getBoxModelOnARealId", async (c, s) => {
    const snap = await call(c, s, "DOMSnapshot.captureSnapshot",
      { computedStyles: COMPUTED_STYLES, includeDOMRects: true });
    if (!snap.ok) return snap;
    const S = snap.result.strings;
    const doc = snap.result.documents[0];
    // The <button id=outside>, which definitely has a box.
    let target = null;
    for (let i = 0; i < doc.nodes.parentIndex.length; i++) {
      const pairs = doc.nodes.attributes[i] ?? [];
      for (let k = 0; k + 1 < pairs.length; k += 2) {
        if (S[pairs[k]] === "id" && S[pairs[k + 1]] === "outside") target = doc.nodes.backendNodeId[i];
      }
    }
    return call(c, s, "DOM.getBoxModel", { backendNodeId: target });
  }],
  // `DOMSnapshot.captureSnapshot` ALONE — the call this fetcher is built around. If it enabled the
  // agent, the order of the three calls inside one fetch would decide loud vs silent.
  ["captureSnapshotOnly", async (c, s) => call(c, s, "DOMSnapshot.captureSnapshot",
    { computedStyles: COMPUTED_STYLES, includeDOMRects: true })],
  ["getDocumentThenReNavigate", async (c, s) => {
    const r = await call(c, s, "DOM.getDocument", { depth: 0, pierce: false });
    const load = c.waitEvent("Page.loadEventFired", s, 45000).catch((x) => String(x));
    await call(c, s, "Page.navigate", { url: url("?open=all&second=1") });
    await load; await sleep(700);
    return r;
  }],
  // THE CONTROL for the arm above. Same navigation, same second page — and a handshake AFTER it.
  // Without this pair, `[]` on the re-navigated session could equally mean "the second page has no
  // modal", and the whole reading would be about the page rather than about the handshake.
  ["getDocumentThenReNavigateThenHandshakeAgain", async (c, s) => {
    await call(c, s, "DOM.getDocument", { depth: 0, pierce: false });
    const load = c.waitEvent("Page.loadEventFired", s, 45000).catch((x) => String(x));
    await call(c, s, "Page.navigate", { url: url("?open=all&second=1") });
    await load; await sleep(700);
    return call(c, s, "DOM.getDocument", { depth: 0, pierce: false });
  }],
]) {
  const p = await barePage(e.wsUrl, url("?open=all"), `ae-${label}`);
  const primed = await prime(p.c, p.sessionId);
  const tl = await call(p.c, p.sessionId, "DOM.getTopLayerElements", {});
  out.agentEnabled[label] = {
    priming: primed.ok ? "ok" : primed.error,
    getTopLayerElements: tl.ok ? { ok: true, nodeIds: tl.result.nodeIds ?? [] }
                               : { ok: false, error: tl.error },
    resolved: tl.ok ? await resolve(p.c, p.sessionId, tl.result.nodeIds ?? []) : null,
  };
  p.c.close();
}
out.agentEnabledVerdict = Object.fromEntries(Object.entries(out.agentEnabled).map(([k, v]) => [
  k,
  v.getTopLayerElements.ok
    // No "with no handshake of its own" in either branch: the last arm's priming IS a handshake,
    // and a label that describes the wrong arm is worse than a bare one (判据 §17).
    ? (v.getTopLayerElements.nodeIds.length === 0
        ? "SILENT: the agent is enabled and the answer is [] — indistinguishable from a page with "
          + "no dialogs, so a missing handshake is a no-op that reports success"
        : `ANSWERED ${v.getTopLayerElements.nodeIds.length} nodeIds`)
    : `LOUD: ${v.getTopLayerElements.error}`,
]));

// ---- C: all three escapers, in ONE list ---------------------------------------------------------
{
  const p = await barePage(e.wsUrl, url("?open=all"), "three");
  const fs1 = await call(p.c, p.sessionId, "Runtime.evaluate",
    { expression: "window.__goFullscreen()", awaitPromise: true, userGesture: true,
      returnByValue: true });
  await sleep(500);
  const opened = await call(p.c, p.sessionId, "Runtime.evaluate",
    { expression: "JSON.stringify(window.__opened)", returnByValue: true });
  await call(p.c, p.sessionId, "DOM.getDocument", { depth: 0, pierce: false });
  const tl = await call(p.c, p.sessionId, "DOM.getTopLayerElements", {});
  const rows = tl.ok ? await resolve(p.c, p.sessionId, tl.result.nodeIds ?? []) : [];
  const ids = rows.map((r) => r.id).filter(Boolean);
  out.allThree = {
    openedFromLoad: JSON.parse(opened.ok ? (opened.result.result.value ?? "[]") : "[]"),
    requestFullscreen: fs1.ok ? fs1.result.result?.value : fs1.error,
    topLayerIds: ids,
    modalListed: ids.includes("tl-dlg"),
    popoverListed: ids.includes("tl-pop"),
    fullscreenListed: ids.includes("tl-fs"),
    negativesAbsent: !ids.includes("neg-nonmodal") && !ids.includes("neg-fixed")
                     && !ids.includes("neg-plain"),
    rows,
  };
  p.c.close();
}

// ---- D: the fixture — the capture and the top-layer list, from ONE render -----------------------
{
  const p = await barePage(e.wsUrl, url("?open=all"), "fixture");
  const fsr = await call(p.c, p.sessionId, "Runtime.evaluate",
    { expression: "window.__goFullscreen()", awaitPromise: true, userGesture: true,
      returnByValue: true });
  await sleep(500);
  // Handshake first, list second, capture third — the same order production runs them in.
  await call(p.c, p.sessionId, "DOM.getDocument", { depth: 0, pierce: false });
  const tl = await call(p.c, p.sessionId, "DOM.getTopLayerElements", {});
  const rows = tl.ok ? await resolve(p.c, p.sessionId, tl.result.nodeIds ?? []) : [];
  const snap = await call(p.c, p.sessionId, "DOMSnapshot.captureSnapshot",
    { computedStyles: COMPUTED_STYLES, includeDOMRects: true, includePaintOrder: true });

  // Read the capture the way `parse_nodes` does, so the probe's own view and the fetcher's cannot
  // be two different readings of one file.
  const view = {};
  if (snap.ok) {
    const S = snap.result.strings;
    const str = (i) => (Number.isInteger(i) && i >= 0 && i < S.length ? S[i] : null);
    const doc = snap.result.documents[0];
    const slotOf = new Map();
    doc.layout.nodeIndex.forEach((n, slot) => { if (!slotOf.has(n)) slotOf.set(n, slot); });
    const idOf = new Map();
    for (let i = 0; i < doc.nodes.parentIndex.length; i++) {
      const pairs = doc.nodes.attributes[i] ?? [];
      for (let k = 0; k + 1 < pairs.length; k += 2) {
        if (str(pairs[k]) === "id") idOf.set(str(pairs[k + 1]), i);
      }
    }
    const styles = (n) => {
      const slot = slotOf.get(n);
      const row = slot === undefined ? null : doc.layout.styles[slot];
      if (!row || row.length < COMPUTED_STYLES.length) return null;
      const o = {}; COMPUTED_STYLES.forEach((k, i) => { o[k] = str(row[i]); }); return o;
    };
    const chain = (n) => {
      const acc = [];
      for (let cur = doc.nodes.parentIndex[n]; cur >= 0; cur = doc.nodes.parentIndex[cur]) {
        const pairs = doc.nodes.attributes[cur] ?? [];
        let id = null;
        for (let k = 0; k + 1 < pairs.length; k += 2) if (str(pairs[k]) === "id") id = str(pairs[k + 1]);
        acc.push(id ? "#" + id : str(doc.nodes.nodeName[cur]));
      }
      return acc;
    };
    const top = new Set(rows.map((r) => r.backendNodeId).filter((b) => Number.isInteger(b)));
    for (const id of ["fade", "wrap-a", "wrap-b", "tl-dlg", "tl-dlg-btn", "neg-plain",
                      "neg-plain-btn", "neg-nonmodal", "neg-nonmodal-btn", "neg-fixed",
                      "neg-fixed-btn", "tl-pop", "tl-pop-btn", "tl-fs", "tl-fs-btn", "outside"]) {
      const n = idOf.get(id);
      view[id] = n === undefined ? { node: null } : {
        node: n, backendNodeId: doc.nodes.backendNodeId[n], laidOut: slotOf.has(n),
        styles: styles(n), inTopLayerPerCdp: top.has(doc.nodes.backendNodeId[n]),
        domAncestors: chain(n),
      };
    }
  }
  out.fixtureRender = {
    requestFullscreen: fsr.ok ? fsr.result.result?.value : fsr.error,
    topLayer: rows,
    snapshotOk: snap.ok,
    view,
  };
  if (write && snap.ok) {
    // The `aleph-cdp` wrapper fixtures: the BYTES of the two replies, verbatim, so the crate's
    // method tests decode what Chrome sent rather than what someone typed. The describeNode one is
    // taken BY NODEID — the direction the existing `chrome-DOM.describeNode.json` cannot record,
    // because that one was captured through a backendNodeId.
    const rawTl = tl.ok ? tl.result : { nodeIds: [] };
    const dialogNodeId = rows.find((r) => r.id === "tl-dlg")?.nodeId;
    const rawDescribe = dialogNodeId === undefined ? null
      : (await call(p.c, p.sessionId, "DOM.describeNode", { nodeId: dialogNodeId })).result;
    writeFixture(CDP_FIXTURES, "chrome-DOM.getTopLayerElements.json", rawTl);
    if (rawDescribe) writeFixture(CDP_FIXTURES, "chrome-DOM.describeNode.bynodeid.json", rawDescribe);
    writeFixture(PAGE_FIXTURES, "local-top-layer.domsnapshot.json", snap.result);
    // The companion. It carries the `nodeIds` Chrome answered AND the backendNodeIds they resolve
    // to, because the fetcher's job is exactly that translation and a fixture that recorded only
    // the destination could not falsify it.
    writeFixture(PAGE_FIXTURES, "local-top-layer.toplayer.json", {
      chrome: out.chrome,
      page: "docs/superpowers/specs/2026-09-06-browser-dual-engine-evidence/probes/t17d-page.html",
      note: "DOM.getTopLayerElements on the SAME render as local-top-layer.domsnapshot.json, "
          + "after a DOM.getDocument{depth:0} handshake, on a session that never saw DOM.enable.",
      nodeIds: tl.ok ? (tl.result.nodeIds ?? []) : [],
      resolved: rows,
    });
  }
  p.c.close();
}

emit(out);
if (write) {
  const p = path.join(PAGE_FIXTURES, "local-top-layer.domsnapshot.json");
  console.error(`[t17d] fixture bytes: ${fs.existsSync(p) ? fs.statSync(p).size : "MISSING"}`);
}
e.kill(); server.close();
