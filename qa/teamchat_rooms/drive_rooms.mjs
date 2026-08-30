// Real-machine driver for the multi-user × team-chat × project-rooms round.
//
//   drive_rooms.mjs <gateway-port> <workspace-dir> <request-log> <delete-path>
//
// Three humans and two agents, on one server, over three real connections:
//
//   operator  loopback, zero-credential, admin
//   QA Alice  a `member` principal, paired over the LAN leg with a real ticket
//   QA Bob    the same, second device
//
// Every assertion below is an EFFECT — a row that appeared, a frame that
// arrived, a file that stopped existing — never "the call returned 200". The
// waits are effect-shaped too (poll until the thing exists, with a budget)
// rather than `sleep`, because a fixed sleep tests the fixture's optimism.
//
// ## Why an unsubscribed socket
//
// `SubscriptionManager::should_receive` answers `true` for a connection with no
// filter, and narrowing to `team.*` would silently drop `projects.changed` —
// the exact "a replayed list is not a restore, it is a narrowing" shape this
// repo has been bitten by. So nothing subscribes; every frame is collected and
// the assertions pick.
import fs from "node:fs";
import os from "node:os";

const [portArg, WORKSPACE, REQUEST_LOG, DELETE_PATH] = process.argv.slice(2);
const PORT = Number(portArg);
if (!PORT || !WORKSPACE || !REQUEST_LOG) {
  console.error("usage: drive_rooms.mjs <gateway-port> <workspace-dir> <request-log> <delete-path>");
  process.exit(2);
}

const T0 = Date.now();
const log = (...a) =>
  console.log(`${((Date.now() - T0) / 1000).toFixed(2)}s`, ...a);

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
      for (const line of String(detail).split("\n").slice(0, 14)) {
        console.log(`      | ${line}`);
      }
    }
  }
  return cond;
};

/** An assertion that could not be attempted. Never renders as a pass. */
const skip = (label, why) => {
  console.log(`SKIP  ${label} (${why})`);
};

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

/**
 * A gateway connection. Replies are matched by id; everything else is a frame.
 *
 * Frames are kept in arrival order per connection because "did BOB see Alice's
 * bubble" is a per-connection question — the event-visibility index answers it
 * separately for each socket, which is the whole point of asking it twice.
 */
class Conn {
  constructor(name) {
    this.name = name;
    this.frames = [];
    this.pendingReplies = new Map();
    this.nextId = 1;
    this.closed = false;
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
    this.ws.addEventListener("close", () => {
      this.closed = true;
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
      // `topic` is overloaded on this wire, in THREE shapes, and reading only
      // one of them makes the tap blind to exactly the frames a scenario is
      // chasing (this fixture reported "no team.<id>.message ever arrived"
      // for a whole round because of it):
      //
      //   {method:"event", params:{topic, data}}  bus events, as delivered
      //   {topic, data}                            the same, un-enveloped
      //   {method:"stream.x", params:{…}}          JSON-RPC notifications
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

  /** `rpc`, but a JSON-RPC error becomes a thrown Error with the server's words. */
  async ok(method, params = {}, budget = 90_000) {
    const r = await this.rpc(method, params, budget);
    if (r.error) {
      throw new Error(`${method} -> [${r.error.code}] ${r.error.message}`);
    }
    return r.result;
  }

  /** `rpc`, tolerating an error reply — returns `{result, error}` for the caller to judge. */
  async attempt(method, params = {}, budget = 90_000) {
    const r = await this.rpc(method, params, budget).catch((e) => ({ error: { message: e.message } }));
    return { result: r.result, error: r.error };
  }

  seen(pred) {
    return this.frames.filter((f) => pred(f));
  }

  async waitFrame(pred, budget = 60_000) {
    const end = Date.now() + budget;
    while (Date.now() < end) {
      const hit = this.frames.find(pred);
      if (hit) return hit;
      await sleep(200);
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

/** A request's system prompt, flattened to text (it may be a string or blocks). */
const systemText = (body) => {
  const s = body?.system;
  if (typeof s === "string") return s;
  if (Array.isArray(s)) return s.map((b) => (typeof b === "string" ? b : b?.text || "")).join("\n");
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

// ---------------------------------------------------------------------------

const LOOPBACK = `ws://127.0.0.1:${PORT}/ws`;

async function main() {
  const ip = lanIp();
  if (!ip) {
    console.log("FAIL  no non-loopback address on this host; the member half cannot run");
    process.exit(1);
  }
  const REMOTE = `ws://${ip}:${PORT}/ws`;
  log(`operator over ${LOOPBACK}; members over ${REMOTE}`);

  // ===== Phase 0: three identities ========================================
  console.log("\n=== phase 0: identities ===");
  const op = new Conn("operator");
  const hello = await op.open(LOOPBACK, { client_type: "cli" });
  check(!hello.error, "operator connects over loopback with no credential", JSON.stringify(hello.error));

  const alice = (await op.ok("users.create", { display_name: "QA Alice", role: "member" })).user;
  const bob = (await op.ok("users.create", { display_name: "QA Bob", role: "member" })).user;
  log(`alice=${alice.user_id} bob=${bob.user_id}`);

  const conns = {};
  for (const [who, user, device] of [
    ["alice", alice, "qa-panel-alice"],
    ["bob", bob, "qa-panel-bob"],
  ]) {
    const { ticket } = await op.ok("gateway.ticket.create", { user_id: user.user_id });
    const c = new Conn(who);
    const redeemed = await c.open(REMOTE, {
      client_type: "panel",
      bootstrap_ticket: ticket,
      device_id: device,
      device_name: `QA ${who}`,
    });
    // A remote connect that was handed no device token means the ticket path
    // never ran — which is exactly what the loopback short-circuit produces,
    // and it must not read as success.
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

  // ===== Phase 1: the room ================================================
  console.log("\n=== phase 1: a room with three people in it ===");
  const project = (await op.ok("projects.create", { name: "QA Room" })).project;
  log(`room ${project.id}`);
  await op.ok("projects.member.add", { id: project.id, user_id: alice.user_id });
  await op.ok("projects.member.add", { id: project.id, user_id: bob.user_id });
  await op.ok("projects.bind_workspace", { id: project.id, path: WORKSPACE });

  const roster = await op.ok("projects.member.list", { id: project.id });
  const memberIds = roster.member_ids ?? roster.members ?? [];
  check(
    memberIds.includes(alice.user_id) && memberIds.includes(bob.user_id),
    "the roster carries both members",
    JSON.stringify(roster),
  );

  const keyA = (await conns.alice.ok("projects.room_session", { id: project.id, agent_id: "main" }))
    .session_key;
  const keyB = (await conns.bob.ok("projects.room_session", { id: project.id, agent_id: "main" }))
    .session_key;
  check(
    Boolean(keyA) && keyA === keyB,
    "both members land on ONE room session key (not a private one each)",
    `alice=${keyA}\nbob=${keyB}`,
  );

  // A stranger must not be able to tell the room exists. Same response as a
  // room that never existed — no existence oracle.
  const strangerRoom = await conns.bob.attempt("projects.get", { id: "p-does-not-exist" });
  const bobRoom = await conns.bob.attempt("projects.get", { id: project.id });
  check(
    Boolean(bobRoom.result) && Boolean(strangerRoom.error),
    "a member reads the room; a nonexistent id is refused",
    JSON.stringify({ bob: bobRoom.error, stranger: strangerRoom.error }),
  );

  // ===== Phase 2: a room run, its prompt, and the team it creates =========
  console.log("\n=== phase 2: room run -> <room_context> + a room-scoped team ===");
  const beforeTeamRun = requests().length;
  const runA = await conns.alice.ok("chat.send", {
    message: "QA-TEAMCREATE please assemble the team",
    session_key: keyA,
    channel: "gui:qa-room",
  });
  log(`alice's room run ${runA.run_id} on ${runA.session_key}`);

  // Every non-idempotent tool a MEMBER's run calls parks for approval: her
  // connection carries the `member` role, which caps the turn's exec tier.
  // Left unanswered a card expires after 120 s with `ApprovalExpired` — which
  // is how the first run of this fixture discovered the gate, twice (once for
  // `team_create`, once for the note in phase 6). Answering it is both the
  // realistic operator action and the cheapest proof that the escalation
  // reaches somebody.
  const approveOperatorCard = async (needle, budget = 90_000) => {
    const found = await until(async () => {
      const r = await op.attempt("exec.approvals.pending", {});
      return (r.result?.pending ?? []).find((p) =>
        JSON.stringify(p.record).includes(needle),
      ) || null;
    }, budget);
    if (!found) return null;
    const resolved = await op.attempt("exec.approval.resolve", {
      id: found.record.id,
      decision: "allow-once",
      resolved_by: "QA operator",
    });
    return { record: found.record, resolved };
  };

  const teamCard = await approveOperatorCard("team_create");
  check(
    Boolean(teamCard),
    "a member's model-driven team_create parks for approval instead of running",
    "no team_create card appeared within 90s",
  );
  if (teamCard) {
    log(`team_create card: ${JSON.stringify(teamCard.record).slice(0, 400)}`);
    check(
      Boolean(teamCard.resolved.result) && !teamCard.resolved.error,
      "the operator can resolve that card",
      JSON.stringify(teamCard.resolved.error ?? teamCard.resolved.result),
    );
  }

  const team = await until(async () => {
    const list = await op.ok("teams.list", {});
    const teams = list.teams ?? list.items ?? [];
    return teams.find((t) => t.scope_id === `project:${project.id}`) || null;
  }, 240_000);
  check(
    Boolean(team),
    "the model's team_create inside a room lands STAMPED with the room scope",
    JSON.stringify((await op.ok("teams.list", {})).teams ?? []).slice(0, 600),
  );

  // The prompt oracle: what the model was actually shown.
  const roomRequests = requests()
    .slice(beforeTeamRun)
    .filter((r) => systemText(r.body).includes("<room_context>"));
  check(
    roomRequests.length > 0,
    "a project-room turn carries a <room_context> block",
    `requests since the run: ${requests().length - beforeTeamRun}; none had the block`,
  );
  const block = roomRequests[0] ? systemText(roomRequests[0].body) : "";
  const roomLine = block.split("<room_context>")[1]?.split("</room_context>")[0] ?? "";
  check(
    roomLine.includes("QA Alice") && roomLine.includes("QA Bob"),
    "the block names both members by DISPLAY NAME, including one who has not spoken",
    roomLine.trim(),
  );
  check(roomLine.includes("(owner)"), "the block marks the room owner", roomLine.trim());

  // Task 11's other half: the user turn itself is speaker-prefixed.
  const labelled = requests()
    .slice(beforeTeamRun)
    .filter((r) => /\[QA Alice\]:/.test(userText(r.body)));
  check(
    labelled.length > 0,
    "the room's user turn reaches the model prefixed with the speaker's name",
    userText(requests()[requests().length - 1]?.body || {}).slice(0, 300),
  );

  if (!team) {
    console.log("\nno room-scoped team: the team-chat phases cannot run");
    return report();
  }

  // ===== Phase 3: two humans in one team thread ===========================
  console.log("\n=== phase 3: activation, observation, and live attribution ===");
  const historyItems = async () => (await op.ok("teams.chat.history", { team_id: team.id })).items ?? [];
  const agentCount = async () => (await historyItems()).filter((i) => i.kind === "agent").length;

  const agentsBefore = await agentCount();
  const send1 = await conns.alice.ok("teams.chat.send", {
    team_id: team.id,
    message: "Kick off please",
  });
  check(
    Boolean(send1.run_id) && send1.observed === false,
    "with ONE human in the thread, a plain message still activates the roster",
    JSON.stringify(send1),
  );

  const gotReply1 = await until(async () => (await agentCount()) > agentsBefore, 240_000);
  check(Boolean(gotReply1), "the activated run answered into the shared transcript");

  // The live echo, asked once PER CONNECTION — the visibility index answers it
  // separately for each socket, so one socket seeing it proves nothing about
  // the other.
  const isAliceBubble = (f) =>
    f.topic === `team.${team.id}.message` && f.data?.author_user_id === alice.user_id;
  for (const who of ["alice", "bob"]) {
    const frame = await conns[who].waitFrame(isAliceBubble, 30_000);
    check(
      Boolean(frame),
      `${who}'s socket receives Alice's message frame, attributed to her`,
      conns[who].seen((f) => String(f.topic || "").startsWith("team.")).map((f) => f.topic).join("\n"),
    );
    if (frame) {
      check(
        frame.data?.author_display_name === "QA Alice",
        `${who}'s frame carries the display name, not just the id`,
        JSON.stringify(frame.data),
      );
    }
  }

  const agentsBefore2 = await agentCount();
  const send2 = await conns.bob.ok("teams.chat.send", {
    team_id: team.id,
    message: "I think that works",
  });
  check(
    send2.run_id === null && send2.observed === true,
    "a SECOND human speaking without an @-mention is observed, not dispatched",
    JSON.stringify(send2),
  );
  check(
    Boolean(send2.message_id),
    "an observed message is still persisted (it has a row id)",
    JSON.stringify(send2),
  );
  await sleep(3000);
  check(
    (await agentCount()) === agentsBefore2,
    "and no agent answered it",
    `agents before=${agentsBefore2} after=${await agentCount()}`,
  );
  const bobBubble = await conns.alice.waitFrame(
    (f) => f.topic === `team.${team.id}.message` && f.data?.author_user_id === bob.user_id,
    20_000,
  );
  check(
    Boolean(bobBubble),
    "an OBSERVED message is still broadcast live to the other human",
    "no team.<id>.message frame with Bob as author reached Alice",
  );

  // ===== Phase 4: @-mention re-activates, and raises a card ==============
  console.log("\n=== phase 4: @coder activates, and the card goes to the speaker ===");
  const deleteExisted = DELETE_PATH ? fs.existsSync(DELETE_PATH) : false;
  check(deleteExisted, "the fixture's delete target exists before the run", DELETE_PATH);

  const send3 = await conns.bob.ok("teams.chat.send", {
    team_id: team.id,
    message: "@coder QA-CARD please clean that up",
  });
  check(
    Boolean(send3.run_id) && send3.observed === false,
    "an @-mention from the second human activates the roster again",
    JSON.stringify(send3),
  );

  // Returns `{card}` when the server answered, `{error}` when it refused or
  // the call never landed. Folding a refusal into "no card" is the mistake the
  // repo's own criteria name — the fixture would then PASS the "the other
  // member cannot see it" assertion for entirely the wrong reason.
  const cardOf = async (conn) => {
    const r = await conn.attempt("exec.approvals.pending", {});
    if (r.error) return { error: r.error };
    const list = r.result?.pending ?? [];
    return { card: list.find((p) => JSON.stringify(p.record).includes("file_ops")) || null };
  };
  const bobsCard = await until(async () => (await cardOf(conns.bob)).card, 180_000);
  check(
    Boolean(bobsCard),
    "the member run's approval card is listed for BOB, who spoke",
    JSON.stringify(await cardOf(conns.bob)).slice(0, 600),
  );

  if (bobsCard) {
    log(`card ${bobsCard.record.id}: ${JSON.stringify(bobsCard.record)}`);
    const opSeen = await cardOf(op);
    check(Boolean(opSeen.card), "the operator sees the same card", JSON.stringify(opSeen.error ?? {}));
    const aliceSeen = await cardOf(conns.alice);
    check(
      !aliceSeen.error && !aliceSeen.card,
      "the OTHER member does not — and the server answered, it did not refuse",
      JSON.stringify(aliceSeen.error ?? aliceSeen.card?.record ?? {}).slice(0, 400),
    );
    check(
      bobsCard.record.originator_user_id === bob.user_id,
      "the card names Bob as its originator",
      JSON.stringify(bobsCard.record).slice(0, 400),
    );

    const resolved = await conns.bob.attempt("exec.approval.resolve", {
      id: bobsCard.record.id,
      decision: "allow-once",
      resolved_by: "QA Bob",
    });
    check(
      Boolean(resolved.result) && !resolved.error,
      "Bob can resolve his own card",
      JSON.stringify(resolved.error ?? resolved.result),
    );
    const gone = await until(async () => (DELETE_PATH ? !fs.existsSync(DELETE_PATH) : false), 90_000);
    check(
      Boolean(gone),
      "the approved tool actually ran (its target is gone)",
      `${DELETE_PATH} still exists — the approval unblocked the call but the call itself did not take effect`,
    );
  } else {
    skip("card ownership assertions", "no card was raised");
  }

  // ===== Phase 5: the speaker's name comes back out ======================
  console.log("\n=== phase 5: the agent addresses the person who spoke ===");
  const items = await historyItems();
  const agentSaid = items.filter((i) => i.kind === "agent").map((i) => i.content);
  check(
    agentSaid.some((c) => c.includes("QA Alice")),
    "an agent reply names the human who spoke (the transcript projection reached the prompt)",
    agentSaid.join("\n").slice(0, 500),
  );
  const humanRows = items.filter((i) => i.kind === "user");
  check(
    humanRows.some((i) => i.author_user_id === alice.user_id && i.author_display_name === "QA Alice") &&
      humanRows.some((i) => i.author_user_id === bob.user_id && i.author_display_name === "QA Bob"),
    "the durable transcript attributes each human row to its own author",
    JSON.stringify(humanRows.map((i) => [i.author_user_id, i.author_display_name, i.content.slice(0, 40)])),
  );

  // ===== Phase 6: the project page's tabs ================================
  console.log("\n=== phase 6: one effect assertion per project-page tab ===");

  // Kanban — the tab filters `teams.list` on the room's scope stamp.
  const teamsForBob = (await conns.bob.ok("teams.list", {})).teams ?? [];
  check(
    teamsForBob.some((t) => t.id === team.id && t.scope_id === `project:${project.id}`),
    "kanban: a room member's teams.list carries the room's team, scope-stamped",
    JSON.stringify(teamsForBob).slice(0, 500),
  );

  // Workspace — the read-only browse of the bound folder.
  const ws = await conns.bob.ok("projects.workspace.list", { project_id: project.id });
  check(
    ws.root_bound === true && (ws.entries ?? []).some((e) => e.name === "README.md"),
    "workspace: a member lists the bound directory",
    JSON.stringify(ws).slice(0, 400),
  );
  const wsRead = await conns.bob.ok("projects.workspace.read", {
    project_id: project.id,
    rel_path: "README.md",
  });
  check(
    (wsRead.content ?? "").includes("QA workspace"),
    "workspace: and reads a file out of it",
    JSON.stringify(wsRead).slice(0, 300),
  );
  const escape = await conns.bob.attempt("projects.workspace.read", {
    project_id: project.id,
    rel_path: "../../etc/hosts",
  });
  check(Boolean(escape.error), "workspace: a path that leaves the root is refused", JSON.stringify(escape.result));

  // Memory — a note written by a room run lands in the room's partition.
  //
  // ⚠️ What this proves, and what it does NOT. It proves the partition is
  // composed correctly end to end on the `chat.send` path, with two real
  // principals and a live run — which no unit test can do.
  //
  // It does NOT cover §3.17② (`request_scope_strings`, the FlowRequest
  // literal's uncorrected scope read). That defect needs a producer that
  // stamps `personal:<speaker>` onto a session key a room has ALREADY
  // claimed. `chat.send` is not one: `handlers::agent::resolve_attribution`
  // reads the scope off the persisted session row, so the metadata arriving
  // at `request_scope_strings` is already corrected and the correction is a
  // no-op here. `request_scope_strings`' own doc says so — "the Panel path
  // ... looks fine" — and so does the guard's: "at runtime a corrected read
  // and an uncorrected one are the same two strings ... all of the fixtures".
  //
  // Measured, not reasoned: on 2026-08-29 the correction was reverted to the
  // raw metadata read and this fixture ran 46/46 green. Do not read a pass
  // here as coverage of that fix; its guard is source-level on purpose.
  //
  // To actually reach it, a run must arrive from `inbound_router/executor.rs`
  // (channel inbound, which stamps from `pairing_store::sender_user`) or from
  // cron, ON a room-claimed session key. That is a channel fixture crossed
  // with this one, and it is the open item — not this phase.
  const beforeNote = requests().length;
  await conns.alice.ok("chat.send", {
    message: "QA-NOTE record that for the room",
    session_key: keyA,
    channel: "gui:qa-room",
  });
  // Same member ceiling as phase 2 — `note_manage` is not idempotent either,
  // so the write parks. Left unanswered it expires at 120 s and the partition
  // stays empty, which reads exactly like "the note went somewhere else".
  const noteCard = await approveOperatorCard("note_manage");
  check(
    Boolean(noteCard) && !noteCard.resolved.error,
    "memory: the room run's note write parks for approval, and the operator clears it",
    JSON.stringify(noteCard?.resolved?.error ?? "no note_manage card within 90s"),
  );
  // `project_scope::scoped_agent_id(base, ns)` is `{base}__{ns}` and the `ns`
  // for a room IS the project id — which already starts with `p-`. Composing
  // another `p-` here produced `main__p-p-…`, a partition nothing writes, and
  // the empty result read exactly like "the note never landed".
  const partition = `main__${project.id}`;
  const fact = await until(async () => {
    const r = await op.attempt("memory.listFacts", { agent_id: partition, limit: 50 });
    const facts = r.result?.facts ?? r.result?.items ?? [];
    return facts.find((f) => (f.content || f.path || "").toLowerCase().includes("qa-room-note")) || null;
  }, 240_000);
  check(
    Boolean(fact),
    `memory: the room run's note is readable back from partition ${partition}`,
    JSON.stringify(
      (await op.attempt("memory.listFacts", { agent_id: partition, limit: 50 })).result ?? {},
    ).slice(0, 500) + `\n(requests since: ${requests().length - beforeNote})`,
  );

  // ===== Phase 7: the delegated child inherits the room ===================
  //
  // `child_environment_context` fills a fresh `ResolvedContext` for a spawned
  // child, and `room_roster` was the one field it left unset — so the
  // `RoomRosterLayer` sitting in the pipeline both paths run rendered nothing
  // for a child, silently. Unit tests can assert the field is populated; only
  // a live run can show that the value it resolves from is still there at the
  // moment the child's prompt is built, one spawn and one task-local
  // re-establishment later.
  //
  // The oracle is the same request log phase 2 uses, read from the other end:
  // the child's own turn is the one whose USER text carries the marker the
  // parent's `subagent` call put in the task.
  console.log("\n=== phase 7: a delegated child inherits <room_context> ===");
  const beforeDelegate = requests().length;
  const runD = await conns.alice.ok("chat.send", {
    message: "QA-DELEGATE hand this one to a helper",
    session_key: keyA,
    channel: "gui:qa-room",
  });
  log(`alice's delegation run ${runD.run_id}`);
  // `subagent` is not on the read-only allowlist, so a member's turn parks it
  // the way phase 2's `team_create` parked — observed, not assumed: every run
  // of this phase has resolved a card here. The short budget is insurance
  // rather than doubt. If that ceiling ever stops applying, this phase should
  // say so in seconds instead of spending 90 s discovering there was no card,
  // and it must not fail for it: the claim being made is about the child's
  // prompt, not about which tier parks a spawn.
  const delegateCard = await approveOperatorCard("subagent", 45_000);
  log(delegateCard ? "subagent card resolved" : "no subagent card (tier allowed it)");

  // Both halves matter. `QA-CHILD` alone would also match the PARENT's next
  // turn if the harness ever flattens a `tool_result` into text — and the
  // parent's prompt does carry `<room_context>`, so that mismatch would pass
  // this phase while proving nothing. Excluding the delegation marker pins it
  // to the isolated child, whose conversation is only the task it was seeded
  // with.
  const isChildTurn = (r) => {
    const t = userText(r.body);
    return t.includes("QA-CHILD") && !t.includes("QA-DELEGATE");
  };
  const childReq = await until(
    async () => requests().slice(beforeDelegate).find(isChildTurn) || null,
    180_000,
  );
  if (childReq) log(`child turn #${childReq.turn}: ${userText(childReq.body).slice(0, 120)}`);
  check(
    Boolean(childReq),
    "the room turn's subagent call spawned a child that reached the provider",
    `requests since the delegation: ${requests().length - beforeDelegate}`,
  );
  if (childReq) {
    const childSystem = systemText(childReq.body);
    check(
      childSystem.includes("<room_context>"),
      "the DELEGATED CHILD's prompt carries <room_context>, not just the parent's",
      childSystem.slice(0, 800),
    );
    const childRoom =
      childSystem.split("<room_context>")[1]?.split("</room_context>")[0] ?? "";
    check(
      childRoom.includes("QA Alice") && childRoom.includes("QA Bob"),
      "the child's block names the same two members the parent's did",
      childRoom.trim() || "(no block)",
    );
  }

  // projects.changed — the sidebar's live refresh signal, per connection.
  for (const c of [conns.alice, conns.bob]) c.frames.length = 0;
  await op.ok("projects.rename", { id: project.id, name: "QA Room Renamed" });
  for (const who of ["alice", "bob"]) {
    const frame = await conns[who].waitFrame(
      (f) => f.topic === "projects.changed" && f.data?.project_id === project.id,
      20_000,
    );
    check(
      Boolean(frame),
      `projects.changed: the rename reaches ${who}'s socket live`,
      conns[who].frames.map((f) => f.topic).join(", ").slice(0, 400),
    );
  }

  for (const c of [op, conns.alice, conns.bob]) c.close();
  return report();
}

function report() {
  console.log(`\n=== ${PASS} passed, ${FAIL} failed ===`);
  for (const f of failures) console.log(`  FAILED: ${f}`);
  process.exit(FAIL === 0 ? 0 : 1);
}

main().catch((e) => {
  console.log(`FAIL  driver aborted: ${e.message}`);
  console.log(e.stack);
  console.log(`\n=== ${PASS} passed, ${FAIL + 1} failed ===`);
  process.exit(1);
});
