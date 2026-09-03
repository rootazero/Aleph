// Real-machine driver for crash-recovery round 2 — the five stages `run.sh`
// adds on top of the round-1 `crash` / `attribute` pair.
//
//   drive_r2.mjs <gateway-port> <qa-root> <cmd> [args…]
//
// Node, not Python, for the same reason every fixture written on this host
// since 2026-08 is Node: there is no usable `python3` here (the Windows
// `WindowsApps` shim exits 0 having done nothing, so a Python fixture reports
// success while measuring nothing), and the gateway's only client transport is
// a WebSocket. The round-1 stages stay Python — they were measured on a host
// that had one, and rewriting a green fixture to prove nothing new is churn.
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
      let topic = null;
      let data = null;
      if (msg.method === "event" && msg.params) {
        topic = msg.params.topic ?? null;
        data = msg.params.data ?? msg.params;
      } else {
        topic = msg.topic ?? msg.method ?? null;
        data = msg.data ?? msg.params ?? null;
      }
      this.frames.push({ topic, data, raw: msg });
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
const sendTurn = async (conn, text, sessionKey) => {
  const params = { message: text, channel: CHANNEL };
  if (sessionKey) params.session_key = sessionKey;
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
async function cmdDangle(marker = "qa-dangle") {
  const conn = new Conn("driver");
  await conn.open();
  const prior = fs.existsSync(SESSION_FILE) ? readSession() : null;
  const started = await sendTurn(conn, `${marker} please run the long command`, prior);
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

/** Stage `rewind`: a rewind past a RunStarted must leave the marker tail balanced. */
async function cmdRewind(sub) {
  const key = readSession();
  const conn = new Conn("driver");
  await conn.open();
  if (sub === "do") {
    // `RewindParams` is `{session_key, seq}` — `seq` is the FIRST event to
    // retire, inclusive, not a count of messages. Aim it at the `RunStarted`
    // of the run that was cut off: that is the whole point of the stage, a
    // rewind that takes the marker's opening half away with it.
    const live = liveEvents(key);
    const started = [...live].reverse().find((r) => r.event_type === "run_started");
    if (!started) {
      console.error("INSTRUMENT FAILURE: no live run_started row to rewind past");
      console.error(`  event types: ${live.map((r) => r.event_type).join(",")}`);
      process.exit(1);
    }
    const before = live.length;
    const r = await conn.attempt("chat.rewind", { session_key: key, seq: started.seq });
    check(!r.error, "chat.rewind is accepted on a session whose run was cut off", show(r.error));
    const after = liveEvents(key).length;
    log(`live events ${before} -> ${after} (rewound at seq ${started.seq})`);
    check(
      after < before,
      "the rewind actually retired the tail — otherwise the balance below is vacuous",
      `${before} -> ${after}`,
    );
    check(
      Number(r.result?.events_retired ?? 0) === before - after,
      "and the reply's events_retired agrees with the log",
      show(r.result),
    );
    const { lastRun } = await lastRunOf(conn, key);
    check(
      lastRun?.disposition === "clean" || lastRun?.disposition === "never_ran",
      "after the rewind the marker tail is balanced — the log no longer claims an open run",
      show(lastRun),
    );
  } else {
    // After the restart: nothing to resume.
    const { lastRun } = await lastRunOf(conn, key);
    check(
      lastRun?.disposition === "clean" || lastRun?.disposition === "never_ran",
      "the rewound session still reads balanced after a restart",
      show(lastRun),
    );
  }
  conn.close();
}

/**
 * Stage `knobs`: the crashed run's SETTINGS come back, not today's.
 *
 * `sub=set` pins the session to model B and records what the session row says
 * now; `sub=assert` reads the request the resumed run put in front of the
 * provider and checks it carries the model the crashed run was executing
 * under — the envelope snapshot, not the session's current value.
 */
async function cmdKnobs(sub, arg) {
  const key = readSession();
  const conn = new Conn("driver");
  await conn.open();
  if (sub === "set") {
    // The move to model B has to be ASSERTED, not attempted. If this RPC is
    // not the knob (wrong method, wrong param, rejected), the session never
    // leaves model A and `assert` below then passes for the one reason it must
    // never pass for: nothing ever changed the model, so "the resumed run
    // still runs under A" is true of a build that dropped the envelope too.
    //
    // MEASURED 2026-09-03, and this stage is RED because of it: `session.update`
    // does not exist — the server answers `-32601 Method not found: session.update`
    // — so the green this stage reported before these two checks existed was
    // exactly the vacuous one described above. Three facts for whoever wires it:
    //
    //   * there is no `session.*` RPC that sets a model; the registry has
    //     artifact / compact / create / export_html / list / truncate / usage,
    //     and the metadata modify path REFUSES `model_pin` on purpose
    //     (`handlers/session/db_handlers/modify.rs:376` — "their legal writer
    //     is elsewhere"), so no amount of param-guessing here will land it;
    //   * the legal writer is the `select_model` TOOL (R8), which stamps
    //     `identity_meta.custom["model_pin"]` (`gateway/session_model_pin.rs`;
    //     the key is `providers::session_model_handle::MODEL_PIN_SESSION_KEY`).
    //     A tool means the MOCK has to dispatch it;
    //   * and it cannot simply be a second turn: the `ask` instrument leaves
    //     the session BUSY on a parked approval card, so a pin turn sent after
    //     the dangle turn queues behind it and dies with the server.
    //
    // Two routes are open, neither guessed at here: write
    // `identity_meta.custom.model_pin` into the session row with the server
    // down (the technique `cmdForgeDenial` already uses, for the same reason —
    // the state is reachable, the in-process path to it is not), or make the
    // dangle a different way so a `select_model` turn can precede the crash on
    // a session that is not parked.
    const r = await conn.attempt("session.update", { session_key: key, model: arg });
    check(!r.error, `session.update moves the session to ${arg}`, show(r.error));
    const { session } = await lastRunOf(conn, key);
    check(
      session?.model_pin === arg || session?.model === arg,
      `and the session row now reads ${arg} — the crashed run's snapshot still says A`,
      show({ model: session?.model ?? null, model_pin: session?.model_pin ?? null }),
    );
  } else {
    const wanted = arg;
    const after = requests();
    const resumed = after.filter((r) => userText(r.body).includes("OUTCOME UNKNOWN"));
    check(resumed.length > 0, "the resumed run reached the provider", `${after.length} requests logged`);
    const models = resumed.map((r) => r.body?.model);
    check(
      models.every((m) => m === wanted),
      `the resumed run runs under the SNAPSHOT model (${wanted}), not the session's current one`,
      show(models),
    );
  }
  conn.close();
}

/**
 * Stage `holes`: a burst that outruns the projector queue must not lose a
 * transcript row, and must not bill the same run twice.
 *
 * The equality is the claim; the deferral is an OBSERVATION and is printed
 * with its number, because a burst that never filled the queue makes the
 * "deferred" half vacuous and a vacuous green is the thing this repo keeps
 * paying for.
 */
async function cmdHoles(serverLog) {
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
  log(`history rows ${msgs.length}; projectable events ${projectable}; total events ${rows.length}`);
  check(
    Array.isArray(msgs) && msgs.length > 0,
    "the burst session has a transcript at all",
    show(h.result ?? h.error, 300),
  );
  check(
    msgs.length >= projectable,
    "no projectable event is missing from the transcript after the burst",
    `history ${msgs.length} < projectable ${projectable}`,
  );
  let deferred = 0;
  try {
    deferred = (fs.readFileSync(serverLog, "utf8").match(/seq deferred/g) || []).length;
  } catch {
    deferred = 0;
  }
  log(`OBSERVATION projector deferrals in this run: ${deferred}` + (deferred === 0 ? " (the queue never filled — the deferral half of this stage is vacuous at this burst size)" : ""));
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
      await cmdDangle(REST[0]);
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
      await cmdRewind(REST[0] ?? "do");
      break;
    case "knobs":
      await cmdKnobs(REST[0], REST[1]);
      break;
    case "holes":
      await cmdHoles(REST[0]);
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
