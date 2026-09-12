// U4c — is RECURSING the OOPIF enumeration even possible?
//
// `fetch_chromium` now confesses a grandchild rather than refusing the page (F-1). The forward
// obligation is recursion, and it rests on one unmeasured fact: **can `DOM.getFrameOwner` on a
// CHILD's session resolve that child's own child?** If it cannot, recursion is not merely
// expensive — it is not available at all, and the confession is the end of the road rather than a
// stepping stone.
//
// Three SITES, which is what site isolation splits on:
//   a  http://127.0.0.1:18999/t0-page.html?child=<b>
//   b  http://localhost:19001/t0-page.html?child=<c>     (t0-page.html nests from ?child=)
//   c  http://[::1]:19001/t0-frame.html
// `127.0.0.1`, `localhost` and `[::1]` are three different hosts, and `serveStatic` already binds
// both loopback families on each port.
//
// Writes nothing into the repo.
import {
  launchChrome, newPage, tryCall, serveStatic, PROBE_DIR, PARENT_PORT, CHILD_PORT, sleep,
} from "./t0-lib.mjs";

const out = { u: "U4c" };
const sa = await serveStatic(PROBE_DIR, PARENT_PORT);
const sb = await serveStatic(PROBE_DIR, CHILD_PORT);

const cUrl = `http://[::1]:${CHILD_PORT}/t0-frame.html`;
const bUrl = `http://localhost:${CHILD_PORT}/t0-page.html?child=${encodeURIComponent(cUrl)}`;
const aUrl = `http://127.0.0.1:${PARENT_PORT}/t0-page.html?child=${encodeURIComponent(bUrl)}`;
out.urls = { a: aUrl, b: bUrl, c: cUrl };

const e = await launchChrome({ extraArgs: ["--site-per-process"] });
if (!e.wsUrl) {
  console.log(JSON.stringify({ ...out, error: "chrome published no endpoint" }, null, 2));
  process.exit(1);
}

const { c, sessionId } = await newPage(e.wsUrl, null, { tag: "u4c" });
const load = c.waitEvent("Page.loadEventFired", sessionId, 45000).catch((x) => String(x));
await c.call("Page.navigate", { url: aUrl }, sessionId, 45000).catch(() => {});
await load;
await sleep(2500);

const t = await tryCall(c, "Target.getTargets", {}, undefined, 10000);
const iframes = t.ok ? t.result.targetInfos.filter((x) => x.type === "iframe") : [];
out.iframeTargets = iframes.map((x) => ({ targetId: x.targetId, url: x.url }));
out.threeRenderers = iframes.length >= 2;
if (!out.threeRenderers) {
  out.verdict = "UNMEASURED: fewer than two iframe targets — the three-site setup did not produce "
    + "three renderers, so the nesting question was never asked. Targets seen: "
    + JSON.stringify(out.iframeTargets);
} else {
  // Which of them does the PAGE session own? Exactly one should: `b`.
  out.fromPageSession = [];
  for (const f of iframes) {
    const r = await tryCall(c, "DOM.getFrameOwner", { frameId: f.targetId }, sessionId, 10000);
    out.fromPageSession.push({ targetId: f.targetId, url: f.url, ok: r.ok,
                               backendNodeId: r.ok ? r.result?.backendNodeId ?? null : null,
                               error: r.ok ? null : r.error.message });
  }
  const b = out.fromPageSession.find((x) => x.ok);
  const c3 = out.fromPageSession.find((x) => !x.ok);
  out.pageSessionOwnsExactlyOne = Boolean(b) && Boolean(c3)
    && out.fromPageSession.filter((x) => x.ok).length === 1;

  // THE QUESTION: attach to b, and ask b's session about c.
  if (b && c3) {
    const att = await tryCall(c, "Target.attachToTarget",
                              { targetId: b.targetId, flatten: true }, undefined, 10000);
    out.attachedToMiddle = att.ok;
    if (att.ok) {
      const mid = att.result.sessionId;
      const r = await tryCall(c, "DOM.getFrameOwner", { frameId: c3.targetId }, mid, 10000);
      out.grandchildFromMiddleSession = { ok: r.ok,
                                          backendNodeId: r.ok ? r.result?.backendNodeId ?? null : null,
                                          error: r.ok ? null : r.error.message };
      // Control: the middle session must NOT resolve itself from its own session.
      const self = await tryCall(c, "DOM.getFrameOwner", { frameId: b.targetId }, mid, 10000);
      out.middleResolvesItself = { ok: self.ok, error: self.ok ? null : self.error.message };
      await tryCall(c, "Target.detachFromTarget", { sessionId: mid }, undefined, 10000);
    }
  }
  out.verdict = out.grandchildFromMiddleSession?.ok
    ? "RECURSION IS AVAILABLE: DOM.getFrameOwner on the MIDDLE frame's own session resolves the "
      + "grandchild (backendNodeId " + out.grandchildFromMiddleSession.backendNodeId + "), in the "
      + "middle frame's node space - which is exactly the coordinate ChildCapture cannot express today."
    : "RECURSION IS NOT AVAILABLE by this route: the middle session refused the grandchild ("
      + JSON.stringify(out.grandchildFromMiddleSession) + ")";
}

c.close();
console.log(JSON.stringify(out, null, 2));
e.kill(); sa.close(); sb.close();
