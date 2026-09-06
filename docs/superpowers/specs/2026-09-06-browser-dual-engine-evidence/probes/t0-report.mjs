// Assemble t0-results.md from the raw probe outputs and rewrite the spec's §11 table from the same
// verdicts. usage: node t0-report.mjs <raw-dir>
import fs from "node:fs";
import path from "node:path";
import { EVIDENCE_DIR } from "./t0-lib.mjs";

const RAW = process.argv[2];
if (!RAW || !fs.existsSync(RAW)) { console.error(`t0-report: raw dir ${RAW} not found`); process.exit(2); }
const read = (n) => {
  const p = path.join(RAW, `${n}.json`);
  if (!fs.existsSync(p)) return null;
  try { return JSON.parse(fs.readFileSync(p, "utf8")); } catch { return null; }
};
const v = (n) => read(n)?.verdict ?? null;
const join = (...names) => {
  const parts = names.map((n) => v(n)).filter(Boolean);
  return parts.length ? parts.join(" · ") : "NO-VERDICT";
};

const VERDICTS = {
  U1: v("u1") ?? "NO-VERDICT",
  U2: join("u2-obscura", "u2-chrome"),
  U3: join("u3-obscura", "u3-chrome"),
  U4: v("u4") ?? "NO-VERDICT",
  U5: join("u5-obscura", "u5-chrome"),
  U6: join("u6-obscura", "u6-chrome"),
  U7: "deferred to Task 14 (`ALEPH_QA_DRIVER=playwright_cli` baseline, then `=cdp`).",
  U8: "deferred to Task 20 (upstream `domsnapshot.rs` diff + `render-repros`).",
  U9: join("u9-obscura", "u9-chrome"),
};

// Rows this script would ADD to §11 that the spec does not already carry.
//
// Empty at HEAD `15ba17c33`: the spec's §11 already has nine rows — U9 was added there in that
// commit with its own wording, and U1 was rewritten to R14's conclusion. So this report only
// fills the verdict column; it never rewrites a question, a method or an impact cell, and it must
// not resurrect the 备选① branch R14 deleted. The table stays because it is what makes a future
// added row idempotent, and because an empty table is the honest way to say "nothing to add".
const EXTRA_ROWS = {};

// How many rows §11 is expected to have. A mismatch means the spec moved under this script, and
// rebuilding the table from a shape we do not recognise is how a hand-written conclusion gets
// silently replaced by a probe's phrasing.
const SPEC_ROWS = 9;

// ---- t0-results.md ----------------------------------------------------------------------------
const md = [];
md.push("# T0 results — spec §11 U1–U6 and U9, measured 2026-09-06");
md.push("");
md.push("Produced by `probes/t0-run.sh` → `probes/t0-report.mjs`. Every number below is a probe's");
md.push("own output; the raw JSON stays in the session scratchpad (evidence README: raw outputs are");
md.push("not committed). Re-run: `bash probes/t0-run.sh`.");
md.push("");
md.push("| U | verdict |");
md.push("|---|---|");
for (const [k, s] of Object.entries(VERDICTS)) md.push(`| ${k} | ${s.replace(/\|/g, "\\|")} |`);
md.push("");
for (const file of fs.readdirSync(RAW).filter((f) => f.endsWith(".json")).sort()) {
  const name = file.replace(/\.json$/, "");
  md.push(`## ${name}`);
  md.push("");
  md.push("```json");
  md.push(JSON.stringify(read(name), null, 2));
  md.push("```");
  md.push("");
}
fs.writeFileSync(path.join(EVIDENCE_DIR, "t0-results.md"), md.join("\n"));
console.log(`wrote ${path.join(EVIDENCE_DIR, "t0-results.md")}`);

// ---- spec §11 table ---------------------------------------------------------------------------
const SPEC = path.join(EVIDENCE_DIR, "..", "2026-09-06-browser-dual-engine-design.md");
const spec = fs.readFileSync(SPEC, "utf8");
const HEAD = "## 11. 未验证 / T0 待实测";
const start = spec.indexOf(HEAD);
if (start < 0) { console.error("t0-report: §11 heading not found"); process.exit(3); }
const after = spec.indexOf("\n---\n", start);
if (after < 0) { console.error("t0-report: §11 terminator not found"); process.exit(3); }
const allRows = spec.slice(start, after).split("\n").filter((l) => /^\| U\d /.test(l));
const cellsOf = (row) => row.split("|").slice(1, -1).map((s) => s.trim());
// Rows this script owns are dropped and rebuilt, so a second run is a no-op rather than a
// duplicate. What is left must be exactly the eight the spec was written with.
const original = allRows.filter((r) => !(cellsOf(r)[0] in EXTRA_ROWS));
if (original.length !== SPEC_ROWS) {
  console.error(
    `t0-report: expected ${SPEC_ROWS} spec-authored U rows, found ${original.length} — §11 moved; `
      + "re-read it before letting this script rebuild the table",
  );
  process.exit(3);
}
const rebuilt = [
  HEAD, "",
  "> `T0 结论` 一列由 `probes/t0-report.mjs` 从",
  "> `2026-09-06-browser-dual-engine-evidence/t0-results.md` 的探针输出直接写入 —— 这里不手抄数字。",
  "> 其余四列原样保留：U1 已由 R14 结案，本轮只是**再测一次**并把观察填进结论列。",
  "",
  "| # | 问题 | 怎么测 | 影响 | T0 结论 |",
  "|---|---|---|---|---|",
];
const emit = (u, question, how, impact) => {
  const verdict = (VERDICTS[u] ?? "NO-VERDICT").replace(/\|/g, "\\|");
  rebuilt.push(`| ${u} | ${question} | ${how} | ${impact} | ${verdict} |`);
};
for (const row of original) {
  const c = cellsOf(row);
  emit(c[0], c[1], c[2], c[3]);
}
for (const [u, [question, how, impact]] of Object.entries(EXTRA_ROWS)) {
  emit(u, question, how, impact);
}
rebuilt.push("");
fs.writeFileSync(SPEC, spec.slice(0, start) + rebuilt.join("\n") + spec.slice(after));
console.log(
  `patched ${SPEC} §11 (${original.length} spec rows + ${Object.keys(EXTRA_ROWS).length} added)`,
);
