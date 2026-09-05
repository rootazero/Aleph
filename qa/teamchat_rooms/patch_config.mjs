// Turn a freshly generated Aleph config into the daemon the fixtures that
// call it need.
//
// ## Callers — a change here changes all three
//
// `qa/teamchat_rooms/run.sh`, `qa/agents_viz/run.sh:136`, and
// `qa/multiuser_audit/run.sh:150` all shell out to this file rather than
// keeping a patcher of their own. It is not this fixture's private file —
// edit it and every caller's real-machine stage moves.
//
// ## Contract
//
//     node patch_config.mjs <config.toml> <gateway-port> <mock-port>
//
// Produces: inert provider (every channel / provider / agent the generator
// wrote is dropped, then exactly one provider — the mock — is added back) +
// LAN leg open + memory on + two agents (`main`, `coder`).
//
// ## Why JavaScript when every sibling is Python
//
// The Python drivers under `qa/` need `websockets`, and the gateway's only
// client-facing transport is a WebSocket. On a Windows host the only `python3`
// on PATH is the WindowsApps stub (it exits 49 and opens the Store), the
// uv-managed CPython has no `websockets`, and installing one needs a network
// this box does not have. Node ships a WHATWG `WebSocket` client and an HTTP
// server in core, with no dependency to install — so the fixture that CAN run
// here is worth more than the one that matches the sibling's language.
//
// ## What each part of the contract is for
//
// 1. **Make it inert.** Drop every channel / provider / agent the generator
//    wrote, then add back exactly one provider (the mock) and two agents.
// 2. **Open the LAN leg.** `resolve_connect_auth` authorises a loopback peer on
//    its first line, before it reads `bootstrap_ticket` — so a ticket redeemed
//    over 127.0.0.1 binds no principal, silently and successfully. Two of the
//    three identities here ARE principals, so the remote leg is not optional.
//    `allow_insecure_remote` is the server's own documented opt-in; the
//    alternative (a self-signed cert plus clients taught to trust it) would
//    test the TLS stack instead of this round's work.
// 3. **Leave memory ON.** The Memory tab's claim is that a room run's note
//    lands in `main__p-<id>` and reads back through `memory.listFacts`. Under
//    `[memory] enabled = false` that assertion could only ever be vacuous —
//    which is why this is a separate file from `qa/busy_input/patch_config.py`
//    rather than a flag on it.
//
// Key matching is done by splitting on the first `=` and trimming, not by a
// regex: TOML keys are plain identifiers here, and a hand-built key regex is a
// second thing that can be wrong about a line this file has to rewrite exactly
// once. (It was wrong the first time — a mangled escape made every dedupe miss,
// and the server refused to boot on `duplicate key`.)
import fs from "node:fs";

const [path, gatewayPort, mockPort] = process.argv.slice(2);
if (!path || !gatewayPort || !mockPort) {
  console.error("usage: patch_config.mjs <config.toml> <gateway-port> <mock-port>");
  process.exit(2);
}

let src = fs.readFileSync(path, "utf8");

/** `[section]` / `[[section]]` header name, or null. */
const headerName = (line) => {
  const t = line.trim();
  if (!t.startsWith("[") || !t.endsWith("]")) return null;
  return t.replace(/^\[+/, "").replace(/\]+$/, "");
};

/** `key` of a `key = value` line, or null. */
const keyName = (line) => {
  const eq = line.indexOf("=");
  if (eq < 0) return null;
  const left = line.slice(0, eq).trim();
  if (!left || left.includes("[") || left.includes("#")) return null;
  return left;
};

// Drop whole sections whose header name starts with one of these.
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

src = dropSections(src, (s) => /^(channels|providers|agents)/.test(s));

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
  // The remote leg: a member connection must arrive from a non-loopback peer
  // or it is authorised as operator before its ticket is ever read.
  ["gateway", "host", '"0.0.0.0"'],
  ["gateway", "allow_insecure_remote", "true"],
  ["gateway", "port", gatewayPort],
  ["cron", "enabled", "false"],
  ["heartbeat", "enabled", "false"],
  ["mcp", "enabled", "false"],
  ["acp", "enabled", "false"],
  ["evolution", "enabled", "false"],
  ["skills", "enabled", "false"],
  // ON, unlike the busy-input sibling — see the module doc.
  ["memory", "enabled", "true"],
  // Dreaming rewrites the corpus on a timer; a note written mid-run must read
  // back the way it was written.
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

# The team needs a second agent to enrol. team_create makes the CALLING agent
# the leader, so this one is the member the room's humans @-mention.
[[agents.list]]
id = "coder"
name = "QA Coder"
default = false
model = "qa-mock-model"
provider = "qa-mock"
system_prompt = "QA fixture member."
`;

fs.writeFileSync(path, src, "utf8");

// Fail loudly rather than hand the server a file it will refuse: a duplicate
// key aborts config loading entirely, and the only symptom downstream is
// "server died" forty lines into a boot log.
// The key set resets at EVERY header, not per header NAME: `[[agents.list]]`
// appears twice on purpose (array-of-tables), and each occurrence opens a new
// table whose `id` is not a duplicate of the previous one's.
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
  console.error(`patch_config: produced duplicate keys: ${dupes.join(", ")}`);
  process.exit(1);
}

console.log(
  `patched ${path}: gateway ${gatewayPort} on 0.0.0.0, mock ${mockPort}, ` +
    "memory on, agents main+coder",
);
