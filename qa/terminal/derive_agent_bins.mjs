// Print `<agent label>\t<interactive executable>` for every agent the engine
// knows, DERIVED from `crates/agent-detect/src/engine.rs`.
//
// The `real` stage needs to find an installed agent on PATH and then say which
// label the product should answer with. Both facts already exist, in
// `agent_label` and `interactive_agent_executable`, and they disagree for
// several agents (`antigravity` -> `agy`, `github-copilot` -> `copilot`) — so a
// hand list in run.sh would be a second copy of a roster that is already wrong
// in two places the day it is written (判据 §1).
//
// A variant this cannot parse is simply omitted, which costs the fixture a
// candidate. That direction is the safe one: fewer candidates can only end in
// the stage SKIPPING, loudly, never in it asserting something false.
//
// Usage:  node derive_agent_bins.mjs path/to/engine.rs
import fs from "node:fs";

/**
 * `Agent::X => "y"` arms of one function's `match`, including the arm Cursor
 * writes as a `cfg!(windows)` block (its non-Windows string is the one this
 * fixture runs under).
 */
const arms = (src, fn) => {
  const start = src.indexOf(`pub fn ${fn}(`);
  if (start === -1) return {};
  // The next `pub fn` after this one bounds the body; the last function in the
  // file is bounded by end-of-file.
  const next = src.indexOf("\npub fn ", start + 1);
  const body = src.slice(start, next === -1 ? src.length : next);
  const out = {};
  for (const m of body.matchAll(/Agent::(\w+) => "([^"]+)"/g)) {
    if (!(m[1] in out)) out[m[1]] = m[2];
  }
  for (const m of body.matchAll(/Agent::(\w+) => \{([\s\S]*?)\n {8}\}/g)) {
    if (m[1] in out) continue;
    const strings = [...m[2].matchAll(/"([^"]+)"/g)].map((s) => s[1]);
    // `if cfg!(windows) { "a.cmd" } else { "a" }` — take the else branch.
    if (strings.length === 2) out[m[1]] = strings[1];
  }
  return out;
};

const path = process.argv[2];
if (!path) {
  console.error("usage: derive_agent_bins.mjs path/to/engine.rs");
  process.exit(2);
}
const src = fs.readFileSync(path, "utf8");
const labels = arms(src, "agent_label");
const bins = arms(src, "interactive_agent_executable");
if (!Object.keys(labels).length || !Object.keys(bins).length) {
  console.error("could not parse engine.rs");
  process.exit(1);
}
for (const variant of Object.keys(labels).filter((v) => v in bins).sort()) {
  console.log(`${labels[variant]}\t${bins[variant]}`);
}
