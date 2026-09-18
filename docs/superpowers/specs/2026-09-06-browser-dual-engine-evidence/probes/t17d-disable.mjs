// T17d fix round 1 / D1: what `DOM.getDocument` turns ON, and what `DOM.disable` turns back off.
//
// `t17d-toplayer.mjs` measured that the handshake must be PER CALL. It did not measure what the
// handshake leaves behind: `DOM.getDocument` enables the session's DOM agent permanently, and every
// later DOM mutation on that tab then pushes an unsolicited `DOM.*` event onto a connection whose
// event broadcast holds 64. This probe measures that cost, measures the one-line mitigation, and —
// the part neither the review nor the first round ran — measures the mitigation IN PRODUCTION
// ORDER, with the disable sitting between the top-layer read and `DOMSnapshot.captureSnapshot`.
//
// Arms, in the order they decide things:
//   A. EVENT VOLUME, three sessions, same page, same churn:
//        A1 cold (the pre-17d shape)          — expected 0
//        A2 handshake, no disable (17d as shipped) — expected many
//        A3 handshake + disable (the fix)     — expected 0
//      A1 is the control that says the counter can count; A2 is the control that says a 0 in A3 is
//      the disable working rather than the page being quiet.
//   B. PRODUCTION ORDER with the disable in it: getDocument → getTopLayerElements → describeNode×k
//      → DOM.disable → captureSnapshot. The capture is taken AFTER the agent is off, which is the
//      exact sequence `capture_session` will run and which nothing has yet executed.
//   C. What a BARE `getTopLayerElements` answers after a disable — refuse, or `[]`? The fake server
//      in `fetch_chromium`'s census models this, so guessing it would make that test a statement
//      about my guess.
//   D. A SECOND snapshot on the same session after a disable still answers correctly.
//   E. `DOM.disable` on a session that never enabled the agent: error, or silently fine? The
//      production code must not fail a good capture on a cleanup step.
//   F. `DOM.getBoxModel` on a real id after the disable — the `browser_click` path.
//
// usage: node t17d-disable.mjs
import { launchEngine, serveStatic, emit, sleep, Cdp,
         PROBE_DIR, PARENT_PORT, COMPUTED_STYLES } from "./t0-lib.mjs";

const server = await serveStatic(PROBE_DIR, PARENT_PORT);
const e = await launchEngine("chrome");
const url = (qs = "") => `http://127.0.0.1:${PARENT_PORT}/t17d-page.html${qs}`;

// Production's session shape: Page only. `DOM.enable` is never sent by `fetch_chromium`, and a
// probe that sends it is measuring a setup production does not have — which is how the first
// round's `[]` reading happened.
async function barePage(wsUrl, pageUrl, tag) {
  const c = new Cdp(wsUrl, tag);
  await c.connect();
  const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await c.call("Target.attachToTarget", { targetId, flatten: true });
  await c.call("Page.enable", {}, sessionId).catch(() => {});
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

// Count every unsolicited frame the connection receives, bucketed by method prefix.
function counter(c) {
  const seen = new Map();
  const off = c.on((m) => {
    if (!m.method) return;
    seen.set(m.method, (seen.get(m.method) ?? 0) + 1);
  });
  return {
    off,
    dom: () => [...seen.entries()].filter(([k]) => k.startsWith("DOM.")).reduce((a, [, v]) => a + v, 0),
    all: () => Object.fromEntries([...seen.entries()].sort()),
  };
}

// The churn: N inserts, N attribute writes, N/2 removals — the same shape the review used, so the
// two readings are about the same work.
const CHURN = 250;
const churn = (n) => `(() => {
  const host = document.createElement('div');
  document.body.appendChild(host);
  const made = [];
  for (let i = 0; i < ${n}; i++) { const d = document.createElement('span'); d.textContent = 'x' + i; host.appendChild(d); made.push(d); }
  for (const d of made) d.setAttribute('data-churn', 'yes');
  for (let i = 0; i < ${Math.floor(n / 2)}; i++) made[i].remove();
  return made.length;
})()`;

const out = { probe: "T17d-disable", chrome: null, churn: CHURN };
{
  const p = await barePage(e.wsUrl, "about:blank", "ver");
  const v = await call(p.c, undefined, "Browser.getVersion", {});
  out.chrome = v.ok ? v.result.product : null;
  p.c.close();
}

// Read the top layer exactly the way `top_layer_backend_ids` does.
async function readTopLayer(c, sessionId) {
  const tl = await call(c, sessionId, "DOM.getTopLayerElements", {});
  if (!tl.ok) return { ok: false, error: tl.error };
  const rows = [];
  for (const nodeId of tl.result.nodeIds ?? []) {
    const d = await call(c, sessionId, "DOM.describeNode", { nodeId });
    const n = d.ok ? d.result.node : null;
    const attrs = n?.attributes ?? [];
    let id = null;
    for (let k = 0; k + 1 < attrs.length; k += 2) if (attrs[k] === "id") id = attrs[k + 1];
    rows.push({ nodeId, backendNodeId: n?.backendNodeId ?? null, id, error: d.ok ? null : d.error });
  }
  return { ok: true, nodeIds: tl.result.nodeIds ?? [], rows };
}

// ---- A. event volume, three sessions -----------------------------------------------------------
out.eventVolume = {};
for (const arm of ["cold", "handshakeNoDisable", "handshakeThenDisable"]) {
  const p = await barePage(e.wsUrl, url("?open=all"), `ev-${arm}`);
  if (arm !== "cold") {
    await call(p.c, p.sessionId, "DOM.getDocument", { depth: 0, pierce: false });
    await readTopLayer(p.c, p.sessionId);
  }
  if (arm === "handshakeThenDisable") {
    const d = await call(p.c, p.sessionId, "DOM.disable", {});
    out.eventVolume[arm + "_disableReply"] = d.ok ? "ok" : d.error;
  }
  // Start counting AFTER the setup, so the handshake's own `setChildNodes` is not the finding.
  const cnt = counter(p.c);
  const made = await call(p.c, p.sessionId, "Runtime.evaluate",
    { expression: churn(CHURN), returnByValue: true });
  await sleep(900);
  cnt.off();
  out.eventVolume[arm] = {
    mutationsMade: made.ok ? made.result.result?.value : made.error,
    domEvents: cnt.dom(),
    byMethod: cnt.all(),
  };
  p.c.close();
}
out.eventVolumeVerdict = {
  counterCanCount: out.eventVolume.handshakeNoDisable.domEvents > 0,
  coldIsZero: out.eventVolume.cold.domEvents === 0,
  disableRestoresZero: out.eventVolume.handshakeThenDisable.domEvents === 0,
  note: "cold is the instrument's control (it must be 0 by construction, pre-17d); "
      + "handshakeNoDisable is the control that says a 0 below is the disable and not a quiet page.",
};

// ---- B. PRODUCTION ORDER: …read → DOM.disable → captureSnapshot --------------------------------
{
  const p = await barePage(e.wsUrl, url("?open=all"), "prod");
  const fs1 = await call(p.c, p.sessionId, "Runtime.evaluate",
    { expression: "window.__goFullscreen()", awaitPromise: true, userGesture: true, returnByValue: true });
  await sleep(500);
  await call(p.c, p.sessionId, "DOM.getDocument", { depth: 0, pierce: false });
  const read = await readTopLayer(p.c, p.sessionId);
  const dis = await call(p.c, p.sessionId, "DOM.disable", {});
  const snap = await call(p.c, p.sessionId, "DOMSnapshot.captureSnapshot",
    { computedStyles: COMPUTED_STYLES, includeDOMRects: true, includePaintOrder: true });

  let dialogInCapture = null, nodeCount = null;
  if (snap.ok) {
    const S = snap.result.strings, doc = snap.result.documents[0];
    nodeCount = doc.nodes.parentIndex.length;
    for (let i = 0; i < nodeCount; i++) {
      const pairs = doc.nodes.attributes[i] ?? [];
      for (let k = 0; k + 1 < pairs.length; k += 2) {
        if (S[pairs[k]] === "id" && S[pairs[k + 1]] === "tl-dlg") dialogInCapture = doc.nodes.backendNodeId[i];
      }
    }
  }
  out.productionOrder = {
    fullscreen: fs1.ok ? fs1.result.result?.value : fs1.error,
    topLayerIds: read.ok ? read.rows.map((r) => r.id ?? "(pseudo)") : read.error,
    disable: dis.ok ? "ok" : dis.error,
    captureOk: snap.ok,
    captureNodeCount: nodeCount,
    dialogBackendNodeIdInCapture: dialogInCapture,
    // The join the fetcher makes: the id the top-layer read named must be the id the capture uses.
    idsAgree: read.ok && dialogInCapture !== null
      && read.rows.some((r) => r.id === "tl-dlg" && r.backendNodeId === dialogInCapture),
  };

  // ---- C. a BARE getTopLayerElements after the disable --------------------------------------
  const bare = await call(p.c, p.sessionId, "DOM.getTopLayerElements", {});
  out.afterDisable = bare.ok
    ? { answered: bare.result.nodeIds ?? [], refused: null }
    : { answered: null, refused: bare.error };

  // ---- F. getBoxModel after the disable — the browser_click path ------------------------------
  const box = dialogInCapture === null ? { ok: false, error: "no id to ask about" }
    : await call(p.c, p.sessionId, "DOM.getBoxModel", { backendNodeId: dialogInCapture });
  out.boxModelAfterDisable = box.ok ? "ok" : box.error;

  // ---- D. a SECOND snapshot on the same session ----------------------------------------------
  await call(p.c, p.sessionId, "DOM.getDocument", { depth: 0, pierce: false });
  const second = await readTopLayer(p.c, p.sessionId);
  const dis2 = await call(p.c, p.sessionId, "DOM.disable", {});
  out.secondSnapshot = {
    topLayerIds: second.ok ? second.rows.map((r) => r.id ?? "(pseudo)") : second.error,
    allBackendIdsResolved: second.ok && second.rows.every((r) => Number.isInteger(r.backendNodeId)),
    disable: dis2.ok ? "ok" : dis2.error,
  };
  p.c.close();
}

// ---- E. DOM.disable with the agent never enabled ------------------------------------------------
{
  const p = await barePage(e.wsUrl, url("?open=all"), "nodisable");
  const d1 = await call(p.c, p.sessionId, "DOM.disable", {});
  const d2 = await call(p.c, p.sessionId, "DOM.disable", {});
  out.disableWithoutEnable = { first: d1.ok ? "ok" : d1.error, secondInARow: d2.ok ? "ok" : d2.error };
  p.c.close();
}

// ---- G. THE WINDOW THAT REMAINS --------------------------------------------------------------
//
// The disable does not take the event count to zero; it takes it from *the tab's life* down to
// *the handshake*. Two readings of that window, counted strictly between `DOM.getDocument` and
// `DOM.disable`:
//   G1 quiet page — what the handshake itself costs, with nothing else happening;
//   G2 a page mutating THROUGH the window — a burst that lands inside it.
// Both are the residual this fix creates, and it goes on the residual list with these numbers.
out.remainingWindow = {};
for (const arm of ["quiet", "churningThroughTheWindow"]) {
  const p = await barePage(e.wsUrl, url("?open=all"), `win-${arm}`);
  if (arm === "churningThroughTheWindow") {
    // Fire and DO NOT await: the mutations are meant to be in flight while the handshake runs.
    call(p.c, p.sessionId, "Runtime.evaluate",
      { expression: `(async () => { for (let i = 0; i < ${CHURN}; i++) { const d = document.createElement('i'); d.textContent = 'w' + i; document.body.appendChild(d); await new Promise((r) => setTimeout(r, 0)); } return ${CHURN}; })()`,
        awaitPromise: true, returnByValue: true });
    await sleep(120);
  }
  const cnt = counter(p.c);
  await call(p.c, p.sessionId, "DOM.getDocument", { depth: 0, pierce: false });
  const read = await readTopLayer(p.c, p.sessionId);
  await call(p.c, p.sessionId, "DOM.disable", {});
  cnt.off();
  out.remainingWindow[arm] = {
    domEventsInsideTheWindow: cnt.dom(),
    byMethod: cnt.all(),
    readStillCorrect: read.ok && read.rows.some((r) => r.id === "tl-dlg"),
  };
  p.c.close();
}

emit(out);
e.kill(); server.close();
