// Turn a freshly generated Aleph config into the daemon the round-2 stages
// need. Structure and the section-scoped key rewriting are lifted from
// `qa/teamchat_rooms/patch_config.mjs`; what differs is listed here so a
// reader does not have to diff two files to find out.
//
//  1. **Two models on one provider.** The `knobs` stage's whole claim is that
//     a resumed run follows the SNAPSHOT its `RunStarted` carried rather than
//     the session's current value, and a single-model provider cannot tell
//     those two apart — the assertion would be green for a build that dropped
//     the envelope entirely.
//  2. **`[resume] enabled`** is set here rather than left to the round-1
//     Python driver, so a stage can boot with resume already on or off
//     without a second tool on the path.
//  3. **The `bash` policy is the stage's instrument.** How a dangling call is
//     MADE here is not the round-1 fixture's way, and the reason is measured,
//     on this host, 2026-09-03:
//       * with the sandbox on (the default), every `bash` call returns in
//         ~240ms with `exit_code -1073741502` and `AppContainer setup failed
//         (0x000000cb); falling back to restricted-token path` — git-bash
//         cannot fork under the restricted token, so `sleep 120` never sleeps;
//       * with `[sandbox] enabled = false`, every call returns in 0ms with
//         `Sandbox error: sandbox disabled: set [sandbox] enabled = true`.
//     Neither can leave a call in flight, so a long-running command is not an
//     instrument on this host at all. `bash = "ask"` is: the dispatch is
//     durably logged and the call then parks on an approval card nobody will
//     answer, which is precisely "dispatched, no receipt". `deny` is what the
//     `denied` stage needs, and `allow` is what the burst stage needs (it
//     wants many fast events, and a command that fails fast is still an
//     event pair).
//  4. **Memory / cron / heartbeat / mcp / acp / evolution / skills off.** None
//     of them is under test, and each is a timer that can rewrite the log this
//     fixture reads.
//
// usage: patch_r2.mjs <config.toml> <gateway-port> <mock-port> <resume:true|false> [bash-policy]
import fs from "node:fs";

const [path, gatewayPort, mockPort, resumeEnabled = "true", bashPolicy = "allow"] =
  process.argv.slice(2);
if (!path || !gatewayPort || !mockPort) {
  console.error("usage: patch_r2.mjs <config.toml> <gateway-port> <mock-port> [resume] [bash-policy]");
  process.exit(2);
}

let src = fs.readFileSync(path, "utf8");

const headerName = (line) => {
  const t = line.trim();
  if (!t.startsWith("[") || !t.endsWith("]")) return null;
  return t.replace(/^\[+/, "").replace(/\]+$/, "");
};

const keyName = (line) => {
  const eq = line.indexOf("=");
  if (eq < 0) return null;
  const left = line.slice(0, eq).trim();
  if (!left || left.includes("[") || left.includes("#")) return null;
  return left;
};

const dropSections = (text, pred) => {
  const out = [];
  let keep = true;
  for (const line of text.split(/\r?\n/)) {
    const h = headerName(line);
    if (h !== null) keep = !pred(h);
    if (keep) out.push(line);
  }
  return out.join("\n") + "\n";
};

src = dropSections(src, (s) => /^(channels|providers|agents|policies\.tool_permissions\.overrides)/.test(s));

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
        out.push(key + " = " + value);
        inserted = true;
      }
      continue;
    }
    if (cur === section && keyName(line) === key) continue; // replaced above
    out.push(line);
  }
  let next = out.join("\n") + "\n";
  if (!inserted) next += "\n[" + section + "]\n" + key + " = " + value + "\n";
  return next;
};

for (const [section, key, value] of [
  ["gateway", "port", gatewayPort],
  ["resume", "enabled", resumeEnabled],
  ["cron", "enabled", "false"],
  ["heartbeat", "enabled", "false"],
  ["mcp", "enabled", "false"],
  ["acp", "enabled", "false"],
  ["evolution", "enabled", "false"],
  ["skills", "enabled", "false"],
  ["memory", "enabled", "false"],
  ["memory.dreaming", "enabled", "false"],
]) {
  src = setKey(src, section, key, value);
}

src += `
[providers.qa-mock]
enabled = true
protocol = "anthropic"
base_url = "http://127.0.0.1:${mockPort}"
api_key = "qa-dummy-not-a-real-key"
models = ["qa-model-a", "qa-model-b"]
timeout_seconds = 600
stream_idle_timeout_secs = 0

[[agents.list]]
id = "main"
name = "QA Main"
default = true
model = "qa-model-a"
provider = "qa-mock"
system_prompt = "QA fixture."

[policies.tool_permissions.overrides]
bash = "${bashPolicy}"
`;

fs.writeFileSync(path, src, "utf8");

// A duplicate key aborts config loading entirely, and the only symptom
// downstream is "server died" forty lines into a boot log. The key set resets
// at EVERY header, not per header NAME.
const dupes = [];
{
  let cur = null;
  let seen = new Set();
  for (const line of src.split(/\r?\n/)) {
    const h = headerName(line);
    if (h !== null) {
      cur = h;
      seen = new Set();
      continue;
    }
    const k = keyName(line);
    if (k === null || cur === null) continue;
    if (seen.has(k)) dupes.push(`${cur}.${k}`);
    else seen.add(k);
  }
}
if (dupes.length > 0) {
  console.error(`patch_r2: produced duplicate keys: ${dupes.join(", ")}`);
  process.exit(1);
}

console.log(
  `patched ${path}: gateway ${gatewayPort}, mock ${mockPort}, resume=${resumeEnabled}, bash=${bashPolicy}`,
);
