#!/usr/bin/env node
// T0 guard: the fixture set Tasks 4/11/16/17 compile against must exist, parse, and carry the keys
// those tasks read. Run from anywhere: `node docs/.../probes/t0-check.mjs`.
// Red before t0-run.sh has been run; green after. Never asserts a *value* an engine produced —
// only that the shape a downstream `include_str!` will index into is present.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const PROBE_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(PROBE_DIR, "../../../../..");
if (!fs.existsSync(path.join(REPO, "Cargo.toml"))) {
  console.error(`FAIL repo root not found from ${PROBE_DIR} (resolved ${REPO})`);
  process.exit(2);
}
const CDP = path.join(REPO, "crates/aleph-cdp/tests/fixtures");
const PAGE = path.join(REPO, "src/browser/page_state/fixtures");
// R48 (controller ruling, 2026-09-06): t0-support-matrix.json lives beside its one reader —
// Task 16's capability-table test in src/browser/engine/capability.rs, via a same-directory
// `include_str!("fixtures/t0-support-matrix.json")` — NOT under crates/aleph-cdp/tests/fixtures/.
const ENGINE = path.join(REPO, "src/browser/engine/fixtures");
const MAX_BYTES = 4 * 1024 * 1024;

// key path helper: "model.content" -> value or undefined. Checks the WHOLE path as a literal
// own-property key first — chrome-void.json is a flat map keyed by full CDP method names, which
// themselves contain a dot ("Page.enable"), so a required entry like "Page.enable" means the
// single flat key, not a nested `v.Page.enable`. Falling back to the split-and-descend form keeps
// every genuinely nested check (e.g. "model.content", "cookies.0.name") working exactly as before,
// since none of those top-level keys are themselves dotted.
const dig = (v, p) => {
  if (v != null && typeof v === "object" && Object.prototype.hasOwnProperty.call(v, p)) return v[p];
  return p.split(".").reduce((a, k) => (a == null ? a : a[k]), v);
};

// [file, [required key paths], optional predicate returning true | "why not"]
//
// R38: a fixture exists only where a NAMED test include_str!s it (判据 §17). Every entry below
// carries the Task-4 test that reads it; a file with no reader is not captured at all, which is
// why there is no obscura-void.json, no chrome-Target.attachToTarget.json and no
// obscura-DOM.getBoxModel.hidden.json — their findings live in t0-support-matrix.json and in
// t0-results.md, which do have readers.
const REQUIRED = [
  // ---- Chrome, under crates/aleph-cdp/tests/fixtures/ ----
  // reader: browser_get_version_parses_what_chrome_sent
  [`${CDP}/chrome-Browser.getVersion.json`, ["product", "userAgent", "protocolVersion"]],
  // reader: target_create_and_close_send_the_id_and_read_the_answer
  [`${CDP}/chrome-Target.createTarget.json`, ["targetId"]],
  [`${CDP}/chrome-Target.closeTarget.json`, ["success"]],
  // reader: target_get_targets_parses_both_engines
  [`${CDP}/chrome-Target.getTargets.json`, ["targetInfos.0.targetId", "targetInfos.0.type", "targetInfos.0.url"]],
  // reader: page_navigate_parses_both_engines_and_keeps_the_loader_id
  [`${CDP}/chrome-Page.navigate.json`, ["frameId"]],
  // reader: page_get_frame_tree_parses_the_chrome_fixture_and_its_child_frames
  [`${CDP}/chrome-Page.getFrameTree.json`, ["frameTree.frame.id", "frameTree.frame.loaderId", "frameTree.frame.url"]],
  // readers: page_get_layout_metrics_reads_only_the_css_pair,
  //          page_get_layout_metrics_refuses_a_reply_that_only_has_the_device_pixel_pair
  [`${CDP}/chrome-Page.getLayoutMetrics.json`, ["cssVisualViewport.clientWidth", "cssContentSize.width"]],
  // reader: page_navigation_history_returns_the_index_and_the_entries
  [`${CDP}/chrome-Page.getNavigationHistory.json`, ["currentIndex", "entries.0.id", "entries.0.url"]],
  // reader: page_screenshot_and_pdf_decode_their_base64_payloads
  [`${CDP}/chrome-Page.captureScreenshot.json`, ["data"]],
  [`${CDP}/chrome-Page.printToPDF.json`, ["data"]],
  // reader: dom_get_document_parses_both_engines_and_asks_for_the_whole_tree
  [`${CDP}/chrome-DOM.getDocument.json`, ["root.nodeId", "root.backendNodeId", "root.nodeName", "root.children"]],
  // reader: dom_query_selector_maps_the_peers_zero_to_none
  [`${CDP}/chrome-DOM.querySelector.json`, ["nodeId"],
    (v) => v.nodeId !== 0 || "querySelector hit must have a non-zero nodeId"],
  [`${CDP}/chrome-DOM.querySelector.miss.json`, ["nodeId"],
    (v) => v.nodeId === 0 || "querySelector miss must be nodeId 0"],
  [`${CDP}/chrome-DOM.querySelectorAll.json`, ["nodeIds"]],
  // reader: dom_resolve_and_describe_and_the_void_dom_verbs_send_their_arguments
  [`${CDP}/chrome-DOM.describeNode.json`, ["node.backendNodeId", "node.nodeName"]],
  [`${CDP}/chrome-DOM.resolveNode.json`, ["object.objectId"]],
  // reader: dom_box_model_parses_both_engines
  [`${CDP}/chrome-DOM.getBoxModel.json`,
    ["model.content", "model.padding", "model.border", "model.margin", "model.width", "model.height"],
    (v) => (v.model?.content?.length === 8 || "content quad must have 8 numbers")
        && ((Number.isInteger(v.model?.width) && Number.isInteger(v.model?.height))
            || "width/height must be integers (aleph-cdp types them i64)")],
  // readers: the_no_box_matcher_is_the_code_and_message_chrome_actually_sends,
  //          dom_get_box_model_answers_none_for_the_refusal_chrome_actually_sent
  [`${CDP}/chrome-DOM.getBoxModel.hidden.json`, ["error.code", "error.message"]],
  // reader: dom_get_box_model_propagates_every_other_protocol_error
  [`${CDP}/chrome-DOM.getBoxModel.badnode.json`, ["error.code", "error.message"]],
  // reader: runtime_evaluate_returns_the_value_and_always_asks_for_it_by_value
  [`${CDP}/chrome-Runtime.evaluate.json`, ["result.type"]],
  // reader: runtime_evaluate_surfaces_a_thrown_error_instead_of_a_value
  [`${CDP}/chrome-Runtime.evaluate.throws.json`, ["exceptionDetails.text"]],
  // reader: runtime_call_function_on_wraps_its_arguments_the_way_cdp_expects
  [`${CDP}/chrome-Runtime.callFunctionOn.json`, ["result.type"]],
  // readers: network_cookies_parse_from_both_engines_and_survive_a_round_trip,
  //          network_set_cookies_supplies_a_url_only_for_cookies_with_no_domain
  [`${CDP}/chrome-Network.getAllCookies.json`,
    ["cookies.0.name", "cookies.0.value", "cookies.0.domain", "cookies.0.path"]],
  // reader: network_get_and_delete_cookies_send_exactly_their_arguments
  [`${CDP}/chrome-Network.getCookies.json`, ["cookies"]],
  // reader: dom_snapshot_keeps_the_raw_reply_and_exposes_its_two_arrays
  [`${CDP}/chrome-DOMSnapshot.captureSnapshot.json`,
    ["documents.0.nodes.backendNodeId", "documents.0.layout.nodeIndex", "documents.0.layout.bounds", "strings"]],
  // readers: every_void_method_the_probe_captured_is_accepted_by_its_wrapper,
  //          target_activate_and_discover_send_exactly_their_arguments,
  //          page_dialog_and_history_and_reload_send_exactly_their_arguments
  [`${CDP}/chrome-void.json`,
    ["Page.enable", "Runtime.enable", "DOM.focus", "Input.dispatchMouseEvent",
     "Emulation.setDeviceMetricsOverride", "Emulation.setEmulatedMedia",
     "Emulation.setCPUThrottlingRate", "Network.emulateNetworkConditions",
     "Network.setExtraHTTPHeaders", "Network.clearBrowserCookies"]],
  // ---- obscura: only the seven a Task-4 test parses ----
  // reader: browser_get_version_parses_what_obscura_sent_with_the_same_type
  [`${CDP}/obscura-Browser.getVersion.json`, ["product", "userAgent", "protocolVersion"]],
  // reader: target_get_targets_parses_both_engines
  [`${CDP}/obscura-Target.getTargets.json`, ["targetInfos"]],
  // reader: page_navigate_parses_both_engines_and_keeps_the_loader_id
  [`${CDP}/obscura-Page.navigate.json`, ["frameId"]],
  // reader: dom_get_document_parses_both_engines_and_asks_for_the_whole_tree
  [`${CDP}/obscura-DOM.getDocument.json`, ["root.nodeId", "root.backendNodeId", "root.nodeName"]],
  // reader: dom_box_model_parses_both_engines
  [`${CDP}/obscura-DOM.getBoxModel.json`, ["model.content", "model.width", "model.height"],
    (v) => (v.model?.content?.length === 8 || "content quad must have 8 numbers")
        && ((Number.isInteger(v.model?.width) && Number.isInteger(v.model?.height))
            || "width/height must be integers (aleph-cdp types them i64)")],
  // reader: runtime_evaluate_returns_the_value_and_always_asks_for_it_by_value
  [`${CDP}/obscura-Runtime.evaluate.json`, ["result.type"]],
  // reader: network_cookies_parse_from_both_engines_and_survive_a_round_trip
  [`${CDP}/obscura-Network.getAllCookies.json`, ["cookies"]],
  // ---- cross-engine support record (R48: lives beside its one reader, src/browser/engine/) ----
  // reader: the_support_matrix_records_both_engines
  [`${ENGINE}/t0-support-matrix.json`, ["chrome", "obscura"]],
  // ---- page_state fixtures: all read by Part 3's Task 11, none by any aleph-cdp test ----
  [`${PAGE}/hn-chromium.domsnapshot.json`, ["documents.0.layout.bounds", "strings"]],
  // R57 (controller ruling, 2026-09-06): DOMSnapshot.captureSnapshot spans same-process frames
  // only — a genuine OOPIF is invisible to the parent session's own call, measured directly
  // (Target.getTargets shows it as a separate attached target; Page.getFrameTree on the parent
  // does not list it either). `local-oopif.domsnapshot.json` (one call assumed to hold both
  // documents) is REPLACED by three raw, unmerged captures — the same-origin path IS reachable
  // with one call, the OOPIF path needs two separate ones and no probe-side stitching.
  // reader: Part 3 Task 11 (same-origin iframe path — the child is document index 1 within one
  // captureSnapshot call; documents.length MUST be 2 for this fixture to mean anything)
  [`${PAGE}/local-sameorigin-iframe.domsnapshot.json`,
    ["documents.1.documentURL", "documents.1.layout.bounds"],
    (v) => v.documents?.length === 2
      || "a same-origin iframe capture must be exactly two documents (one call, in-process)"],
  // reader: Part 3 Task 11 (OOPIF path, parent half — one document only; the child is invisible
  // to this call, which is the whole point of this fixture)
  [`${PAGE}/local-oopif-parent.domsnapshot.json`,
    ["documents.0.documentURL", "documents.0.layout.bounds"],
    (v) => v.documents?.length === 1
      || "an OOPIF parent capture must be exactly one document (the child lives in a different renderer)"],
  // reader: Part 3 Task 11 (OOPIF path, child half — the child's OWN captureSnapshot on its own
  // auto-attached session, coordinates left frame-local exactly as CDP returned them, no shift)
  [`${PAGE}/local-oopif-child.domsnapshot.json`,
    ["documents.0.documentURL", "documents.0.layout.bounds"],
    (v) => v.documents?.length === 1
      || "an OOPIF child's own capture must be exactly one document"],
];


let failures = 0;
const fail = (msg) => { console.error(`FAIL ${msg}`); failures += 1; };
for (const [file, keys, pred] of REQUIRED) {
  const rel = path.relative(REPO, file);
  if (!fs.existsSync(file)) { fail(`missing ${rel}`); continue; }
  const bytes = fs.statSync(file).size;
  if (bytes === 0) { fail(`${rel}: empty`); continue; }
  if (bytes > MAX_BYTES) {
    fail(`${rel}: ${bytes} bytes > ${MAX_BYTES} cap — report to the controller, do not commit`);
    continue;
  }
  let v;
  try { v = JSON.parse(fs.readFileSync(file, "utf8")); }
  catch (e) { fail(`${rel}: not JSON (${e.message})`); continue; }
  for (const k of keys) if (dig(v, k) === undefined) fail(`${rel}: missing ${k}`);
  if (pred) {
    // F1 (review round 1): a predicate that dereferences a key it does not itself guard
    // (`v.model.content`, `v.documents.length`) throws on a missing/wrong-typed key, and an
    // uncaught throw here used to abort the WHOLE scan — every entry after the first crash was
    // never checked, and the operator saw a stack trace instead of the failure list. Catching it
    // turns that into a named FAIL and lets the scan continue, which is what "every predicate must
    // survive a missing or wrong-typed key" requires structurally, independent of whether any one
    // predicate remembers to use optional chaining.
    let r;
    try { r = pred(v); }
    catch (e) { r = `predicate threw ${e.constructor.name}: ${e.message}`; }
    if (r !== true) fail(`${rel}: ${r === false ? "predicate failed" : r}`);
  }
}
// t0-results.md must exist and must not still carry the marker the report writes when a probe
// produced no verdict at all.
const results = path.join(PROBE_DIR, "..", "t0-results.md");
if (!fs.existsSync(results)) fail("missing t0-results.md");
else if (fs.readFileSync(results, "utf8").includes("NO-VERDICT")) {
  fail("t0-results.md still carries a NO-VERDICT row");
}

console.log(failures === 0
  ? `PASS ${REQUIRED.length} fixtures + t0-results.md`
  : `${failures} failure(s)`);
process.exit(failures === 0 ? 0 : 1);
