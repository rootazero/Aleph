// Real-machine driver for crash-recovery round 2 onwards — the Node stages
// `run.sh` adds on top of the round-1 `crash` / `attribute` pair (its `case`
// list is the roster; this header deliberately does not count them).
//
//   drive_r2.mjs <gateway-port> <qa-root> <cmd> [args…]
//
// Node, not Python, for the same reason every fixture written on this host
// since 2026-08 is Node: there is no usable `python3` here — `python3` and
// `python` are both the Windows `WindowsApps` stub, which prints nothing and
// exits 49 (measured 2026-09-03; this comment previously claimed it "exits 0
// having done nothing", a mechanism nobody had checked) — and the gateway's
// only client transport is a WebSocket. The round-1 stages stay Python; they
// were measured on a host that had one, and `run.sh` now refuses them here by
// name rather than letting them fail on an unexplained exit code.
//
// ## Every assertion is an effect
//
// Not "the RPC returned 200". The oracles here are:
//
//   * the frame/reply that arrived on a real WebSocket (`chat.history` →
//     `session.last_run` — the exact field the Panel sidebar and the TUI
//     picker render),
//   * the mock provider's REQUEST LOG — what was actually put in front of the
//     model on the turn after the restart,
//   * the durable event log (`<ALEPH_HOME>/data/sessions.db`), read directly
//     with `node:sqlite`, which is the only place a dangling dispatch is a
//     fact rather than a server's opinion of one,
//   * the receipt `aleph-server resume --json` printed, parsed as the
//     `ResumeReceipt` shape `shared/protocol/src/resume.rs` defines.
//
// ## Why the driver is a set of small commands rather than one script
//
// `kill -9` has to happen between two of them, and only bash can kill the
// process it started. So the shell owns the process lifecycle and this file
// owns every assertion; each command exits non-zero on its first failed
// claim and prints the evidence it had.
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { normalizeFrame } from "../lib/ws.mjs";

const [portArg, QA_ROOT, CMD = "help", ...REST] = process.argv.slice(2);
const PORT = Number(portArg);
if (!PORT || !QA_ROOT) {
  console.error("usage: drive_r2.mjs <gateway-port> <qa-root> <cmd> [args…]");
  process.exit(2);
}

const ALEPH_HOME = path.join(QA_ROOT, "home", ".aleph");
const EVENTS_DB = path.join(ALEPH_HOME, "data", "sessions.db");
// The engine's task ledger (`agent_tasks`), a different file from the event
// log: `persist_run_task_started` writes a row here at admission, BEFORE the
// `BeforeAgentStart` hook and BEFORE the seed — which is why a kill inside
// that hook leaves a task row and no `UserMessage` (the §8.2(b) shape).
const STATE_DB = path.join(ALEPH_HOME, "data", "state.db");
// Where `tracing` writes. NOT `$QA_ROOT/server.log` (that is the process's
// stdout: the banner and the boot stamp); the boot-scan line every phase
// below reads lives in the daily-rotated file under here (measured
// 2026-09-13 on a kept run: `server.log` holds no INFO line at all).
const LOG_DIR = path.join(ALEPH_HOME, "logs");
const MOCK_LOG = path.join(QA_ROOT, "mock.log");
const REQUEST_LOG = path.join(QA_ROOT, "requests.jsonl");
const SESSION_FILE = path.join(QA_ROOT, "session_key.txt");
const LOOPBACK = `ws://127.0.0.1:${PORT}/ws`;
const CHANNEL = "gui:qa-resume-r2";

const T0 = Date.now();
const log = (...a) => console.log(`${((Date.now() - T0) / 1000).toFixed(2)}s`, ...a);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let PASS = 0;
let FAIL = 0;
const check = (cond, label, detail = "") => {
  if (cond) {
    PASS += 1;
    console.log(`PASS  ${label}`);
  } else {
    FAIL += 1;
    console.log(`FAIL  ${label}`);
    if (detail) {
      for (const line of String(detail).split("\n").slice(0, 12)) console.log(`      | ${line}`);
    }
  }
  return cond;
};

// ---------------------------------------------------------------------------
// Connection. The three-envelope tap is not optional: a reader that only looks
// at `msg.topic ?? msg.method` files every bus event under the topic "event",
// which on a failure reads exactly like "the frame never arrived".
// ---------------------------------------------------------------------------

class Conn {
  constructor(name) {
    this.name = name;
    this.frames = [];
    this.pending = new Map();
    this.nextId = 1;
  }

  async open(params = { client_type: "cli" }) {
    this.ws = new WebSocket(LOOPBACK);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${this.name}: connect timeout`)), 30_000);
      this.ws.addEventListener("open", () => {
        clearTimeout(timer);
        resolve();
      });
      this.ws.addEventListener("error", () => {
        clearTimeout(timer);
        reject(new Error(`${this.name}: websocket error`));
      });
    });
    this.ws.addEventListener("message", (ev) => {
      let msg;
      try {
        msg = JSON.parse(typeof ev.data === "string" ? ev.data : String(ev.data));
      } catch {
        return;
      }
      if (msg.id !== undefined && msg.id !== null && this.pending.has(msg.id)) {
        this.pending.get(msg.id)(msg);
        this.pending.delete(msg.id);
        return;
      }
      this.frames.push(normalizeFrame(msg));
    });
    return this.rpc("connect", params);
  }

  rpc(method, params = {}, budget = 90_000) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${this.name}: no reply to ${method} within ${budget}ms`));
      }, budget);
      this.pending.set(id, (msg) => {
        clearTimeout(timer);
        resolve(msg);
      });
      this.ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    });
  }

  async ok(method, params = {}, budget = 90_000) {
    const r = await this.rpc(method, params, budget);
    if (r.error) throw new Error(`${this.name}: ${method} -> ${JSON.stringify(r.error)}`);
    return r.result;
  }

  attempt(method, params = {}, budget = 90_000) {
    return this.rpc(method, params, budget).catch((e) => ({ error: { message: e.message } }));
  }

  async waitFrame(pred, budget = 90_000) {
    const end = Date.now() + budget;
    while (Date.now() < end) {
      const hit = this.frames.find(pred);
      if (hit) return hit;
      await sleep(150);
    }
    return null;
  }

  close() {
    try {
      this.ws?.close();
    } catch {
      /* teardown */
    }
  }
}

const until = async (fn, budget = 120_000, every = 500) => {
  const end = Date.now() + budget;
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() >= end) return null;
    await sleep(every);
  }
};

// ---------------------------------------------------------------------------
// The durable log, read directly. `readonly` + a copy: the server holds the
// same file open in WAL mode, and a reader that takes a write lock can stall
// the very run it is measuring.
// ---------------------------------------------------------------------------

const withEvents = (fn) => {
  if (!fs.existsSync(EVENTS_DB)) return fn(null);
  const db = new DatabaseSync(EVENTS_DB, { readOnly: true });
  try {
    return fn(db);
  } finally {
    db.close();
  }
};

/**
 * Rows of `session_events`, oldest first.
 *
 * NOT filtered by the wire `session_key`, and that is deliberate rather than
 * lazy: the durable log keys its rows by the SERIALISED `SessionId`
 * (`{"type":"main","agent_id":"main","main_key":"main","epoch":1}`), which is a
 * different string from the `agent:main:main:s1` a client sees — filtering on
 * the client's key silently answers "this session has no events" for every
 * session. This fixture's `ALEPH_HOME` is minted per run and holds exactly one
 * conversation, so the whole table IS this session; a fixture that grows a
 * second conversation must resolve the id instead of widening this.
 */
const eventsOf = (_sessionKey) =>
  withEvents((db) => {
    if (!db) return [];
    return db
      .prepare("SELECT seq, event_type, payload_json, retired_at FROM session_events ORDER BY seq ASC")
      .all();
  });

/** Dispatched-but-unanswered call ids, from the log alone. */
const danglingIds = (sessionKey) => {
  const rows = eventsOf(sessionKey);
  const open = new Map();
  for (const r of rows) {
    let p = {};
    try {
      p = JSON.parse(r.payload_json);
    } catch {
      /* a row we cannot read is not a claim we can make */
    }
    const id = p.call_id ?? p?.ToolCallRequested?.call_id ?? null;
    if (!id) continue;
    if (r.event_type.includes("tool_call_requested")) open.set(id, r.seq);
    if (r.event_type.includes("tool_result") || r.event_type.includes("tool_error")) open.delete(id);
  }
  return [...open.keys()];
};

/** Every request body the mock has logged so far. */
const requests = () => {
  if (!fs.existsSync(REQUEST_LOG)) return [];
  return fs
    .readFileSync(REQUEST_LOG, "utf8")
    .split("\n")
    .filter(Boolean)
    .map((l) => {
      try {
        return JSON.parse(l);
      } catch {
        return null;
      }
    })
    .filter(Boolean);
};

const userText = (body) =>
  (body?.messages || [])
    .map((m) =>
      typeof m.content === "string"
        ? m.content
        : (m.content || [])
            .map((b) => b?.text ?? b?.content ?? "")
            .map((t) => (typeof t === "string" ? t : JSON.stringify(t)))
            .join(" "),
    )
    .join("\n");

// The lead words of every boundary-repair arm the server can write for a
// dangling call: "OUTCOME UNKNOWN" (may have run), "NOT EXECUTED" (denied, or
// parked on an approval / hook card — §6.1's fourth arm, which the `knobs`
// stage's dangle takes because it parks at the `ask` gate) and "NOT ANSWERED"
// (parked on a delivered `ask_user` question — the same arm's clarification
// body). A stage that only wants to know "did the resumed run reach the
// provider carrying a repair" asks this; a stage that asserts WHICH arm
// matches the arm's own sentence.
const REPAIR_MARKERS = ["OUTCOME UNKNOWN", "NOT EXECUTED", "NOT ANSWERED"];
const carriesRepair = (text) => REPAIR_MARKERS.some((m) => text.includes(m));

const show = (v, max = 700) => (JSON.stringify(v ?? null) ?? "null").slice(0, max);

const readSession = () => fs.readFileSync(SESSION_FILE, "utf8").trim();

/** `chat.history`, and the `session.last_run` a Panel/TUI renderer reads off it. */
const lastRunOf = async (conn, sessionKey) => {
  const r = await conn.attempt("chat.history", { session_key: sessionKey });
  const session = r.result?.session ?? null;
  return { reply: r, session, lastRun: session?.last_run ?? null };
};

// ---------------------------------------------------------------------------
// Session-scoped readers, for the stages that hold MORE than one conversation
// in this ALEPH_HOME (`unanswered` mints a second session for its lost-input
// sub-step). `eventsOf` above is the whole table on purpose and says so; this
// is the "resolve the id instead" it asks for. The durable log keys rows by
// the serialised `SessionId` (`{"type":"main","agent_id":…,"main_key":…,
// "epoch":N}`, `#[serde(tag = "type")]`), the wire hands out
// `SessionKey::to_key_string()` (`agent:<agent_id>:<main_key>[:s<epoch>]`,
// the suffix only when epoch > 0). Matched field by field through
// `json_extract` rather than by rebuilding the JSON string, so serde's field
// order is not something this fixture has an opinion about. Main keys only —
// every session this fixture mints is one, and a key of another shape is an
// instrument failure, not a session with no events.
// ---------------------------------------------------------------------------

const MAIN_KEY = /^agent:([^:]+):([^:]+)(?::s(\d+))?$/;

const sessionEvents = (wireKey) => {
  const m = MAIN_KEY.exec(wireKey);
  if (!m) {
    console.error(`INSTRUMENT FAILURE: ${wireKey} is not a Main session key; cannot scope the log to it`);
    process.exit(1);
  }
  return withEvents((db) => {
    if (!db) return [];
    return db
      .prepare(
        "SELECT seq, event_type, payload_json, retired_at, created_at FROM session_events \
         WHERE json_extract(session_id, '$.type') = 'main' \
           AND json_extract(session_id, '$.agent_id') = ? \
           AND json_extract(session_id, '$.main_key') = ? \
           AND json_extract(session_id, '$.epoch') = ? \
         ORDER BY seq ASC",
      )
      .all(m[1], m[2], Number(m[3] ?? 0));
  });
};

/** Live event types of ONE session, oldest first. */
const kinds = (wireKey) => sessionEvents(wireKey).filter((r) => r.retired_at === null).map((r) => r.event_type);
const countKind = (wireKey, k) => kinds(wireKey).filter((t) => t === k).length;
/** Live rows of one kind, with their payload decoded (`{}` for a row we cannot read). */
const rowsOfKind = (wireKey, k) =>
  sessionEvents(wireKey)
    .filter((r) => r.retired_at === null && r.event_type === k)
    .map((r) => {
      let p = {};
      try {
        p = JSON.parse(r.payload_json);
      } catch {
        /* a row we cannot read is not a claim we can make */
      }
      return { ...r, payload: p };
    });
const abandonedCount = (wireKey) =>
  rowsOfKind(wireKey, "run_finished").filter((r) => r.payload.outcome === "abandoned").length;

/** `agent_tasks` rows for one session, oldest first — `[]` when the ledger does not exist yet. */
const taskRowsOf = (wireKey) => {
  if (!fs.existsSync(STATE_DB)) return [];
  const db = new DatabaseSync(STATE_DB, { readOnly: true });
  try {
    return db
      .prepare(
        "SELECT id, status, lane, task_prompt, created_at, adjudicated_at_ms, metadata_json \
         FROM agent_tasks WHERE parent_session_id = ? ORDER BY created_at ASC, rowid ASC",
      )
      .all(wireKey);
  } finally {
    db.close();
  }
};

/** Everything `tracing` wrote so far, every rotated file in name order. */
const serverLogText = () => {
  if (!fs.existsSync(LOG_DIR)) return "";
  return fs
    .readdirSync(LOG_DIR)
    .filter((f) => f.startsWith("aleph-server.log"))
    .sort()
    .map((f) => fs.readFileSync(path.join(LOG_DIR, f), "utf8"))
    .join("");
};
const logLines = (needle) => serverLogText().split("\n").filter((l) => l.includes(needle));

// The one line per boot that reports the boot scan, in the form
// `…: ResumeCoordinator boot scan finished, scanned=0, resumed=1, abandoned=0,
// skipped=0, notified=0`. It prints only after `ResumeLaunch::settle`
// returns, and `settle` JOINS every candidate task — including one whose
// retrigger is blocked inside a `BeforeAgentStart` hook — so a boot killed
// while that hook holds the resumed run leaves NO line (the `ratchet` stage
// asserts exactly that on its first two boots). -1 for a counter the line
// does not carry, never 0: an absent number is not a zero (判据 #8).
const BOOT_LINE = "ResumeCoordinator boot scan finished";
const bootLines = () =>
  logLines(BOOT_LINE).map((raw) => {
    const num = (k) => Number((raw.match(new RegExp(`\\b${k}=(\\d+)`)) || [])[1] ?? -1);
    return {
      scanned: num("scanned"),
      resumed: num("resumed"),
      abandoned: num("abandoned"),
      skipped: num("skipped"),
      notified: num("notified"),
      raw: raw.trim(),
    };
  });
// Which boot a line belongs to. EVERY resume-ON boot prints one — the first
// boot over an empty log included (`load_run_markers` on nothing is a walked,
// empty scan) — and a resume-OFF boot prints none, so "the n-th line" is a
// count the shell would have to keep in step with the config it patched.
// Instead the shell takes a MARK (`drive boot-mark`) right before the
// `start_server` whose scan a phase wants, and the phase asks for the line
// after it — or asserts that none arrived, which is the `ratchet` stage's
// claim on its held boots.
const BOOT_MARK = path.join(QA_ROOT, "boot_mark.json");
const EXEC_STARTED_LINE = "Agent execution started";
const cmdBootMark = () => {
  const mark = bootLines().length;
  const execStarted = logLines(EXEC_STARTED_LINE).length;
  const requestsLogged = requests().length;
  // `at`: a durable stamp older than the mark was written by an EARLIER
  // boot — the way a phase says which boot did something, instead of
  // inferring it from a line that boot may not have printed.
  const at = Date.now();
  fs.writeFileSync(BOOT_MARK, JSON.stringify({ mark, execStarted, requestsLogged, at }));
  log(
    `boot mark at ${iso(at)}: ${mark} boot-scan line(s), ${execStarted} '${EXEC_STARTED_LINE}' line(s), ` +
      `${requestsLogged} provider request(s) so far`,
  );
};
const readBootMark = () => {
  try {
    const m = JSON.parse(fs.readFileSync(BOOT_MARK, "utf8"));
    return {
      mark: Number(m.mark),
      execStarted: Number(m.execStarted),
      requestsLogged: Number(m.requestsLogged),
      at: Number(m.at),
    };
  } catch {
    console.error("INSTRUMENT FAILURE: no boot mark — the shell must `drive boot-mark` before the boot it asks about");
    process.exit(1);
  }
};
const bootMark = () => readBootMark().mark;
/** The boot-scan line of the boot after the mark: waits for it, returns the newest line (or `null`). */
const awaitBootLineAfterMark = async (budget = 120_000) => {
  const mark = bootMark();
  const lines = await until(() => (bootLines().length > mark ? bootLines() : null), budget, 500);
  return lines ? lines[lines.length - 1] : null;
};

/**
 * Install an approved `BeforeAgentStart` command hook that sleeps `ms` —
 * or, with `ms === "off"`, remove everything the install wrote.
 *
 * Two files, both under `$ALEPH_HOME` (the loaders follow `get_config_dir`):
 * `hooks.json` in the Claude-Code-shaped format `user_settings.rs` parses
 * (global layer → `plugin_name = "user:global"`), and the shell-hook consent
 * registry `shell-hooks-allowlist.json`, without which the hook is recorded
 * `pending` and SKIPPED — a stage whose hook never ran would then be
 * measuring a window that was never opened. The fingerprint is
 * `ShellHookConsent::fingerprint`: `sha256(plugin_name ‖ 0x00 ‖ command)`,
 * first 16 hex chars.
 *
 * The sleeper is a SCRIPT FILE beside `hooks.json`, run by a relative path
 * with no quotes and no shell metacharacters — `node qa-resume-sleeper.mjs`
 * — and that is measured, not fussy: the brief's `node -e "setTimeout(()=>
 * {}, N)"` exited 1 on the spot (2026-09-18, `Hook command exited with
 * status Some(1)`), because hooks run under `cmd /C` on Windows
 * (`executor.rs`), Rust's argument quoting hands cmd
 * `"node -e \"…(()=>{}…)\""`, and cmd's own quote toggling leaves the `>`
 * outside any quote — a redirection. The relative path works because the
 * hook's cwd IS `$ALEPH_HOME` (the `user:global` layer's `plugin_root` is the
 * directory `hooks.json` was read from, `user_settings.rs::load_into`); on
 * Windows that cwd is also what keeps the scratch root from being removed
 * while an orphan sleeps, so the file name doubles as the marker `run.sh`
 * kills them by. `timeout_secs` is clamped to `MAX_HOOK_TIMEOUT_SECS` (300)
 * by the executor; the sleep is what bounds an orphan, not that.
 */
const SLEEPER_FILE = "qa-resume-sleeper.mjs";
// The three files `hooks` writes — the one list `hooks off` removes, so a
// kept root re-used for a later stage does not hold every turn 120–300 s and
// read as "server slow".
const HOOK_FILES = [SLEEPER_FILE, "hooks.json", "shell-hooks-allowlist.json"];
function cmdHooks(ms) {
  if (ms === "off") {
    const removed = HOOK_FILES.filter((f) => {
      const p = path.join(ALEPH_HOME, f);
      if (!fs.existsSync(p)) return false;
      fs.rmSync(p);
      return true;
    });
    log(`hook uninstalled: removed ${removed.length ? removed.join(", ") : "nothing (not installed)"}`);
    return;
  }
  fs.writeFileSync(path.join(ALEPH_HOME, SLEEPER_FILE), `setTimeout(() => {}, ${Number(ms)});\n`);
  const command = `node ${SLEEPER_FILE}`;
  fs.writeFileSync(
    path.join(ALEPH_HOME, "hooks.json"),
    JSON.stringify(
      { hooks: { BeforeAgentStart: [{ hooks: [{ type: "command", command, timeout_secs: 300 }] }] } },
      null,
      2,
    ),
  );
  const fingerprint = crypto
    .createHash("sha256")
    .update("user:global")
    .update(Buffer.from([0]))
    .update(command)
    .digest("hex")
    .slice(0, 16);
  const now = Math.floor(Date.now() / 1000);
  fs.writeFileSync(
    path.join(ALEPH_HOME, "shell-hooks-allowlist.json"),
    JSON.stringify(
      {
        version: 1,
        entries: [
          {
            fingerprint,
            plugin_name: "user:global",
            command,
            event: "BeforeAgentStart",
            status: "approved",
            first_seen: now,
            approved_at: now,
          },
        ],
      },
      null,
      2,
    ),
  );
  log(`hook installed: ${command} (fp ${fingerprint})`);
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/** Send `text` into (or minting) a session and return once the run is under way. */
const sendTurn = async (conn, text, sessionKey, model, tier) => {
  const params = { message: text, channel: CHANNEL };
  if (sessionKey) params.session_key = sessionKey;
  // The per-turn execution tier (`chat.send` → `ChatSendParams::exec_tier` →
  // `request.metadata["exec_tier"]` → `resolve_exec_tier`'s `requested` rung).
  // Unlike the model half this ALSO stamps the session row on a non-resume
  // turn (`knob_to_stamp`), which is exactly what the `knobs` stage wants: the
  // crashed run and the row it leaves behind start out agreeing, so the later
  // divergence is one the fixture made on purpose.
  if (tier) params.exec_tier = tier;
  // A per-turn directive, and the `knobs` stage cannot do without one.
  // MEASURED 2026-09-03: the envelope's `model` half is `routing_directive`
  // (runner_impl.rs:364), which folds the `select_model` pick and the agent's
  // model_hint — the agent's CONFIGURED model is not a directive, so a turn
  // sent without this records `model: None` on its marker ("this run walked
  // the default chain"), and a resume then walks today's chain. That is the
  // documented contract, not a defect; it is simply not the replay path, so a
  // stage that wants to test replay has to give the run a directive to freeze.
  if (model) params.model_override = { kind: "qualified", provider: "qa-mock", model };
  const started = await conn.ok("chat.send", params);
  log(`run ${started.run_id} on ${started.session_key}: ${text.slice(0, 48)}`);
  return started;
};

/**
 * Send the marker that makes the mock dispatch `bash sleep 120`, then wait
 * until that dispatch is DURABLE — a row in `session_events`, not a frame.
 *
 * A blind sleep here is the trap this whole fixture exists to avoid: too
 * short and every later assertion runs over an empty set, and an empty set
 * passes an "is there no X" question for the wrong reason.
 */
async function cmdDangle(marker = "qa-dangle", model = null, tier = null) {
  const conn = new Conn("driver");
  await conn.open();
  await dangleOn(conn, marker, model, tier);
  conn.close();
}

/** The body of `cmdDangle`, on a connection the caller owns. */
async function dangleOn(conn, marker, model = null, tier = null) {
  const prior = fs.existsSync(SESSION_FILE) ? readSession() : null;
  const started = await sendTurn(conn, `${marker} please run the long command`, prior, model, tier);
  fs.writeFileSync(SESSION_FILE, started.session_key);
  const landed = await until(() => danglingIds(started.session_key).length > 0, 180_000, 400);
  if (!landed) {
    console.error("INSTRUMENT FAILURE: no dangling dispatch ever reached the durable log");
    console.error(`  events for ${started.session_key}: ${eventsOf(started.session_key).length}`);
    process.exit(1);
  }
  log(`dangling now: ${danglingIds(started.session_key).join(",")}`);
}

/** Dangling call ids that also have a `tool_call_parked` row — the park as a FACT, not a card. */
const parkedIds = () => {
  const open = new Set(danglingIds());
  return eventsOf()
    .filter((r) => r.event_type === "tool_call_parked")
    .map((r) => {
      try {
        return JSON.parse(r.payload_json).call_id;
      } catch {
        return null;
      }
    })
    .filter((id) => id && open.has(id));
};

/**
 * Stage `parked`'s dangle: the same `ask`-gated `bash` dispatch as every r2
 * stage, but the kill must land AFTER the gate's `tool_call_parked` stamp is
 * durable — a kill between the dispatch and the stamp is the `claims` shape
 * (OUTCOME UNKNOWN), and this stage would then be measuring that one. The
 * second check proves the park is live in the process, not merely logged.
 *
 * ONE connection for both halves. Measured 2026-09-13 on this host: a second
 * WebSocket opened and closed in the same driver process made Node v24 abort
 * inside `process.exit` (`Assertion failed: !(handle->flags &
 * UV_HANDLE_CLOSING), src\win\async.c:76`) AFTER both checks had passed —
 * the shell then read the abort as "no parked dangle".
 */
async function cmdDangleParked(marker = "qa-dangle") {
  const conn = new Conn("driver");
  await conn.open();
  await dangleOn(conn, marker);
  const landed = await until(() => (parkedIds().length > 0 ? parkedIds() : null), 60_000, 300);
  check(
    Boolean(landed),
    "a tool_call_parked row landed for the dangling call BEFORE the kill",
    show(eventsOf().slice(-4)),
  );
  if (!landed) process.exit(1);
  const pending = (await conn.attempt("exec.approvals.pending")).result?.pending ?? [];
  check(
    pending.some((p) => p.record?.tool_call_id === landed[0]),
    "and exec.approvals.pending holds that call id — the gate is parked on it, not merely logged",
    show(pending),
  );
  conn.close();
}

/**
 * Stage `parked`: a call the log shows parked at a gate when the server died
 * must be reported as NEVER RAN, not "unknown". `sub=wire` runs before the
 * resume (the reducer's reading of the log, on the exact field the Panel and
 * TUI render); `sub=model` after it (what the repair put in front of the
 * model, and the receipt that says the resume happened at all).
 */
async function cmdParked(sub, receiptFile) {
  const key = readSession();
  if (sub === "model") {
    const hit = await until(
      () => requests().find((r) => userText(r.body).includes("never ran")) || null,
      180_000,
      1000,
    );
    check(
      Boolean(hit),
      "the model's next request says the call NEVER RAN",
      `${requests().length} requests, none carrying the phrase`,
    );
    const text = hit ? userText(hit.body) : "";
    check(text.includes("operator approval"), "and names what it was waiting for", text.slice(0, 400));
    check(text.includes("bash"), "and names the tool", text.slice(0, 400));
    check(
      !requests().some((r) => userText(r.body).includes("OUTCOME UNKNOWN")),
      "and no request calls that parked call's outcome UNKNOWN",
    );
    let receipt = null;
    try {
      receipt = JSON.parse(fs.readFileSync(receiptFile, "utf8"));
    } catch {
      /* asserted below */
    }
    check(receipt?.resumed === 1, "the receipt counts one resumed run", show(receipt));
    return;
  }
  const conn = new Conn("driver");
  await conn.open();
  const { lastRun } = await lastRunOf(conn, key);
  const d = (lastRun?.dangling || [])[0] || {};
  check(
    d.parked === "approval",
    "the wire face says the dangling call was parked awaiting approval",
    show(lastRun?.dangling),
  );
  check(d.denied === false, "and not denied", show(d));
  check(
    lastRun?.disposition === "interrupted",
    "and still reads the run as interrupted",
    show(lastRun?.disposition),
  );
  conn.close();
}

/** The instrument self-check the shell runs after the kill. */
async function cmdAssertDangling(min) {
  const key = readSession();
  const ids = danglingIds(key);
  check(ids.length >= Number(min), `at least ${min} dangling dispatch in the durable log`, ids.join(","));
}

/**
 * Stage `claims`. After the restart, three faces of ONE reduction:
 *   the wire face (`chat.history` → `session.last_run`),
 *   the operator face (`resume --json` → every `ResumeReceipt` counter key),
 *   and the effect (the session settles to `clean` once the resume ran).
 */
async function cmdClaimsWire() {
  const key = readSession();
  const conn = new Conn("driver");
  await conn.open();

  const { reply, lastRun } = await lastRunOf(conn, key);
  check(Boolean(lastRun), "chat.history carries session.last_run", show(reply.result ?? reply.error));
  if (lastRun) {
    check(
      lastRun.disposition === "interrupted",
      "last_run.disposition is `interrupted` after a kill -9 mid-call",
      show(lastRun),
    );
    check(lastRun.inspected === true, "last_run.inspected is true on the history face", show(lastRun));
    check(
      Array.isArray(lastRun.dangling) && lastRun.dangling.length >= 1,
      "last_run.dangling names the cut-off call",
      show(lastRun.dangling),
    );
    const d = (lastRun.dangling || [])[0] || {};
    check(d.tool_name === "bash", "the dangling call names the tool that was dispatched", show(d));
    check(
      d.provenance === "this_restart",
      "the dangling call is attributed to THIS restart, not an earlier run",
      show(d),
    );
    check(
      Boolean(lastRun.progress) && lastRun.progress.tool_calls_dispatched >= 1,
      "last_run.progress says how far the run got",
      show(lastRun.progress),
    );
  }

  // §0.1 forwarded cost, measured rather than adjectival: `chat.history`
  // reads the whole log on every attach.
  log(`COST chat.history load_all_events for this session: ${eventsOf(key).length} events`);
  conn.close();
}

/**
 * The operator face, run AFTER `aleph-server resume --json`: every counter key
 * on the receipt, then the effect — the session's own face settling to
 * `clean`. Split from the wire half because the wire half's claim is
 * "interrupted", which the resume is about to stop being true.
 */
async function cmdClaimsReceipt(receiptFile) {
  const key = readSession();
  const conn = new Conn("driver");
  await conn.open();
  let receipt = null;
  try {
    receipt = JSON.parse(fs.readFileSync(receiptFile, "utf8"));
  } catch (e) {
    receipt = null;
    console.log(`      | could not read ${receiptFile}: ${e.message}`);
  }
  check(Boolean(receipt), "aleph-server resume --json printed a receipt", receiptFile);
  if (receipt) {
    const KEYS = [
      "status",
      "scanned",
      "resumed",
      "abandoned",
      "skipped",
      "busy",
      "delegated",
      "refused",
      "contradictions",
      "degraded",
      "unsnapshotted",
      "skipped_unknown_age",
      "error",
      "agent_id",
      "session_key",
    ];
    const missing = KEYS.filter((k) => !(k in receipt));
    check(missing.length === 0, "every ResumeReceipt key is present on the CLI face", `missing: ${missing.join(",")}`);
    check(
      typeof receipt.status === "string" && receipt.status.length > 0,
      "the receipt carries a status word",
      show(receipt),
    );
    check(Array.isArray(receipt.refused), "`refused` is a list of entries, not a counter", show(receipt.refused));
  }

  // The effect: once the resume has run, the session's own face stops saying
  // "interrupted". This is the arm that fails if the repair only ever wrote a
  // receipt.
  const settled = await until(async () => {
    const { lastRun: lr } = await lastRunOf(conn, key);
    return lr && lr.disposition === "clean" ? lr : null;
  }, 180_000, 2000);
  check(Boolean(settled), "after the resume the session's last_run reads `clean`", show(settled));
  conn.close();
}

/**
 * Write the ONE event a crash inside the denial window would have left.
 *
 * The `denied` stage's subject is `DanglingDeniedCall`: a dispatch that was
 * denied and whose `ToolError` receipt never landed, which
 * `boundary_repair_text` answers with "NOT EXECUTED" instead of "OUTCOME
 * UNKNOWN". Producing that from OUTSIDE the process is not possible, and this
 * is measured rather than assumed:
 *
 *   * with a static `bash = "deny"` policy the gate refuses, appends
 *     `tool_call_denied` AND the tool's own `ToolError` receipt in the same
 *     turn — so the call is answered and nothing is ever dangling (this is
 *     what the first version of the stage hit: `cmdDangle` timed out with
 *     "no dangling dispatch ever reached the durable log", because there was
 *     correctly none);
 *   * with `ask`, the denial only exists if a card is answered, and the
 *     receipt follows it microseconds later inside the same process.
 *
 * A `kill -9` cannot be aimed between those two appends from a shell. So the
 * fixture appends that row itself — with the server DOWN, at head+1, carrying
 * the real dispatch's own `turn_id`/`call_id`, in the exact `#[serde(tag =
 * "type")]` shape `SqliteEventStore::append` writes. Nothing downstream is
 * simulated: the reduction, the `denied` flag on the wire, the repair text and
 * the resume receipt are all the product reading its own log off disk.
 */
function cmdForgeDenial() {
  const ids = danglingIds();
  if (ids.length === 0) {
    console.error("INSTRUMENT FAILURE: no dangling dispatch to deny");
    process.exit(1);
  }
  const db = new DatabaseSync(EVENTS_DB);
  try {
    const row = db
      .prepare(
        "SELECT session_id, seq, turn_id, payload_json FROM session_events \
         WHERE event_type = 'tool_call_requested' AND retired_at IS NULL \
         ORDER BY seq DESC LIMIT 1",
      )
      .get();
    if (!row) {
      console.error("INSTRUMENT FAILURE: no tool_call_requested row in the log");
      process.exit(1);
    }
    const dispatch = JSON.parse(row.payload_json);
    if (!ids.includes(dispatch.call_id)) {
      console.error(`INSTRUMENT FAILURE: newest dispatch ${dispatch.call_id} is not dangling`);
      process.exit(1);
    }
    const head = db
      .prepare("SELECT MAX(seq) AS m FROM session_events WHERE session_id = ?")
      .get(row.session_id).m;
    const at = Date.now();
    const payload = JSON.stringify({
      type: "tool_call_denied",
      turn_id: dispatch.turn_id,
      call_id: dispatch.call_id,
      reason: "operator denied the card; the server died before the receipt",
      at,
    });
    db.prepare(
      "INSERT INTO session_events (session_id, seq, turn_id, event_type, payload_json, created_at) \
       VALUES (?, ?, ?, ?, ?, ?)",
    ).run(row.session_id, Number(head) + 1, row.turn_id, "tool_call_denied", payload, at);
    log(`forged tool_call_denied for ${dispatch.call_id} at seq ${Number(head) + 1}`);
  } finally {
    db.close();
  }
}

/**
 * Stage `denied`: a call the approval gate refused must not be reported as
 * "unknown". `sub=wire` runs before the resume (the reducer's reading of the
 * log); `sub=model` after it (what the repair actually put in front of the
 * model).
 */
async function cmdDenied(sub) {
  const key = readSession();
  if (sub === "model") {
    const wanted = "denied by the approval gate and did not run";
    const hit = await until(
      () => requests().find((r) => userText(r.body).includes(wanted)) || null,
      180_000,
      1000,
    );
    check(
      Boolean(hit),
      "the model's next request says the call was DENIED, not `OUTCOME UNKNOWN`",
      `${requests().length} requests logged, none carrying the phrase`,
    );
    const unknown = requests().some((r) => userText(r.body).includes("OUTCOME UNKNOWN"));
    check(!unknown, "and no request calls that same denied call's outcome UNKNOWN", String(unknown));
    return;
  }
  const conn = new Conn("driver");
  await conn.open();
  const { lastRun } = await lastRunOf(conn, key);
  const denied = (lastRun?.dangling || []).some((d) => d.denied === true);
  check(denied, "the wire face flags the dangling call as denied", show(lastRun?.dangling));
  check(
    lastRun?.disposition === "interrupted",
    "and still reads the run as interrupted — a denial does not close a run",
    show(lastRun?.disposition),
  );
  conn.close();
}

/** Live (non-retired) rows of the durable log. A rewind retires, never deletes. */
const liveEvents = (key) => eventsOf(key).filter((r) => r.retired_at === null);

/**
 * Stage `rewind`: a rewind that shortens a run's tail must leave the marker
 * tail balanced.
 *
 * The rewind is aimed ONE ROW PAST the open `RunStarted`, never at the marker
 * itself. Aiming at the marker retires the opening half too, and then
 * `close_open_run_after_retire` finds `reduction.open_run == None` and returns
 * `Ok(None)` without appending anything (src/session/marker_balance.rs:57-59):
 * the stage is green on a build where the balancer does not exist or is never
 * called, the tail reads `never_ran` instead of `clean`, and the receipt reads
 * `no_runs` (`scanned: 0`) because the log has no markers left at all. That is
 * the arrangement this stage shipped with in the first round and it could not
 * go red. With the marker deliberately left OPEN, the only thing in this stage
 * that can produce a `RunFinished` is the balancer.
 */
async function cmdRewind(sub, arg) {
  if (sub === "receipt") {
    // Parsed, not grepped. Every counter of `ResumeReceipt` is serialised
    // unconditionally (`#[serde(default)]`, no `skip_serializing_if` —
    // shared/protocol/src/resume.rs), so a `grep '"scanned"'` matches ANY
    // well-formed receipt, the `no_runs` one included: it is a predicate with
    // no red state.
    let receipt = null;
    try {
      receipt = JSON.parse(fs.readFileSync(arg, "utf8"));
    } catch (e) {
      console.log(`      | could not read ${arg}: ${e.message}`);
    }
    check(Boolean(receipt), "aleph-server resume --json printed a receipt for the rewound session", String(arg));
    check(
      receipt?.status === "already_finished",
      "the receipt reads `already_finished` — the balanced marker settles the session and nothing is re-run",
      show(receipt),
    );
    check(
      Number(receipt?.scanned ?? 0) > 0,
      "and it got there by SCANNING a session that still has run markers, not by finding none at all",
      show(receipt),
    );
    return;
  }
  const key = readSession();
  const conn = new Conn("driver");
  await conn.open();
  if (sub === "do") {
    // `RewindParams` is `{session_key, seq}` — `seq` is the FIRST event to
    // retire, inclusive, not a count of messages.
    const live = liveEvents(key);
    const started = [...live].reverse().find((r) => r.event_type === "run_started");
    if (!started) {
      console.error("INSTRUMENT FAILURE: no live run_started row to leave open");
      console.error(`  event types: ${live.map((r) => r.event_type).join(",")}`);
      process.exit(1);
    }
    const target = live.find((r) => r.seq > started.seq);
    if (!target) {
      console.error("INSTRUMENT FAILURE: the run_started is the newest live row, so there is no tail to retire");
      console.error(`  event types: ${live.map((r) => `${r.seq}:${r.event_type}`).join(",")}`);
      process.exit(1);
    }
    const before = live.length;
    const retiredBefore = eventsOf(key).filter((e) => e.retired_at !== null).length;
    const r = await conn.attempt("chat.rewind", { session_key: key, seq: target.seq });
    check(!r.error, "chat.rewind is accepted on a session whose run was cut off", show(r.error));
    const after = liveEvents(key);
    log(
      `live events ${before} -> ${after.length} (rewound at seq ${target.seq} = ${target.event_type}, ` +
        `run_started@${started.seq} deliberately left live)`,
    );
    // Counted as RETIRED rows, not as a drop in the live count: the balancer
    // appends its closer inside the same call, so the live log shrinks by one
    // less than the rewind retired (MEASURED 2026-09-03: 5 live -> 4 live while
    // `events_retired` said 2). A live-count subtraction reads that difference
    // as a disagreement and goes red on the very effect this stage proves.
    const retiredAfter = eventsOf(key).filter((e) => e.retired_at !== null).length;
    check(
      retiredAfter > retiredBefore,
      "the rewind actually retired rows — otherwise the balance below is vacuous",
      `retired ${retiredBefore} -> ${retiredAfter}, live ${before} -> ${after.length}`,
    );
    check(
      Number(r.result?.events_retired ?? 0) === retiredAfter - retiredBefore,
      "and the reply's events_retired agrees with the log",
      show(r.result),
    );
    // Anti-vacuity. If this ever goes red the stage has silently degraded back
    // to retiring the marker itself, and everything below it becomes a no-op
    // that still reports green.
    check(
      after.some((e) => e.seq === started.seq && e.event_type === "run_started"),
      "the opening `RunStarted` survived the rewind — the marker really was left open for the balancer to close",
      `seq ${started.seq} among ${after.map((e) => `${e.seq}:${e.event_type}`).join(",")}`,
    );
    const closer = after.find((e) => e.event_type === "run_finished" && e.seq > started.seq);
    check(
      Boolean(closer),
      "the retire appended a `RunFinished` of its own — nothing else in this stage writes one",
      show(after.map((e) => `${e.seq}:${e.event_type}`)),
    );
    check(
      JSON.parse(closer?.payload_json ?? "{}").outcome === "cancelled",
      "closed as `cancelled` — a deliberate user edit, not a failed recovery",
      show(closer?.payload_json),
    );
    const { lastRun } = await lastRunOf(conn, key);
    check(
      lastRun?.disposition === "clean",
      "after the rewind the marker tail is balanced — the log no longer claims an open run",
      show(lastRun),
    );
  } else {
    // After the restart: nothing to resume. `clean` and not `never_ran` — the
    // markers are still there, they are simply balanced.
    const { lastRun } = await lastRunOf(conn, key);
    check(
      lastRun?.disposition === "clean",
      "the rewound session still reads balanced after a restart",
      show(lastRun),
    );
  }
  conn.close();
}
/**
 * Write one key into a session row's identity metadata, with the server down.
 *
 * The row is `sessions.metadata` in the SAME `sessions.db` the event log lives
 * in — NOT a `metadata.json` under `data/sessions/`. That was worth measuring
 * rather than reading off `default_session_store_backend()` ("file"): the pin
 * reader is `stored_model_pin`, which asks `self.session_manager`, and the
 * session MANAGER is sqlite unconditionally. This fixture's first attempt went
 * looking for the file backend's directory and found none on disk (measured
 * 2026-09-03) — the `SessionStore` backend knob selects a different store than
 * the one this pin travels through.
 *
 * The column holds a serialised `SessionIdentityMeta`, whose `custom` bag is
 * `#[serde(flatten)]` — so the knob keys sit at the TOP level of that object,
 * beside `role` / `identity_id` / `source_channel`, and a nested `custom`
 * object would be read by nobody.
 */
const stampSessionMeta = (key, patch) => {
  const db = new DatabaseSync(EVENTS_DB);
  try {
    const row = db.prepare("SELECT key, metadata FROM sessions WHERE key = ?").get(key);
    if (!row) {
      const keys = db.prepare("SELECT key FROM sessions").all().map((r) => r.key);
      console.error(`INSTRUMENT FAILURE: no sessions row for ${key}; rows: ${keys.join(", ") || "none"}`);
      process.exit(1);
    }
    let meta = {};
    try {
      meta = JSON.parse(row.metadata || "{}");
    } catch {
      console.error(`INSTRUMENT FAILURE: sessions.metadata for ${key} is not JSON: ${row.metadata}`);
      process.exit(1);
    }
    const next = { ...meta, ...patch };
    db.prepare("UPDATE sessions SET metadata = ? WHERE key = ?").run(JSON.stringify(next), key);
    return JSON.parse(db.prepare("SELECT metadata FROM sessions WHERE key = ?").get(key).metadata);
  } finally {
    db.close();
  }
};

/**
 * Stage `knobs`: the crashed run's SETTINGS come back, not today's.
 *
 * `sub=pin` moves the session to model B with the server DOWN; `sub=assert`
 * reads what the resumed run actually put in front of the provider and checks
 * it carries the model the crashed run was executing under — the envelope
 * snapshot, not the session's current value.
 */
/**
 * Every live `RunStarted` marker in this log, oldest first, with its envelope
 * decoded. The envelope is what the turn was ACTUALLY running under —
 * `run_envelope_snapshot` reads it off the same `TurnEnvelope` the turn used,
 * so the resumed run's marker is a durable record of the tier `resolve_exec_
 * tier_with_ceiling` returned for it, not of what anyone asked for.
 */
const runMarkers = (key) =>
  eventsOf(key)
    .filter((r) => !r.retired_at && r.event_type.includes("run_started"))
    .map((r) => {
      let env = null;
      try {
        env = JSON.parse(r.payload_json ?? "{}").envelope ?? null;
      } catch {
        env = null;
      }
      return { seq: r.seq, env };
    });

async function cmdKnobs(sub, arg, tier) {
  const key = readSession();
  if (sub === "pin") {
    // Why the fixture writes this row itself, with the server stopped.
    //
    // MEASURED 2026-09-03: there is no in-process path to it from outside.
    // `session.update` does not exist (`-32601`); no `session.*` method sets a
    // model (the registry has artifact / compact / create / export_html / list
    // / truncate / usage); and the metadata modify path REFUSES `model_pin` on
    // purpose (`handlers/session/db_handlers/modify.rs:376` — "their legal
    // writer is elsewhere"). The legal writer is the `select_model` TOOL (R8),
    // which needs the mock to dispatch it on a turn of its own — and the `ask`
    // instrument leaves this session BUSY on a parked approval card, so a pin
    // turn queues behind the dangle and dies with the server.
    //
    // So the pin is written where `StoreBackedPinSink` writes it
    // (`identity_meta`, keys `model_pin` / `model_pin_provider`, both flattened
    // to the top level of that object by `#[serde(flatten)] custom`) and every
    // reader downstream is the product: `stored_model_pin` hydrates the process
    // map from this row on the next turn, `snapshot_from_metadata` publishes it
    // on the wire, and the resume replays the ENVELOPE against it.
    // Instrument self-check, and it has to come first: if the crashed run's
    // marker carries no envelope there is no snapshot to replay, and the
    // assertion after the restart would be measuring the ABSENCE of a producer
    // while reading like a resume that ignored one.
    const marker = runMarkers(key).pop();
    const env = marker?.env ?? null;
    check(
      env?.model === "qa-model-a",
      "the crashed run's RunStarted marker snapshotted model qa-model-a",
      show({ envelope: env, marker_seq: marker?.seq ?? null }),
    );
    // The second knob, and it is deliberately moved in the LOOSENING direction
    // — the opposite of what the round's plan wrote down. 判据 #14: the two
    // directions of this gate are not the same claim. Snapshot `full` + a
    // session since pulled down to `ask` resolves to `ask` for a build with NO
    // ceiling at all (the session rung already says `ask`), so that
    // arrangement cannot tell `resolve_exec_tier_with_ceiling` from
    // `resolve_exec_tier`. Snapshot `ask` + a session since opened to `full`
    // can: without the ceiling the resumed run executes at `full`, unattended,
    // at a tier nobody granted it for that run.
    check(
      env?.exec_tier === tier,
      `the crashed run's RunStarted marker snapshotted exec tier ${tier}`,
      show({ envelope: env, marker_seq: marker?.seq ?? null }),
    );
    const back = stampSessionMeta(key, {
      model_pin: arg,
      model_pin_provider: "qa-mock",
      exec_tier: "full",
    });
    check(back?.model_pin === arg, `the session row on disk now pins ${arg}`, show(back));
    check(
      back?.exec_tier === "full",
      "the session row on disk has since been opened up to exec tier full",
      show(back),
    );
    log(`pinned ${arg} on ${key}`);
    return;
  }
  const wanted = arg;
  const conn = new Conn("driver");
  await conn.open();
  // Anti-vacuity, and it is the whole stage: if the session never left model A
  // then "the resumed run still runs under A" is equally true of a build that
  // dropped the envelope on the floor (判据 #2). This asserts the SERVER read
  // the moved row back — not that the fixture wrote a file.
  const { session } = await lastRunOf(conn, key);
  check(
    session?.model_pin === "qa-model-b",
    "the restarted server reads the session as pinned to qa-model-b",
    show({ model_pin: session?.model_pin ?? null, model: session?.model ?? null }),
  );
  const resumed = await until(
    () => {
      const hits = requests().filter((r) => carriesRepair(userText(r.body)));
      return hits.length > 0 ? hits : null;
    },
    180_000,
    1000,
  );
  check(
    Boolean(resumed),
    "the resumed run reached the provider",
    `${requests().length} requests logged, none carrying the repair text`,
  );
  const models = (resumed || []).map((r) => r.body?.model);
  check(
    models.length > 0 && models.every((m) => m === wanted),
    `the resumed run runs under the SNAPSHOT model (${wanted}), not the session's current one`,
    show(models),
  );

  // The exec-tier half. Same shape, opposite direction (see `pin`): the row is
  // now `full` and the snapshot was `ask`, so a resume that ignored the ceiling
  // would run this turn at `full`.
  check(
    session?.exec_tier === "full",
    "the restarted server reads the session as opened up to exec tier full",
    show({ exec_tier: session?.exec_tier ?? null }),
  );
  // The oracle is the RESUMED run's own marker: `run_envelope_snapshot` stamps
  // it from the `TurnEnvelope` that turn is executing under, so this is the
  // tier the turn actually got — not a request, not a log line, and not the
  // snapshot read back to itself (that value lives on the OLDER marker, and
  // the count check below is what keeps these two from being the same row).
  const markers = runMarkers(key);
  check(
    markers.length >= 2,
    "the resume started a run of its own — otherwise the marker below is the crashed run's",
    show(markers),
  );
  const resumedEnv = markers[markers.length - 1]?.env ?? null;
  check(
    resumedEnv?.exec_tier === tier,
    `the resumed run runs under the SNAPSHOT exec tier (${tier}), not the session's looser one`,
    show({ resumed_envelope: resumedEnv, markers: markers.map((m) => m.seq) }),
  );
  conn.close();
}

/**
 * The burst run has to FINISH before the kill, or this stage measures the wrong
 * thing: `cmdDangle` returns as soon as ONE dispatch is durable, which during a
 * burst is a few milliseconds in. Killing there leaves dangling calls, the
 * restart resumes them, and the extra turn's usage would make the
 * "billed once" comparison below fail for a reason that has nothing to do with
 * the projector.
 *
 * Settled = every `run_started` in this log has a `run_finished`. Counting the
 * markers rather than watching a frame keeps the oracle on disk.
 */
async function cmdHolesSettle() {
  const markers = () => {
    const rows = eventsOf(null).filter((r) => !r.retired_at);
    const started = rows.filter((r) => r.event_type.includes("run_started")).length;
    const finished = rows.filter((r) => r.event_type.includes("run_finished")).length;
    return { started, finished };
  };
  const done = await until(() => {
    const m = markers();
    return m.started > 0 && m.started === m.finished;
  }, 300_000, 1000);
  const m = markers();
  check(Boolean(done), "the burst run finished before the kill", `run_started ${m.started}, run_finished ${m.finished}`);
  log(`markers settled: started ${m.started}, finished ${m.finished}`);
}

/**
 * Stage `holes`: a burst that outruns the projector queue must not lose a
 * transcript row, and must not bill the same run twice.
 *
 * Two claims, one per phase:
 *   `before` — the transcript covers every projectable event, and the numbers
 *              are recorded;
 *   `after`  — the restart's heal pass did not lose a row AND did not add a
 *              token. The second half is the one a heal can silently break:
 *              re-stamping a row that was already stamped bills the same run
 *              twice, and a token counter that grew while nobody ran anything
 *              is the only outside evidence of it.
 *
 * The deferral is an OBSERVATION printed with its number, not a claim: a burst
 * that never filled the queue makes the "deferred" half vacuous, and a vacuous
 * green is what this repo keeps paying for.
 */
async function cmdHoles(serverLog, phase = "before") {
  const key = readSession();
  const conn = new Conn("driver");
  await conn.open();
  const rows = eventsOf(key);
  const projectable = rows.filter(
    (r) =>
      !r.retired_at &&
      (r.event_type.includes("user_message") ||
        r.event_type.includes("assistant_message") ||
        r.event_type.includes("tool_call_requested") ||
        r.event_type.includes("tool_result") ||
        r.event_type.includes("tool_error")),
  ).length;
  const h = await conn.attempt("chat.history", { session_key: key, limit: 100000 });
  const msgs = h.result?.messages ?? h.result?.history ?? [];
  const session = h.result?.session ?? null;
  const tokens = session?.total_tokens ?? null;
  // `total` is the server's own count of the whole transcript, and the page
  // above is a page. Absent, it is NOT read as zero (判据 #8) — the row count
  // is used instead and the log says which answered.
  const total = typeof h.result?.total === "number" ? h.result.total : null;
  const held = total ?? msgs.length;
  // `compaction_count` / `message_count` are printed beside the total because
  // a transcript shorter than the log has TWO candidate mechanisms — a
  // projector that lost rows, and the store's own compaction trimming them on
  // purpose — and a number without the one that tells them apart cannot
  // adjudicate between the two.
  log(
    `[${phase}] history total ${show(total)} (page carried ${msgs.length} rows); ` +
      `session message_count ${show(session?.message_count)}, ` +
      `compaction_count ${show(session?.compaction_count)}`,
  );
  log(
    `[${phase}] history rows ${msgs.length}; projectable events ${projectable}; ` +
      `total events ${rows.length}; total_tokens ${show(tokens)}`,
  );
  check(
    Array.isArray(msgs) && msgs.length > 0,
    `[${phase}] the burst session has a transcript at all`,
    show(h.result ?? h.error, 300),
  );
  // The precondition, and it comes first so that raising the burst can never
  // make the NEXT assertion read like data loss. MEASURED 2026-09-03 at
  // `QA_BURST=900`: 1803 projectable events, a server-reported history total of
  // 69 — and `compaction_count 34`. The store had trimmed the projection on
  // purpose 34 times; nothing was lost. At the stage's burst the count is 0 and
  // the two sides are comparable (83 == 83). So this claim is only assertable
  // BELOW the store's compaction bound, and the fixture says which side of that
  // bound it is on rather than letting one number stand for both mechanisms.
  check(
    session?.compaction_count === 0,
    `[${phase}] the burst stayed under the store's compaction bound, so the transcript is comparable to the log`,
    `compaction_count ${show(session?.compaction_count)} — above this bound the store trims the projection ON PURPOSE and the next check would be red for a designed behaviour, not a hole`,
  );
  // `>=`, not `==`: the direction under test is LOSS (a hole), and the server
  // legitimately carries rows the durable log does not project one-for-one
  // (the boundary-repair line is one). An exact equality would go red for a
  // row being ADDED, which is a different claim — the token check below is the
  // one that catches an addition.
  check(
    held >= projectable,
    `[${phase}] no projectable event is missing from the transcript`,
    `held ${held} (${total === null ? "page rows — the reply carried no total" : "server total"}) < projectable ${projectable}`,
  );

  const stateFile = path.join(QA_ROOT, "holes_before.json");
  if (phase === "before") {
    check(
      typeof tokens === "number",
      "[before] the session row carries a token total to compare against",
      show(session),
    );
    fs.writeFileSync(stateFile, JSON.stringify({ rows: msgs.length, projectable, tokens }));
  } else {
    const prior = fs.existsSync(stateFile) ? JSON.parse(fs.readFileSync(stateFile, "utf8")) : null;
    check(Boolean(prior), "[after] the before-phase numbers were recorded", show(prior));
    if (prior) {
      check(
        msgs.length >= prior.rows,
        "[after] the restart did not drop a transcript row",
        `after ${msgs.length} < before ${prior.rows}`,
      );
      check(
        tokens === prior.tokens,
        "[after] the finished run is billed exactly once — the heal pass added no tokens",
        `before ${show(prior.tokens)} -> after ${show(tokens)}`,
      );
    }
    const { lastRun } = await lastRunOf(conn, key);
    check(
      lastRun?.disposition === "clean",
      "[after] a burst run that ended normally reads `clean` across the restart",
      show(lastRun?.disposition),
    );
  }

  let full = 0;
  let stopped = 0;
  try {
    const text = fs.readFileSync(serverLog, "utf8");
    full = (text.match(/projector queue full/g) || []).length;
    stopped = (text.match(/projector drain task stopped/g) || []).length;
  } catch {
    /* no log to read is not a claim either way */
  }
  log(
    `OBSERVATION [${phase}] projector queue-full deferrals: ${full}, drain-restart deferrals: ${stopped}` +
      (full === 0
        ? " (the queue never filled — the deferral half of this stage is vacuous at this burst size)"
        : ""),
  );
  conn.close();
}

/** §0.1 forwarded cost #2: `sessions.list` loads every run marker, unfiltered. */
async function cmdCost() {
  const conn = new Conn("driver");
  await conn.open();
  const t0 = Date.now();
  const list = await conn.attempt("sessions.list", { limit: 1 });
  const ms = Date.now() - t0;
  const rows = list.result?.sessions ?? list.result?.items ?? [];
  const total = withEvents((db) =>
    db ? db.prepare("SELECT COUNT(*) AS n FROM session_events").get().n : 0,
  );
  const markers = withEvents((db) =>
    db
      ? db
          .prepare(
            "SELECT COUNT(*) AS n FROM session_events WHERE event_type LIKE '%run_started%' OR event_type LIKE '%run_finished%'",
          )
          .get().n
      : 0,
  );
  log(
    `COST sessions.list(limit:1) returned ${rows.length} row(s) in ${ms}ms; ` +
      `the unfiltered marker load behind it reads ${markers} markers out of ${total} events`,
  );
  conn.close();
}

// ---------------------------------------------------------------------------
// Stage `unanswered` (§5.2): a crash between the seed and `RunStarted`.
// ---------------------------------------------------------------------------

const iso = (ms) => new Date(ms).toISOString();

/**
 * Send a turn and return once the seed is durable but no `RunStarted` is —
 * the window this stage kills in — and only after proving the window is
 * being HELD, not merely glimpsed.
 *
 * Without the embedding stall the window is ~30 ms wide (measured, see
 * `patch_r2.mjs` point 5), so a 100 ms poll that happened to land inside it
 * would let the kill fall on a run that had already reached `RunStarted`,
 * and the stage would then be measuring the `claims` shape. So: once the
 * window is seen open, hold for `HOLD_MS` and require it to STILL be open.
 * A window that closed during the hold means the stall is not on this path —
 * `INSTRUMENT FAILURE`, exit 1, never a pass.
 */
const HOLD_MS = 2000;
async function cmdWindow(marker = "qa-unanswered") {
  const conn = new Conn("driver");
  await conn.open();
  const started = await sendTurn(conn, `${marker} hello, are you there`, null, null, null);
  fs.writeFileSync(SESSION_FILE, started.session_key);
  const key = started.session_key;
  const open = () => countKind(key, "user_message") >= 1 && countKind(key, "run_started") === 0;
  const opened = await until(open, 60_000, 100);
  conn.close();
  if (!opened) {
    console.error("INSTRUMENT FAILURE: the seed→RunStarted window never opened (embedding stall not on the path?)");
    console.error(`  event types: ${kinds(key).join(",") || "none"}`);
    process.exit(1);
  }
  const seenAt = Date.now();
  await sleep(HOLD_MS);
  if (!open()) {
    console.error(`INSTRUMENT FAILURE: the window closed within ${HOLD_MS}ms — nothing is holding the run before RunStarted`);
    console.error(`  event types: ${kinds(key).join(",")}`);
    process.exit(1);
  }
  const seed = rowsOfKind(key, "user_message")[0];
  log(
    `window open on ${key}: user_message seq ${seed.seq} at ${iso(seed.created_at)}, ` +
      `no run_started ${HOLD_MS}ms after ${iso(seenAt)}`,
  );
}

/** The `embeddings request at <iso>; stalling <ms>ms` lines the mock logged, as epoch ms. */
const embedRequestsAt = () => {
  if (!fs.existsSync(MOCK_LOG)) return [];
  return [...fs.readFileSync(MOCK_LOG, "utf8").matchAll(/embeddings request at (\S+); stalling (\d+)ms/g)].map(
    (m) => ({ at: Date.parse(m[1]), stall: Number(m[2]) }),
  );
};

async function cmdUnanswered(phase) {
  const key = readSession();
  if (phase === "after-kill") {
    check(
      countKind(key, "user_message") === 1 && countKind(key, "run_started") === 0,
      "kill landed inside the seed→RunStarted window (one user_message, no run_started)",
      kinds(key).join(","),
    );
    // Step 0, as an assertion rather than a thing to eyeball: the stall was
    // ON the path. The mock's embedding request must sit after the seed row
    // and before now (the kill has already happened), and it must have been
    // the stalling kind — a 0 ms stall that happened to be there proves the
    // route, not the window.
    const seed = rowsOfKind(key, "user_message")[0];
    const now = Date.now();
    const hits = embedRequestsAt();
    const between = hits.filter((h) => seed && h.at >= seed.created_at && h.at <= now && h.stall > 0);
    check(
      between.length >= 1,
      "Step 0: the mock logged a stalling embeddings request between the user_message row and the kill",
      `user_message at ${seed ? iso(seed.created_at) : "?"}; embed requests ${show(hits.map((h) => `${iso(h.at)}/${h.stall}ms`))}; now ${iso(now)}`,
    );
    if (between.length >= 1) {
      log(`Step 0 evidence: user_message ${iso(seed.created_at)} < embeddings request ${iso(between[0].at)} (stall ${between[0].stall}ms) < kill ≤ ${iso(now)}`);
    }
    return;
  }
  const conn = new Conn("driver");
  await conn.open();
  if (phase === "before-resume") {
    const { lastRun } = await lastRunOf(conn, key);
    check(lastRun?.disposition === "unanswered", "chat.history reads the session as `unanswered`", show(lastRun));
    check(lastRun?.inspected === true, "and says it looked (inspected: true)", show(lastRun));
    conn.close();
    return;
  }
  // after-resume. The boot line prints only after the scan SETTLES, and the
  // scan settles only after the resumed run finishes (the retrigger awaits
  // `execute`), so wait for the line rather than reading it on arrival.
  const b = await awaitBootLineAfterMark();
  check(Boolean(b), "the resume-ON boot printed its boot-scan line", show(bootLines()));
  check(b?.scanned === 1, "the activity window found exactly this one session (scanned=1)", b?.raw);
  check(b?.resumed === 1, "the boot scan resumed the unanswered message (resumed=1)", b?.raw);
  check(b?.abandoned === 0, "and abandoned nothing", b?.raw);
  // What this boot's line can witness is only THIS boot's pass. The seeded
  // arm itself was decided one boot earlier: a resume-OFF boot still settles
  // and adjudicates (`start/mod.rs`, the disabled branch) and stamps the
  // crashed turn's task row without printing any line — so the two checks
  // after this one are the ones that pin "seeded ⇒ no notice", on the
  // durable log and on the row, whatever boot did the deciding.
  check(b?.notified === 0, "this boot wrote no lost-input notice (notified=0)", b?.raw);
  check(
    lostInputNotes(key).length === 0,
    "a seeded session gets no lost-input SystemMessage on any boot — the seed HAD landed",
    show(kinds(key)),
  );
  const crashedRow = taskRowsOf(key).find((t) => t.task_prompt.includes("hello, are you there"));
  const stampedAt = crashedRow?.adjudicated_at_ms ?? null;
  const markAt = readBootMark().at;
  check(
    stampedAt !== null && stampedAt < markAt,
    "the crashed turn's task row was adjudicated BEFORE this boot — the resume-OFF boot's settle ran the 8.2(b) pass and decided `seeded`",
    `row ${show(crashedRow)}; boot mark at ${iso(markAt)}`,
  );
  check(countKind(key, "resume_attempted") === 1, "exactly one ResumeAttempted stamp", kinds(key).join(","));
  check(countKind(key, "tool_error") === 0, "no boundary repair was written (nothing dangled)", kinds(key).join(","));
  // The MARKER tail, not the last row: `AssistantRunMeta` rides after the
  // `RunFinished` it bills (measured 2026-09-18: `…,run_finished,
  // assistant_run_meta`), so "the last event is the closer" is the wrong
  // shape. What settles the run is the last marker being its closer.
  const MARKERS = new Set(["run_started", "run_finished", "resume_attempted"]);
  const lastMarker = () => kinds(key).filter((t) => MARKERS.has(t)).at(-1);
  const answered = await until(
    () => countKind(key, "assistant_message") >= 1 && lastMarker() === "run_finished",
    120_000,
  );
  check(Boolean(answered), "the transcript carries an assistant answer and its last marker is the RunFinished", kinds(key).join(","));
  // A TOOL-SURFACED request logged AFTER the resume-ON boot: the turn itself.
  // Not "any request carrying the text" — the strategy planner's side channel
  // (`Task objective: …`, no tools) fires at run start, BEFORE the seed, so
  // the crashed turn already left one such request behind; under the T7
  // mutant (no resume at all) that predicate stayed green (measured
  // 2026-09-18). The mock files no-tools requests as side channels for the
  // same reason.
  const since = readBootMark().requestsLogged;
  const turns = requests()
    .slice(since)
    .filter((r) => Array.isArray(r.body?.tools) && r.body.tools.length > 0);
  check(
    turns.some((r) => userText(r.body).includes("hello, are you there")),
    "the resumed run put the original message in front of the model (a tool-surfaced request after the boot)",
    `${requests().length} requests logged, ${turns.length} tool-surfaced since the boot mark (${since})`,
  );
  const settled = await until(async () => {
    const { lastRun: lr } = await lastRunOf(conn, key);
    return lr?.disposition === "clean" ? lr : null;
  }, 60_000, 1000);
  check(Boolean(settled), "last_run settles to clean", show(settled));
  const h = await conn.attempt("chat.history", { session_key: key });
  const rows = h.result?.messages ?? [];
  check(rows.at(-1)?.role === "assistant", "chat.history's last row is the assistant", show(rows.at(-1)));
  conn.close();
}

/**
 * The §8.2(b) twin, on a SECOND session in the same home: a kill inside the
 * `BeforeAgentStart` hook, i.e. after `persist_run_task_started` wrote the
 * engine's task row and BEFORE the orchestrator seeded — so the message is
 * gone from every log the resume scan reads, and the only trace of it is
 * that row. The boot must tell the user, once, and stamp the row so the
 * next boot stays silent.
 *
 *   `send`        — send, wait for the task row (state.db), prove no seed.
 *   `after-kill`  — the same two facts, now that the process is gone.
 *   `first-boot`  — `notified=1`, ONE SystemMessage carrying the notice.
 *   `second-boot` — `notified=0`, STILL one (`adjudicated_at_ms` holds).
 */
const LOST_INPUT_TEXT = "was lost before it was recorded";
const NOTICE_PROMPT = "qa-notice this message will be lost before it is recorded";

const lostInputNotes = (key) =>
  rowsOfKind(key, "system_message").filter((r) => String(r.payload.content ?? "").includes(LOST_INPUT_TEXT));

async function cmdNotice(phase) {
  if (phase === "send") {
    const conn = new Conn("driver");
    await conn.open();
    const started = await sendTurn(conn, NOTICE_PROMPT, null, null, null);
    fs.writeFileSync(SESSION_FILE, started.session_key);
    const key = started.session_key;
    // The row is the instrument: without it there is nothing for the boot to
    // adjudicate and `notified=0` would be green for the wrong reason.
    const row = await until(() => taskRowsOf(key).find((t) => t.status === "running") || null, 30_000, 100);
    conn.close();
    if (!row) {
      console.error("INSTRUMENT FAILURE: no running agent_tasks row for the held turn (persist_run_task_started did not run?)");
      console.error(`  rows for ${key}: ${show(taskRowsOf(key))}`);
      process.exit(1);
    }
    await sleep(1000);
    if (countKind(key, "user_message") !== 0) {
      console.error("INSTRUMENT FAILURE: a user_message landed while the hook should have been holding the run");
      console.error(`  event types: ${kinds(key).join(",")}`);
      process.exit(1);
    }
    log(`held on ${key}: task ${row.id} running since ${iso(row.created_at * 1000)}, no user_message after 1000ms`);
    return;
  }
  const key = readSession();
  if (phase === "after-kill") {
    const rows = taskRowsOf(key);
    check(countKind(key, "user_message") === 0, "no user_message ever landed for the held turn", kinds(key).join(",") || "(no events)");
    check(countKind(key, "run_started") === 0, "and no run_started", kinds(key).join(",") || "(no events)");
    check(
      rows.length === 1 && rows[0].status === "running" && rows[0].task_prompt === NOTICE_PROMPT,
      "exactly one agent_tasks row for that session, still `running`, carrying the prompt",
      show(rows),
    );
    return;
  }
  const b = await awaitBootLineAfterMark();
  check(Boolean(b), `${phase}: the boot scan printed its line`, show(bootLines()));
  const notes = lostInputNotes(key);
  if (phase === "first-boot") {
    check(b?.notified === 1, "first boot: the scan wrote one lost-input notice (notified=1)", b?.raw);
    check(b?.resumed === 0, "and resumed nothing — there was no seed to resume", b?.raw);
    check(notes.length === 1, "exactly one SystemMessage on that session says the message was lost", show(kinds(key)));
    check(
      String(notes[0]?.payload.content ?? "").includes("«qa-notice this message"),
      "and quotes the head of the lost prompt so the user knows which one",
      show(notes[0]?.payload),
    );
    const row = taskRowsOf(key)[0];
    check(
      row?.status === "interrupted" && row?.adjudicated_at_ms !== null && row?.adjudicated_at_ms !== undefined,
      "the task row now reads `interrupted` and carries adjudicated_at_ms",
      show(row),
    );
    return;
  }
  // second-boot: idempotence.
  check(b?.notified === 0, "second boot: no further notice (notified=0)", b?.raw);
  check(notes.length === 1, "STILL exactly one lost-input SystemMessage on that session", show(kinds(key)));
}

// ---------------------------------------------------------------------------
// Stage `ratchet` (§5.1): `[resume] max_attempts` counts every crash AFTER the
// `ResumeAttempted` stamp, and abandons at the cap.
// ---------------------------------------------------------------------------

const RETRIGGER_LINE = "resume: re-triggering interrupted run";

/**
 * Wait until boot `n`'s resume is admitted and (by construction) held by the
 * `BeforeAgentStart` sleeper, so the shell can kill there.
 *
 * The signal is the engine's own `Agent execution started` line — the one it
 * logs right after `persist_run_task_started`, before the run loop reaches the
 * hook — one more of them than the boot mark recorded. NOT the boot-scan
 * line: `settle` joins the retrigger, the retrigger awaits the run, the run is
 * inside the hook — that line cannot print before the kill (see `bootLines`).
 * And deliberately NOT the stamp: waiting for the stamp here would turn the
 * T6 mutation (stamp moved after the retrigger) into a hold timeout instead
 * of the named red `cmdRatchet` carries for it. The grace covers the gap
 * between that line and the hook's spawn; a kill inside it is still before
 * `RunStarted` (a resume skips the seed, and the marker comes after the
 * prompt build).
 */
async function cmdRatchetHold(boot) {
  const n = Number(boot);
  const before = readBootMark().execStarted;
  const seen = await until(() => (logLines(EXEC_STARTED_LINE).length > before ? logLines(EXEC_STARTED_LINE).length : null), 120_000, 300);
  if (!seen) {
    console.error(
      `INSTRUMENT FAILURE: boot ${n}: the resumed run was never admitted (still ${before} '${EXEC_STARTED_LINE}' line(s))`,
    );
    console.error(`  boot lines: ${show(bootLines())}`);
    process.exit(1);
  }
  await sleep(1500);
  // Exactly one more, not "more": the only run this boot may admit is the
  // resume (driver detached, channels / cron / heartbeat off, no survivors).
  // A second one would be a run the stage did not ask for — it is re-read
  // after the grace so a late second admission is seen too.
  const now = logLines(EXEC_STARTED_LINE).length;
  if (now !== before + 1) {
    console.error(
      `INSTRUMENT FAILURE: boot ${n}: expected exactly one admitted run (${before} -> ${before + 1} '${EXEC_STARTED_LINE}' lines), saw ${now}`,
    );
    process.exit(1);
  }
  log(`boot ${n}: resumed run admitted (${before} -> ${now} '${EXEC_STARTED_LINE}' lines); the hook holds it`);
}

async function cmdRatchet(boot) {
  const key = readSession();
  const n = Number(boot);
  const stamps = countKind(key, "resume_attempted");
  if (n <= 2) {
    check(stamps === n, `boot ${n}: ${n} ResumeAttempted stamp(s) on the log — the ratchet moved BEFORE the retrigger`, kinds(key).join(","));
    check(
      logLines(RETRIGGER_LINE).length === n,
      `boot ${n}: ${n} retrigger(s) actually launched (the stamp is followed by a real re-run)`,
      show(logLines(RETRIGGER_LINE)),
    );
    check(countKind(key, "run_started") === 1, `boot ${n}: the resumed run never reached RunStarted (hook held it)`, kinds(key).join(","));
    check(
      bootLines().length === bootMark(),
      `boot ${n}: no boot-scan line for this boot — the scan was still joined on the held retrigger when the kill landed`,
      show(bootLines()),
    );
    check(abandonedCount(key) === 0, `boot ${n}: nothing abandoned yet`, kinds(key).join(","));
    return;
  }
  // Boots 3 and 4 settle (nothing is held), so their line arrives.
  const b = await awaitBootLineAfterMark();
  check(Boolean(b), `boot ${n}: the boot scan printed its line`, show(bootLines()));
  if (n === 3) {
    check(b?.resumed === 0 && b?.abandoned === 1, "boot 3: attempts == max_attempts(2) → abandoned=1, resumed=0", b?.raw);
    check(abandonedCount(key) === 1, "one RunFinished{abandoned} closer on the log", kinds(key).join(","));
    check(stamps === 2, "no third stamp", kinds(key).join(","));
    check(logLines(RETRIGGER_LINE).length === 2, "and no third retrigger", show(logLines(RETRIGGER_LINE)));
  } else {
    check(b?.resumed === 0 && b?.abandoned === 0, "boot 4: nothing to resume, nothing abandoned", b?.raw);
    check(abandonedCount(key) === 1, "boot 4: the closer is still the only one", kinds(key).join(","));
    check(stamps === 2, "boot 4: still two stamps", kinds(key).join(","));
  }
  // T16 on this log: every task row here is either the dangle turn (whose
  // seed DID land) or a resume re-trigger (`task_prompt == ""`); neither is
  // a lost message, so no notice may be written for them.
  check(b?.notified === 0, `boot ${n}: no lost-input notice for a resume proxy or a seeded turn (notified=0)`, b?.raw);
  check(lostInputNotes(key).length === 0, `boot ${n}: and no lost-input SystemMessage on the session`, show(kinds(key)));
}

// ---------------------------------------------------------------------------

const main = async () => {
  switch (CMD) {
    case "dangle":
      await cmdDangle(REST[0], REST[1], REST[2]);
      break;
    case "dangle-parked":
      await cmdDangleParked(REST[0]);
      break;
    case "assert-dangling":
      await cmdAssertDangling(REST[0] ?? 1);
      break;
    case "claims-wire":
      await cmdClaimsWire();
      break;
    case "claims-receipt":
      await cmdClaimsReceipt(REST[0]);
      break;
    case "forge-denial":
      cmdForgeDenial();
      break;
    case "denied":
      await cmdDenied(REST[0] ?? "wire");
      break;
    case "parked":
      await cmdParked(REST[0] ?? "wire", REST[1]);
      break;
    case "rewind":
      await cmdRewind(REST[0] ?? "do", REST[1]);
      break;
    case "knobs":
      await cmdKnobs(REST[0], REST[1], REST[2]);
      break;
    case "holes-settle":
      await cmdHolesSettle();
      break;
    case "holes":
      await cmdHoles(REST[0], REST[1] ?? "before");
      break;
    case "cost":
      await cmdCost();
      break;
    case "hooks":
      cmdHooks(REST[0] ?? 300_000);
      break;
    case "boot-mark":
      cmdBootMark();
      break;
    case "window":
      await cmdWindow(REST[0]);
      break;
    case "unanswered":
      await cmdUnanswered(REST[0] ?? "after-kill");
      break;
    case "notice":
      await cmdNotice(REST[0] ?? "send");
      break;
    case "ratchet-hold":
      await cmdRatchetHold(REST[0]);
      break;
    case "ratchet":
      await cmdRatchet(REST[0]);
      break;
    default:
      console.error(`unknown command: ${CMD}`);
      process.exit(2);
  }
  console.log(`\n${PASS} passed, ${FAIL} failed`);
  process.exit(FAIL === 0 ? 0 : 1);
};

main().catch((e) => {
  console.error(`driver error: ${e.stack || e.message}`);
  process.exit(1);
});
