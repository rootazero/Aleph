// Real-machine driver for the tasks + agents visualization round.
//
//   drive_agents_viz.mjs <gateway-port> <request-log> claims
//   drive_agents_viz.mjs <gateway-port> <request-log> session
//   drive_agents_viz.mjs <gateway-port> <request-log> delegate <session_key>
//   drive_agents_viz.mjs <gateway-port> <request-log> plan <session_key>
//
// `claims` runs the assertions listed in run.sh. The other three exist for the
// `panel` scenario, where a browser is attached to /dashboard/subagents and a
// human (or an agent driving a browser) needs one command that makes a
// delegation happen NOW: `session` mints a session and prints its key on the
// last line; `delegate` / `plan` send the marker into it and wait for the run
// to complete.
//
// Every assertion is an EFFECT — a frame that arrived on a specific socket, a
// row a later RPC returned — never "the call returned 200". Waits are
// effect-shaped (poll until the frame exists, with a budget); a fixed sleep
// tests the fixture's optimism, not the server.
//
// ## Three connections, three subscription shapes
//
// `SubscriptionManager::should_receive` answers "everything" for a connection
// that never subscribed and "only what you subscribed to" for one that did.
// The two severed wires this round fixed live on opposite sides of that line:
//
//   tui         never subscribes           — the TUI's shape (wire 1)
//   panel       subscribes to config.** (so a filter EXISTS, as the Panel's
//               BASE_TOPICS seed one at connect) and then to the tree topic
//                                          — the Panel's shape (wire 2)
//   panel_blind subscribes to config.** only
//                                          — the negative arm: a filtered
//               socket that did not ask for the topic must see nothing, or
//               D4 proves nothing about the subscription
//
// All three are loopback operator connections. The visibility index gates
// `run.subagent_tree` by ROOT SESSION, and the operator sees every session, so
// what separates them is the subscription state alone.
import fs from "node:fs";
import path from "node:path";
import { normalizeFrame } from "../lib/ws.mjs";

const [portArg, REQUEST_LOG, MODE = "claims", MODE_ARG] = process.argv.slice(2);
const PORT = Number(portArg);
if (!PORT || !REQUEST_LOG) {
  console.error(
    "usage: drive_agents_viz.mjs <gateway-port> <request-log> claims|session|delegate <key>|plan <key>",
  );
  process.exit(2);
}

const TOPIC = "run.subagent_tree";
const T0 = Date.now();
const log = (...a) => console.log(`${((Date.now() - T0) / 1000).toFixed(2)}s`, ...a);

let PASS = 0;
let FAIL = 0;
const failures = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** One assertion. `detail` is printed on failure only — evidence, not noise. */
const check = (cond, label, detail = "") => {
  if (cond) {
    PASS += 1;
    console.log(`PASS  ${label}`);
  } else {
    FAIL += 1;
    failures.push(label);
    console.log(`FAIL  ${label}`);
    if (detail) {
      for (const line of String(detail).split("\n").slice(0, 14)) console.log(`      | ${line}`);
    }
  }
  return cond;
};

// ---------------------------------------------------------------------------
// Connection — the same three-envelope tap qa/teamchat_rooms carries, because
// a tap that reads only `msg.topic ?? msg.method` reports every bus event as
// topic "event", which reads exactly like "the frame never arrived".
// ---------------------------------------------------------------------------

const ALL_CONNS = [];

class Conn {
  constructor(name) {
    this.name = name;
    ALL_CONNS.push(this);
    this.frames = [];
    this.pendingReplies = new Map();
    this.nextId = 1;
  }

  async open(url, connectParams) {
    this.ws = new WebSocket(url);
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
      if (msg.id !== undefined && msg.id !== null && this.pendingReplies.has(msg.id)) {
        this.pendingReplies.get(msg.id)(msg);
        this.pendingReplies.delete(msg.id);
        return;
      }
      this.frames.push(normalizeFrame(msg));
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

  /** `rpc` that throws on a JSON-RPC error — for steps the run cannot go on without. */
  async ok(method, params = {}, budget = 90_000) {
    const r = await this.rpc(method, params, budget);
    if (r.error) throw new Error(`${this.name}: ${method} -> ${JSON.stringify(r.error)}`);
    return r.result;
  }

  /** `rpc` that returns the raw reply, error or not. */
  attempt(method, params = {}, budget = 90_000) {
    return this.rpc(method, params, budget).catch((e) => ({ error: { message: e.message } }));
  }

  async waitFrame(pred, budget = 60_000) {
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

/** Poll `fn` until it returns something truthy, or give up and return null. */
const until = async (fn, budget = 120_000, every = 500) => {
  const end = Date.now() + budget;
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() >= end) return null;
    await sleep(every);
  }
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

/**
 * Depth-first: the first value under a key named `key` anywhere in `obj`.
 *
 * Descends into STRINGS that parse as JSON objects too: `ToolResult.output`
 * on the `stream.tool_end` frame is the tool's JSON re-encoded as a string
 * (`Option<String>` on the wire), so the scratchpad snapshot a renderer
 * reads is one `JSON.parse` below the frame — a search that stops at the
 * string reports "no snapshot" for a frame that carries one.
 */
const findKey = (obj, key, depth = 0) => {
  if (depth > 12) return undefined;
  if (typeof obj === "string" && obj.startsWith("{")) {
    try {
      return findKey(JSON.parse(obj), key, depth + 1);
    } catch {
      return undefined;
    }
  }
  if (!obj || typeof obj !== "object") return undefined;
  if (Object.prototype.hasOwnProperty.call(obj, key)) return obj[key];
  for (const v of Array.isArray(obj) ? obj : Object.values(obj)) {
    const hit = findKey(v, key, depth + 1);
    if (hit !== undefined) return hit;
  }
  return undefined;
};

/** `JSON.stringify` that never throws on `undefined` and always yields a string. */
const show = (v, max = 600) => (JSON.stringify(v ?? null) ?? "null").slice(0, max);

/** Persist every frame each connection saw, next to the request log — the post-mortem. */
const dumpFrames = (conns) => {
  const dir = path.dirname(REQUEST_LOG);
  for (const c of conns) {
    try {
      fs.writeFileSync(
        path.join(dir, `frames-${c.name}.jsonl`),
        c.frames.map((f) => JSON.stringify(f.raw)).join("\n") + "\n",
      );
    } catch {
      /* post-mortem only */
    }
  }
};

/** A plan snapshot's `[text, status]` rows, whatever field names the carrier used. */
const planRows = (snapshot) => {
  const items = snapshot?.items ?? snapshot?.steps ?? [];
  return (Array.isArray(items) ? items : []).map((i) => [
    i.text ?? i.title ?? i.content ?? "",
    i.status ?? "",
  ]);
};

// The mock's QA-PLAN arm writes the mixed list first (the live carrier is
// asserted on it), then answers the stop guard's veto by ticking every box —
// the terminal state the run-end and cold carriers hold. See mock_llm.mjs.
const PLAN_MIXED = [
  ["QA step one", "completed"],
  ["QA step two", "in_progress"],
  ["QA step three", "pending"],
];
const PLAN_DONE = PLAN_MIXED.map(([text]) => [text, "completed"]);
const samePlan = (rows, expected) => JSON.stringify(rows) === JSON.stringify(expected);

const isTree = (f) => f.topic === TOPIC;
const treeKind = (kind) => (f) => isTree(f) && f.data?.kind === kind;

/**
 * A `subagent` spawn from an operator's own turn is not expected to park for
 * approval — the loopback operator carries the full tier — but if it ever
 * does, an unanswered card expires after 120 s and this whole fixture reads
 * as "no child was ever spawned". Answer it if it appears; report that it did.
 */
const approveIfParked = async (op, needle, budget) => {
  const found = await until(async () => {
    const r = await op.attempt("exec.approvals.pending", {});
    return (
      (r.result?.pending ?? []).find((p) => JSON.stringify(p.record).includes(needle)) || null
    );
  }, budget, 1000);
  if (!found) return false;
  await op.attempt("exec.approval.resolve", {
    id: found.record.id,
    decision: "allow-once",
    resolved_by: "QA operator",
  });
  return true;
};

/** Send `message` into `sessionKey` (or a fresh session) and wait for its run to end. */
const runTurn = async (conn, message, sessionKey, budget = 120_000) => {
  const params = { message, channel: "gui:qa-agents-viz" };
  if (sessionKey) params.session_key = sessionKey;
  const started = await conn.ok("chat.send", params);
  const runId = started.run_id;
  log(`run ${runId} on ${started.session_key}: ${message.slice(0, 40)}`);
  const done = await conn.waitFrame(
    (f) =>
      (f.topic === "stream.run_complete" || f.topic === "stream.run_error") &&
      f.data?.run_id === runId,
    budget,
  );
  return { runId, sessionKey: started.session_key, done };
};

const LOOPBACK = `ws://127.0.0.1:${PORT}/ws`;

// ---------------------------------------------------------------------------

async function claims() {
  const tui = new Conn("tui");
  const panel = new Conn("panel");
  const blind = new Conn("panel_blind");
  const hello = await tui.open(LOOPBACK, { client_type: "cli" });
  check(!hello.error, "an unfiltered (TUI-shaped) connection opens over loopback", JSON.stringify(hello.error));
  await panel.open(LOOPBACK, { client_type: "panel" });
  await blind.open(LOOPBACK, { client_type: "panel" });

  // Give both Panel-shaped sockets a filter. `events.subscribe` with any
  // pattern flips `should_receive` from "all" to "subscribed only" — the
  // Panel's BASE_TOPICS seed does exactly this at connect.
  await panel.ok("events.subscribe", { topics: ["config.**"] });
  await blind.ok("events.subscribe", { topics: ["config.**"] });
  // …and only ONE of them asks for the tree, the way the fixed view does.
  await panel.ok("events.subscribe", { topics: [TOPIC] });

  // ===== plan carriers =====================================================
  console.log("\n=== plan: one mutating scratchpad call, three carriers ===");
  const planRun = await runTurn(tui, "QA-PLAN write the checklist");
  check(Boolean(planRun.done), "the QA-PLAN run completes", `frames: ${tui.frames.map((f) => f.topic).join(",")}`);
  const sessionKey = planRun.sessionKey;

  // P1 — the live carrier: a scratchpad snapshot inside this run's
  // trace/tool_end frames (the TUI applies it from EITHER projection).
  const liveFrames = tui.frames.filter(
    (f) =>
      (f.topic === "stream.agent_trace" || f.topic === "stream.tool_end") &&
      f.data?.run_id === planRun.runId,
  );
  const liveSnapshots = liveFrames
    .map((f) => findKey(f.data, "snapshot"))
    .filter((s) => s && (s.items || s.steps))
    .map(planRows);
  check(
    liveSnapshots.length >= 2 &&
      samePlan(liveSnapshots[0], PLAN_MIXED) &&
      samePlan(liveSnapshots[liveSnapshots.length - 1], PLAN_DONE),
    "P1 the live trace/tool_end frames carry each scratchpad snapshot in order (mixed, then done)",
    JSON.stringify({ frames: liveFrames.length, snapshots: liveSnapshots }),
  );

  // P2 — the authoritative carrier at run end holds the terminal state.
  const summaryPlan = planRun.done?.data?.summary?.plan;
  check(
    Boolean(summaryPlan) && samePlan(planRows(summaryPlan), PLAN_DONE),
    "P2 RunSummary.plan at stream.run_complete carries the terminal list",
    show(planRun.done?.data?.summary ?? planRun.done?.data, 600),
  );

  // P3 — the cold carrier a reconnecting client reads.
  const history = await tui.attempt("chat.history", { session_key: sessionKey });
  check(
    Boolean(history.result?.plan) && samePlan(planRows(history.result.plan), PLAN_DONE),
    "P3 chat.history.plan (cold) carries the terminal list",
    show(history.result?.plan ?? history.error, 600),
  );

  // ===== the tree topic, three subscription shapes =========================
  console.log("\n=== agents: one delegation, three sockets ===");
  for (const c of [tui, panel, blind]) c.frames.length = 0;
  const beforeDelegate = requests().length;
  const started = await tui.ok("chat.send", {
    message: "QA-DELEGATE-BG hand this one to a helper",
    session_key: sessionKey,
    channel: "gui:qa-agents-viz",
  });
  log(`delegation run ${started.run_id}`);
  // Concurrent with the wait below: a parked card would otherwise be the
  // reason nothing spawns, and it must be reported as such, not as D1.
  const cardTask = approveIfParked(tui, "subagent", 30_000);

  const spawned = await tui.waitFrame(treeKind("spawned"), 90_000);
  const parked = await cardTask;
  if (parked) log("a subagent approval card was parked and approved");
  // The snapshot face of the same tree (`subagent.tree`), printed whenever
  // the event face is silent: it answers "did a node exist at all, and under
  // which root_session" — the two questions that separate "never spawned"
  // from "spawned, but the frame was gated or never relayed".
  if (!spawned) {
    const snap = await tui.attempt("subagent.tree", {});
    log(`subagent.tree snapshot: ${show(snap.result ?? snap.error, 900)}`);
  }

  // D1 — the envelope, not just the topic. `classify_frame` now delivers
  // `{"method":"event","params":{"topic":…}}`; a flat `{topic:…}` frame would
  // reach this tap too but would NOT reach the Rust client, so the shape is
  // part of the claim.
  check(
    Boolean(spawned) &&
      spawned.raw?.method === "event" &&
      spawned.raw?.params?.topic === TOPIC,
    "D1 an UNFILTERED connection receives run.subagent_tree spawned, double-nested",
    show(spawned?.raw ?? tui.frames.map((f) => f.topic), 600),
  );
  const node = spawned?.data?.node;
  check(
    typeof node?.child_session === "string" &&
      node.child_session.length > 0 &&
      node.root_session === sessionKey,
    "D2 the spawned node carries child_session and names the parent as root_session",
    show(node, 600),
  );

  const settled = await tui.waitFrame(treeKind("settled"), 120_000);
  check(
    settled?.data?.lifecycle === "completed" && typeof settled?.data?.total_tokens === "number",
    "D3 the settled frame arrives with lifecycle=completed and a numeric total_tokens",
    show(settled?.data ?? tui.frames.map((f) => f.topic), 600),
  );

  const done = await tui.waitFrame(
    (f) => f.topic === "stream.run_complete" && f.data?.run_id === started.run_id,
    120_000,
  );
  check(Boolean(done), "the delegation run completes", tui.frames.map((f) => f.topic).join(","));

  // D4 / D5 — same frames, decided by subscription state alone. Settle-time
  // ordering across sockets is not guaranteed, so give the subscribed socket
  // a short grace before reading either.
  await panel.waitFrame(treeKind("settled"), 10_000);
  const panelKinds = panel.frames.filter(isTree).map((f) => f.data?.kind);
  check(
    panelKinds.includes("spawned") && panelKinds.includes("settled"),
    "D4 a FILTERED connection that subscribed to the topic receives spawned + settled",
    `panel tree kinds: ${JSON.stringify(panelKinds)}; all: ${panel.frames.map((f) => f.topic).join(",")}`,
  );
  const blindKinds = blind.frames.filter(isTree).map((f) => f.data?.kind);
  check(
    blindKinds.length === 0,
    "D5 a FILTERED connection that did NOT subscribe receives none of them",
    `panel_blind tree kinds: ${JSON.stringify(blindKinds)}`,
  );

  // D6 — the agent-run view opens the child by its session key over the
  // EXISTING chat.history (the round added no RPC for it, on purpose).
  if (node?.child_session) {
    // Keep the LAST reply: when the wait gives up, "null" is not evidence —
    // the shape of what the server actually answered is.
    let lastReply = null;
    const child = await until(async () => {
      const r = await tui.attempt("chat.history", { session_key: node.child_session });
      lastReply = r;
      const msgs = r.result?.messages ?? [];
      return msgs.some((m) => JSON.stringify(m).includes("QA-CHILD")) ? r : null;
    }, 30_000);
    check(
      Boolean(child),
      "D6 chat.history on child_session returns the child's own turn",
      `child_session=${node.child_session}\nlast reply: ${show(lastReply?.result ?? lastReply?.error, 900)}`,
    );
  } else {
    check(false, "D6 chat.history on child_session returns the child's own turn", "no child_session to ask about");
  }

  // The provider-side oracle: the child really reached the mock as its own turn.
  const childReq = requests()
    .slice(beforeDelegate)
    .find((r) => userText(r.body).includes("QA-CHILD") && !userText(r.body).includes("QA-DELEGATE"));
  check(
    Boolean(childReq),
    "the delegated child reached the provider as its own request",
    `requests since delegation: ${requests().length - beforeDelegate}`,
  );

  for (const c of [tui, panel, blind]) c.close();
  return report();
}

async function session() {
  const c = new Conn("operator");
  await c.open(LOOPBACK, { client_type: "cli" });
  const turn = await runTurn(c, "hello from the panel scenario");
  c.close();
  // Last line = the key; run.sh takes `tail -1`.
  console.log(turn.sessionKey);
}

async function marker(text, sessionKey) {
  if (!sessionKey) {
    console.error("this mode needs a session key");
    process.exit(2);
  }
  const c = new Conn("operator");
  await c.open(LOOPBACK, { client_type: "cli" });
  const cardTask = approveIfParked(c, "subagent", 20_000);
  const turn = await runTurn(c, text, sessionKey, 180_000);
  await cardTask;
  const tree = c.frames.filter(isTree).map((f) => f.data?.kind);
  console.log(`run ${turn.runId} ${turn.done ? turn.done.topic : "DID NOT COMPLETE"}; tree frames: ${JSON.stringify(tree)}`);
  c.close();
  process.exit(turn.done ? 0 : 1);
}

function report() {
  dumpFrames(ALL_CONNS);
  console.log(`\n=== ${PASS} passed, ${FAIL} failed ===`);
  for (const f of failures) console.log(`  FAILED: ${f}`);
  process.exit(FAIL === 0 ? 0 : 1);
}

const main = async () => {
  switch (MODE) {
    case "claims":
      return claims();
    case "session":
      return session();
    case "delegate":
      return marker("QA-DELEGATE-BG hand this one to a helper", MODE_ARG);
    case "plan":
      return marker("QA-PLAN write the checklist", MODE_ARG);
    default:
      console.error(`unknown mode: ${MODE}`);
      process.exit(2);
  }
};

main().catch((e) => {
  dumpFrames(ALL_CONNS);
  console.log(`FAIL  driver aborted: ${e.message}`);
  console.log(e.stack);
  console.log(`\n=== ${PASS} passed, ${FAIL + 1} failed ===`);
  process.exit(1);
});
