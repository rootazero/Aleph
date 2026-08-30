// Turn a freshly generated Aleph config into the daemon this fixture needs.
//
// Structure and the key-rewriting helpers are lifted from
// `qa/teamchat_rooms/patch_config.mjs` deliberately — see that file for why
// these fixtures are JavaScript rather than Python (no usable `python3` on a
// Windows host, and the gateway's only client transport is a WebSocket).
//
// What differs here, and why:
//
//  1. **A channel exists.** Every claim in this round is about a *channel
//     group conversation* bound to a room, so the fixture needs a channel that
//     really enters through `InboundMessageRouter`. `webhook` is the one
//     channel type a test can drive with nothing but an HTTP POST and an HMAC:
//     no upstream service to mock, no OAuth, no polling loop. The binding
//     mechanism is channel-agnostic — `(channel_id, peer_kind, peer_id)` — so
//     `webhook` stands in for the brief's `telegram` without weakening a
//     single assertion. `qa/rooms_channel_bind/README.md` says so in one line
//     so a reader does not have to infer it.
//
//  2. **The instance is named after its type.** `subsystems.rs` registers the
//     per-channel policy block under the *instance* id while several factories
//     hardcode the runtime id to the channel TYPE. Naming the instance
//     `webhook` is the documented way to make the two meet; under any other
//     name `permission_level` below would silently do nothing.
//
//  3. **`permission_level = "config"`.** Without it a channel run carries
//     `caller_role = "guest"`, which caps the turn at `ExecTier::Ask`, and
//     every `note_manage` the mock issues parks for approval — roughly a dozen
//     120-second approval races in a fixture whose subject is *attribution*,
//     not approvals. The tier is orthogonal to everything scenarios 1–8b
//     assert (scope comes from `pairing_store::sender_user`, never from the
//     role), and the one claim that IS about the tier gate — addendum A —
//     is driven over a member's *Panel* connection, which is the surface that
//     gate is written for. Stated here rather than left to be discovered,
//     because a reader who assumes a stock Telegram posture would read the
//     absence of approval cards as evidence of something.
//
//  4. **Memory ON, dreaming OFF.** Every partition assertion in this fixture
//     reads a note the mock's `note_manage` call wrote. Under
//     `[memory] enabled = false` those assertions could only ever be vacuous;
//     under a live dreaming daemon the corpus is rewritten on a timer.
import fs from "node:fs";

const [path, gatewayPort, mockPort, secret] = process.argv.slice(2);
if (!path || !gatewayPort || !mockPort || !secret) {
  console.error(
    "usage: patch_config.mjs <config.toml> <gateway-port> <mock-port> <webhook-secret>",
  );
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

/** Drop whole sections whose header name satisfies `pred`. */
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
  // or `resolve_connect_auth` authorises it as operator on its first line,
  // before it ever reads the ticket.
  ["gateway", "host", '"0.0.0.0"'],
  ["gateway", "allow_insecure_remote", "true"],
  ["gateway", "port", gatewayPort],
  ["cron", "enabled", "false"],
  ["heartbeat", "enabled", "false"],
  ["mcp", "enabled", "false"],
  ["acp", "enabled", "false"],
  ["evolution", "enabled", "false"],
  ["skills", "enabled", "false"],
  ["memory", "enabled", "true"],
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

# Scenario 7 switches the channel's active agent to this one. A second agent is
# the only way to observe that an \`agent_switch\` mints a DIFFERENT session key
# for the same conversation — which is exactly the case the binding table is
# keyed on the conversation (not the key) to survive.
[[agents.list]]
id = "coder"
name = "QA Coder"
default = false
model = "qa-mock-model"
provider = "qa-mock"
system_prompt = "QA fixture second agent."

# The instance id MUST be \`webhook\` — see this file's header, point 2.
[channels.webhook]
type = "webhook"
enabled = true
secret = "${secret}"
callback_url = "http://127.0.0.1:${mockPort}/outbound"
path = "/webhook/qa"
permission_level = "config"
`;

fs.writeFileSync(path, src, "utf8");

// Fail loudly rather than hand the server a file it will refuse: a duplicate
// key aborts config loading entirely, and the only symptom downstream is
// "server died" forty lines into a boot log. The key set resets at EVERY
// header, not per header NAME — `[[agents.list]]` appears twice on purpose.
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
    "memory on, agents main+coder, channel webhook on /webhook/qa",
);
