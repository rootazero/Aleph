// T17b fix-r2 / D3, 先数一遍: which constructs escape a DOM ancestor's `opacity` group, and is
// `DOM.getTopLayerElements` exactly that set?
//
// One candidate per page render. For each candidate the page is rendered twice — ancestor
// `opacity: 0` and `opacity: 1` — and the two screenshots are compared as raw base64:
//
//   identical  => the candidate's painting is UNAFFECTED by the ancestor's opacity  => ESCAPES
//   different  => the ancestor's opacity changed what was painted                   => contained
//
// `#filler` is inside the container on every variant and never escapes, so the `none` row is the
// instrument's control: if THAT came back identical, the comparison would be measuring nothing.
//
// usage: node t17b-escape.mjs
import { launchEngine, newPage, serveStatic, emit, sleep,
         PROBE_DIR, PARENT_PORT } from "./t0-lib.mjs";

const server = await serveStatic(PROBE_DIR, PARENT_PORT);
const e = await launchEngine("chrome");
const MODE = process.argv[2] ?? "opacity";
const url = (only, fade) =>
  `http://127.0.0.1:${PARENT_PORT}/t17b-escape.html?only=${only}&fade=${fade}&mode=${MODE}`;

const CANDS = [
  ["none", "no candidate — #filler only. The CONTROL: must be `contained`, or the test is blind"],
  ["dlg", "<dialog>.showModal() — the modal dialog"],
  ["nonmodal", "<dialog>.show() — open but NOT modal, so not in the top layer"],
  ["pop", "[popover=manual].showPopover()"],
  ["fixed", "position: fixed — opacity makes a containing block for it, but does it escape paint?"],
  ["fs", "Element.requestFullscreen() — the third way into the top layer"],
];

const render = async (only, fade, tag) => {
  const p = await newPage(e.wsUrl, url(only, fade), { width: 260, height: 160, tag });
  await sleep(400);
  let opened = await p.c.call("Runtime.evaluate",
    { expression: "JSON.stringify(window.__result)", returnByValue: true }, p.sessionId, 30000)
    .then((r) => JSON.parse(r.result.value ?? "{}")).catch(() => ({}));
  if (only === "fs") {
    // Fullscreen needs user activation, which `userGesture` supplies.
    const r = await p.c.call("Runtime.evaluate",
      { expression: "window.__goFullscreen()", awaitPromise: true, userGesture: true,
        returnByValue: true }, p.sessionId, 30000).catch((err) => ({ err: String(err) }));
    opened.fullscreen = r?.result?.value ?? r?.err ?? "unknown";
    await sleep(400);
  }
  // Which elements does CDP call top-layer, on this exact render?
  let topLayer = [];
  try {
    await p.c.call("DOM.getDocument", { depth: -1 }, p.sessionId, 30000);
    const tl = await p.c.call("DOM.getTopLayerElements", {}, p.sessionId, 30000);
    for (const nodeId of tl.nodeIds ?? []) {
      const d = await p.c.call("DOM.describeNode", { nodeId }, p.sessionId, 30000).catch(() => null);
      const attrs = d?.node?.attributes ?? [];
      let id = null;
      for (let k = 0; k + 1 < attrs.length; k += 2) if (attrs[k] === "id") id = attrs[k + 1];
      topLayer.push(id ?? d?.node?.nodeName ?? "?");
    }
  } catch (err) { topLayer = ["ERROR: " + String(err?.message ?? err)]; }

  const shot = await p.c.call("Page.captureScreenshot",
    { format: "png", captureBeyondViewport: false }, p.sessionId, 60000);
  p.c.close();
  return { png: shot.data, opened, topLayer };
};

const out = { probe: "T17b-escape", rows: [] };
const version = await (async () => {
  const p = await newPage(e.wsUrl, "about:blank", { tag: "v" });
  const v = await p.c.call("Browser.getVersion", {}, undefined, 30000).catch(() => null);
  p.c.close();
  return v?.product ?? null;
})();
out.chrome = version;

for (const [only, what] of CANDS) {
  const faded = await render(only, "0", `f-${only}`);
  const opaque = await render(only, "1", `o-${only}`);
  const escapes = faded.png === opaque.png;
  out.rows.push({
    candidate: only, what,
    opened: faded.opened,
    topLayerPerCdp: faded.topLayer,
    identicalUnderOpacityZeroAndOne: escapes,
    verdict: escapes
      ? "ESCAPES the ancestor's opacity group — painted the same whether the ancestor is "
        + "opacity:0 or opacity:1"
      : "CONTAINED — the ancestor's opacity changed what was painted",
  });
}

// The question the fix turns on: is CDP's top-layer set exactly the escaping set?
const escaping = out.rows.filter((r) => r.identicalUnderOpacityZeroAndOne && r.candidate !== "none")
  .map((r) => r.candidate);
const listedByCdp = out.rows.filter((r) => r.candidate !== "none"
  && r.topLayerPerCdp.some((t) => t === "cand-" + r.candidate)).map((r) => r.candidate);
out.escapingCandidates = escaping;
out.candidatesCdpCallsTopLayer = listedByCdp;
out.cdpSetEqualsEscapingSet =
  escaping.length === listedByCdp.length && escaping.every((x) => listedByCdp.includes(x));
out.controlHeld = out.rows.find((r) => r.candidate === "none")
  ?.identicalUnderOpacityZeroAndOne === false;

emit(out);
e.kill(); server.close();
