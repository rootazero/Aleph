// The `qa/terminal` stages, driven against a real booted gateway.
//
// Every assertion here is on an EFFECT — a field value in `runtime.agents.list`
// or in a `terminal{...}` answer — never on "the call happened". The reason is
// specific to this round: phase 1 shipped an agent panel that identified
// sessions from the `$SHELL` recorded at `pty.spawn` time, so every row in
// production read `Unknown` while twenty-one detection manifests and their unit
// tests stayed green. A test that calls the sampler with the agent's name in
// its hand cannot see that; only a shell that is spawned as a plain shell and
// has `claude` typed into it afterwards can.
//
//   identify  the probe names the foreground program and the manifest names the
//             agent, for a session whose SPAWN LABEL is the shell — with a
//             control session that ran no agent, so a green cannot come from
//             "everything is claude"
//   wait      `terminal{wait}` blocks on the table's watch and returns
//             `reached` when the state arrives — with the negative arm, a state
//             the session never enters, which must answer `timeout` and the
//             CURRENT entry rather than dressing the last one up as a final
//             state
//   quiet     30 s of silence publishes `quiet_since` and does NOT move `state`
//             (spec R2-3), and a frame clears it again
//   cwd       the merged cwd order — OSC 7 › foreground probe › spawn dir —
//             over three directories that are actually different, so the winner
//             is identifiable
//   real      a REAL agent binary off PATH (Unix only — see SHELL below)
//   panel/tui set the board for a browser / for `aleph-tui`
//
// ## Why this is Node and not Python
//
// It was Python until 2026-09-05, and on Windows that made every stage UNRUN:
// this host has no Python interpreter installed (see `run.sh`'s "Platform"
// section for the measured detail — the first draft of that paragraph got the
// reason wrong). Node is the interpreter both platforms here have. The port is
// deliberately not a
// translation of the shell strings as well — those are now a per-platform kit
// (`SHELL` below), because "type an agent into a shell" is the one thing this
// fixture cannot fake and it is spelled differently on the two platforms.
//
// Usage:
//   node drive_terminal.mjs <ws-url> <stage> <bin-dir> <work-dir> <chrome.json>
import fs from "node:fs";
import path from "node:path";

const [URL, STAGE, BIN_DIR, WORK, CHROME_PATH] = process.argv.slice(2);
if (!URL || !STAGE || !BIN_DIR || !WORK || !CHROME_PATH) {
  console.error("usage: drive_terminal.mjs <ws-url> <stage> <bin-dir> <work-dir> <chrome.json>");
  process.exit(64);
}
const CHROME = JSON.parse(fs.readFileSync(CHROME_PATH, "utf8"));

let rc = 0;
const SPAWNED = [];
let KEEP_SESSIONS = false;

const check = (ok, label, detail = "") => {
  console.log(`  [${ok ? "PASS" : "FAIL"}] ${label}${detail ? ` — ${detail}` : ""}`);
  if (!ok) rc = 1;
};
const note = (msg) => console.log(`  ... ${msg}`);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const j = (v, n = 300) => JSON.stringify(v ?? null).slice(0, n);

/**
 * Compare two directory paths for "the same directory".
 *
 * The `cwd` stage's subject is WHICH TIER won, not how a platform spells a
 * path, and the two are spelled differently on Windows in two ways that are
 * both the product's documented behaviour rather than a defect (see
 * TERMINAL_RUNTIME §3.2.2): the foreground probe answers with backslashes AND
 * a trailing separator (`C:\Users\me\work\probe2\`), while OSC 7 carries
 * whatever the emitter wrote. Asserting on the spelling would make this stage
 * fail for a reason it is not about — and, worse, it would make the tier
 * assertions unreachable, so the stage could never say the tier order is
 * wrong (判据 §2).
 *
 * Deliberately NOT case-insensitive: Windows paths are case-preserving, both
 * sides here come from the same `mktemp` root, and folding case is one more
 * way for two different directories to compare equal.
 */
const samePath = (a, b) => {
  const norm = (p) => String(p ?? "").replace(/\\/g, "/").replace(/\/+$/, "");
  return norm(a) === norm(b);
};

// ---------------------------------------------------------------------------
// The one platform-shaped thing in this file
// ---------------------------------------------------------------------------

/**
 * How to drive an interactive shell, per platform.
 *
 * Kept as ONE object rather than `cfg`-style branches at each call site,
 * because the stages' subject is the product and not the shell — a per-site
 * branch is where a Windows arm silently stops typing the agent and the stage
 * still reports the control session's row (判据 §2).
 *
 * `agentIsChild` is the fact the `identify` stage's tree depends on: on Unix
 * the installed fake IS the process the shell execs, so the shell's own child
 * is the agent. On Windows `claude` resolves to the `claude.cmd` shim, which
 * cmd.exe runs IN-PROCESS and which starts `node` as a child — so the agent is
 * one level below where Unix puts it. Both are handled by the SAME product
 * code (`foreground::foreground_fact_for_shell` walks descendants), which is
 * why this fixture can assert the same rows on both.
 */
const WINDOWS = process.platform === "win32";
const SHELL = WINDOWS
  ? {
      command: "cmd.exe",
      nl: "\r\n",
      prependPath: (dir) => `set "PATH=${dir};%PATH%"`,
      // cmd has no `VAR=value cmd` prefix form; the assignment is its own
      // statement and persists for the rest of the session.
      withEnv: (env, cmd) => [...Object.entries(env).map(([k, v]) => `set ${k}=${v}`), cmd],
      // The fake agent spawned WITHOUT a shell: `node <file>`, so the pty child
      // is the runtime and the token that identifies is the script path.
      // Absolute path, from run.sh: this host node is an fnm per-shell shim
      // and the bare word does not resolve inside a PTY child (see run.sh).
      directFake: (fake) => ({ command: process.env.QA_NODE || "node", args: [fake] }),
      echo: (text) => `echo ${text}`,
    }
  : {
      command: "sh",
      nl: "\n",
      prependPath: (dir) => `export PATH="${dir}:$PATH"`,
      withEnv: (env, cmd) => [
        `${Object.entries(env)
          .map(([k, v]) => `${k}=${v}`)
          .join(" ")} ${cmd}`,
      ],
      directFake: (fake) => ({ command: fake }),
      echo: (text) => `echo ${text}`,
    };

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

class Conn {
  constructor(name) {
    this.name = name;
    this.n = 0;
    this.pending = new Map();
  }

  async open() {
    this.ws = new WebSocket(URL);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${this.name}: connect timeout`)), 30_000);
      this.ws.addEventListener("open", () => {
        clearTimeout(timer);
        resolve();
      });
      this.ws.addEventListener("error", (e) => {
        clearTimeout(timer);
        reject(new Error(`${this.name}: ${e.message ?? "socket error"}`));
      });
    });
    this.ws.addEventListener("message", (ev) => {
      let msg;
      try {
        msg = JSON.parse(ev.data);
      } catch {
        return;
      }
      // Only replies are consumed here. Bus events reach this fixture as
      // nothing — every assertion is a poll of `runtime.agents.list`, which is
      // the face a Panel actually reads.
      const waiter = msg.id != null && this.pending.get(msg.id);
      if (waiter) {
        this.pending.delete(msg.id);
        waiter(msg);
      }
    });
    await this.call("connect", { client: `qa-terminal-${this.name}`, version: "1" });
    return this;
  }

  call(method, params, timeoutMs = 60_000) {
    this.n += 1;
    const id = this.n;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${this.name}: ${method} timed out after ${timeoutMs} ms`));
      }, timeoutMs);
      this.pending.set(id, (msg) => {
        clearTimeout(timer);
        resolve(msg);
      });
      this.ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    });
  }

  /** `[data, error]` — `data` is null when the tool refused. */
  async tool(args, timeoutMs = 60_000) {
    const r = await this.call("tools.invoke", { tool_name: "terminal", arguments: args }, timeoutMs);
    if (r.error) return [null, j(r.error, 400)];
    const out = r.result.result;
    if (!out.success) return [null, out.message ?? "<no message>"];
    return [out.data ?? null, ""];
  }

  async spawn(params) {
    const r = await this.call("pty.spawn", { rows: 24, cols: 100, ...params });
    if (r.error) throw new Error(`pty.spawn failed: ${j(r.error, 400)}`);
    SPAWNED.push(r.result.session_id);
    return r.result;
  }

  /** Type a line, with the platform's newline. */
  async type(sessionId, line) {
    const r = await this.call("pty.input", { session_id: sessionId, data: line + SHELL.nl });
    if (r.error) throw new Error(`pty.input failed: ${j(r.error, 400)}`);
  }

  async agents() {
    const r = await this.call("runtime.agents.list", {});
    if (r.error) throw new Error(`runtime.agents.list failed: ${j(r.error, 400)}`);
    return Object.fromEntries(r.result.agents.map((e) => [e.session_id, e]));
  }

  async sessions() {
    const r = await this.call("pty.list", {});
    if (r.error) throw new Error(`pty.list failed: ${j(r.error, 400)}`);
    return Object.fromEntries(r.result.sessions.map((s) => [s.session_id, s]));
  }

  async entry(sessionId) {
    return (await this.agents())[sessionId] ?? null;
  }

  /**
   * Poll one row until `pred` holds. Returns `[entry, elapsedSeconds]`, and
   * the LAST entry seen either way — so a failing assertion can print what the
   * row actually said instead of `null`.
   */
  async until(sessionId, pred, seconds, what) {
    const started = Date.now();
    let last = null;
    while ((Date.now() - started) / 1000 < seconds) {
      last = await this.entry(sessionId);
      if (last !== null && pred(last)) return [last, (Date.now() - started) / 1000];
      await sleep(300);
    }
    note(`timed out after ${seconds}s waiting for ${what}; last row: ${j(last)}`);
    return [last, (Date.now() - started) / 1000];
  }
}

const connect = (name) => new Conn(name).open();

/** Nudge a fresh shell so its first frame is not something we merely hope for. */
async function firstFrame(c, session, marker) {
  await c.type(session, SHELL.echo(marker));
  await c.until(session, () => true, 15, `${marker}'s first frame`);
}

/** Put the fake agent's directory on PATH and start it. */
async function startFake(c, session, env = {}) {
  await c.type(session, SHELL.prependPath(BIN_DIR));
  for (const line of SHELL.withEnv(env, "claude")) await c.type(session, line);
}

// ---------------------------------------------------------------------------
// identify
// ---------------------------------------------------------------------------

async function stageIdentify(c) {
  const agentS = (await c.spawn({ command: SHELL.command, cwd: `${WORK}/spawn` })).session_id;
  const plainS = (await c.spawn({ command: SHELL.command, cwd: `${WORK}/spawn` })).session_id;
  note(`agent session ${agentS}, control session ${plainS}`);

  await firstFrame(c, plainS, "qa-control-shell");
  await firstFrame(c, agentS, "qa-agent-shell");
  await startFake(c, agentS);

  const want = CHROME.screens.working;
  let [entry, took] = await c.until(
    agentS,
    (e) => e.agent === "claude" && e.program === "claude" && e.state === "working",
    40,
    "the typed agent to be identified and reach working",
  );
  entry = entry ?? {};
  note(`identified after ${took.toFixed(1)}s: ${j(entry)}`);

  const rows = await c.sessions();
  const label = rows[agentS]?.shell;
  // If this were `claude`, every row below would be satisfied by the
  // spawn-label path phase 1 already had, and the stage would prove nothing.
  check(
    label === SHELL.command,
    `the agent session's SPAWN LABEL is still \`${SHELL.command}\``,
    `pty.list shell=${j(label)}`,
  );
  check(entry.program === "claude", "the foreground probe put the PROGRAM on the wire", `program=${j(entry.program)}`);
  check(entry.agent === "claude", "the manifest identified the AGENT from that program", `agent=${j(entry.agent)}`);
  check(
    entry.state === want.state,
    `the screen rules were reachable — state is ${want.state}`,
    `state=${j(entry.state)}`,
  );

  const [data, err] = await c.tool({ action: "explain", session_id: agentS });
  if (data === null) {
    check(false, "terminal{explain} answered", err);
  } else {
    const rule = data.matched_rule?.id;
    check(
      rule === want.rule,
      "explain names the manifest rule the fixture's screen was built from",
      `matched_rule=${j(rule)}, expected ${j(want.rule)}; screen_tail=${j(data.inputs?.screen_tail ?? "", 200)}`,
    );
    check(
      data.source === "bundled" && data.manifest_version === CHROME.manifest_version,
      "explain reports the bundled manifest at the version the screens came from",
      `source=${j(data.source)} version=${j(data.manifest_version)} expected ${j(CHROME.manifest_version)}`,
    );
  }

  // The falsifying half. Without it, a sampler that answered `claude` for every
  // session would satisfy every assertion above.
  const control = (await c.entry(plainS)) ?? {};
  note(`control row: ${j(control)}`);
  // `program: null` is "we could not look", not "no agent is running": the two
  // arms below would both be satisfied by a probe that never answered.
  check(control.program != null, "the probe ANSWERED for the control session too", `program=${j(control.program)}`);
  check(control.program !== "claude", "the control session's program is not the agent's", `program=${j(control.program)}`);
  check(control.agent == null, "no manifest matched the control session, and none was guessed", `agent=${j(control.agent)}`);
  check(control.state === "unknown", "an unidentified program is `unknown`, never `idle`", `state=${j(control.state)}`);
}

// ---------------------------------------------------------------------------
// wait
// ---------------------------------------------------------------------------

async function stageWait(c) {
  const session = (await c.spawn({ command: SHELL.command, cwd: `${WORK}/spawn` })).session_id;
  note(`session ${session}`);
  await firstFrame(c, session, "qa-wait-shell");
  // 8 s per screen: the wait has to be ISSUED while the session is working,
  // and a 2 s working phase can be over before the poll notices it.
  await startFake(c, session, { PHASE_SECS: "8" });

  const [entry] = await c.until(
    session,
    (e) => e.agent === "claude" && e.state === "working",
    40,
    "the agent to reach working",
  );
  if (entry?.state !== "working") {
    check(false, "the session reached working before the wait was issued", j(entry));
    return;
  }
  note(`working: ${j(entry)}`);

  const waiter = await connect("waiter");
  let started = Date.now();
  let [data, err] = await waiter.tool(
    { action: "wait", session_id: session, until: ["blocked"], timeout_ms: 30000 },
    60_000,
  );
  let tookMs = Date.now() - started;
  if (data === null) {
    check(false, "terminal{wait} answered", err);
  } else {
    note(`wait returned after ${tookMs} ms: ${j(data)}`);
    check(data.outcome === "reached", "a wait whose state arrives answers `reached`", `outcome=${j(data.outcome)}`);
    check(
      data.agent?.state === "blocked",
      "the answer carries the entry that says so",
      `agent.state=${j(data.agent?.state)}`,
    );
    check(
      tookMs >= 500 && tookMs < 30000,
      "it BLOCKED and then woke — it neither returned instantly nor burned the whole window",
      `${tookMs} ms`,
    );
  }

  // The negative arm. The session is holding `blocked`; `idle` never comes.
  started = Date.now();
  [data, err] = await waiter.tool(
    { action: "wait", session_id: session, until: ["idle"], timeout_ms: 4000 },
    60_000,
  );
  tookMs = Date.now() - started;
  if (data === null) {
    check(false, "terminal{wait} answered on the negative arm", err);
  } else {
    note(`negative wait returned after ${tookMs} ms: ${j(data)}`);
    check(
      data.outcome === "timeout",
      "a state the session never enters answers `timeout`, not `reached`",
      `outcome=${j(data.outcome)}`,
    );
    check(
      data.agent?.state === "blocked",
      "the timeout carries the CURRENT entry, not a manufactured final state",
      `agent.state=${j(data.agent?.state)}`,
    );
    check(tookMs >= 3800, "the window was actually spent", `${tookMs} ms for a 4000 ms window`);
  }
  waiter.ws.close();
}

// ---------------------------------------------------------------------------
// quiet
// ---------------------------------------------------------------------------

async function stageQuiet(c) {
  const session = (await c.spawn({ command: SHELL.command, cwd: `${WORK}/spawn` })).session_id;
  note(`session ${session}`);
  await firstFrame(c, session, "qa-quiet-shell");
  await startFake(c, session, { QUIET: "1" });

  let [working] = await c.until(
    session,
    (e) => e.agent === "claude" && e.state === "working",
    40,
    "the agent to reach working",
  );
  working = working ?? {};
  note(`working: ${j(working)}`);
  if (working.state !== "working") {
    check(false, "the session reached working before the silence began", j(working));
    return;
  }
  // Without this the stage proves nothing: a row that was ALREADY marked quiet
  // would satisfy the assertion below without any clock running.
  check(working.quiet_since == null, "a session that just painted is not quiet", `quiet_since=${j(working.quiet_since)}`);

  let [quiet, took] = await c.until(session, (e) => e.quiet_since != null, 60, "the 30 s quiet clock to publish");
  quiet = quiet ?? {};
  note(`quiet after ${took.toFixed(1)}s: ${j(quiet)}`);
  check(quiet.quiet_since != null, "silence is published as `quiet_since`", `quiet_since=${j(quiet.quiet_since)}`);
  check(
    quiet.state === "working",
    "SILENCE IS NOT IDLE — the state the working screen established stands",
    `state=${j(quiet.state)}`,
  );
  check(
    quiet.agent === "claude" && quiet.program === "claude",
    "the identification survives the silence",
    `agent=${j(quiet.agent)} program=${j(quiet.program)}`,
  );
  check(
    took >= 25 && took <= 45,
    "the mark appeared on the 30 s clock, not immediately",
    `${took.toFixed(1)}s after the working screen`,
  );

  // A frame ends it. Without this the mark could be a sticky flag that nothing
  // ever clears, and the stage above would not know the difference.
  let [cleared] = await c.until(session, (e) => e.quiet_since == null, 30, "the next frame to clear the quiet mark");
  cleared = cleared ?? {};
  note(`after the next paint: ${j(cleared)}`);
  check(cleared.quiet_since == null, "a real frame clears the quiet mark", `quiet_since=${j(cleared.quiet_since)}`);
  check(
    cleared.state === "blocked",
    "and the screen that broke the silence is the one now reported",
    `state=${j(cleared.state)}`,
  );
}

// ---------------------------------------------------------------------------
// cwd
// ---------------------------------------------------------------------------

async function stageCwd(c) {
  const oscDir = `${WORK}/osc`;
  const probeDir = `${WORK}/probe`;
  const probe2Dir = `${WORK}/probe2`;
  const spawnDir = `${WORK}/spawn`;
  const fake = SHELL.directFake(path.join(BIN_DIR, "claude"));

  const oscS = (
    await c.spawn({
      ...fake,
      cwd: spawnDir,
      env: { QA_FAKE_CD: probeDir, QA_FAKE_OSC7: oscDir, PHASE_SECS: "2" },
    })
  ).session_id;
  const probeS = (
    await c.spawn({ ...fake, cwd: spawnDir, env: { QA_FAKE_CD: probe2Dir, PHASE_SECS: "2" } })
  ).session_id;
  note(`osc session ${oscS}, probe-only session ${probeS}`);

  // ⚠️ The wait condition must be STRICTLY WEAKER than what is asserted below,
  // and `program === "claude"` alone is not: the fake `chdir`s before it paints,
  // but the probe can name the program from a fact it read while the runtime was
  // still starting, so the row can carry the SPAWN dir at the moment the program
  // arrives. Measured 2026-09-05 on Windows — the same assertion passed one run
  // and failed the next, reporting `work\spawn\` where `work\probe2\` was
  // expected. This is the shape 附录 D.4.41 records: a break condition weaker
  // than the assertion is the assertion racing itself.
  //
  // "no longer the spawn dir" is the right strength — it is implied by both
  // expected answers and names neither, so a probe that moved to some THIRD
  // directory still fails below rather than hanging here.
  const settled = (e) => e.program === "claude" && !samePath(e.cwd, spawnDir);
  let [a] = await c.until(oscS, settled, 30, "the OSC session's cwd to leave the spawn dir");
  let [b] = await c.until(probeS, settled, 30, "the probe-only session's cwd to leave the spawn dir");
  a = a ?? {};
  b = b ?? {};
  note(`osc row:   ${j(a)}`);
  note(`probe row: ${j(b)}`);

  const rows = await c.sessions();
  note(`pty.list spawn dirs: ${j(Object.entries(rows).map(([k, v]) => [k.slice(0, 8), v.cwd]))}`);

  // The three directories must actually differ, or nothing below discriminates.
  check(
    new Set([oscDir, probeDir, spawnDir]).size === 3,
    "the three cwd tiers are three different directories",
    `${oscDir} / ${probeDir} / ${spawnDir}`,
  );
  check(
    a.program === "claude" && b.program === "claude",
    "the probe answered for BOTH sessions",
    `programs=${j(a.program)} / ${j(b.program)}`,
  );
  check(
    samePath(b.cwd, probe2Dir),
    "with no OSC 7, the live cwd is the FOREGROUND PROCESS's, not the spawn dir",
    `cwd=${j(b.cwd)}, spawned in ${spawnDir}`,
  );
  check(
    samePath(a.cwd, oscDir),
    "OSC 7 outranks both the probe's cwd and the spawn dir",
    `cwd=${j(a.cwd)}, probe was in ${probeDir}, spawned in ${spawnDir}`,
  );
  check(
    samePath(rows[oscS]?.cwd, spawnDir),
    "`pty.list` still reports the SPAWN directory — the two cwds are different facts, not two spellings of one",
    `pty.list cwd=${j(rows[oscS]?.cwd)}`,
  );
}

// ---------------------------------------------------------------------------
// real
// ---------------------------------------------------------------------------

async function stageReal(c) {
  const real = process.env.QA_REAL_AGENT ?? "";
  const name = process.env.QA_REAL_AGENT_NAME ?? "";
  if (!real || !name) {
    note(`SKIP: no real agent binary found on PATH — tried: ${process.env.QA_REAL_AGENT_TRIED || "(not reported)"}`);
    note("this stage asserts NOTHING on this machine; it is not a pass");
    return;
  }
  note(`real agent: ${name} -> ${real}`);

  // ---- 1. the real binary, run directly ---------------------------------
  const direct = (await c.spawn({ command: SHELL.command, cwd: `${WORK}/spawn` })).session_id;
  await firstFrame(c, direct, "qa-real-shell");
  await c.type(direct, `exec ${real}`);

  let [entry, took] = await c.until(direct, (e) => e.agent === name, 60, `the real \`${name}\` binary to be identified`);
  entry = entry ?? {};
  note(`identified after ${took.toFixed(1)}s: ${j(entry)}`);
  check(
    entry.agent === name,
    `a REAL ${name} binary is identified as ${name}`,
    `agent=${j(entry.agent)} program=${j(entry.program)}`,
  );
  let program = entry.program ?? "";
  // The program label is a string a human reads in the panel. A process title
  // can be `npm exec claude` and a macOS command line can be
  // `pi TERM_PROGRAM=Apple_Terminal`; neither is a program name.
  check(
    program !== "" && !program.includes(" "),
    "the PROGRAM label is one word — not a process title, not a title with an environment variable glued to it",
    `program=${j(program)}`,
  );
  check(!program.includes("="), "no environment assignment reached the program label", `program=${j(program)}`);

  // ---- 2. the same binary behind a real npx ------------------------------
  const npx = process.env.QA_REAL_NPX ?? "";
  if (!npx) {
    note("SKIP the wrapper half: no npx on PATH (or no local .bin could be staged)");
    return;
  }
  note(`wrapper: npx -> ${npx}`);
  const wrapped = (await c.spawn({ command: SHELL.command, cwd: npx })).session_id;
  await firstFrame(c, wrapped, "qa-wrapper-shell");
  await c.type(wrapped, `exec npx ${name}`);

  [entry, took] = await c.until(wrapped, (e) => e.agent === name, 90, `\`npx ${name}\` to be identified as ${name}`);
  entry = entry ?? {};
  note(`wrapper identified after ${took.toFixed(1)}s: ${j(entry)}`);
  check(
    entry.agent === name,
    `\`npx ${name}\` identifies as ${name} — the leader is npm, the agent is its child, and the leader's own command line is what names it`,
    `agent=${j(entry.agent)} program=${j(entry.program)}`,
  );
  program = entry.program ?? "";
  check(program === name, "the wrapper's program label is the AGENT, not `npm exec <agent>`", `program=${j(program)}`);
}

// ---------------------------------------------------------------------------
// panel / tui
// ---------------------------------------------------------------------------

async function stagePanel(c) {
  KEEP_SESSIONS = true;

  const real = process.env.QA_REAL_AGENT ?? "";
  const name = process.env.QA_REAL_AGENT_NAME ?? "";

  const agentS = (await c.spawn({ command: SHELL.command, cwd: `${WORK}/spawn` })).session_id;
  const plainS = (await c.spawn({ command: SHELL.command, cwd: `${WORK}/spawn` })).session_id;
  await firstFrame(c, agentS, "qa-panel-agent");
  await firstFrame(c, plainS, "qa-panel-plain");

  let expect;
  if (real && name) {
    note(`running the REAL ${name} at ${real}`);
    await c.type(agentS, `exec ${real}`);
    expect = name;
  } else {
    note("no real agent found; falling back to the fake `claude`");
    await startFake(c, agentS);
    expect = "claude";
  }

  let [entry, took] = await c.until(agentS, (e) => e.agent === expect, 60, `the agent session to be identified as ${expect}`);
  entry = entry ?? {};
  note(`identified after ${took.toFixed(1)}s: ${j(entry)}`);
  check(
    entry.agent === expect,
    "the board is set: the wire already says which agent this is, so a tab that disagrees is the PANEL's defect and not the server's",
    `agent=${j(entry.agent)} program=${j(entry.program)}`,
  );

  const control = (await c.entry(plainS)) ?? {};
  check(
    control.agent == null && control.program != null,
    "the control session answered and named no agent",
    `program=${j(control.program)} agent=${j(control.agent)}`,
  );

  // run.sh reads these back to print session-specific checklist lines.
  const board = {
    agent_session: agentS,
    plain_session: plainS,
    expected_agent: expect,
    expected_program: entry.program ?? null,
    control_program: control.program ?? null,
  };
  if (process.env.QA_PANEL_BOARD) {
    fs.writeFileSync(process.env.QA_PANEL_BOARD, `${JSON.stringify(board, null, 2)}\n`);
  }
  note(`board: ${JSON.stringify(board)}`);
}

const STAGES = {
  identify: stageIdentify,
  wait: stageWait,
  quiet: stageQuiet,
  cwd: stageCwd,
  real: stageReal,
  panel: stagePanel,
  // Same board, different consumer: `panel` hands it to a browser, `tui` hands
  // it to `drive_tui.py`. One setup so the two faces cannot be shown different
  // worlds (判据 §9).
  tui: stagePanel,
};

const main = async () => {
  if (!(STAGE in STAGES)) {
    console.error(`unknown stage ${STAGE}`);
    return 64;
  }
  const c = await connect("main");
  try {
    await STAGES[STAGE](c);
  } finally {
    // The `panel` stage hands the board to a browser; closing its sessions here
    // would leave the checklist with nothing to look at.
    for (const s of KEEP_SESSIONS ? [] : SPAWNED) {
      try {
        await c.call("pty.close", { session_id: s }, 10_000);
      } catch (e) {
        console.log(`  ... could not close ${s}: ${e.message}`);
      }
    }
    c.ws.close();
  }
  return rc;
};

main().then(
  (code) => process.exit(code),
  (e) => {
    console.error(`drive_terminal: ${e.stack ?? e}`);
    process.exit(1);
  },
);
