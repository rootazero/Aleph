// The generated config, patched into the one this fixture can run against.
//
// A separate file from `qa/busy_input/patch_config.py` for the same reason
// `qa/teamchat_rooms/patch_config.mjs` is one — a per-fixture patcher is the
// established shape in this tree, because the differences are not flags. This
// one also folds in what used to be `patch_terminal.py`: two scripts writing
// two keys into the same file, in the same run, is one derivation split in two
// (判据 §1), and the failure mode of forgetting the second is a stage that
// spawns into the wrong directory.
//
// ## The keys that are not obvious
//
// `[agents.defaults] workspace_root` — `pty.spawn`'s `cwd` is a REQUEST, not
// an authorisation: `gateway::pty::jail::resolve_spawn_cwd` refuses any
// directory outside the registered workspace roots, and `workspace_roots()`
// resolves exactly this key. Without it every spawn in the `cwd` stage would
// land in the scratch home's default workspace instead of the three
// directories the stage needs to tell apart, and the refusal ("cwd … is
// outside every registered workspace") reads like a fixture path bug rather
// than a policy answer.
//
// `[policies.terminal] enabled` — default-on today, written anyway. A fixture
// whose subject is the embedded terminal must not be able to go green because
// some future default flipped and every stage silently spawned nothing.
//
// The inline provider block is what keeps `tools.invoke` real: with no key the
// server boots `Mode: Simulated`, where `tools.catalog` answers normally and
// `tools.invoke` replies "boot phase 2" — which reads like a missing
// registration and is not.
//
// Nothing here APPENDS a table that the generated config already has: a
// duplicate header is `duplicate key`, which the server reports AFTER printing
// a banner with the default port, so it reads like a port clash rather than a
// config error.
//
// Usage:  node patch_config.mjs <config.toml> <gateway-port> <mock-port> <workspace-root>
import fs from "node:fs";

const [configPath, gatewayPort, mockPort, workspaceRoot] = process.argv.slice(2);
if (!configPath || !gatewayPort || !mockPort || !workspaceRoot) {
  console.error("usage: patch_config.mjs <config.toml> <gateway-port> <mock-port> <workspace-root>");
  process.exit(2);
}

let src = fs.readFileSync(configPath, "utf8");

/** `[section]` / `[[section]]` header name, or null. */
const headerName = (line) => {
  const t = line.trim();
  if (!t.startsWith("[") || !t.endsWith("]")) return null;
  return t.replace(/^\[+/, "").replace(/\]+$/, "");
};

/**
 * `key` of a `key = value` line, or null. Split on the first `=` and trim
 * rather than matching a hand-built key regex — a second thing that can be
 * wrong about a line this file has to rewrite exactly once.
 */
const keyName = (line) => {
  const eq = line.indexOf("=");
  if (eq < 0) return null;
  const left = line.slice(0, eq).trim();
  if (!left || left.includes("[") || left.includes("#")) return null;
  return left;
};

/** Drop whole sections whose header name satisfies `pred`. */
const dropSections = (text, pred) => {
  const out = [];
  let keep = true;
  for (const line of text.split(/\r?\n/)) {
    const h = headerName(line);
    if (h !== null) keep = !pred(h);
    if (keep) out.push(line);
  }
  return `${out.join("\n")}\n`;
};

/** Set `key = value` inside `[section]`, creating the section if absent. */
const setKey = (text, section, key, value) => {
  const out = [];
  let cur = null;
  let inserted = false;
  for (const line of text.split(/\r?\n/)) {
    const h = headerName(line);
    if (h !== null) {
      cur = h;
      out.push(line);
      if (cur === section) {
        out.push(`${key} = ${value}`);
        inserted = true;
      }
      continue;
    }
    // A key of the same name under a different header is a DIFFERENT key;
    // rewriting it would edit someone else's setting and leave ours absent.
    if (cur === section && keyName(line) === key) continue;
    out.push(line);
  }
  let next = `${out.join("\n")}\n`;
  if (!inserted) next += `\n[${section}]\n${key} = ${value}\n`;
  return next;
};

src = dropSections(src, (s) => /^(channels|providers|agents)/.test(s));

// TOML basic strings take backslash escapes, and on Windows every path in this
// fixture is full of them. Forward slashes are accepted by every path API the
// server uses and cannot be mis-escaped.
const tomlPath = (p) => JSON.stringify(p.replace(/\\/g, "/"));

for (const [section, key, value] of [
  ["gateway", "host", '"127.0.0.1"'],
  ["gateway", "port", gatewayPort],
  ["cron", "enabled", "false"],
  ["heartbeat", "enabled", "false"],
  ["mcp", "enabled", "false"],
  ["acp", "enabled", "false"],
  ["evolution", "enabled", "false"],
  ["memory", "enabled", "false"],
  ["skills", "enabled", "false"],
  ["agents.defaults", "workspace_root", tomlPath(workspaceRoot)],
  ["policies.terminal", "enabled", "true"],
]) {
  src = setKey(src, section, key, value);
}

src += `
[providers.qa-mock]
enabled = true
protocol = "anthropic"
base_url = "http://127.0.0.1:${mockPort}"
api_key = "qa-dummy-not-a-real-key"
models = ["qa-mock-model"]
timeout_seconds = 600
stream_idle_timeout_secs = 0

[[agents.list]]
id = "main"
name = "QA Main"
default = true
model = "qa-mock-model"
provider = "qa-mock"
system_prompt = "QA fixture."
`;

fs.writeFileSync(configPath, src);
console.log(
  `patched ${configPath}: gateway ${gatewayPort}, mock ${mockPort}, ` +
    `workspace_root ${tomlPath(workspaceRoot)}`,
);
