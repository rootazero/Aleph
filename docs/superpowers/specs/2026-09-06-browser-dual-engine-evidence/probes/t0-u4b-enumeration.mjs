// U4b — how does a Task-12-shaped fetcher ENUMERATE the out-of-process children of a page that
// is ALREADY LOADED?
//
// Task 0's U4 probe measured the create-time shape: it turned auto-attach on BEFORE navigating
// and caught `Target.attachedToTarget` as Chrome spawned the child. A snapshot verb does not get
// to do that — the page is already there when the verb is called. Two candidate mechanisms, and
// Task 0 measured neither in this shape:
//
//   A. setAutoAttach on an already-loaded page   — does Chrome re-announce EXISTING children?
//      and does false->true re-announce them a second time (the repeat-snapshot case)?
//   B. Target.getTargets filtered to type=="iframe" + Target.attachToTarget per child
//      — pure request/response, no event timing at all (R57 names it as the alternative).
//
// Also measured: DOM.getFrameOwner on the PARENT session, which is the join both mechanisms need.
//
// Writes NOTHING into the repo. Read-only against the probe pages.
import {
  launchChrome, newPage, tryCall, parentUrl, childUrl, serveStatic, PROBE_DIR,
  PARENT_PORT, CHILD_PORT, sleep,
} from "./t0-lib.mjs";

const out = { u: "U4b" };
const parent = await serveStatic(PROBE_DIR, PARENT_PORT);
const child = await serveStatic(PROBE_DIR, CHILD_PORT);
const e = await launchChrome({ extraArgs: ["--site-per-process"] });
if (!e.wsUrl) {
  console.log(JSON.stringify({ ...out, error: `chrome published no endpoint: ${e.stderr.slice(0, 400)}` }, null, 2));
  process.exit(1);
}

// Navigate with NO auto-attach at all, so the page reaches its loaded state exactly as it would
// before a `browser_snapshot` call. Everything measured below happens after this point.
const { c, sessionId } = await newPage(e.wsUrl, null, { tag: "u4b" });
const load = c.waitEvent("Page.loadEventFired", sessionId, 45000).catch((err) => String(err));
await c.call("Page.navigate", { url: parentUrl(`?child=${encodeURIComponent(childUrl())}`) }, sessionId, 45000)
  .catch(() => {});
await load;
await sleep(1500);

// ---------------------------------------------------------------------------------------------
// B — getTargets, both filters. This is the one that would let the fetcher be pure
// request/response.
// ---------------------------------------------------------------------------------------------
const targetsDefault = await tryCall(c, "Target.getTargets", {}, undefined, 10000);
out.getTargets_defaultFilter = targetsDefault.ok
  ? targetsDefault.result.targetInfos.map((t) => ({ type: t.type, targetId: t.targetId, url: t.url }))
  : `ERROR: ${targetsDefault.error.message}`;
out.getTargets_defaultFilter_hasIframe = targetsDefault.ok
  && targetsDefault.result.targetInfos.some((t) => t.type === "iframe");

const targetsFiltered = await tryCall(c, "Target.getTargets", { filter: [{ type: "iframe" }] }, undefined, 10000);
out.getTargets_iframeFilter = targetsFiltered.ok
  ? targetsFiltered.result.targetInfos.map((t) => ({ type: t.type, targetId: t.targetId, url: t.url }))
  : `ERROR: ${targetsFiltered.error.message}`;

// The child's frameId, straight from the page, so the join below is checked against a value this
// probe did not invent.
const frameIdFromPage = await tryCall(c, "Page.getFrameTree", {}, sessionId, 10000);
out.parentFrameTree_childFrameIds = frameIdFromPage.ok
  ? (frameIdFromPage.result.frameTree.childFrames ?? []).map((f) => ({ id: f.frame.id, url: f.frame.url }))
  : `ERROR: ${frameIdFromPage.error.message}`;

const iframeTargets = (targetsFiltered.ok ? targetsFiltered.result.targetInfos : [])
  .concat(targetsDefault.ok ? targetsDefault.result.targetInfos.filter((t) => t.type === "iframe") : [])
  .filter((t, i, a) => a.findIndex((x) => x.targetId === t.targetId) === i);
out.iframeTargetIds = iframeTargets.map((t) => t.targetId);

// B2 — can an iframe target be attached directly, and does the response carry the session id?
out.attachToTarget = [];
for (const t of iframeTargets) {
  const att = await tryCall(c, "Target.attachToTarget", { targetId: t.targetId, flatten: true }, undefined, 10000);
  const row = { targetId: t.targetId, ok: att.ok };
  if (att.ok) {
    row.sessionIdInResponse = att.result?.sessionId ?? null;
    if (row.sessionIdInResponse) {
      const snap = await tryCall(c, "DOMSnapshot.captureSnapshot",
        { computedStyles: ["display"], includeDOMRects: true, includePaintOrder: false },
        row.sessionIdInResponse, 30000);
      row.childCaptureOk = snap.ok;
      row.childDocumentCount = snap.ok ? snap.result.documents.length : snap.error.message;
      const det = await tryCall(c, "Target.detachFromTarget", { sessionId: row.sessionIdInResponse }, undefined, 10000);
      row.detachOk = det.ok;
      row.detachError = det.ok ? null : det.error.message;
    }
  } else {
    row.error = att.error.message;
  }
  out.attachToTarget.push(row);
}

// ---------------------------------------------------------------------------------------------
// A — setAutoAttach on an ALREADY-LOADED page, twice, with false in between.
// ---------------------------------------------------------------------------------------------
const seen = [];
const off = c.on((m) => {
  if (m.method === "Target.attachedToTarget") {
    seen.push({ phase: out._phase ?? "?", type: m.params?.targetInfo?.type,
                targetId: m.params?.targetInfo?.targetId, sessionId: m.params?.sessionId });
  }
  if (m.method === "Target.detachedFromTarget") {
    seen.push({ phase: out._phase ?? "?", detached: m.params?.sessionId });
  }
});

out._phase = "first-true";
const t1 = await tryCall(c, "Target.setAutoAttach",
  { autoAttach: true, waitForDebuggerOnStart: false, flatten: true }, sessionId, 10000);
out.setAutoAttach_first_ok = t1.ok ? true : t1.error.message;
await sleep(800);
out.afterFirstTrue = seen.filter((s) => s.phase === "first-true");

out._phase = "false";
await tryCall(c, "Target.setAutoAttach",
  { autoAttach: false, waitForDebuggerOnStart: false, flatten: true }, sessionId, 10000);
await sleep(800);
out.afterFalse = seen.filter((s) => s.phase === "false");

out._phase = "second-true";
await tryCall(c, "Target.setAutoAttach",
  { autoAttach: true, waitForDebuggerOnStart: false, flatten: true }, sessionId, 10000);
await sleep(800);
out.afterSecondTrue = seen.filter((s) => s.phase === "second-true");
off();
delete out._phase;

// ---------------------------------------------------------------------------------------------
// The join both mechanisms need: DOM.getFrameOwner on the PARENT session.
// ---------------------------------------------------------------------------------------------
const frameIds = Array.isArray(out.parentFrameTree_childFrameIds) ? out.parentFrameTree_childFrameIds.map((f) => f.id) : [];
const candidates = [...new Set([...out.iframeTargetIds, ...frameIds])];
out.getFrameOwner = [];
for (const fid of candidates) {
  const owner = await tryCall(c, "DOM.getFrameOwner", { frameId: fid }, sessionId, 10000);
  out.getFrameOwner.push({ frameId: fid, ok: owner.ok,
                           backendNodeId: owner.ok ? owner.result?.backendNodeId ?? null : null,
                           error: owner.ok ? null : owner.error.message });
}
// And the control: a frameId that is not in this page must REFUSE, or getFrameOwner cannot be
// used as a membership test.
const bogus = await tryCall(c, "DOM.getFrameOwner", { frameId: "DEADBEEFDEADBEEFDEADBEEFDEADBEEF" }, sessionId, 10000);
out.getFrameOwner_unknownFrameRefuses = !bogus.ok;
out.getFrameOwner_unknownFrameError = bogus.ok ? `ACCEPTED: ${JSON.stringify(bogus.result)}` : bogus.error.message;

c.close();
console.log(JSON.stringify(out, null, 2));
e.kill(); parent.close(); child.close();
