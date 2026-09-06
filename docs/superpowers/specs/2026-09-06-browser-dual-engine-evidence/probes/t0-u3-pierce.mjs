// U3: does DOM.getDocument{depth:-1,pierce:true} carry iframe content?
//
// F4 (review round 1, 2026-09-06): the ORIGINAL script measured only a cross-origin child and the
// spec/report stated the answer unconditionally. R57 already established, for U4, that
// DOMSnapshot.captureSnapshot treats same-process and out-of-process children differently — the
// same split applies here (this task's own `local-sameorigin-iframe.domsnapshot.json` shows
// `contentDocumentIndex {index:[85],value:[1]}`, i.e. a same-origin child IS reachable from one
// call), so this probe now measures BOTH halves and states both separately: what pierce reaches
// in-process (same-origin), and what it needs for an out-of-process (cross-origin) child.
//
// F5 (same round): neither half's verdict may rest on cosmetic evidence — the original script read
// the iframe ELEMENT's own `src`/width, which reads identically whether the child document loaded
// or 404'd. Each half below is gated on POSITIVE evidence the child DOCUMENT itself rendered:
// same-origin can read `contentDocument.title` directly (same-origin JS access, no CORS involved);
// cross-origin cannot (that IS same-origin policy working), so this probe auto-attaches to the
// OOPIF's own session (mirroring `t0-u4-oopif.mjs`) and reads `document.title` from THAT session
// instead. A verdict is only ever built once that check succeeds; otherwise it is UNMEASURED, never
// a false "no" (判据 §8).
//
// usage: node t0-u3-pierce.mjs <chrome|obscura>
import { launchEngine, newPage, tryCall, parentUrl, childUrl, serveStatic, PROBE_DIR,
         PARENT_PORT, CHILD_PORT, sleep, emit } from "./t0-lib.mjs";

const engine = process.argv[2] ?? "obscura";
const CHILD_TITLE = "T0 child frame"; // <title> in t0-frame.html — the positive-load anchor.
const parent = await serveStatic(PROBE_DIR, PARENT_PORT);
const child = await serveStatic(PROBE_DIR, CHILD_PORT);
const e = await launchEngine(engine);

// Shared tree-walk: total nodes, contentDocuments crossed, shadow roots, nodes carrying a frameId,
// and every element `id` seen (so `#probe` presence is the same check both halves use).
function walkTree(root) {
  let total = 0, contentDocs = 0, shadowRoots = 0, withFrameId = 0;
  const ids = new Set();
  const walk = (n) => {
    total += 1;
    if (n.frameId) withFrameId += 1;
    if (Array.isArray(n.attributes)) {
      for (let i = 0; i < n.attributes.length - 1; i += 2) {
        if (n.attributes[i] === "id") ids.add(n.attributes[i + 1]);
      }
    }
    for (const ch of n.children ?? []) walk(ch);
    for (const sr of n.shadowRoots ?? []) { shadowRoots += 1; walk(sr); }
    if (n.contentDocument) { contentDocs += 1; walk(n.contentDocument); }
  };
  walk(root);
  return { total, contentDocs, shadowRoots, withFrameId, ids };
}

const out = { u: "U3", engine };

// -------------------------------------------------------------------------------------------
// HALF 1 — same-origin (in-process) child: same host+port as the parent, different path only,
// the same construction `t0-u4-oopif.mjs` uses for its reachable `documents.length==2` case.
// -------------------------------------------------------------------------------------------
const sameOriginChildUrl = `http://127.0.0.1:${PARENT_PORT}/t0-frame.html`;
out.sameOrigin = { childUrl: sameOriginChildUrl };
{
  const { c, sessionId } = await newPage(e.wsUrl, parentUrl(`?child=${encodeURIComponent(sameOriginChildUrl)}`));
  await sleep(1200);

  // F5: positive evidence — same-origin JS can read `contentDocument` directly, so read its
  // `title` rather than trust the iframe element's own (cosmetic) `src`/width.
  const loadCheck = await tryCall(c, "Runtime.evaluate", {
    expression: `(()=>{const f=document.getElementById('frame-host');
      let title = null, err = null;
      try { title = f && f.contentDocument && f.contentDocument.title; } catch (ex) { err = String(ex); }
      return JSON.stringify({src: f && f.src, contentDocumentTitle: title, accessError: err});})()`,
    returnByValue: true }, sessionId, 30000);
  out.sameOrigin.loadCheck = loadCheck.ok ? JSON.parse(loadCheck.result.result.value) : loadCheck.error.message;
  out.sameOrigin.childLoaded = loadCheck.ok && out.sameOrigin.loadCheck.contentDocumentTitle === CHILD_TITLE;

  const r = await tryCall(c, "DOM.getDocument", { depth: -1, pierce: true }, sessionId, 120000);
  out.sameOrigin.ok = r.ok;
  if (!r.ok) {
    out.sameOrigin.error = r.error.message;
  } else {
    const w = walkTree(r.result.root);
    out.sameOrigin.nodes = w.total;
    out.sameOrigin.contentDocuments = w.contentDocs;
    out.sameOrigin.shadowRoots = w.shadowRoots;
    out.sameOrigin.nodesWithFrameId = w.withFrameId;
    out.sameOrigin.sawChildProbe = w.ids.has("probe");
    out.sameOrigin.sawChildLink = w.ids.has("child-link");
  }

  out.sameOrigin.verdict = !out.sameOrigin.childLoaded
    ? "UNMEASURED: the same-origin child iframe did not demonstrably load ("
      + JSON.stringify(out.sameOrigin.loadCheck) + ", expected contentDocumentTitle="
      + JSON.stringify(CHILD_TITLE) + ") — not a finding about pierce."
    : !r.ok
      ? engine + " same-origin: getDocument{pierce:true} failed — " + out.sameOrigin.error
      : out.sameOrigin.sawChildProbe
        ? engine + " same-origin (in-process child, load confirmed via contentDocument.title="
          + JSON.stringify(out.sameOrigin.loadCheck.contentDocumentTitle) + "): pierce:true DOES "
          + "carry iframe content (" + out.sameOrigin.contentDocuments + " contentDocument(s), "
          + "child `#probe` present, " + out.sameOrigin.nodes + " nodes, "
          + out.sameOrigin.nodesWithFrameId + " carry a frameId) ⇒ the interim fetcher CAN flatten "
          + "a same-process child from this one call."
        : "UNEXPECTED — " + engine + " same-origin (load confirmed): pierce:true does NOT carry "
          + "iframe content (" + out.sameOrigin.contentDocuments + " contentDocument(s), child "
          + "`#probe` absent, " + out.sameOrigin.nodes + " nodes) despite the child being "
          + "in-process and loaded; report to the controller before relying on this half.";
  c.close();
}

// -------------------------------------------------------------------------------------------
// HALF 2 — cross-origin (OOPIF) child: the original measurement, now gated on positive load
// evidence read from the CHILD's OWN auto-attached session, since the parent's JS cannot reach
// cross-origin content at all (that is same-origin policy working, not proof of anything about
// whether the child loaded).
// -------------------------------------------------------------------------------------------
out.crossOrigin = { childUrl: childUrl() };
{
  const { c, sessionId } = await newPage(e.wsUrl, null, { tag: "u3-oopif" });
  let childSessionId = null;
  const offAttach = c.on((m) => {
    if (m.method === "Target.attachedToTarget" && m.params?.targetInfo?.type === "iframe") {
      childSessionId = m.params.sessionId;
    }
  });
  // Best-effort on both engines: if the engine does not support auto-attach, this simply never
  // fires and `childSessionId` stays null — handled below as "could not look", not as a refusal.
  await tryCall(c, "Target.setAutoAttach",
    { autoAttach: true, waitForDebuggerOnStart: false, flatten: true }, sessionId, 10000);
  const load = c.waitEvent("Page.loadEventFired", sessionId, 45000).catch((err) => String(err));
  await c.call("Page.navigate", { url: parentUrl(`?child=${encodeURIComponent(childUrl())}`) }, sessionId, 45000)
    .catch(() => {});
  await load;
  await sleep(1500);
  offAttach();

  out.crossOrigin.childSessionId = childSessionId;
  if (childSessionId) {
    const t = await tryCall(c, "Runtime.evaluate",
      { expression: "document.title", returnByValue: true }, childSessionId, 10000);
    out.crossOrigin.childOwnSessionTitle = t.ok ? t.result?.result?.value : t.error.message;
    out.crossOrigin.childLoaded = t.ok && out.crossOrigin.childOwnSessionTitle === CHILD_TITLE;
  } else {
    out.crossOrigin.childOwnSessionTitle = null;
    // null, not false: "we could not look" (no session to check) is not "nothing happened" (判据 §8).
    out.crossOrigin.childLoaded = null;
  }
  // Cosmetic evidence kept only as a secondary data point, never as the verdict's gate.
  const cosmetic = await tryCall(c, "Runtime.evaluate", {
    expression: `(()=>{const f=document.getElementById('frame-host');
      return JSON.stringify({src:f&&f.src, w:f&&f.getBoundingClientRect().width});})()`,
    returnByValue: true }, sessionId, 30000);
  out.crossOrigin.frameHostCosmetic = cosmetic.ok ? cosmetic.result?.result?.value : cosmetic.error.message;

  const r = await tryCall(c, "DOM.getDocument", { depth: -1, pierce: true }, sessionId, 120000);
  out.crossOrigin.ok = r.ok;
  if (!r.ok) {
    out.crossOrigin.error = r.error.message;
  } else {
    const w = walkTree(r.result.root);
    out.crossOrigin.nodes = w.total;
    out.crossOrigin.contentDocuments = w.contentDocs;
    out.crossOrigin.shadowRoots = w.shadowRoots;
    out.crossOrigin.nodesWithFrameId = w.withFrameId;
    out.crossOrigin.sawChildProbe = w.ids.has("probe");
  }

  out.crossOrigin.verdict = out.crossOrigin.childLoaded === null
    ? "UNMEASURED: no OOPIF child session was auto-attached on " + engine
      + " (childSessionId=null) — could not independently confirm the child loaded, so this half "
      + "is not a finding about pierce either way. Cosmetic frameHost=" + JSON.stringify(out.crossOrigin.frameHostCosmetic) + "."
    : out.crossOrigin.childLoaded === false
      ? "UNMEASURED: the cross-origin child's own session answered document.title="
        + JSON.stringify(out.crossOrigin.childOwnSessionTitle) + ", not " + JSON.stringify(CHILD_TITLE)
        + " — the child did not demonstrably load; not a finding about pierce."
      : !r.ok
        ? engine + " cross-origin: getDocument{pierce:true} failed — " + out.crossOrigin.error
        : out.crossOrigin.sawChildProbe
          ? "UNEXPECTED — " + engine + " cross-origin OOPIF (load confirmed via the child's own "
            + "session, document.title=" + JSON.stringify(out.crossOrigin.childOwnSessionTitle)
            + "): pierce:true DOES carry iframe content; report to the controller before relying "
            + "on this."
          : engine + " cross-origin OOPIF (load confirmed via the child's own session, "
            + "document.title=" + JSON.stringify(out.crossOrigin.childOwnSessionTitle)
            + "): pierce:true does NOT carry iframe content (" + out.crossOrigin.contentDocuments
            + " contentDocument(s), child `#probe` absent, " + out.crossOrigin.nodes
            + " nodes) ⇒ the interim fetcher must attach a session per frame for cross-origin "
            + "children.";
  c.close();
}

// One combined top-level verdict stating both halves plainly — `t0-report.mjs` reads a single
// `.verdict` per probe file, and U3 now measures two paths from the same run.
out.verdict = "SAME-ORIGIN: " + out.sameOrigin.verdict + " || CROSS-ORIGIN: " + out.crossOrigin.verdict;
emit(out);
e.kill(); parent.close(); child.close();
