// Freeze the one HN capture a Rust test reads: Part 3's Task 11 parses
// src/browser/page_state/fixtures/hn-chromium.domsnapshot.json. Network-dependent by design —
// the output is a snapshot, never re-fetched. usage: node t0-hn.mjs
import { launchEngine, newPage, tryCall, PAGE_FIXTURES, COMPUTED_STYLES, writeFixture, emit } from "./t0-lib.mjs";

const URL_ = process.env.T0_HN_URL ?? "https://news.ycombinator.com/";
const e = await launchEngine("chrome");
const { c, sessionId } = await newPage(e.wsUrl, URL_, { settleMs: 2500 });

const out = { engine: "chromium", url: URL_, capturedAt: new Date().toISOString(), files: [] };
const title = await tryCall(c, "Runtime.evaluate", { expression: "document.title", returnByValue: true }, sessionId, 30000);
out.title = title.ok ? title.result?.result?.value : title.error.message;
if (!out.title || String(out.title).length === 0) {
  out.verdict = "HN UNREACHABLE (document.title empty) — do NOT commit a fixture of an error page. "
    + "Re-run when the network is up, or tell the controller so Task 11 falls back to its "
    + "hand-written two-document fixture.";
  emit(out); c.close(); e.kill(); process.exit(1);
}

const snap = await tryCall(c, "DOMSnapshot.captureSnapshot",
  { computedStyles: COMPUTED_STYLES, includeDOMRects: true, includePaintOrder: true }, sessionId, 180000);
if (!snap.ok) {
  out.error = snap.error.message;
  out.verdict = "chromium HN captureSnapshot failed: " + out.error;
  emit(out); c.close(); e.kill(); process.exit(1);
}
out.files.push(writeFixture(PAGE_FIXTURES, "hn-chromium.domsnapshot.json", snap.result));
out.documents = snap.result.documents?.length ?? 0;
out.layoutNodes = snap.result.documents?.[0]?.layout?.nodeIndex?.length ?? 0;

out.totalBytes = out.files.reduce((a, f) => a + f.bytes, 0);
out.oversize = out.files.filter((f) => f.bytes > 4 * 1024 * 1024).map((f) => f.path);
out.verdict = `chromium HN capture: ${out.documents} document(s), ${out.layoutNodes} layout nodes, `
  + `${out.totalBytes} bytes`
  + (out.oversize.length ? ` — OVERSIZE: ${out.oversize.join(", ")}; report to the controller, do not commit` : "");
emit(out);
c.close(); e.kill();
