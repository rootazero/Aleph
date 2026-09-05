// Real-machine driver for crash-recovery round 2 — the five stages `run.sh`
// adds on top of the round-1 `crash` / `attribute` pair.
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

const EVENTS_DB = path.join(QA_ROOT, "home", ".aleph", "data", "sessions.db");
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

const show = (v, max = 700) => (JSON.stringify(v ?? null) ?? "null").slice(0, max);

const readSession = () => fs.readFileSync(SESSION_FILE, "utf8").trim();

/** `chat.history`, and the `session.last_run` a Panel/TUI renderer reads off it. */
const lastRunOf = async (conn, sessionKey) => {
  const r = await conn.attempt("chat.history", { session_key: sessionKey });
  const session = r.result?.session ?? null;
  return { reply: r, session, lastRun: session?.last_run ?? null };
};

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
  const prior = fs.existsSync(SESSION_FILE) ? readSession() : null;
  const started = await sendTurn(conn, `${marker} please run the long command`, prior, model, tier);
  fs.writeFileSync(SESSION_FILE, started.session_key);
  const landed = await until(() => danglingIds(started.session_key).length > 0, 180_000, 400);
  conn.close();
  if (!landed) {
    console.error("INSTRUMENT FAILURE: no dangling dispatch ever reached the durable log");
    console.error(`  events for ${started.session_key}: ${eventsOf(started.session_key).length}`);
    process.exit(1);
  }
  log(`dangling now: ${danglingIds(started.session_key).join(",")}`);
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
      const hits = requests().filter((r) => userText(r.body).includes("OUTCOME UNKNOWN"));
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

const main = async () => {
  switch (CMD) {
    case "dangle":
      await cmdDangle(REST[0], REST[1], REST[2]);
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
