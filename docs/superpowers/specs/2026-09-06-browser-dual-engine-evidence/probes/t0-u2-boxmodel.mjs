// U2: what does each engine return from DOM.getBoxModel for elements that are not laid out?
// usage: node t0-u2-boxmodel.mjs <chrome|obscura>
import { launchEngine, newPage, locate, tryCall, parentUrl, serveStatic, PROBE_DIR,
         PARENT_PORT, emit } from "./t0-lib.mjs";

const engine = process.argv[2] ?? "obscura";
const server = await serveStatic(PROBE_DIR, PARENT_PORT);
const e = await launchEngine(engine);
const { c, sessionId } = await newPage(e.wsUrl, parentUrl());

const CASES = ["#go", "#hidden-none", "#hidden-vis", "#zero-size", "#offscreen", "#opacity-zero"];
const out = { u: "U2", engine, families: server.families, cases: {} };
for (const sel of CASES) {
  const loc = await locate(c, sessionId, sel);
  // backendNodeId, because that is what `methods::dom::get_box_model` sends; a refusal for a
  // bogus nodeId is a different lookup and need not be worded the same.
  const r = loc.backendNodeId
    ? await tryCall(c, "DOM.getBoxModel", { backendNodeId: loc.backendNodeId }, sessionId, 30000)
    : { ok: false, error: { code: null, message: "no backendNodeId (querySelector found nothing)" } };
  out.cases[sel] = {
    nodeId: loc.nodeId, backendNodeId: loc.backendNodeId, clientRect: loc.clientRect,
    boxModel: r.ok ? r.result.model : null, error: r.ok ? null : r.error.message,
  };
}
// A "constant quadrilateral" is the shape the spec worries about: every not-laid-out element
// answering with the SAME quad. Compare the four not-laid-out cases against each other.
const quads = ["#hidden-none", "#hidden-vis", "#zero-size", "#offscreen"]
  .map((s) => out.cases[s].boxModel?.content)
  .filter(Boolean).map((q) => JSON.stringify(q));
out.distinct_hidden_quads = [...new Set(quads)].length;
out.hidden_all_errored = ["#hidden-none", "#hidden-vis", "#offscreen"].every((s) => out.cases[s].error);

const none = out.cases["#hidden-none"];
out.verdict = none.error
  ? engine + ": `display:none` ⇒ honest failure, error text " + JSON.stringify(none.error)
    + " (" + (out.hidden_all_errored ? "all three not-laid-out cases fail" : "only some fail") + ")."
  : engine + ": `display:none` ⇒ a box IS returned, content quad "
    + JSON.stringify(none.boxModel.content) + " (" + out.distinct_hidden_quads
    + " distinct quads across the four not-laid-out cases) — a getBoxModel failure is NOT a "
    + "visibility signal on this engine; the interim fetcher must read `computed.display_none`.";
emit(out);
c.close(); e.kill(); server.close();
