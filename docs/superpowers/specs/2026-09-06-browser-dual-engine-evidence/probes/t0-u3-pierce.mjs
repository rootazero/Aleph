// U3: does DOM.getDocument{depth:-1,pierce:true} carry iframe content?
// usage: node t0-u3-pierce.mjs <chrome|obscura>
import { launchEngine, newPage, tryCall, parentUrl, childUrl, serveStatic, PROBE_DIR,
         PARENT_PORT, CHILD_PORT, sleep, emit } from "./t0-lib.mjs";

const engine = process.argv[2] ?? "obscura";
const parent = await serveStatic(PROBE_DIR, PARENT_PORT);
const child = await serveStatic(PROBE_DIR, CHILD_PORT);
const e = await launchEngine(engine);
const { c, sessionId } = await newPage(e.wsUrl, parentUrl(`?child=${encodeURIComponent(childUrl())}`));
await sleep(1200);

const out = { u: "U3", engine, childUrl: childUrl(),
              families: { parent: parent.families, child: child.families } };
const r = await tryCall(c, "DOM.getDocument", { depth: -1, pierce: true }, sessionId, 120000);
out.ok = r.ok;
if (!r.ok) { out.error = r.error.message; }
else {
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
  walk(r.result.root);
  out.nodes = total;
  out.contentDocuments = contentDocs;
  out.shadowRoots = shadowRoots;
  out.nodesWithFrameId = withFrameId;
  out.sawChildProbe = ids.has("probe");
  out.sawChildLink = ids.has("child-link");
  out.ids = [...ids];
}
// Did the child frame actually load? If it did not, U3's answer is "unmeasured", not "no".
const loaded = await tryCall(c, "Runtime.evaluate", {
  expression: `(()=>{const f=document.getElementById('frame-host');
    return JSON.stringify({src:f&&f.src, w:f&&f.getBoundingClientRect().width});})()`,
  returnByValue: true }, sessionId, 30000);
out.frameHost = loaded.ok ? loaded.result?.result?.value : loaded.error.message;

out.verdict = !r.ok
  ? engine + ": `getDocument{pierce:true}` failed — " + out.error
  : out.sawChildProbe
    ? engine + ": `pierce:true` DOES carry iframe content (" + out.contentDocuments
      + " contentDocument(s), child `#probe` present, " + out.nodes + " nodes, "
      + out.nodesWithFrameId + " carry a frameId) ⇒ the interim fetcher can flatten frames from "
      + "this one call."
    : engine + ": `pierce:true` does NOT carry iframe content (" + out.contentDocuments
      + " contentDocument(s), child `#probe` absent, " + out.nodes + " nodes; frame host src "
      + JSON.stringify(out.frameHost) + ") ⇒ the interim fetcher must attach a session per frame.";
emit(out);
c.close(); e.kill(); parent.close(); child.close();
