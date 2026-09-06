// U9: on obscura, does an INLINE element get a box?
// Part 5 measured a zero quad for <a>/<span> on its fixture page from all three of
// DOM.getBoxModel, DOM.getContentQuads and getBoundingClientRect(); the spike's M11 measured a
// real box for HN's <a> ([130,11,83,15]). Both cannot describe the same rule, so this probe
// varies the one thing that differed — where the inline element sits — and reads all three
// sources for each placement, on both engines.
// usage: node t0-u9-inline.mjs <chrome|obscura>
import { launchEngine, newPage, locate, tryCall, serveStatic, PROBE_DIR, PARENT_PORT, emit } from "./t0-lib.mjs";

const engine = process.argv[2] ?? "obscura";
const INLINE_URL = `http://127.0.0.1:${PARENT_PORT}/t0-inline.html`;
const HN_URL = process.env.T0_HN_URL ?? "https://news.ycombinator.com/";

// The four local placements, plus the two HN selectors that tie this to M11: `a[href="news"]` is
// the element M11 quoted [130,11,83,15] for, `span.titleline a` is the first story link. Both are
// measured so the tie is to a named element rather than to whichever one "the link" meant.
const LOCAL = [
  ["a-in-body", "#a-in-body"],
  ["a-in-p", "#a-in-p"],
  ["a-in-td", "#a-in-td"],
  ["span-in-block", "#span-in-block"],
];
const HN = [
  ["hn-title-link", 'a[href="news"]'],
  ["hn-first-story", "span.titleline a"],
];

const allZero = (nums) => Array.isArray(nums) && nums.length > 0 && nums.every((n) => Math.round(n) === 0);

async function measure(c, sessionId, name, selector) {
  const loc = await locate(c, sessionId, selector);
  const out = { name, selector, nodeId: loc.nodeId, backendNodeId: loc.backendNodeId, clientRect: loc.clientRect };
  if (!loc.nodeId) {
    out.absent = true;
    return out;
  }
  const bm = await tryCall(c, "DOM.getBoxModel", { backendNodeId: loc.backendNodeId }, sessionId, 30000);
  if (bm.ok) {
    const m = bm.result.model;
    out.boxModel = { width: m.width, height: m.height, content: m.content };
    out.boxModelHasBox = !allZero(m.content) && m.width > 0 && m.height > 0;
  } else {
    out.boxModelError = bm.error.message;
    out.boxModelHasBox = false;
  }
  const cq = await tryCall(c, "DOM.getContentQuads", { nodeId: loc.nodeId }, sessionId, 30000);
  if (cq.ok) {
    const quads = cq.result.quads ?? [];
    out.contentQuads = quads.slice(0, 2);
    out.contentQuadsHasBox = quads.length > 0 && quads.some((q) => !allZero(q));
  } else {
    out.contentQuadsError = cq.error.message;
    out.contentQuadsHasBox = false;
  }
  const r = loc.clientRect;
  out.clientRectHasBox = Boolean(r && r.w > 0 && r.h > 0);
  // Three sources that disagree is itself the finding — record it rather than picking a winner.
  out.sourcesAgree = out.boxModelHasBox === out.contentQuadsHasBox
                  && out.boxModelHasBox === out.clientRectHasBox;
  return out;
}

const server = await serveStatic(PROBE_DIR, PARENT_PORT);
const e = await launchEngine(engine);
const out = { u: "U9", engine, localUrl: INLINE_URL, hnUrl: HN_URL, local: [], hn: null };

{
  const { c, sessionId } = await newPage(e.wsUrl, INLINE_URL);
  for (const [name, selector] of LOCAL) out.local.push(await measure(c, sessionId, name, selector));
  c.close();
}

// The HN half is network-dependent. It must not take the local half down with it: an unreachable
// site is "not measured", never "no box" (判据 §8).
try {
  const { c, sessionId } = await newPage(e.wsUrl, HN_URL, { settleMs: 2500 });
  const title = await tryCall(c, "Runtime.evaluate", { expression: "document.title", returnByValue: true }, sessionId, 30000);
  const titleText = title.ok ? title.result?.result?.value : null;
  if (!titleText) {
    out.hn = { reachable: false, why: `document.title empty (${title.ok ? "no value" : title.error.message})` };
  } else {
    out.hn = { reachable: true, title: titleText, elements: [] };
    for (const [name, selector] of HN) out.hn.elements.push(await measure(c, sessionId, name, selector));
  }
  c.close();
} catch (err) {
  out.hn = { reachable: false, why: String(err?.message ?? err) };
}

const withBox = out.local.filter((m) => m.boxModelHasBox).map((m) => m.name);
const withoutBox = out.local.filter((m) => !m.boxModelHasBox).map((m) => m.name);
out.placementsWithBox = withBox;
out.placementsWithoutBox = withoutBox;
out.disagreeing = out.local.filter((m) => !m.sourcesAgree).map((m) => m.name);
const shape = withBox.length === out.local.length
  ? "all-inline-have-boxes"
  : withBox.length === 0
    ? "none"
    : `only-${withBox.join(",")}-have-boxes`;
out.shape = shape;

const hnPart = out.hn?.reachable
  ? "HN: " + out.hn.elements.map((m) => `${m.name} ${m.boxModelHasBox ? `box ${JSON.stringify(m.clientRect)}` : "NO BOX"}`).join(", ")
  : `HN UNMEASURED (${out.hn?.why ?? "not attempted"})`;
out.verdict = `${engine}: ${shape}`
  + (withoutBox.length ? ` (no box for [${withoutBox.join(",")}])` : "")
  + (out.disagreeing.length
      ? `; getBoxModel / getContentQuads / getBoundingClientRect DISAGREE for [${out.disagreeing.join(",")}]`
      : "; all three sources agree on every placement")
  + `; ${hnPart}.`;
emit(out);
e.kill(); server.close();
