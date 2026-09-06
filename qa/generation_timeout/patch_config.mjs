// Point a freshly generated Aleph config at the stalling mock, with exactly
// one generation provider.
//
//   patch_config.mjs <config.toml> <gateway-port> <mock-port> [timeout-seconds]
//
// The fourth argument is the whole experiment. Pass a number and the provider
// entry carries `timeout_seconds = N`; omit it and the entry does not mention
// the key at all — which is the state this round made expressible and is NOT
// the same as writing 120.
//
// The provider is `provider_type = "openai"` on purpose. Before this round
// `create_provider` applied `timeout_seconds` in four places, and
// `openai_compat` was one of them — a fixture built on that arm would be green
// against the pre-round tree and prove nothing. The `"openai" | "openai_image"
// | "dalle"` arm was one of the fifteen that discarded the knob, so it is the
// arm whose green means something (判据 §2: in what case does this go red?).
//
// Key matching splits on the first `=` rather than using a regex, following
// `qa/teamchat_rooms/patch_config.mjs` — a hand-built key regex there was
// wrong once and produced a config the server refused to boot on.
import fs from "node:fs";

const [path, gatewayPort, mockPort, timeoutSeconds] = process.argv.slice(2);
if (!path || !gatewayPort || !mockPort) {
  console.error(
    "usage: patch_config.mjs <config.toml> <gateway-port> <mock-port> [timeout-seconds]",
  );
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
    if (cur === section && keyName(line) === key) continue;
    out.push(line);
  }
  let next = out.join("\n") + "\n";
  if (!inserted) next += "\n[" + section + "]\n" + key + " = " + value + "\n";
  return next;
};

// Channels can deliver to the operator's real accounts. `providers`, `agents`
// and `generation` are all rebuilt below, following `qa/terminal`: dropping and
// re-adding is the only way to know exactly what is configured, rather than
// inheriting whatever the generator happened to write.
//
// A chat provider WITH an api_key is not optional here, even though this
// fixture never sends a chat turn. `register_agent_handlers` selects the real
// `ExecutionEngine` only "when an API key is available (env or config)" and
// otherwise installs the simulated `AgentRunManager` — and `tools.invoke` is
// wired in `tool_catalog_init.rs` only `if let Some(reg) = tool_reg_out`, which
// is `None` in simulated mode. Without a chat provider every phase gets
// `-32099 tools.invoke requires ToolRegistry (boot phase 2)` and measures the
// boot mode instead of a timeout. The first draft of this file kept `agents`
// and reasoned about `is_tool_allowed`; that gate is real but it is the SECOND
// one, and the first was never reached.
src = dropSections(src, (s) => /^(channels|providers|agents|generation)/.test(s));

for (const [section, key, value] of [
  ["gateway", "host", '"127.0.0.1"'],
  ["gateway", "port", gatewayPort],
  // Keep the box quiet: nothing in this fixture needs either, and the
  // generator's cron db_path is a literal `~/.aleph/...` that points back at
  // the REAL home.
  ["cron", "enabled", "false"],
  ["memory.dreaming", "enabled", "false"],
]) {
  src = setKey(src, section, key, value);
}

const timeoutLine =
  timeoutSeconds === undefined || timeoutSeconds === "" || timeoutSeconds === "none"
    ? "# timeout_seconds deliberately absent -- this is the `unset` state\n"
    : `timeout_seconds = ${Number(timeoutSeconds)}\n`;

// The chat provider points at a port nothing listens on, NOT at the mock. The
// mock stalls every path except `/health`, so a boot-time probe against it
// would hang for the full stall window inside the instrument whose whole
// subject is elapsed time (判据 §18 — do not build noise next to your own
// meter). Connection-refused is instant and, since `tools.invoke` bypasses the
// LLM loop entirely, nothing in any phase ever calls this provider. It exists
// to make the api_key present, which is all the mode selection reads.
const deadPort = Number(mockPort) + 1;
src +=
  "\n[providers.qa-chat]\n" +
  "enabled = true\n" +
  'protocol = "anthropic"\n' +
  `base_url = "http://127.0.0.1:${deadPort}"\n` +
  'api_key = "qa-dummy-not-a-real-key"\n' +
  'models = ["qa-mock-model"]\n' +
  "\n[[agents.list]]\n" +
  'id = "main"\n' +
  'name = "QA Main"\n' +
  "default = true\n" +
  'model = "qa-mock-model"\n' +
  'provider = "qa-chat"\n' +
  'system_prompt = "QA fixture."\n';

src +=
  "\n[generation]\n" +
  'default_image_provider = "qa-stall"\n' +
  "\n[generation.image_providers.qa-stall]\n" +
  'provider_type = "openai"\n' +
  // Any non-empty key: `hydrate_key_and_gate` drops a provider whose key is
  // absent or empty, and the mock never checks it.
  'api_key = "sk-qa-stall"\n' +
  // Domain-only, so `resolve_base_url` classifies it Standard and appends
  // `/v1/images/generations` — the path the mock serves.
  `base_url = "http://127.0.0.1:${mockPort}"\n` +
  'models = ["dall-e-3"]\n' +
  "enabled = true\n" +
  'color = "#10a37f"\n' +
  'capabilities = ["image"]\n' +
  timeoutLine;

fs.writeFileSync(path, src);
console.log(
  `patched ${path}: gateway ${gatewayPort}, mock ${mockPort}, timeout_seconds ${
    timeoutSeconds === undefined || timeoutSeconds === "none" ? "UNSET" : timeoutSeconds
  }`,
);
