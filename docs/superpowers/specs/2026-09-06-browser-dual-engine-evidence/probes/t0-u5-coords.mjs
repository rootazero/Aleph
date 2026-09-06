// U5: after scrolling, does Input.dispatchMouseEvent take VIEWPORT or PAGE coordinates?
// #deep-target sits below a 2000px spacer and records clientY/pageY/scrollY when clicked.
// usage: node t0-u5-coords.mjs <chrome|obscura>
import { launchEngine, newPage, locate, tryCall, parentUrl, serveStatic, PROBE_DIR,
         PARENT_PORT, sleep, emit } from "./t0-lib.mjs";

const engine = process.argv[2] ?? "obscura";
const server = await serveStatic(PROBE_DIR, PARENT_PORT);
const e = await launchEngine(engine);
const { c, sessionId } = await newPage(e.wsUrl, parentUrl());

const ev = async (expr) => {
  const r = await tryCall(c, "Runtime.evaluate", { expression: expr, returnByValue: true }, sessionId, 30000);
  return r.ok ? r.result?.result?.value : `ERR ${r.error.message}`;
};
const out = { u: "U5", engine };
// Scroll so #deep-target sits comfortably INSIDE the 800px viewport, not near its edge. A fixed
// `scrollTo(0,1500)` measured `#deep-target` at clientRect.y=826 on this page (past the 800px
// viewport bottom by 26px) — a borderline miss that reads as "ambiguous coordinate space" when it
// is really "off-screen by 26px", not a finding about which coordinate space the engine takes.
// Scrolling to place the element's top a fixed distance below viewport-top instead is robust to
// exactly how tall the content above `#deep-target` renders.
out.scrollResult = await ev(`(()=>{
  const el = document.getElementById('deep-target');
  const top = el.getBoundingClientRect().top + window.scrollY;
  window.scrollTo(0, Math.max(0, top - 400));
  return String(window.scrollY);
})()`);
await sleep(400);
const loc = await locate(c, sessionId, "#deep-target");
out.target = loc;
const sy = loc.clientRect?.scrollY ?? 0;
out.scrolledTo = sy;
const viewportPoint = loc.clientRect
  ? { x: loc.clientRect.x + Math.round(loc.clientRect.w / 2),
      y: loc.clientRect.y + Math.round(loc.clientRect.h / 2) }
  : null;
const pagePoint = viewportPoint ? { x: viewportPoint.x, y: viewportPoint.y + sy } : null;
out.viewportPoint = viewportPoint;
out.pagePoint = pagePoint;

const clickAt = async (pt) => {
  await ev("window.__clicks = []");
  for (const type of ["mousePressed", "mouseReleased"]) {
    await tryCall(c, "Input.dispatchMouseEvent",
      { type, x: pt.x, y: pt.y, button: "left", clickCount: 1,
        buttons: type === "mousePressed" ? 1 : 0 }, sessionId, 30000);
  }
  await sleep(350);
  return await ev("JSON.stringify(window.__clicks)");
};
if (viewportPoint) {
  out.clickAtViewportPoint = await clickAt(viewportPoint);
  if (sy > 0 && pagePoint.y !== viewportPoint.y) out.clickAtPagePoint = await clickAt(pagePoint);
}
const hitV = typeof out.clickAtViewportPoint === "string" && out.clickAtViewportPoint.includes("deep-target");
const hitP = typeof out.clickAtPagePoint === "string" && out.clickAtPagePoint.includes("deep-target");
out.verdict = out.scrolledTo === 0
  ? engine + ": the page did not scroll (`window.scrollY` stayed 0 after `scrollTo(0,1500)`; "
    + "scroll result " + JSON.stringify(out.scrollResult) + ") — U5 UNMEASURED on this engine. "
    + "The `Coordinates` conversion must be re-measured once scrolling works, and `browser_scroll` "
    + "cannot be claimed for it either."
  : hitV && !hitP
    ? engine + ": Input.dispatchMouseEvent takes VIEWPORT coordinates (hit at y="
      + out.viewportPoint.y + " with scrollY=" + out.scrolledTo + ", miss at y=" + out.pagePoint.y
      + ") ⇒ `ActionTarget::Coordinates` (page coords) converts by subtracting scroll."
    : hitP && !hitV
      ? engine + ": Input.dispatchMouseEvent takes PAGE coordinates (hit at y=" + out.pagePoint.y
        + ", miss at y=" + out.viewportPoint.y + ") ⇒ `Coordinates` passes page coords through."
      : engine + ": ambiguous — viewport hit=" + hitV + ", page hit=" + hitP + ", scrollY="
        + out.scrolledTo + "; raw click logs viewport=" + JSON.stringify(out.clickAtViewportPoint)
        + " page=" + JSON.stringify(out.clickAtPagePoint) + ".";
emit(out);
c.close(); e.kill(); server.close();
