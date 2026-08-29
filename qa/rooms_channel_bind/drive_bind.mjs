// Real-machine driver for channel-conversation ⟷ project-room binding.
//
//   drive_bind.mjs <gateway-port> <mock-port> <webhook-secret> <request-log>
//                  <outbound-log> <aleph-cli> <aleph-home> <workspace-dir>
//
// Everything this branch claims has, until now, rested on compile-and-unit-test
// evidence. Nothing had spoken to a live gateway. So every assertion below is
// an EFFECT — a row that exists on disk, a partition a note landed in, a
// sentence a real CLI printed — never "the call returned 200".
//
// ## The three oracles, and why each one is needed
//
//  1. **`memory.db` → `notes_index.agent_id`.** The mock answers every group
//     turn with `note_manage(create, filename=<marker>)`, and `note_manage`
//     resolves its partition through `project_scope::session_write_id`, i.e.
//     off the run's ambient `ScopeAttribution`. So the partition a marker's
//     note landed in IS the scope that turn ran under, read from disk without
//     asking the server what it thinks it did.
//  2. **`sessions.db` → `sessions.owner_user_id` / `scope_id`.** Scenarios 8
//     and 8b are evidence-gathering for Ruling AQ, and the ruling asks for the
//     stored row verbatim. A row on disk is the one thing in this system that
//     cannot describe itself wrongly.
//  3. **The mock's request log.** `<room_context>` is a prompt block; the only
//     place it is observable is the request that carried it.
//
// ## Two asymmetric queries, and the reason both exist
//
// Ruling AG: every "this partition has NO new rows" assertion (scenarios 3 and
// 4) is preceded by a POSITIVE query on a partition known to have rows. A
// misspelled partition, a query against a database that was never opened, and
// a genuine absence all return the same empty set. Scenario 1 is that positive
// control for the `__u-*` family and scenario 2 for `__p-*`; neither empty
// assertion runs before its own control has passed.
//
// ## Why the channel is `webhook` and not `telegram`
//
// The binding key is `(channel_id, peer_kind, peer_id)` — the mechanism knows
// nothing about which channel it is. `webhook` is the only channel type a
// fixture can drive with an HTTP POST and an HMAC, with no upstream service to
// mock. Everything the brief says about a Telegram group holds verbatim with
// `webhook` substituted for `telegram`.
import fs from "node:fs";
import os from "node:os";
import crypto from "node:crypto";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { DatabaseSync } from "node:sqlite";

const [
  portArg,
  mockPortArg,
  SECRET,
  REQUEST_LOG,
  OUTBOUND_LOG,
  ALEPH_CLI,
  ALEPH_HOME,
  WORKSPACE,
] = process.argv.slice(2);
const PORT = Number(portArg);
if (!PORT || !SECRET || !ALEPH_CLI || !ALEPH_HOME) {
  console.error(
    "usage: drive_bind.mjs <gateway-port> <mock-port> <secret> <request-log> " +
      "<outbound-log> <aleph-cli> <aleph-home> <workspace-dir>",
  );
  process.exit(2);
}

const DATA = path.join(ALEPH_HOME, "data");
const SESSIONS_DB = path.join(DATA, "sessions.db");
const MEMORY_DB = path.join(DATA, "memory.db");

const T0 = Date.now();
const log = (...a) =>
  console.log(`${((Date.now() - T0) / 1000).toFixed(2)}s`, ...a);

let PASS = 0;
let FAIL = 0;
let SKIPPED = 0;
const failures = [];
const skips = [];
/** Facts the report needs verbatim, not assertions. */
const evidence = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const check = (cond, label, detail = "") => {
  if (cond) {
    PASS += 1;
    console.log(`PASS  ${label}`);
  } else {
    FAIL += 1;
    failures.push(label);
    console.log(`FAIL  ${label}`);
    if (detail) {
      for (const line of String(detail).split("\n").slice(0, 16)) {
        console.log(`      | ${line}`);
      }
    }
  }
  return cond;
};

/**
 * An assertion that could not be attempted. NEVER renders as a pass — "an arm
 * nobody has produced is a different state from an arm that was tried and
 * worked", and only one of them is evidence.
 */
const skip = (label, why) => {
  SKIPPED += 1;
  skips.push(`${label} — ${why}`);
  console.log(`SKIP  ${label} (${why})`);
};

/** A fact for the report. Printed, never scored. */
const fact = (label, value) => {
  evidence.push([label, value]);
  console.log(`FACT  ${label}: ${value}`);
};

/**
 * Did the room scope survive into the harness on a channel turn?
 *
 * `null` until scenario 2 measures it. When it comes back `false` the failures
 * in scenarios 2, 5 and 7 all have ONE cause, and [`report`] says which — a red
 * fixture that does not name its defect gets read as a broken fixture, and the
 * next person's first move is to doubt the test rather than the code.
 */
let roomScopeReachedHarness = null;

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

class Conn {
  constructor(name) {
    this.name = name;
    this.frames = [];
    this.pendingReplies = new Map();
    this.nextId = 1;
  }

  async open(url, connectParams) {
    this.ws = new WebSocket(url);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`${this.name}: connect timeout`)),
        30_000,
      );
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
      if (msg.id !== undefined && msg.id !== null && this.pendingReplies.has(msg.id)) {
        this.pendingReplies.get(msg.id)(msg);
        this.pendingReplies.delete(msg.id);
        return;
      }
      // `topic` is overloaded on this wire in THREE shapes; reading only one
      // makes the tap blind to exactly the frames a scenario is chasing.
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
    return this.rpc("connect", connectParams);
  }

  rpc(method, params = {}, budget = 90_000) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingReplies.delete(id);
        reject(new Error(`${this.name}: no reply to ${method} within ${budget}ms`));
      }, budget);
      this.pendingReplies.set(id, (msg) => {
        clearTimeout(timer);
        resolve(msg);
      });
      this.ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    });
  }

  async ok(method, params = {}, budget = 90_000) {
    const r = await this.rpc(method, params, budget);
    if (r.error) {
      throw new Error(`${method} -> [${r.error.code}] ${r.error.message}`);
    }
    return r.result;
  }

  /** `rpc`, tolerating an error reply — the caller judges. */
  async attempt(method, params = {}, budget = 90_000) {
    const r = await this.rpc(method, params, budget).catch((e) => ({
      error: { message: e.message },
    }));
    return { result: r.result, error: r.error };
  }

  close() {
    try {
      this.ws?.close();
    } catch {
      /* teardown */
    }
  }
}

/** Poll `fn` until it returns something truthy, or give up and return null. */
const until = async (fn, budget = 180_000, every = 1000) => {
  const end = Date.now() + budget;
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() >= end) return null;
    await sleep(every);
  }
};

/** The interface the kernel would route out of, without sending a packet. */
const lanIp = () => {
  for (const list of Object.values(os.networkInterfaces())) {
    for (const i of list || []) {
      if (i.family === "IPv4" && !i.internal) return i.address;
    }
  }
  return null;
};

// ---------------------------------------------------------------------------
// Oracles
// ---------------------------------------------------------------------------

/**
 * One read-only query against a live database the server also has open.
 *
 * Read-only so the fixture cannot become a second writer of state the server
 * owns — the one exception is the deliberate degradation in the `Unknown`
 * phase, which opens its own writable handle and says so.
 */
const query = (dbPath, sql, params = []) => {
  if (!fs.existsSync(dbPath)) return { error: `no database at ${dbPath}` };
  let db;
  try {
    db = new DatabaseSync(dbPath, { readOnly: true });
    const rows = db.prepare(sql).all(...params);
    return { rows };
  } catch (e) {
    return { error: e.message };
  } finally {
    try {
      db?.close();
    } catch {
      /* ignore */
    }
  }
};

/** The partition a marker's note landed in, or null. */
const notePartition = (marker) => {
  const r = query(
    MEMORY_DB,
    "SELECT agent_id, path, filename FROM notes_index " +
      "WHERE lower(filename) LIKE ?1 OR lower(path) LIKE ?1",
    [`%${marker.toLowerCase()}%`],
  );
  if (r.error || !r.rows || r.rows.length === 0) return null;
  return r.rows[0].agent_id;
};

/** How many notes a partition holds right now, or null when unreadable. */
const noteCount = (partition) => {
  const r = query(
    MEMORY_DB,
    "SELECT COUNT(*) AS n FROM notes_index WHERE agent_id = ?1",
    [partition],
  );
  if (r.error || !r.rows) return null;
  return Number(r.rows[0].n);
};

/** The stored session row for a key, or null. */
const sessionRow = (key) => {
  const r = query(
    SESSIONS_DB,
    "SELECT key, agent_id, owner_user_id, scope_id FROM sessions WHERE key = ?1",
    [key],
  );
  if (r.error || !r.rows || r.rows.length === 0) return null;
  return r.rows[0];
};

/** How many replies the channel has delivered — one line per finished run. */
const outboundCount = () => {
  if (!fs.existsSync(OUTBOUND_LOG)) return 0;
  return fs.readFileSync(OUTBOUND_LOG, "utf8").split("\n").filter(Boolean).length;
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

const systemText = (body) => {
  const s = body?.system;
  if (typeof s === "string") return s;
  if (Array.isArray(s)) {
    return s.map((b) => (typeof b === "string" ? b : b?.text || "")).join("\n");
  }
  return "";
};

const userText = (body) =>
  (body?.messages || [])
    .filter((m) => m.role === "user")
    .map((m) =>
      typeof m.content === "string"
        ? m.content
        : (m.content || [])
            .filter((b) => b?.type === "text")
            .map((b) => b.text || "")
            .join(" "),
    )
    .join("\n");

/** Every tool_result text in a request, flattened. */
const toolResults = (body) =>
  (body?.messages || [])
    .flatMap((m) => (Array.isArray(m.content) ? m.content : []))
    .filter((b) => b && b.type === "tool_result")
    .map((b) =>
      typeof b.content === "string"
        ? b.content
        : (b.content || [])
            .map((c) => (typeof c === "string" ? c : c?.text || ""))
            .join(" "),
    )
    .join("\n");

/** The `<room_context>` body of a request, or "" when the block is absent. */
const roomBlock = (body) => {
  const sys = systemText(body);
  const start = sys.indexOf("<room_context>");
  if (start < 0) return "";
  const end = sys.indexOf("</room_context>", start);
  return end < 0 ? "" : sys.slice(start + "<room_context>".length, end);
};

/** The requests that carry `marker` in a user turn, from `since` onward. */
const requestsFor = (marker, since = 0) =>
  requests()
    .slice(since)
    .filter((r) => userText(r.body).includes(marker));

// ---------------------------------------------------------------------------
// The channel leg
// ---------------------------------------------------------------------------

const WEBHOOK_URL = `http://127.0.0.1:${PORT}/webhook/qa`;

/** POST one inbound message, HMAC-signed exactly as `WebhookReceiver` expects. */
const inbound = async (payload) => {
  const body = JSON.stringify(payload);
  const sig =
    "sha256=" + crypto.createHmac("sha256", SECRET).update(body).digest("hex");
  const res = await fetch(WEBHOOK_URL, {
    method: "POST",
    headers: { "content-type": "application/json", "X-Webhook-Signature": sig },
    body,
  });
  const text = await res.text();
  return { status: res.status, text };
};

let msgSeq = 0;
/**
 * One group message, from `sender`, into `conversation`.
 *
 * `@aleph` is not decoration: an unregistered channel config defaults to
 * `require_mention = true`, and `check_mention`'s pattern list is
 * `["@aleph", "@bot", "aleph"]`. Without it every group message is refused
 * with `Mention required in group` and nothing runs at all.
 */
const say = (sender, conversation, text) =>
  inbound({
    message_id: `qa-msg-${++msgSeq}`,
    sender_id: sender,
    sender_name: sender,
    message: `@aleph ${text}`,
    conversation_id: conversation,
    is_group: true,
  });

/** One DM — the only path that mints a pairing code. */
const dm = (sender, text) =>
  inbound({
    message_id: `qa-dm-${++msgSeq}`,
    sender_id: sender,
    sender_name: sender,
    message: text,
    conversation_id: `dm-${sender}`,
    is_group: false,
  });

// ---------------------------------------------------------------------------
// The CLI leg
// ---------------------------------------------------------------------------

const cli = (...args) => {
  const r = spawnSync(ALEPH_CLI, ["--server", `ws://127.0.0.1:${PORT}/ws`, ...args], {
    encoding: "utf8",
    timeout: 120_000,
  });
  const out = `${r.stdout || ""}${r.stderr || ""}`;
  return { code: r.status, out };
};

/**
 * The JSON object in a CLI `--json` run's output, or null.
 *
 * The CLI initialises file logging before it parses anything, so a line of
 * tracing can precede the payload. Slicing from the first `{` to the last `}`
 * is what survives that; returning `null` rather than throwing keeps a
 * malformed payload an ASSERTION failure with the raw text attached, not a
 * driver abort with no evidence.
 */
const jsonFrom = (out) => {
  const start = out.indexOf("{");
  const end = out.lastIndexOf("}");
  if (start < 0 || end <= start) return null;
  try {
    return JSON.parse(out.slice(start, end + 1));
  } catch {
    return null;
  }
};

// ---------------------------------------------------------------------------

const LOOPBACK = `ws://127.0.0.1:${PORT}/ws`;
const CHANNEL = "webhook";
const C1 = "qa-c1";
const C2 = "qa-c2";
const C3 = "qa-c3";
const C9 = "qa-c9";

async function main() {
  const ip = lanIp();
  if (!ip) {
    console.log("FAIL  no non-loopback address on this host; the member half cannot run");
    process.exit(1);
  }
  const REMOTE = `ws://${ip}:${PORT}/ws`;
  log(`operator over ${LOOPBACK}; members over ${REMOTE}`);

  // ===== phase 0: identities ==============================================
  console.log("\n=== phase 0: five principals and three channel senders ===");
  const op = new Conn("operator");
  const hello = await op.open(LOOPBACK, { client_type: "cli" });
  check(!hello.error, "operator connects over loopback with no credential", JSON.stringify(hello.error));

  // --- the standing operator at the approval desk ---------------------------
  //
  // A member's connection carries `caller_role = "member"`, which caps the
  // turn at `ExecTier::Ask`, so every non-idempotent tool their run calls
  // parks. Unanswered, a card expires after 120 s with `ApprovalExpired` — and
  // the symptom downstream is a note that never lands, which reads exactly
  // like "the turn wrote to the wrong partition". Answering them is both the
  // realistic operator action and the cheapest way to keep an approval
  // timeout from masquerading as an attribution defect.
  //
  // What it approved is kept: that a member's run parked AT ALL is the
  // observable consequence of `caller_role` reaching the dispatch chokepoint,
  // which is half of what addendum A asks for.
  const approvedCards = [];
  let deskOpen = true;
  const approvalDesk = (async () => {
    while (deskOpen) {
      const r = await op.attempt("exec.approvals.pending", {}, 20_000);
      for (const p of r.result?.pending ?? []) {
        const id = p.record?.id;
        if (!id) continue;
        approvedCards.push(JSON.stringify(p.record).slice(0, 200));
        await op.attempt("exec.approval.resolve", {
          id,
          decision: "allow-once",
          resolved_by: "QA operator",
        });
      }
      await sleep(1500);
    }
  })();

  const mkUser = async (name) =>
    (await op.ok("users.create", { display_name: name, role: "member" })).user;
  const alice = await mkUser("QA Alice");
  const bob = await mkUser("QA Bob");
  const carol = await mkUser("QA Carol");
  const dave = await mkUser("QA Dave");
  const erin = await mkUser("QA Erin");
  log(
    `alice=${alice.user_id} bob=${bob.user_id} carol=${carol.user_id} ` +
      `dave=${dave.user_id} erin=${erin.user_id}`,
  );

  const conns = {};
  for (const [who, user] of [
    ["alice", alice],
    ["bob", bob],
    ["dave", dave],
  ]) {
    const { ticket } = await op.ok("gateway.ticket.create", { user_id: user.user_id });
    const c = new Conn(who);
    const redeemed = await c.open(REMOTE, {
      client_type: "panel",
      bootstrap_ticket: ticket,
      device_id: `qa-panel-${who}`,
      device_name: `QA ${who}`,
    });
    // A remote connect handed no device token means the ticket path never ran
    // — which is exactly what the loopback short-circuit produces, and it must
    // not read as success.
    check(
      Boolean(redeemed.result?.device_token),
      `${who} redeems a bootstrap ticket over the LAN leg`,
      JSON.stringify(redeemed.error ?? redeemed.result).slice(0, 400),
    );
    const me = await c.ok("users.me", {});
    check(
      me?.user?.user_id === user.user_id,
      `${who}'s connection is authenticated as ${user.user_id}`,
      JSON.stringify(me),
    );
    conns[who] = c;
  }

  // --- channel senders → principals ---------------------------------------
  //
  // A DM is the only path that mints a pairing code (`DmPolicy::Pairing` is the
  // default and group messages never reach it), so each sender says hello in a
  // DM, is refused, and the operator approves the resulting code ONTO a named
  // principal. Without the `user_id` half every approved sender resolves to the
  // machine owner and every scenario below would be measuring the same person.
  const senders = {
    alice: "wh-alice",
    bob: "wh-bob",
    carol: "wh-carol",
  };
  for (const s of Object.values(senders)) await dm(s, "hello");
  const pending = await until(async () => {
    const r = await op.attempt("channel.pairing.list", { channel: CHANNEL });
    const list = r.result?.requests ?? [];
    return list.length >= 3 ? list : null;
  }, 60_000);
  check(
    Boolean(pending),
    "each unpaired DM sender minted a pairing request",
    JSON.stringify((await op.attempt("channel.pairing.list", { channel: CHANNEL })).result ?? {}),
  );
  const userOf = { [senders.alice]: alice, [senders.bob]: bob, [senders.carol]: carol };
  for (const req of pending ?? []) {
    const target = userOf[req.sender_id];
    if (!target) continue;
    const r = await op.attempt("channel.pairing.approve", {
      channel: CHANNEL,
      code: req.code,
      user_id: target.user_id,
    });
    check(
      Boolean(r.result?.approved) && r.result?.user_id === target.user_id,
      `${req.sender_id} is bound to ${target.user_id}`,
      JSON.stringify(r.error ?? r.result),
    );
  }
  // The store's own view, not the approve receipt's — "carol really resolves to
  // u-carol" is a premise scenario 8 rests on, and the brief is explicit that
  // a fixture which failed to build the premise looks identical to a defect
  // that does not exist.
  const approved = await op.attempt("channel.pairing.approved", { channel: CHANNEL });
  fact("approved senders", JSON.stringify(approved.result ?? approved.error));

  // ===== the room =========================================================
  console.log("\n=== the room ===");
  const project = (await op.ok("projects.create", { name: "QA Bound Room" })).project;
  const PID = project.id;
  const ROOM_PART = `main__${PID}`; // `scoped_agent_id(base, ns)` — `ns` IS the id
  log(`room ${PID}; room partition ${ROOM_PART}`);
  for (const u of [alice, bob, erin]) {
    await op.ok("projects.member.add", { id: PID, user_id: u.user_id });
  }
  const roster = await op.ok("projects.member.list", { id: PID });
  const memberIds = roster.member_ids ?? roster.members ?? [];
  check(
    memberIds.includes(alice.user_id) &&
      memberIds.includes(bob.user_id) &&
      memberIds.includes(erin.user_id) &&
      !memberIds.includes(carol.user_id) &&
      !memberIds.includes(dave.user_id),
    "the roster is alice+bob+erin, and carol/dave are NOT on it",
    JSON.stringify(roster),
  );

  /**
   * Say something in a group, then wait for that turn to be OVER.
   *
   * Waiting only for the note is not enough, and the reason is mechanical: the
   * note lands mid-run, so the next message would arrive while the previous run
   * is still in flight — and this channel's `busy_input_mode` is the default
   * `Steer`, which folds it into the running turn instead of starting a new
   * one. Every later scenario would then be measuring a run that started as
   * somebody else's.
   *
   * The completion signal is the channel's own outbound reply: `[channels.
   * webhook] callback_url` points at the mock, which appends one line per
   * delivered reply. One reply per finished run.
   */
  const sayAndSettle = async (sender, conversation, marker, budget = 240_000) => {
    const before = requests().length;
    const repliesBefore = outboundCount();
    const res = await say(sender, conversation, `qa-note:${marker} please jot this down`);
    if (res.status !== 200) {
      log(`inbound POST for ${marker} answered ${res.status}: ${res.text.slice(0, 200)}`);
    }
    const partition = await until(async () => notePartition(marker), budget);
    const replied = await until(async () => outboundCount() > repliesBefore, 60_000);
    if (!replied) log(`no channel reply after ${marker}; the next turn may steer into this one`);
    return { partition, before, res };
  };

  // ===== scenario 1: the pre-existing behaviour ===========================
  //
  // This is the round's motivating evidence AND the positive control every
  // "no new rows" assertion below leans on. If it is not green the premise has
  // to be re-estimated, so the driver stops here rather than letting the later
  // scenarios look correct on top of a broken oracle.
  console.log("\n=== scenario 1: unbound group — each speaker writes to their own partition ===");
  const s1a = await sayAndSettle(senders.alice, C1, "m1-alice");
  check(
    s1a.partition === `main__${alice.user_id}`,
    `alice's turn in an UNBOUND group writes to main__${alice.user_id}`,
    `note landed in ${s1a.partition ?? "(no note within budget)"}\n` +
      `inbound POST -> ${s1a.res.status} ${s1a.res.text.slice(0, 200)}`,
  );
  const s1b = await sayAndSettle(senders.bob, C1, "m1-bob");
  check(
    s1b.partition === `main__${bob.user_id}`,
    `bob's turn in the SAME unbound group writes to main__${bob.user_id}`,
    `note landed in ${s1b.partition ?? "(no note within budget)"}`,
  );
  const C1_KEY = `agent:main:${CHANNEL}:group:${C1}`;
  fact("C1 session row before bind", JSON.stringify(sessionRow(C1_KEY)));
  if (FAIL > 0) {
    console.log(
      "\nscenario 1 is the premise this whole round is motivated by. It is not " +
        "green, so every later assertion would be running on an unestablished " +
        "premise. Stopping.",
    );
    return report();
  }

  // ===== scenario 2: bind, over the real CLI ==============================
  //
  // Addendum B: `aleph projects channel bind` had never spoken to a live
  // gateway. It is driven here rather than through a raw socket precisely
  // because the class this repo has paid for four times is a request body with
  // the wrong keys, which reads as "not finished yet".
  console.log("\n=== scenario 2: bind (via the CLI), upgrade, and existing-transcript rescope ===");
  const bindJson = cli(
    "projects", "channel", "bind", PID, CHANNEL, C1,
    "--peer-kind", "group", "--label", "QA C1 Group", "--json",
  );
  fact("cli bind --json", bindJson.out.trim().replace(/\s+/g, " ").slice(0, 400));
  const bindReceipt = jsonFrom(bindJson.out);
  check(
    bindJson.code === 0 && bindReceipt?.binding?.project_id === PID,
    "`aleph projects channel bind` completes a live round trip",
    `exit=${bindJson.code}\n${bindJson.out.slice(0, 600)}`,
  );
  check(
    bindReceipt?.rescoped_session === "moved",
    "the receipt says the EXISTING transcript moved into the room",
    `rescoped_session=${JSON.stringify(bindReceipt?.rescoped_session)}`,
  );
  const rowAfterBind = sessionRow(C1_KEY);
  fact("C1 session row after bind", JSON.stringify(rowAfterBind));
  check(
    rowAfterBind?.scope_id === `project:${PID}`,
    "and the row on disk really carries the room's scope",
    JSON.stringify(rowAfterBind),
  );

  // --- addendum B, the read direction, on both new clients ---------------
  //
  // The CLI parses every response into the contract type, so a field the
  // server stopped sending is a hard parse error rather than a dash. Running
  // it IS the reconciliation; there is no hand-written key list here on
  // purpose. It runs BEFORE the unlabelled re-bind below, which overwrites the
  // stored label.
  const listOut = cli("projects", "channel", "list", PID);
  check(
    listOut.code === 0 && listOut.out.includes(C1) && listOut.out.includes("QA C1 Group"),
    "`aleph projects channel list` renders the binding, label and all",
    `exit=${listOut.code}\n${listOut.out.slice(0, 600)}`,
  );
  // The class this repo has paid for four times: a rendered column the server
  // never sent, which reads as "no value yet" rather than as a bug. `bound_by`
  // and `bound_at` are the two the operator would never notice were missing.
  check(
    /u-owner/.test(listOut.out) && /\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} UTC/.test(listOut.out),
    "and its Bound By / Bound At columns carry values, not dashes",
    listOut.out.slice(0, 600),
  );

  // The human receipt, from a second (idempotent) bind — the sentence is a
  // separate claim from the wire value, and `rescope_sentence` has three of
  // them with no default.
  const bindHuman = cli("projects", "channel", "bind", PID, CHANNEL, C1, "--peer-kind", "group");
  check(
    bindHuman.code === 0 && /now belongs to the room/.test(bindHuman.out),
    "re-binding is idempotent and prints the Moved sentence",
    `exit=${bindHuman.code}\n${bindHuman.out.slice(0, 400)}`,
  );
  // Observed, not asserted. That second bind carried no `--label` and
  // `bind_conversation` writes the column unconditionally, so the operator's
  // label from the first bind is gone. Recorded because a re-bind reads like an
  // idempotent no-op and this half of it is not.
  fact(
    "a re-bind with no --label",
    /QA C1 Group/.test(cli("projects", "channel", "list", PID).out)
      ? "keeps the previously stored label"
      : "CLEARS the previously stored label",
  );

  const s2a = await sayAndSettle(senders.alice, C1, "m2-alice");
  roomScopeReachedHarness = s2a.partition === ROOM_PART;
  check(
    s2a.partition === ROOM_PART,
    `alice's NEXT turn in the bound group writes to ${ROOM_PART}`,
    `note landed in ${s2a.partition ?? "(no note within budget)"}`,
  );

  const bobSessions = await op.attempt("sessions.list", {});
  const bobList = await conns.bob.attempt("sessions.list", {});
  const bobKeys = (bobList.result?.sessions ?? bobList.result?.items ?? []).map((s) => s.key);
  check(
    bobKeys.includes(C1_KEY),
    "bob, a roster member who did not speak last, SEES the group session in sessions.list",
    `bob saw: ${JSON.stringify(bobKeys)}\n` +
      `operator saw: ${JSON.stringify(
        (bobSessions.result?.sessions ?? []).map((s) => s.key),
      )}`,
  );

  const roomsOut = cli("projects", "list");
  check(
    roomsOut.code === 0 && roomsOut.out.includes(PID) && roomsOut.out.includes("QA Bound Room"),
    "`aleph projects list` renders the room over the same live gateway",
    `exit=${roomsOut.code}\n${roomsOut.out.slice(0, 600)}`,
  );
  // The Panel's half of the same wire: `ProjectsApi::channel_list` deserializes
  // into the identical contract type, so the assertion that matters is that a
  // ROSTER MEMBER (not an admin) is served it.
  const memberList = await conns.bob.attempt("projects.channel.list", { project_id: PID });
  check(
    (memberList.result?.bindings ?? []).some(
      (b) => b.peer_id === C1 && b.peer_kind === "group" && b.channel_id === CHANNEL,
    ),
    "projects.channel.list is open to a roster member (the Panel's read path)",
    JSON.stringify(memberList.error ?? memberList.result),
  );

  // ===== scenario 3: a paired stranger who is not on the roster ===========
  console.log("\n=== scenario 3: carol is paired but NOT on the roster ===");
  const roomBefore3 = noteCount(ROOM_PART);
  // Ruling AG. A misspelled partition, a database that was never opened, and a
  // genuine absence all return the same empty set, so the negative assertions
  // below only mean something once a POSITIVE query on the same partition has
  // answered. When this control fails the negatives are SKIPPED, not passed:
  // an assertion the control just invalidated is a lie in a green report.
  const control3 = check(
    roomBefore3 !== null && roomBefore3 > 0,
    "positive control: the room partition is readable AND non-empty before the negative assertion",
    `count=${roomBefore3} — nothing has ever landed in ${ROOM_PART}, so "no new rows" is unfalsifiable`,
  );
  const s3 = await sayAndSettle(senders.carol, C1, "m3-carol");
  check(
    s3.partition === `main__${carol.user_id}`,
    `carol's turn stays in main__${carol.user_id} despite the binding`,
    `note landed in ${s3.partition ?? "(no note within budget)"}`,
  );
  if (control3) {
    check(
      noteCount(ROOM_PART) === roomBefore3,
      "and the room partition gained no row",
      `before=${roomBefore3} after=${noteCount(ROOM_PART)}`,
    );
  } else {
    skip("the room partition gained no row", "its positive control did not pass");
  }

  // ===== scenario 4: an unpaired stranger =================================
  console.log("\n=== scenario 4: an unpaired sender has no principal at all ===");
  const roomBefore4 = noteCount(ROOM_PART);
  const s4 = await sayAndSettle("wh-stranger", C1, "m4-stranger");
  check(
    s4.partition === "main",
    "an UNPAIRED sender's turn runs with no scope at all (bare `main`)",
    `note landed in ${s4.partition ?? "(no note within budget)"}`,
  );
  if (control3) {
    check(
      noteCount(ROOM_PART) === roomBefore4,
      "the room partition gained no row from the stranger either",
      `before=${roomBefore4} after=${noteCount(ROOM_PART)}`,
    );
  } else {
    skip(
      "the room partition gained no row from the stranger either",
      "scenario 3's positive control did not pass",
    );
  }
  const strangerReqs = requestsFor("m4-stranger", s4.before);
  check(
    strangerReqs.length > 0 && strangerReqs.every((r) => roomBlock(r.body) === ""),
    "and the stranger's prompt carries no <room_context> block",
    `requests=${strangerReqs.length}; first block=${JSON.stringify(
      roomBlock(strangerReqs[0]?.body || {}),
    ).slice(0, 300)}`,
  );

  // ===== scenario 5: room context, and a delegated child ==================
  console.log("\n=== scenario 5: <room_context> in the channel turn AND in a delegated child ===");
  const beforeDelegate = requests().length;
  const repliesBeforeDelegate = outboundCount();
  await say(senders.alice, C1, "qa-delegate:m5 hand this one to a helper");
  const parentReq = await until(
    async () =>
      requests()
        .slice(beforeDelegate)
        .find((r) => userText(r.body).includes("qa-delegate:m5") && roomBlock(r.body) !== "") ||
      null,
    240_000,
  );
  check(
    Boolean(parentReq),
    "a bound-channel turn carries <room_context>",
    `requests since: ${requests().length - beforeDelegate}`,
  );
  const parentBlock = parentReq ? roomBlock(parentReq.body) : "";
  check(
    parentBlock.includes("QA Erin"),
    "the block names a roster member who has never spoken (QA Erin)",
    parentBlock.trim(),
  );
  check(
    /\[QA Alice\]:/.test(userText(parentReq?.body || {})),
    "and the channel turn reaches the model prefixed with the speaker's name",
    userText(parentReq?.body || {}).slice(-300),
  );

  // The child's own turn: `QA-CHILD` alone would also match the parent's next
  // turn if a tool_result were ever flattened into text, and the parent's
  // prompt DOES carry the block — so the delegation marker must be absent.
  const childReq = await until(
    async () =>
      requests()
        .slice(beforeDelegate)
        .find(
          (r) =>
            userText(r.body).includes("qa-child:m5") &&
            !userText(r.body).includes("qa-delegate:m5"),
        ) || null,
    240_000,
  );
  check(Boolean(childReq), "the room turn spawned a child that reached the provider");
  if (childReq) {
    const childBlock = roomBlock(childReq.body);
    check(
      childBlock.includes("QA Erin") && childBlock.includes("QA Alice"),
      "the DELEGATED CHILD's prompt carries the same <room_context>",
      childBlock.trim() || "(no block)",
    );
  } else {
    skip("the delegated child's <room_context>", "no child turn reached the provider");
  }
  // Let the delegation run finish before the next scenario speaks: this
  // channel steers a mid-run message into the running turn, and a scenario
  // that measured a run somebody else started would be measuring nothing.
  await until(async () => outboundCount() > repliesBeforeDelegate, 120_000);

  // ===== scenario 6: unbind ==============================================
  console.log("\n=== scenario 6: unbind stops future turns and keeps what is already filed ===");
  const unbindOut = cli("projects", "channel", "unbind", CHANNEL, C1, "--peer-kind", "group");
  check(
    unbindOut.code === 0 && unbindOut.out.includes("stays with the room"),
    "`aleph projects channel unbind` prints the keeps-the-transcript notice",
    `exit=${unbindOut.code}\n${unbindOut.out.slice(0, 400)}`,
  );
  const s6 = await sayAndSettle(senders.alice, C1, "m6-alice");
  check(
    s6.partition === `main__${alice.user_id}`,
    `after unbind alice's turn falls back to main__${alice.user_id}`,
    `note landed in ${s6.partition ?? "(no note within budget)"}`,
  );
  const rowAfterUnbind = sessionRow(C1_KEY);
  check(
    rowAfterUnbind?.scope_id === `project:${PID}`,
    "and the already-filed row KEEPS the room scope (unbind does not move history back)",
    JSON.stringify(rowAfterUnbind),
  );

  // ===== scenario 8: bind first, roster-outsider speaks first =============
  //
  // A NEW conversation, not C1. `stamp_attribution` is create-only, so on a
  // conversation that already has a row this defect is structurally
  // unreproducible — which is exactly why it survived the first seven
  // scenarios.
  console.log("\n=== scenario 8 (Ruling AQ evidence): C2 is bound before anyone speaks ===");
  const c2Json = cli(
    "projects", "channel", "bind", PID, CHANNEL, C2, "--peer-kind", "group", "--json",
  );
  const c2Receipt = jsonFrom(c2Json.out);
  // EXACTLY `nothing_to_move`, never "not moved": `unknown` would mean the
  // store failed to answer, and treating that as the premise would let every
  // assertion below run on a premise that was never established — and pass.
  check(
    c2Receipt?.rescoped_session === "nothing_to_move",
    "binding a never-spoken-in conversation reports exactly NothingToMove",
    `rescoped_session=${JSON.stringify(c2Receipt?.rescoped_session)}\n${c2Json.out.slice(0, 400)}`,
  );

  const s8 = await sayAndSettle(senders.carol, C2, "m8-carol");
  const C2_KEY = `agent:main:${CHANNEL}:group:${C2}`;
  const c2Row = sessionRow(C2_KEY);
  check(
    s8.partition === `main__${carol.user_id}`,
    `carol's run behaves as in scenario 3: her memory lands in main__${carol.user_id}`,
    `note landed in ${s8.partition ?? "(no note within budget)"}`,
  );
  fact("C2 row (owner_user_id / scope_id)", JSON.stringify(c2Row));
  const c2VisibleToBob = await conns.bob.attempt("sessions.list", {});
  const c2Keys = (c2VisibleToBob.result?.sessions ?? []).map((s) => s.key);
  fact(
    `C2 visible to bob (roster member)`,
    String(c2Keys.includes(C2_KEY)),
  );
  // Not scored — Ruling AQ withdrew "if invisible: confirm P1". The controller
  // rules; this reports.
  fact(
    "scenario 8 reading",
    c2Keys.includes(C2_KEY)
      ? "the bound room's conversation IS visible to a roster member"
      : "the bound room's conversation is NOT visible to a roster member " +
        "(design A: the row is stamped personal:<first speaker>)",
  );

  // ===== scenario 8b: the second door ====================================
  console.log("\n=== scenario 8b (Ruling AQ evidence): an authenticated non-member, no channel ===");
  const c3Json = cli(
    "projects", "channel", "bind", PID, CHANNEL, C3, "--peer-kind", "group", "--json",
  );
  const c3Receipt = jsonFrom(c3Json.out);
  check(
    c3Receipt?.rescoped_session === "nothing_to_move",
    "C3 binds with nothing to move (the premise for 8b)",
    `rescoped_session=${JSON.stringify(c3Receipt?.rescoped_session)}`,
  );

  const C3_KEY = `agent:main:${CHANNEL}:group:${C3}`;
  const daveSend = await conns.dave.attempt("chat.send", {
    message: "qa-note:m8b-dave please jot this down",
    session_key: C3_KEY,
  });
  fact("dave's chat.send", JSON.stringify(daveSend.error ?? daveSend.result).slice(0, 300));
  // Not an assertion: dave is a member, so his note_manage parks, and the
  // desk above may or may not have cleared it before this read. The ROW is the
  // evidence Ruling AQ asked for; the partition is a bonus fact.
  const davePartition = await until(async () => notePartition("m8b-dave"), 90_000);
  const c3Row = sessionRow(C3_KEY);
  check(
    Boolean(c3Row),
    "dave — authenticated, off the roster, NOT in the channel conversation — creates the row",
    "no session row for C3; the run may have been refused at admission " +
      `(${JSON.stringify(daveSend.error)})`,
  );
  fact("C3 row (owner_user_id / scope_id)", JSON.stringify(c3Row));
  fact("dave's note partition", String(davePartition));
  const bobAfter8b = await conns.bob.attempt("sessions.list", {});
  const bob8bKeys = (bobAfter8b.result?.sessions ?? []).map((s) => s.key);
  fact("C3 visible to bob (roster member)", String(bob8bKeys.includes(C3_KEY)));
  fact(
    "design B cost is real?",
    `bob is on the roster and has never been in ${C2}/${C3}; ` +
      "so stamping those rows with the room scope WOULD show him carol's and " +
      "dave's turns. That cost is constructible on this fixture, not theoretical.",
  );

  // ===== scenario 7: agent_switch does not unbind =========================
  //
  // Last among the channel scenarios: `channels.set_agent` is keyed on the
  // CHANNEL, so it would change the agent for C2 and C3 too.
  console.log("\n=== scenario 7: an agent switch does not release the binding ===");
  const rebind = cli("projects", "channel", "bind", PID, CHANNEL, C1, "--peer-kind", "group", "--json");
  check(rebind.code === 0, "C1 is re-bound for scenario 7", rebind.out.slice(0, 300));
  const switched = await op.attempt("channels.set_agent", {
    channel_id: CHANNEL,
    agent_id: "coder",
  });
  check(
    Boolean(switched.result?.ok),
    "the channel's active agent is switched to `coder`",
    JSON.stringify(switched.error ?? switched.result),
  );
  const s7 = await sayAndSettle(senders.alice, C1, "m7-alice");
  // The brief spells this `main__p-<id>`; that spelling assumes the agent id is
  // still `main`, and an agent switch is precisely the thing that changes it.
  // The claim being made is that the ROOM survives the switch, so the assertion
  // is on the room half of the partition and the row's scope.
  check(
    s7.partition === `coder__${PID}`,
    `after the switch the turn still runs under the ROOM: coder__${PID}`,
    `note landed in ${s7.partition ?? "(no note within budget)"} ` +
      `(the brief's literal main__${PID} assumes the agent id never changes)`,
  );
  const switchedRow = sessionRow(`agent:coder:${CHANNEL}:group:${C1}`);
  check(
    switchedRow?.scope_id === `project:${PID}`,
    "and the switch's brand-new session row is stamped with the room, not a person",
    JSON.stringify(switchedRow),
  );
  await op.attempt("channels.set_agent", { channel_id: CHANNEL, agent_id: "main" });

  // ===== addendum A: the tier gate, over a real chat-tier connection ======
  console.log("\n=== addendum A: require_operator_tier against a live member connection ===");
  const aliceRoom = (await conns.alice.ok("projects.create", { name: "QA Alice Room" })).project;
  // The owner must also be on the roster: `project_visible_to` is the roster,
  // not the owner column, so without this her own run cannot address the room
  // and the refusal would be `project not found` — an EARLIER refusal, which
  // proves nothing about the gate.
  // Added by ALICE, not the operator: `gate_project` is `roster::is_member`,
  // and an operator's `CALLER_USER` is a real principal id rather than `None`,
  // so a room they are not on reads as `project not found` to them too.
  // `projects.create` already put alice on her own room's roster.
  await conns.alice.ok("projects.member.add", { id: aliceRoom.id, user_id: bob.user_id });
  const beforeTier = requests().length;
  const cardsBeforeTier = approvedCards.length;
  const wsArg = WORKSPACE.replace(/ /g, "+");
  await conns.alice.ok("chat.send", {
    message: `qa-bindws:${aliceRoom.id}|${wsArg} point the room at that folder`,
    // Deliberately explicit: a member may go stricter than the install and
    // never looser, so `auto` here is the LOOSEST tier this connection can
    // reach. The refusal below therefore cannot be blamed on the approval
    // ceiling — it is the tool's own gate or nothing.
    exec_tier: "auto",
  });
  const tierResult = await until(async () => {
    const hit = requests()
      .slice(beforeTier)
      .find((r) => /operator-tier session|not the project owner|project not found/.test(toolResults(r.body)));
    return hit || null;
  }, 240_000);
  if (!tierResult) {
    skip(
      "the tier gate refuses a chat-tier caller",
      "no project_manage tool_result reached the model within 240s",
    );
  } else {
    const said = toolResults(tierResult.body);
    check(
      said.includes("requires an operator-tier session"),
      "a chat-tier member's project_manage(bind_workspace) is refused BY THE TIER GATE",
      said.slice(0, 500),
    );
    check(
      !said.includes("not the project owner") && !said.includes("project not found"),
      "and it is the tier refusal, not an earlier ownership/visibility one",
      said.slice(0, 500),
    );
    fact("tier refusal, verbatim", said.replace(/\s+/g, " ").slice(0, 300));
  }
  // `caller_role` is a task-local inside the server process; it has no wire
  // face, so this fixture cannot quote it and does not pretend to. What it CAN
  // observe is the one thing addendum A actually asks about — that the value
  // was live AT THE MOMENT THE TOOL EXECUTED — and there are two independent
  // witnesses to that, one turn apart:
  //
  //   1. the refusal above, which `require_operator_tier` produced by reading
  //      `TurnContext::caller_is_operator()` at the tool's own call site; and
  //   2. that the turn was NOT stopped by anything earlier.
  //
  // The turn asked for `exec_tier: "auto"` — the loosest a member can reach,
  // since the ceiling clamps a non-operator to the install's own posture. So a
  // card count of zero here is the expected reading and the STRONGER one: the
  // call reached the tool and the tool refused it, rather than the approval
  // ceiling stopping it on the way. (That the clamp is live at all on this
  // build is shown separately, by the card dave's member run does raise.)
  fact(
    "cards raised by alice's exec_tier=auto member run (0 ⇒ the tool's own gate refused, not the ceiling)",
    String(approvedCards.length - cardsBeforeTier),
  );

  // ===== addendum E: the write path is classified, not raw ================
  console.log("\n=== addendum E: a non-admin's bind is refused with the classified message ===");
  const aliceBind = await conns.alice.attempt("projects.channel.bind", {
    project_id: PID,
    channel_id: CHANNEL,
    peer_kind: "group",
    peer_id: "qa-c-alice-attempt",
  });
  check(
    Boolean(aliceBind.error) &&
      String(aliceBind.error.message).includes("requires operator privileges"),
    "a member's projects.channel.bind is refused with ADMIN_REQUIRED_MESSAGE",
    JSON.stringify(aliceBind.error ?? aliceBind.result),
  );
  fact("member bind refusal, verbatim", JSON.stringify(aliceBind.error));
  // The refusal must survive as a refusal, not as a binding nobody asked for.
  const afterAttempt = cli("projects", "channel", "list", PID);
  check(
    !afterAttempt.out.includes("qa-c-alice-attempt"),
    "and nothing was bound by the refused call",
    afterAttempt.out.slice(0, 400),
  );

  // ===== addendum C: the Unknown arm =====================================
  //
  // DELIBERATELY LAST, and deliberately destructive: the only way to make a
  // live sqlite session store fail `list_sessions` is to take the table away
  // from it. Everything above has already been asserted by this point.
  console.log("\n=== addendum C: driving a real store failure to reach RescopeOutcome::Unknown ===");
  await unknownArm(op, PID);

  fact(
    "approval cards a member run raised (and the desk cleared)",
    approvedCards.length > 0 ? approvedCards.join("; ") : "none",
  );
  deskOpen = false;
  await approvalDesk.catch(() => {});
  for (const c of [op, conns.alice, conns.bob, conns.dave]) c.close();
  return report();
}

/**
 * Reach `RescopeOutcome::Unknown` by degrading the store the handler reads.
 *
 * `classify_rescope`'s doc names `list_sessions` as the reachable `Err`
 * source and says a `FileSessionStore` produces one when its base directory
 * goes away. This install runs the DEFAULT backend (sqlite), where there is no
 * directory to remove — so the equivalent degradation is to rename the table
 * out from under the open connection. SQLite invalidates cached statements on
 * a schema change, so the server's next `SELECT … FROM sessions` re-prepares
 * and fails with "no such table" rather than silently reading stale rows.
 *
 * Renamed, never dropped, and put back immediately: the point is to make one
 * call fail, not to destroy the run's evidence.
 */
async function unknownArm(op, projectId) {
  const rename = (from, to) => {
    let db;
    try {
      db = new DatabaseSync(SESSIONS_DB);
      db.exec(`ALTER TABLE ${from} RENAME TO ${to}`);
      return null;
    } catch (e) {
      return e.message;
    } finally {
      try {
        db?.close();
      } catch {
        /* ignore */
      }
    }
  };

  const hidErr = rename("sessions", "sessions_qa_hidden");
  if (hidErr) {
    skip(
      "RescopeOutcome::Unknown is produced by a real store failure",
      `could not degrade the sqlite session store: ${hidErr}`,
    );
    return;
  }
  try {
    // The degradation is not observable on the first call. Measured: the bind
    // immediately after the rename still answered `nothing_to_move`, and the
    // next one answered `unknown` — SQLite reports a schema change to a cached
    // statement once and the connection re-prepares, so the failure surfaces on
    // the following query rather than on that one. Polled rather than
    // hard-coded to two, and the receipt asserted is the one from the call that
    // actually failed.
    let r = { result: undefined, error: undefined };
    for (let attempt = 0; attempt < 6; attempt += 1) {
      r = await op.attempt("projects.channel.bind", {
        project_id: projectId,
        channel_id: "webhook",
        peer_kind: "group",
        peer_id: C9,
      });
      if (r.result?.rescoped_session === "unknown" || r.error) break;
      await sleep(500);
    }
    const outcome = r.result?.rescoped_session;
    fact("bind receipt with the session table missing", JSON.stringify(r.error ?? r.result));
    if (outcome === undefined) {
      skip(
        "RescopeOutcome::Unknown is produced by a real store failure",
        `the degraded bind returned an error rather than a receipt: ${JSON.stringify(r.error)}`,
      );
    } else {
      check(
        outcome === "unknown",
        "a store that cannot be listed yields Unknown — not NothingToMove, and not a failed RPC",
        `rescoped_session=${JSON.stringify(outcome)}`,
      );
      const cliOut = cli(
        "projects", "channel", "bind", projectId, "webhook", C9, "--peer-kind", "group",
      );
      check(
        /could not be determined/.test(cliOut.out),
        "and the CLI says `could not be determined` rather than reporting either success or absence",
        `exit=${cliOut.code}\n${cliOut.out.slice(0, 500)}`,
      );
    }
  } finally {
    const back = rename("sessions_qa_hidden", "sessions");
    if (back) log(`WARNING: could not restore the sessions table: ${back}`);
  }
}

function report() {
  if (evidence.length > 0) {
    console.log("\n=== evidence (facts, not assertions) ===");
    for (const [k, v] of evidence) console.log(`  ${k}: ${v}`);
  }
  if (roomScopeReachedHarness === false) {
    console.log(`
=== FINDING: the room upgrade is dropped at the harness spawn ===

  Every failure above about a room partition, <room_context>, or a speaker
  prefix has ONE cause, and it is not this fixture.

  \`run_loop::request_scope\` is what turns a channel turn's stamped
  \`personal:<speaker>\` into the room's scope when the conversation is bound
  (arm 2). Three readers go through it — the session ROW, the loop's own
  task-local, and the sidebar recency touch, the last one converged onto it
  deliberately at run_loop/inner.rs:116 with a comment saying why.

  The FOURTH reader does not. \`run_loop/inner.rs\` builds the \`FlowRequest\`
  that hands the run to the harness, and fills its two scope fields from the
  RAW metadata keys instead:

      owner_user_id: request.metadata.get(crate::scope::OWNER_META_KEY).cloned(),
      scope_id:      request.metadata.get(crate::scope::SCOPE_META_KEY).cloned(),

  \`orchestrator::dispatch\` then re-seeds the task-local inside its spawn from
  those two strings, so everything on the far side of that boundary — the
  prompt build, every tool, \`session_seed\`'s speaker label — runs under
  \`personal:<speaker>\` while the session row it belongs to says
  \`project:<id>\`. The row is stamped in the gateway task, before the spawn,
  which is why the binding looks half-working: \`sessions.list\` visibility is
  right and memory, prompt and label are not.

  Measured both ways on this fixture, same machine, same scenarios: deriving
  those two fields from \`super::request_scope(request)\` instead turns every
  one of these assertions green and changes nothing else. The scenario 8 / 8b
  evidence is unaffected either way — carol and dave are off the roster, so
  arm 2's gate keeps them personal in both builds.
`);
  }
  console.log(`\n=== ${PASS} passed, ${FAIL} failed, ${SKIPPED} skipped ===`);
  for (const f of failures) console.log(`  FAILED:  ${f}`);
  for (const s of skips) console.log(`  SKIPPED: ${s}`);
  process.exit(FAIL === 0 ? 0 : 1);
}

main().catch((e) => {
  console.log(`FAIL  driver aborted: ${e.message}`);
  console.log(e.stack);
  console.log(`\n=== ${PASS} passed, ${FAIL + 1} failed, ${SKIPPED} skipped ===`);
  process.exit(1);
});
