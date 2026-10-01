// qa-events-probe.mjs — the two event-arrival readings C3 (browser_qa) owes,
// per engine, on REAL binaries:
//
//   node qa-events-probe.mjs                  # obscura (default)
//   node qa-events-probe.mjs --engine chrome  # real chromium
//
// 1. `Runtime.exceptionThrown` — the event `browser_qa`'s check_errors
//    dimension is folded from (cdp_backend/events.rs turns it into the
//    console ring's `[error]` lines). The obscura half is ALSO measured by
//    qa/browser_dual's caps stage; this script is the independent second
//    reading and the only CHROMIUM one (the caps stage probes the obscura
//    Aleph launched, never a chromium). C1-tightened criterion: arrival of
//    OUR exception — the marker must ride the event's exceptionDetails — and
//    the throwing callback is proven to have run by a flag set before the
//    throw (判据 §2: the silence is the engine's only when the callback
//    fired). The trigger is an ASYNC throw: a synchronous throw inside
//    Runtime.evaluate comes back in the call's own exceptionDetails and an
//    engine is entitled to never broadcast it as an event.
// 2. `Target.targetDestroyed` — the event C3 Task 1's tab-death detector
//    folds (events.rs destroyed arm → TabTable fold → TabGone). The arm is
//    pinned against FakeCdpServer; whether a REAL engine emits the event at
//    all was unmeasured, and this build has silent-miss precedents in
//    exactly this class (Page.javascriptDialogOpening — the js_dialogs row).
//    The production pump calls `Target.setDiscoverTargets(true)` (events.rs),
//    so the probe does too, and the destroy is provoked two ways: the CDP
//    `Target.closeTarget` Aleph's own close_tab sends, and `Page.close`
//    (the user-clicks-the-X shape — the C2 recording residual's "single tab
//    dies while the engine lives" case). The event must name OUR targetId:
//    arrival of any death is not arrival of this one.
//
// Lives beside the spec that demanded it (this directory's parent), reusing
// the 2026-09-06 dual-engine evidence round's launch helpers rather than
// growing a second copy (the launch argv is a measured property of those
// helpers).
import path from "node:path";
import { fileURLToPath } from "node:url";
import { launchObscura, launchChrome, Cdp, sleep, emit }
  from "../../2026-09-06-browser-dual-engine-evidence/probes/t0-lib.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const ENGINE = args.includes("--engine") ? args[args.indexOf("--engine") + 1] : "obscura";
const MARKER = `qa-events-probe-${ENGINE}`;

const report = { engine: ENGINE, steps: {} };
const finish = async (e, c, out) => { try { c?.close(); } catch {} e.kill(); emit(out); };

const e = ENGINE === "chrome" || ENGINE === "chromium"
  ? await launchChrome({})
  : await launchObscura({});
if (!e.wsUrl) { emit({ verdict: "LAUNCH_FAILED", engine: ENGINE, stderr: e.stderr?.slice(-400) }); }
report.wsUrl = e.wsUrl;
report.pid = e.pid;

const c = new Cdp(e.wsUrl, "qa-events");
await c.connect();

// ---------------------------------------------------------------- step 1 --
// Runtime.exceptionThrown arrival, on an attached page session.
const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
const { sessionId } = await c.call("Target.attachToTarget", { targetId, flatten: true });
await c.call("Page.enable", {}, sessionId).catch(() => {});
await c.call("Runtime.enable", {}, sessionId).catch(() => {});

const evalJs = async (expr) => {
  const r = await c.call("Runtime.evaluate", { expression: expr, returnByValue: true },
    sessionId, 10000).catch((err) => ({ __error: String(err) }));
  return r?.result?.value ?? null;
};

const thrownWait = c.waitEvent("Runtime.exceptionThrown", sessionId, 10000)
  .then((p) => p).catch(() => null);
await evalJs(`setTimeout(function(){ window.__errFired = '${MARKER}';`
  + ` throw new Error('${MARKER}'); }, 50)`);
const thrown = await thrownWait;
const fired = await evalJs("window.__errFired");
report.steps.exceptionThrown = {
  callbackRan: fired === MARKER,
  arrived: !!thrown && JSON.stringify(thrown).includes(MARKER),
  got: thrown ? JSON.stringify(thrown).slice(0, 240) : null,
};

// ---------------------------------------------------------------- step 2 --
// Target.targetDestroyed arrival, browser-level, after setDiscoverTargets —
// once per destroy provocation. The existing target from step 1 is closed
// by Target.closeTarget (Aleph's close_tab shape); a fresh one by Page.close
// (the user-closes-the-tab shape).
const disc = await c.call("Target.setDiscoverTargets", { discover: true })
  .then(() => "ok").catch((err) => String(err));
report.steps.setDiscoverTargets = disc;

const destroyReads = {};
for (const [label, tid, how] of [
  ["closeTarget", targetId, null], // the step-1 target, closed Aleph's way
]) {
  const wait = c.waitEvent("Target.targetDestroyed", null, 10000)
    .then((p) => p).catch(() => null);
  await c.call("Target.closeTarget", { targetId: tid }).catch((err) => String(err));
  const ev = await wait;
  destroyReads[label] = { arrived: !!ev && ev.targetId === tid, got: ev ?? null };
}

// Page.close on a fresh target — a destroy the ENGINE initiates at the
// page's request rather than one we requested over the wire.
const { targetId: tid2 } = await c.call("Target.createTarget", { url: "about:blank" });
const { sessionId: s2 } = await c.call("Target.attachToTarget", { targetId: tid2, flatten: true });
await c.call("Page.enable", {}, s2).catch(() => {});
const wait2 = c.waitEvent("Target.targetDestroyed", null, 10000)
  .then((p) => p).catch(() => null);
const closeCall = await c.call("Page.close", {}, s2).then(() => "ok").catch((err) => String(err));
if (closeCall === "ok") {
  const ev2 = await wait2;
  destroyReads.pageClose = { provocable: true, arrived: !!ev2 && ev2.targetId === tid2, got: ev2 ?? null };
} else {
  // An engine that does not implement Page.close cannot be provoked this
  // way — that is a fixture limit, not a reading of the event (判据 §2),
  // so the shape is recorded unprovocable and left out of the verdict.
  destroyReads.pageClose = { provocable: false, error: closeCall };
}
report.steps.targetDestroyed = destroyReads;

const et = report.steps.exceptionThrown;
const td = report.steps.targetDestroyed;
const readings = [et.arrived, td.closeTarget?.arrived,
  ...(td.pageClose?.provocable ? [td.pageClose.arrived] : [])];
const verdict =
  !et.callbackRan ? "FIXTURE_BROKEN"
  : readings.every(Boolean) ? "ALL_ARRIVE"
  : readings.some(Boolean) ? "PARTIAL"
  : "NONE_ARRIVE";
await finish(e, c, { verdict, ...report });
