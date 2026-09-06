// U4 (R57 — controller ruling, 2026-09-06, superseding the original single-fixture design):
// DOMSnapshot.captureSnapshot returns documents[] covering every frame that lives in the CALLING
// session's renderer, and no more. A same-origin (same-process) child iframe is in that renderer,
// so one call sees both documents. A cross-origin (out-of-process) iframe is a SEPARATE target
// with its own session — the parent's call cannot see it, exactly as Page.getFrameTree on the
// parent session cannot see it either (measured separately, same finding). A Chromium fetcher
// therefore needs BOTH paths, so this probe measures and fixtures both, RAW, with no merging or
// coordinate-shifting done here — that stitching is Task 11's `fetch_chromium` code, and Task 11's
// test has to be able to fail it, which it cannot do if the fixture already contains the answer
// (判据 §10: an assertion that only reads a literal it just wrote is always green).
//
// usage: node t0-u4-oopif.mjs
//
// Two static servers on different ORIGINS for the OOPIF half, load-bearing not tidy:
//   parent page  http://127.0.0.1:18999/t0-page.html   (T0_PARENT_PORT)
//   child iframe http://localhost:19001/t0-frame.html  (T0_CHILD_PORT)
// `127.0.0.1` and `localhost` are different hosts and, to Chrome's site isolation, different
// SITES — a port change alone would not be, since a site is scheme + eTLD+1 and the port is
// ignored. `--site-per-process` then makes the out-of-process iframe certain. The same-origin half
// reuses the PARENT's own server (same host, same port, different path: `t0-frame.html` right
// beside `t0-page.html`), which is genuinely in-process — no flag needed for that half.
import { launchChrome, newPage, tryCall, parentUrl, childUrl, serveStatic, PROBE_DIR,
         PARENT_PORT, CHILD_PORT, PAGE_FIXTURES, COMPUTED_STYLES, writeFixture, sleep, emit } from "./t0-lib.mjs";

const parent = await serveStatic(PROBE_DIR, PARENT_PORT);
const child = await serveStatic(PROBE_DIR, CHILD_PORT);
const e = await launchChrome({ extraArgs: ["--site-per-process"] });
if (!e.wsUrl) {
  emit({ u: "U4", error: `chrome published no endpoint: ${e.stderr.slice(0, 400)}`,
         verdict: "U4 UNMEASURED: chrome did not start." });
  process.exit(1);
}

const out = { u: "U4", engine: "chrome", flags: ["--site-per-process"] };
const expected_frame_local = [30, 40, 150, 20];
const eq = (a1, a2) => Boolean(a1 && a2 && a1[0] === a2[0] && a1[1] === a2[1]);

// Find the layout bounds of the element carrying `id=wantId` in a raw DOMSnapshot `documents[i]`.
const findBoundsById = (doc, strings, wantId) => {
  const attrs = doc.nodes?.attributes ?? [];
  let ni = -1;
  for (let i = 0; i < attrs.length; i += 1) {
    const a = attrs[i] ?? [];
    for (let k = 0; k + 1 < a.length; k += 2) {
      if (strings[a[k]] === "id" && strings[a[k + 1]] === wantId) { ni = i; break; }
    }
    if (ni >= 0) break;
  }
  if (ni < 0) return { nodeIndex: -1, bounds: null };
  const layoutPos = (doc.layout?.nodeIndex ?? []).indexOf(ni);
  return { nodeIndex: ni, bounds: layoutPos >= 0 ? doc.layout.bounds[layoutPos].map((n) => Math.round(n)) : null };
};

// -------------------------------------------------------------------------------------------
// PART 1 — same-origin (in-process) iframe: the plan's original design, reachable when the
// child is genuinely same-origin. ONE captureSnapshot call must show documents.length === 2.
// -------------------------------------------------------------------------------------------
const sameOriginChildUrl = `http://127.0.0.1:${PARENT_PORT}/t0-frame.html`;
out.sameOrigin = { childUrl: sameOriginChildUrl };
{
  const { c, sessionId } = await newPage(e.wsUrl, parentUrl(`?child=${encodeURIComponent(sameOriginChildUrl)}`));
  await sleep(1200);
  const iframeRect = await tryCall(c, "Runtime.evaluate", {
    expression: `(()=>{const r=document.getElementById('frame-host').getBoundingClientRect();
      return JSON.stringify({x:Math.round(r.x),y:Math.round(r.y),w:Math.round(r.width),h:Math.round(r.height)});})()`,
    returnByValue: true }, sessionId, 30000);
  out.sameOrigin.iframeRectInParent = iframeRect.ok ? JSON.parse(iframeRect.result.result.value) : iframeRect.error.message;

  const snap = await tryCall(c, "DOMSnapshot.captureSnapshot", {
    computedStyles: COMPUTED_STYLES, includeDOMRects: true, includePaintOrder: true }, sessionId, 60000);
  out.sameOrigin.ok = snap.ok;
  if (!snap.ok) {
    out.sameOrigin.error = snap.error.message;
    out.sameOrigin.verdict = "UNMEASURED: captureSnapshot failed — " + out.sameOrigin.error;
  } else {
    out.sameOrigin.documentCount = snap.result.documents.length;
    if (snap.result.documents.length !== 2) {
      // Per R57: if this is not 2, that is itself a finding — do not work around it.
      out.sameOrigin.verdict = "UNEXPECTED: a same-origin iframe produced documents.length="
        + snap.result.documents.length + ", not 2 — this contradicts the plan's same-process "
        + "assumption. Do not act on it; report to the controller before Task 11 relies on this "
        + "path at all.";
    } else {
      writeFixture(PAGE_FIXTURES, "local-sameorigin-iframe.domsnapshot.json", snap.result);
      const strings = snap.result.strings ?? [];
      const { bounds } = findBoundsById(snap.result.documents[1], strings, "probe");
      out.sameOrigin.probeBoundsInChildDoc = bounds;
      const expected_page_absolute = [30 + (out.sameOrigin.iframeRectInParent?.x ?? 0),
                                      40 + (out.sameOrigin.iframeRectInParent?.y ?? 0), 150, 20];
      out.sameOrigin.expected_frame_local = expected_frame_local;
      out.sameOrigin.expected_page_absolute = expected_page_absolute;
      out.sameOrigin.verdict = !bounds
        ? "UNMEASURED: #probe has no layout entry in documents[1] of the same-origin capture."
        : eq(bounds, expected_frame_local)
          ? "Chrome same-origin (in-process) child: documents[1] bounds are FRAME-LOCAL — #probe "
            + "reads " + JSON.stringify(bounds) + " against a frame-local expectation of "
            + JSON.stringify(expected_frame_local) + " ⇒ `fetch_chromium`'s same-process path "
            + "must add the parent iframe's own rect as the child's offset."
          : eq(bounds, expected_page_absolute)
            ? "Chrome same-origin (in-process) child: documents[1] bounds are ALREADY "
              + "PAGE-ABSOLUTE — #probe reads " + JSON.stringify(bounds) + " ⇒ the same-process "
              + "path needs no added offset."
            : "Chrome same-origin (in-process) child: documents[1] bounds are NEITHER — #probe "
              + "reads " + JSON.stringify(bounds) + ", frame-local would be "
              + JSON.stringify(expected_frame_local) + ", page-absolute would be "
              + JSON.stringify(expected_page_absolute) + "; derive the offset from the "
              + "difference, do not assume either.";
    }
  }
  c.close();
}

// -------------------------------------------------------------------------------------------
// PART 2 — cross-origin OOPIF: parent and child RAW captures, no merge, no shift. Plus the join
// key a Chromium fetcher needs to place the child's frame-local bounds under the right parent
// `<iframe>` element, since DOMSnapshot's own `contentDocumentIndex` is empty for a node whose
// content document lives in a different renderer (measured below) — there is nothing in the
// PARENT's DOMSnapshot response alone that points at the child.
// -------------------------------------------------------------------------------------------
out.oopif = { parentUrl: parentUrl(), childUrl: childUrl() };
out.origins = { parent: new URL(parentUrl()).origin, child: new URL(childUrl()).origin };
out.oopif.crossOrigin = out.origins.parent !== out.origins.child;
if (!out.oopif.crossOrigin) {
  out.oopif.verdict = "UNMEASURED: the iframe is same-origin with the page ("
    + out.origins.parent + "). Set T0_CHILD_PORT and keep the child on a different HOST "
    + "(localhost vs 127.0.0.1), not just a different port.";
} else {
  // Set up auto-attach BEFORE navigating, so the OOPIF's own session id is captured the moment
  // Chrome spawns it for the cross-process child — `newPage(url:null)` stops short of navigating
  // so this can be inserted at exactly that point.
  const { c, sessionId } = await newPage(e.wsUrl, null, { tag: "oopif" });
  let childSessionId = null;
  let childTargetId = null;
  const offAttach = c.on((m) => {
    if (m.method === "Target.attachedToTarget" && m.params?.targetInfo?.type === "iframe") {
      childSessionId = m.params.sessionId;
      childTargetId = m.params.targetInfo.targetId;
    }
  });
  await tryCall(c, "Target.setAutoAttach",
    { autoAttach: true, waitForDebuggerOnStart: false, flatten: true }, sessionId, 10000);
  const load = c.waitEvent("Page.loadEventFired", sessionId, 45000).catch((err) => String(err));
  await c.call("Page.navigate", { url: parentUrl(`?child=${encodeURIComponent(childUrl())}`) }, sessionId, 45000)
    .catch(() => {});
  await load;
  await sleep(1500);
  offAttach();

  const iframeRect = await tryCall(c, "Runtime.evaluate", {
    expression: `(()=>{const r=document.getElementById('frame-host').getBoundingClientRect();
      return JSON.stringify({x:Math.round(r.x),y:Math.round(r.y),w:Math.round(r.width),h:Math.round(r.height)});})()`,
    returnByValue: true }, sessionId, 30000);
  out.oopif.iframeRectInParent = iframeRect.ok ? JSON.parse(iframeRect.result.result.value) : iframeRect.error.message;
  out.oopif.childSessionId = childSessionId;
  out.oopif.childTargetId = childTargetId;

  // ---- the join-key measurement: what does the PARENT's own DOM say about the iframe node? ----
  // `DOM.Node` carries an optional `frameId` on frame-owner elements (CDP's own field for exactly
  // this purpose). Measure it directly rather than assume it is populated or that it matches
  // anything — an empty/undefined value here is itself the answer if that is what CDP sends.
  const parentDoc = await tryCall(c, "DOM.getDocument", { depth: -1 }, sessionId, 30000);
  let iframeNodeFrameId = "NOT FOUND: no id=frame-host node in DOM.getDocument depth:-1";
  if (parentDoc.ok) {
    const walk = (n) => {
      if (Array.isArray(n.attributes)) {
        for (let i = 0; i < n.attributes.length - 1; i += 2) {
          if (n.attributes[i] === "id" && n.attributes[i + 1] === "frame-host") return n;
        }
      }
      for (const ch of n.children ?? []) { const f = walk(ch); if (f) return f; }
      return null;
    };
    const iframeNode = walk(parentDoc.result.root);
    iframeNodeFrameId = iframeNode ? (iframeNode.frameId ?? "PRESENT ON NODE BUT undefined/absent") : iframeNodeFrameId;
  } else {
    iframeNodeFrameId = "DOM.getDocument failed: " + parentDoc.error.message;
  }
  out.oopif.parentIframeNodeFrameId = iframeNodeFrameId;

  // ---- parent's own RAW captureSnapshot: one document, the child is invisible to it ----
  const parentSnap = await tryCall(c, "DOMSnapshot.captureSnapshot", {
    computedStyles: COMPUTED_STYLES, includeDOMRects: true, includePaintOrder: true }, sessionId, 60000);
  if (!parentSnap.ok) {
    out.oopif.parentError = parentSnap.error.message;
  } else {
    out.oopif.parentDocumentCount = parentSnap.result.documents.length;
    // `documents[i].frameId` is a STRING-TABLE INDEX, exactly like `documentURL` — resolve it
    // through this capture's OWN `strings[]`, never compare the raw integer to a literal frameId.
    {
      const pfid = parentSnap.result.documents[0]?.frameId;
      const pstrings = parentSnap.result.strings ?? [];
      out.oopif.parentDocumentFrameId = typeof pfid === "number" ? (pstrings[pfid] ?? null) : (pfid ?? null);
    }
    // Confirms the finding directly: contentDocumentIndex has no entry for the OOPIF's owner node.
    const cdi = parentSnap.result.documents[0]?.nodes?.contentDocumentIndex;
    out.oopif.parentContentDocumentIndexEmpty = Array.isArray(cdi?.index) ? cdi.index.length === 0 : null;
    writeFixture(PAGE_FIXTURES, "local-oopif-parent.domsnapshot.json", parentSnap.result);
  }

  // ---- child's own RAW captureSnapshot, on the CHILD's own auto-attached session ----
  if (childSessionId) {
    const childSnap = await tryCall(c, "DOMSnapshot.captureSnapshot", {
      computedStyles: COMPUTED_STYLES, includeDOMRects: true, includePaintOrder: true }, childSessionId, 60000);
    if (!childSnap.ok) {
      out.oopif.childError = childSnap.error.message;
    } else {
      out.oopif.childDocumentCount = childSnap.result.documents.length;
      // Same string-table resolution as the parent side, against the CHILD's own `strings[]`
      // (a snapshot's string table is local to that one capture, not shared across sessions).
      {
        const cfid = childSnap.result.documents[0]?.frameId;
        const cstrings = childSnap.result.strings ?? [];
        out.oopif.childDocumentFrameId = typeof cfid === "number" ? (cstrings[cfid] ?? null) : (cfid ?? null);
      }
      const strings = childSnap.result.strings ?? [];
      const { bounds } = findBoundsById(childSnap.result.documents[0], strings, "probe");
      out.oopif.probeBoundsInChildOwnSnapshot = bounds;
      // No shift, no merge — write exactly what the child's own session returned.
      writeFixture(PAGE_FIXTURES, "local-oopif-child.domsnapshot.json", childSnap.result);
    }
  } else {
    out.oopif.childError = "no OOPIF child session was auto-attached";
  }

  // ---- the three join-key candidates, compared, with one real pair of values named ----
  out.oopif.joinKey = {
    parentIframeNode_frameId: out.oopif.parentIframeNodeFrameId,
    childTarget_targetId: out.oopif.childTargetId,
    childDocument_frameId: out.oopif.childDocumentFrameId,
    iframeNodeFrameId_equals_childTargetId: out.oopif.parentIframeNodeFrameId === out.oopif.childTargetId,
    iframeNodeFrameId_equals_childDocumentFrameId: out.oopif.parentIframeNodeFrameId === out.oopif.childDocumentFrameId,
    childTargetId_equals_childDocumentFrameId: out.oopif.childTargetId === out.oopif.childDocumentFrameId,
  };

  const b = out.oopif.probeBoundsInChildOwnSnapshot;
  out.oopif.verdict = (out.oopif.parentDocumentCount === 1 ? "confirmed: " : "UNEXPECTED (parentDocumentCount="
      + out.oopif.parentDocumentCount + "): ")
    + "the parent session's own captureSnapshot sees only its own document ("
    + (out.oopif.parentContentDocumentIndexEmpty === true
        ? "contentDocumentIndex is empty for the iframe's owner node, i.e. nothing on the parent "
          + "side points at the child at all"
        : "contentDocumentIndex.index=" + JSON.stringify(parentSnap.ok ? parentSnap.result.documents[0]?.nodes?.contentDocumentIndex?.index : null))
    + "). The child's OWN captureSnapshot (its own session) returns its own document with #probe "
    + "at " + JSON.stringify(b) + (eq(b, expected_frame_local) ? " — exactly frame-local, matching the static page" : " — NOT the expected frame-local value, re-check the page")
    + ". Join key: parent iframe node's `frameId` = " + JSON.stringify(out.oopif.parentIframeNodeFrameId)
    + ", OOPIF target's `targetId` = " + JSON.stringify(out.oopif.childTargetId)
    + ", child's own document `frameId` = " + JSON.stringify(out.oopif.childDocumentFrameId) + " — "
    + (out.oopif.joinKey.iframeNodeFrameId_equals_childTargetId && out.oopif.joinKey.childTargetId_equals_childDocumentFrameId
        ? "all three are the SAME value ⇒ `fetch_chromium` places a child by matching the parent DOM node's `frameId` (from DOM.getDocument, since DOMSnapshot's own node has no such field) against the auto-attached child target's `targetId`, which is also the child's own document `frameId`."
        : "they do NOT all agree — `fetch_chromium` cannot assume this equality; use whichever pair actually matched above.")
    + " `fetch_chromium` must therefore: (1) enumerate iframe sub-targets via Target.setAutoAttach, "
    + "(2) capture each child session's own DOMSnapshot separately, (3) place each child's "
    + "frame-local bounds by adding the OWNER `<iframe>` element's own rect (from the PARENT's "
    + "snapshot, looked up by this join key) as the offset — the same arithmetic as the "
    + "same-origin path, just sourced from two separate captures instead of one.";
  c.close();
}

// One combined top-level verdict, joining the two sub-measurements — `t0-report.mjs` reads a
// single `.verdict` per probe file, same as every other probe; U4 now measures two paths (same-
// origin one-call, OOPIF two-call) from the same run, so both are named here rather than only one
// surviving into t0-results.md's summary table.
out.verdict = (out.sameOrigin.verdict ?? "sameOrigin UNMEASURED") + " · " + (out.oopif.verdict ?? "oopif UNMEASURED");

emit(out);
e.kill(); parent.close(); child.close();
