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
//  5. **`embed-stall` (6th argument) turns memory back ON with the mock as the
//     embedding provider.** The `unanswered` stage needs a crash between the
//     seed (`UserMessage`) and `RunStarted`, and that window is ~30 ms wide
//     on this host (measured 2026-09-13: seq 2 at …762, seq 3 at …790). The
//     only remote call inside it is the memory recall's query embedding
//     (`prompt_build.rs` → `build_memory_user_message` → `embedder.embed`), so
//     a provider that stalls `/v1/embeddings` is what stretches the window to
//     something a `kill -9` can be aimed into. No production failpoint.
//     `preset = "ollama"` is load-bearing: `validate_api_base`
//     (`src/memory/embedding_provider.rs`) refuses `http://` and loopback
//     for every other preset, and the brief's `custom` would have made the
//     provider fail to initialise — memory silently FTS-only, no embed call,
//     no window, and a stage that "passes" by measuring nothing.
//  6. **`QA_MAX_ATTEMPTS`** (env) sets `[resume] max_attempts` for the
//     `ratchet` stage; **`QA_MAX_CONCURRENT`** (env) sets `[resume]
//     max_concurrent` for the `parallel` stage. Env rather than a 7th
//     positional: argv[7] is `embed-stall`, and a stage that wants both would
//     otherwise have to spell the one it does not want.
//  7. **`QA_WINDOWS_SANDBOX_OFF=1`** (env) turns the three `[sandbox.windows]`
//     primitives off (`use_restricted_token` / `use_app_container` /
//     `use_job_object`) for the `tombstone` stage, and only that stage. Point
//     3's measurement (every `bash` call dies in ~240 ms under the restricted
//     token) predates the PowerShell-shell round, and that stage needs the
//     opposite of what every other stage needs: a command that RUNS and
//     OUTLIVES the server. With the primitives on, (a) `child.id()` — the pid
//     the journal records — is the `sandbox-init-windows` launcher, not the
//     shell, and (b) the job object is `KILL_ON_JOB_CLOSE`, so the server's
//     death takes the child with it and the "still running" arm is
//     unreachable by construction. The stage re-measures point 3 as a
//     pre-flight (`drive bg`: a job that settles within 2 s is exit 78,
//     never green). The sandbox itself stays `enabled = true`.
//
// usage: patch_r2.mjs <config.toml> <gateway-port> <mock-port> <resume:true|false> [bash-policy] [embed-stall]
import fs from "node:fs";

const [path, gatewayPort, mockPort, resumeEnabled = "true", bashPolicy = "allow", mode = ""] =
  process.argv.slice(2);
if (!path || !gatewayPort || !mockPort) {
  console.error("usage: patch_r2.mjs <config.toml> <gateway-port> <mock-port> [resume] [bash-policy] [embed-stall]");
  process.exit(2);
}
if (mode !== "" && mode !== "embed-stall") {
  console.error(`patch_r2: unknown 6th argument ${JSON.stringify(mode)} (only "embed-stall" is known)`);
  process.exit(2);
}
const embedStall = mode === "embed-stall";

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
// Every `[[memory.embedding.providers]]` table goes, the generated defaults
// included: this file is re-run on the same config several times per stage,
// so anything appended below has to be dropped here first or the second pass
// leaves two tables with the same id. The defaults are not needed either — an
// `auto` resolver that could fall back to one of them would give the stage a
// provider that answers instead of stalling.
if (embedStall) src = dropSections(src, (s) => s === "memory.embedding.providers");

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

/** Remove `key = …` from `[section]` (a no-op when either is absent). */
const dropKey = (text, section, key) => {
  const out = [];
  let cur = null;
  for (const line of text.split(/\r?\n/)) {
    const h = headerName(line);
    if (h !== null) cur = h;
    else if (cur === section && keyName(line) === key) continue;
    out.push(line);
  }
  return out.join("\n") + "\n";
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

if (process.env.QA_MAX_ATTEMPTS) {
  src = setKey(src, "resume", "max_attempts", process.env.QA_MAX_ATTEMPTS);
}
if (process.env.QA_MAX_CONCURRENT) {
  src = setKey(src, "resume", "max_concurrent", process.env.QA_MAX_CONCURRENT);
}
const windowsSandboxOff = process.env.QA_WINDOWS_SANDBOX_OFF === "1";
if (windowsSandboxOff) {
  for (const k of ["use_restricted_token", "use_app_container", "use_job_object"]) {
    src = setKey(src, "sandbox.windows", k, "false");
  }
}
if (embedStall) {
  src = setKey(src, "memory", "enabled", "true");
  src = setKey(src, "memory.embedding", "active_provider_id", '"qa-embed"');
  // The generated config spells the (empty) provider list INLINE —
  // `providers = []` under `[memory.embedding]` (measured 2026-09-18) — and
  // TOML refuses an array-of-tables header for a key that inline array
  // already defines (`duplicate key providers in table memory.embedding`,
  // and the server boots on defaults: memory OFF, no window, no stage).
  src = dropKey(src, "memory.embedding", "providers");
  src += `
[[memory.embedding.providers]]
id = "qa-embed"
name = "QA embed (stalls)"
preset = "ollama"
api_base = "http://127.0.0.1:${mockPort}/v1"
api_key = "qa-dummy"
models = ["qa-embed"]
dimensions = 8
timeout_ms = 60000
`;
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
  `patched ${path}: gateway ${gatewayPort}, mock ${mockPort}, resume=${resumeEnabled}, bash=${bashPolicy}` +
    (embedStall ? ", memory=on (qa-embed via the mock)" : "") +
    (process.env.QA_MAX_ATTEMPTS ? `, max_attempts=${process.env.QA_MAX_ATTEMPTS}` : "") +
    (process.env.QA_MAX_CONCURRENT ? `, max_concurrent=${process.env.QA_MAX_CONCURRENT}` : "") +
    (windowsSandboxOff ? ", sandbox.windows primitives=off" : ""),
);
