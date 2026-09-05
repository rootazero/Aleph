// Build the three screens `qa/terminal/fake-claude.cjs` paints, FROM the
// shipped manifest — never by hand-copying its strings into a script.
//
// The fake agent's whole job is to make
// `crates/agent-detect/src/manifests/claude.toml` fire. A fixture that
// hard-codes `esc to interrupt` next to a manifest that owns that literal is
// the same fact written twice (判据 §1): the day the manifest's wording moves,
// the fixture keeps painting the old chrome, no rule matches, `state` stays
// `unknown` — and the stage reads as "detection is broken" rather than "the
// fixture is stale".
//
// So the literals are EXTRACTED from the rules by id:
//
//     live_prompt_box    (idle,    region prompt_box_body)
//     live_turn_working  (working, region bottom_non_empty_lines(12))
//     live_blocked_form  (blocked, region after_last_horizontal_rule)
//
// `contains` literals are taken as they are written; `line_regex` literals are
// recovered by walking the pattern and keeping its maximal literal runs (the
// `.*`/`\s*`/character-class parts are skipped, so `^\s*[⏸⏵].*esc to interrupt…`
// yields the class's first glyph and the run `esc to interrupt`). Each built
// line is then CHECKED against the manifest's own pattern — a rule whose shape
// this script cannot satisfy any more is a loud failure here, at generation
// time, instead of a mysterious `unknown` forty seconds into a stage.
//
// What this script deliberately does NOT do is decide which rule *wins*. That
// would mean reimplementing the region extractor and the priority walk here —
// a second engine, which is the very shape §1 warns about, and one that would
// be wrong in a different way from the real one. The winner is asserted at
// RUNTIME instead, by `terminal{explain}`, which runs the shipped engine and
// names the rule it matched.
//
// ONE output, `chrome.json`, read by both consumers (the fake agent and the
// driver). The bash-era second output `chrome.env` is gone with the bash fake:
// two renderings of one dict is the §1 shape this file's own header warns
// about, and it only existed because a shell script cannot read JSON.
//
// Usage:  node derive_chrome.mjs <claude.toml> <out-dir>
import fs from "node:fs";
import { parseToml } from "./toml_min.mjs";

const die = (msg) => {
  console.error(`derive_chrome: ${msg}`);
  process.exit(1);
};

// Metacharacters that end a literal run. `-` and `,` are NOT here: they are
// literal outside a character class, and dropping them would silently shorten
// a run that a future rule depends on.
const META = new Set([..."^$.|?*+()[]{}"]);
// A quantifier binds only the character before it, so `abc*` contributes the
// run `ab`, not `abc`. Recorded here rather than special-cased below because
// it is the one place this walker can be wrong in a way that still *looks*
// like a plausible literal.
const QUANTIFIERS = new Set([..."*+?"]);

/** `\x{2800}` is Rust-regex spelling; JS wants `\u{2800}` with the `u` flag. */
const rustRegexToJs = (pattern) => pattern.replace(/\\x\{([0-9A-Fa-f]+)\}/g, "\\u{$1}");

/**
 * Compile a manifest pattern the way the engine would read it.
 *
 * `(?m)` and `(?i)` are Rust-regex INLINE flags, which JS does not accept in
 * the pattern body — they become RegExp flags. Unrecognised inline groups are
 * left alone so the compile throws rather than silently matching something
 * else.
 */
const compile = (pattern) => {
  let body = rustRegexToJs(pattern);
  let flags = "u";
  for (;;) {
    const m = /^\(\?([ims]+)\)/.exec(body);
    if (!m) break;
    if (m[1].includes("m")) flags += "m";
    if (m[1].includes("i")) flags += "i";
    if (m[1].includes("s")) flags += "s";
    body = body.slice(m[0].length);
  }
  return new RegExp(body, flags);
};

/** Index just past the escape sequence starting at `pattern[i] === "\\"`. */
const skipEscape = (pattern, i) => {
  if (i + 1 >= pattern.length) return i + 1;
  if (pattern[i + 1] === "x" && pattern[i + 2] === "{") {
    const end = pattern.indexOf("}", i + 2);
    if (end !== -1) return end + 1;
  }
  return i + 2;
};

/** Index just past the character class starting at `pattern[i] === "["`. */
const skipClass = (pattern, i) => {
  let j = i + 1;
  while (j < pattern.length && pattern[j] !== "]") {
    j = pattern[j] === "\\" ? skipEscape(pattern, j) : j + 1;
  }
  return j + 1;
};

/**
 * The maximal runs of literal text in a regex, longest first.
 *
 * Escapes, groups and character classes end a run; a trailing quantifier takes
 * its own character with it.
 */
const literalRuns = (pattern) => {
  const runs = [];
  let cur = [];
  const flush = (dropLast = false) => {
    if (dropLast) cur.pop();
    if (cur.length) runs.push(cur.join(""));
    cur = [];
  };
  let i = 0;
  while (i < pattern.length) {
    const ch = pattern[i];
    if (ch === "\\") {
      flush();
      i = skipEscape(pattern, i);
      continue;
    }
    if (ch === "[") {
      flush();
      i = skipClass(pattern, i);
      continue;
    }
    if (QUANTIFIERS.has(ch)) {
      flush(true);
      i += 1;
      continue;
    }
    if (META.has(ch)) {
      flush();
      i += 1;
      continue;
    }
    cur.push(ch);
    i += 1;
  }
  flush();
  return runs.filter((r) => r.trim()).sort((a, b) => b.length - a.length);
};

/**
 * The first alternative of the pattern's first character class.
 * `[⏸⏵]` -> `⏸`. Used for the working rule's spinner glyph, which is a set of
 * equivalent choices rather than a literal.
 */
const firstClassGlyph = (pattern) => {
  const start = pattern.indexOf("[");
  if (start === -1) die(`no character class in ${JSON.stringify(pattern)}`);
  const body = pattern.slice(start + 1, skipClass(pattern, start) - 1);
  if (body.startsWith("^")) die(`negated character class is not a source of glyphs: ${pattern}`);
  if (!body) die(`empty character class in ${pattern}`);
  if (body.startsWith("\\")) {
    const escaped = body.slice(0, skipEscape(body, 0));
    const expanded = escaped.replace(/\\x\{([0-9A-Fa-f]+)\}/g, (_, hex) =>
      String.fromCodePoint(parseInt(hex, 16)),
    );
    if ([...expanded].length === 1) return expanded;
    die(`cannot expand ${escaped} to one glyph`);
  }
  return [...body][0];
};

const ruleById = (manifest, id) => {
  const rule = (manifest.rules ?? []).find((r) => r.id === id);
  if (!rule) {
    die(
      `rule ${id} is gone from the manifest. The fixture's screens are derived ` +
        `from it; pick the rule that replaced it rather than pasting its old ` +
        `text back into the fake agent.`,
    );
  }
  return rule;
};

const expectRegion = (rule, want) => {
  if (rule.region !== want) {
    die(
      `rule ${rule.id} now reads region ${rule.region}, not ${want}. The screen ` +
        `this script builds is shaped for ${want} (horizontal rules, prompt box), ` +
        `so it would no longer be shown to the rule.`,
    );
  }
};

/** The built line must satisfy the manifest's own pattern. */
const checkLine = (pattern, line, ruleId) => {
  if (!compile(pattern).test(line)) {
    die(
      `the line built for ${ruleId} does not match its own pattern.\n` +
        `  pattern: ${pattern}\n  line:    ${JSON.stringify(line)}`,
    );
  }
};

/** None of the rule's `not` clauses may match the region we built. */
const checkNotGates = (rule, regionText) => {
  const lowered = regionText.toLowerCase();
  for (const gate of rule.not ?? []) {
    const needles = gate.contains ?? [];
    if (needles.length && needles.every((n) => lowered.includes(n.toLowerCase()))) {
      die(`rule ${rule.id}'s \`not\` clause ${JSON.stringify(needles)} matches the built screen`);
    }
    for (const pattern of [...(gate.regex ?? []), ...(gate.line_regex ?? [])]) {
      if (compile(pattern).test(regionText)) {
        die(`rule ${rule.id}'s \`not\` pattern ${pattern} matches the built screen`);
      }
    }
  }
};

// `is_horizontal_rule` (crates/agent-detect/src/manifest.rs) accepts a line of
// `─` when the run is at least 3 long. 40 is comfortably inside a 100-column
// QA terminal, so the rule never wraps into two lines — a wrapped rule is not
// a rule, and the region would silently become the whole screen.
const HRULE = "─".repeat(40);

const build = (manifestPath) => {
  const manifest = parseToml(fs.readFileSync(manifestPath, "utf8"));
  if (manifest.id !== "claude") die(`expected the claude manifest, got id=${manifest.id}`);

  // --- idle: a prompt box whose body carries the prompt glyph --------------
  const idleRule = ruleById(manifest, "live_prompt_box");
  expectRegion(idleRule, "prompt_box_body");
  const idlePattern = (idleRule.line_regex ?? [])[0];
  if (!idlePattern) die("live_prompt_box no longer carries a line_regex to derive the prompt glyph from");
  const idleRuns = literalRuns(idlePattern);
  if (!idleRuns.length) die(`no literal to build a prompt line from in ${idlePattern}`);
  const idleBody = ` ${idleRuns[0]} `;
  checkLine(idlePattern, idleBody, idleRule.id);
  checkNotGates(idleRule, idleBody);
  // `prompt_box_top_border_index` walks up from the bottom and wants the
  // SECOND rule it meets, so the box needs both borders.
  const idleScreen = [HRULE, idleBody, HRULE].join("\n");

  // --- working: the live-turn spinner line ---------------------------------
  const workingRule = ruleById(manifest, "live_turn_working");
  expectRegion(workingRule, "bottom_non_empty_lines(12)");
  const branch = (workingRule.any ?? []).find((b) => b.line_regex);
  if (!branch) die("live_turn_working no longer has an `any` branch with a line_regex");
  const workingPattern = branch.line_regex[0];
  const workingRuns = literalRuns(workingPattern);
  if (!workingRuns.length) die(`no literal to build a working line from in ${workingPattern}`);
  const workingScreen = `${firstClassGlyph(workingPattern)} qa fixture ${workingRuns[0]}`;
  checkLine(workingPattern, workingScreen, workingRule.id);
  checkNotGates(workingRule, workingScreen);

  // --- blocked: a confirmation form below a horizontal rule ----------------
  const blockedRule = ruleById(manifest, "live_blocked_form");
  expectRegion(blockedRule, "after_last_horizontal_rule");
  const blockedNeedles = [...(blockedRule.contains ?? [])];
  if (!blockedNeedles.length) die("live_blocked_form no longer carries `contains` literals");
  const anyBranch = (blockedRule.any ?? []).find((b) => b.contains && !b.any);
  if (!anyBranch) die("live_blocked_form has no simple `any` branch to satisfy");
  const blockedBody = [...anyBranch.contains, ...blockedNeedles].join(" · ");
  checkNotGates(blockedRule, blockedBody);
  const lowered = blockedBody.toLowerCase();
  const missing = [...blockedNeedles, ...anyBranch.contains].filter(
    (n) => !lowered.includes(n.toLowerCase()),
  );
  if (missing.length) die(`built blocked line is missing its own literals: ${JSON.stringify(missing)}`);
  const blockedScreen = [HRULE, blockedBody].join("\n");

  const screen = (rule, text) => ({
    rule: rule.id,
    state: rule.state,
    region: rule.region,
    priority: rule.priority,
    text,
  });

  return {
    manifest: manifestPath,
    agent_id: manifest.id,
    manifest_version: manifest.version,
    screens: {
      idle: screen(idleRule, idleScreen),
      working: screen(workingRule, workingScreen),
      blocked: screen(blockedRule, blockedScreen),
    },
  };
};

const [manifestPath, outDir] = process.argv.slice(2);
if (!manifestPath || !outDir) die("usage: derive_chrome.mjs <claude.toml> <out-dir>");
const built = build(manifestPath);
fs.writeFileSync(`${outDir}/chrome.json`, `${JSON.stringify(built, null, 2)}\n`);
for (const [name, s] of Object.entries(built.screens)) {
  console.log(`  ${name.padEnd(8)} <- ${s.rule} (priority ${s.priority})`);
}
console.log(`  manifest version ${built.manifest_version}`);
