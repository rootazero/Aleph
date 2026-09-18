// The provider stub the round-2 stages drive.
//
// A new file rather than a reuse, and the reason is worth one paragraph: the
// two Node mocks this repo already has (`qa/teamchat_rooms`,
// `qa/rooms_channel_bind`) answer their markers with `note_manage` /
// `subagent` / `project_manage` — tools that RETURN. Every claim in this round
// needs a call that does NOT return (so it is still in flight when the server
// is killed) and, for the burst stage, one assistant message carrying many
// calls at once. Teaching either of those files a `bash sleep` arm would make
// a third fixture depend on it; the Python `mock_anthropic.py` next door
// already has the arm but cannot run on this host (no usable `python3`).
//
//   qa-dangle  -> tool_use bash{cmd:"sleep 120"}  — never returns, so the
//                 kill -9 lands on a genuinely open dispatch
//   qa-burst   -> ONE assistant message with $QA_BURST tool_use blocks of
//                 bash{cmd:"echo n"} — the projector queue is what is under
//                 test, so the calls must be cheap and simultaneous
//   qa-spawn   -> tool_use subagent{action:"run", task:"qa-child-slow: …"}.
//                 The `attribute` stage needs a call that is GENUINELY in
//                 flight when the server is killed: after §6.1 a call parked
//                 at the `ask` gate is answered by the fourth arm ("NOT
//                 EXECUTED"), not by the "OUTCOME UNKNOWN" wording whose
//                 provenance that stage asserts. A foreground sub-agent whose
//                 own model turn this mock holds for 120 s is one — the
//                 parent's `subagent` dispatch is durable and unanswered for
//                 as long as the child's request sits here.
//   qa-child-slow (in the user side of a tool-surfaced request) -> the
//                 child's turn: held $QA_CHILD_HOLD_MS (120 s), then end_turn
//   qa-bg      -> tool_use bash{cmd:"sleep 300", background:true} — the
//                 `tombstone` stage's orphan: a real OS process that outlives
//                 the `kill -9` of the server that spawned it. `sleep` is one
//                 spelling on both hosts (the bash tool's Windows shell is the
//                 probed PowerShell, where `sleep` aliases `Start-Sleep`).
//   qa-poll:<N>-<tag> -> bash{process_action:"poll", process_id:N}; the tag
//                 makes each turn's marker unique, so every poll is answered
//                 exactly once. `qa-kill:<N>-<tag>` is the same for `kill`.
//   the repair text ("OUTCOME UNKNOWN" / "NOT EXECUTED") -> end_turn, so the
//                 resumed run FINISHES and the session's own `last_run` face
//                 can be observed settling to `clean`. When a marker's tag
//                 starts with `slow` (`qa-dangle:slow-a`), the end is DELAYED
//                 $QA_SLOW_MS (4 s) first: the `parallel` stage needs two
//                 resumed runs to overlap long enough for a 150 ms poll of
//                 `gateway.metrics.run_concurrency` to see both in flight.
//   anything else -> end_turn
//
// Every request body is appended to the request log as one JSON object per
// line. That file is the only oracle for "what was actually put in front of
// the model", which is the question every producer-side unit test in
// `session::boundary_repair` cannot answer.
//
// `POST /v1/embeddings` is the one route that is NOT a model turn: the
// `unanswered` stage points the memory layer's embedding provider here
// (`patch_r2.mjs embed-stall`) and this route waits `$QA_EMBED_STALL_MS`
// before answering a valid 8-dim vector. The wait is the stage's instrument —
// it stretches the seed→RunStarted window (~30 ms on this host) into
// something a `kill -9` can be aimed into — so the line it logs carries a
// wall-clock ISO stamp: the driver's Step-0 check reads it back and proves
// the request fell between the `user_message` row and the kill. Deliberately
// kept out of the request log and the turn counter: neither an embedding
// request nor its answer was ever put in front of the model.
//
// usage: mock_r2.mjs <port> <request-log>
import http from "node:http";
import fs from "node:fs";

const PORT = Number(process.argv[2] || 18932);
const REQUEST_LOG = process.argv[3] || "";
const BURST = Number(process.env.QA_BURST || 40);

const T0 = Date.now();
const log = (...a) => console.log(`${((Date.now() - T0) / 1000).toFixed(2)}s [mock]`, ...a);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// How long a `slow`-tagged marker's END is held (the `parallel` overlap
// window) and how long a sub-agent child's turn is held (the `attribute`
// in-flight window). The child hold is a hard 120 s by default: it only has to
// outlive the kill, and a kill lands seconds after the dispatch is durable.
const SLOW_MS = Number(process.env.QA_SLOW_MS || 4000);
const CHILD_HOLD_MS = Number(process.env.QA_CHILD_HOLD_MS || 120_000);

let turns = 0;
/** Markers already answered with a tool call. One answer each, ever. */
const answered = new Set();

const textOf = (content) => {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((b) => (b && typeof b === "object" ? (b.text ?? b.content ?? "") : ""))
    .map((t) => (typeof t === "string" ? t : JSON.stringify(t)))
    .join(" ");
};

const MARKER = /qa-(dangle|burst|spawn|bg|poll|kill)(?::([\w-]+))?/g;

// `async` because two arms wait: the `slow`-tagged end and the child hold.
// One handler per request, so a held turn never blocks another session's.
const decide = async (body) => {
  const msgs = body.messages || [];
  // A request with no tool surface is a side channel (topic naming, strategy
  // synthesis, compaction). It carries the conversation's text, markers and
  // all, but cannot execute anything — answering one with a tool call burns
  // the marker on a turn that could never have dispatched it.
  if (!Array.isArray(body.tools) || body.tools.length === 0) {
    return { kind: "end", text: "QA side-channel answer." };
  }
  const userSide = msgs
    .filter((m) => m.role === "user")
    .map((m) => textOf(m.content))
    .join("\n");

  const hits = [...userSide.matchAll(MARKER)];
  // A `slow` tag anywhere in the conversation slows the END of every later
  // turn on it — the resumed run's included, which is the one that has to
  // overlap. The dispatching turn itself is never delayed: the dangle must
  // be durable before the shell's kill, and that timing is the driver's.
  const slow = hits.some((h) => (h[2] ?? "").startsWith("slow"));
  const end = async (text) => {
    if (slow) await sleep(SLOW_MS);
    return { kind: "end", text };
  };

  // The boundary repair reached this turn: answer it and let the run END, so
  // the session's `last_run` can be watched settling to `clean`.
  if (userSide.includes("OUTCOME UNKNOWN") || userSide.includes("NOT EXECUTED")) {
    return end("QA: I see the previous call's outcome. Stopping here.");
  }

  // A sub-agent child's own turn (`qa-spawn` above): hold it, so the parent's
  // `subagent` dispatch stays in flight across the kill. Checked AFTER the
  // repair arm: the parent's resumed request may quote the child's task text
  // inside the repair, and that turn must end, not hang.
  if (userSide.includes("qa-child-slow")) {
    log(`child turn held ${CHILD_HOLD_MS}ms`);
    await sleep(CHILD_HOLD_MS);
    return { kind: "end", text: "QA child: done waiting." };
  }

  const pending = hits.filter((h) => !answered.has(h[0]));
  if (pending.length === 0) return end("QA: nothing to do.");
  const [whole, verb, arg] = pending[pending.length - 1];
  answered.add(whole);

  // `qa-poll:12-a` → process 12; the suffix after the id is only there to
  // keep the marker unique per turn.
  const pid = Number.parseInt(String(arg ?? ""), 10);
  if (verb === "bg") {
    return { kind: "tools", calls: [{ name: "bash", input: { cmd: "sleep 300", background: true } }] };
  }
  if (verb === "poll") {
    return { kind: "tools", calls: [{ name: "bash", input: { process_action: "poll", process_id: pid } }] };
  }
  if (verb === "kill") {
    return { kind: "tools", calls: [{ name: "bash", input: { process_action: "kill", process_id: pid } }] };
  }
  if (verb === "spawn") {
    return {
      kind: "tools",
      calls: [{ name: "subagent", input: { action: "run", task: "qa-child-slow: wait for the operator" } }],
    };
  }
  if (verb === "burst") {
    return {
      kind: "tools",
      calls: Array.from({ length: BURST }, (_, i) => ({
        name: "bash",
        input: { cmd: `echo qa-burst-${i}` },
      })),
    };
  }
  // `BashExecArgs.cmd` (src/builtin_tools/bash_exec.rs), NOT `command`: the
  // wrong key deserialises to an EMPTY command under `#[serde(default)]`,
  // which returns instantly and never dangles at all.
  return { kind: "tools", calls: [{ name: "bash", input: { cmd: "sleep 120" } }] };
};

const sse = (p) => Buffer.from(`event: ${p.type}\ndata: ${JSON.stringify(p)}\n\n`);

const EMBED_STALL_MS = Number(process.env.QA_EMBED_STALL_MS || 0);

const server = http.createServer((req, res) => {
  if (req.method !== "POST") {
    const raw = JSON.stringify({ data: [{ id: "qa-model-a", type: "model" }] });
    res.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(raw) });
    res.end(raw);
    return;
  }
  if (req.url.endsWith("/embeddings")) {
    // Drain the body so the connection is reusable, but never log it: this
    // is not a model turn (see the header).
    req.on("data", () => {});
    req.on("end", () => {
      log(`embeddings request at ${new Date().toISOString()}; stalling ${EMBED_STALL_MS}ms`);
      setTimeout(() => {
        const raw = JSON.stringify({
          object: "list",
          data: [{ object: "embedding", index: 0, embedding: [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8] }],
          model: "qa-embed",
          usage: { prompt_tokens: 1, total_tokens: 1 },
        });
        res.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(raw) });
        res.end(raw);
      }, EMBED_STALL_MS);
    });
    req.on("error", (e) => log("embeddings request stream error (server killed?):", e.message));
    res.on("error", (e) => log("embeddings response stream error (server killed?):", e.message));
    return;
  }
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", async () => {
    let body = {};
    try {
      body = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
    } catch {
      body = {};
    }
    const turn = ++turns;
    if (REQUEST_LOG) {
      try {
        fs.appendFileSync(REQUEST_LOG, JSON.stringify({ turn, body }) + "\n");
      } catch (e) {
        log("could not append to the request log:", e.message);
      }
    }
    const act = await decide(body);
    // A held turn usually outlives the server that asked for it (that is the
    // point of holding it); nothing to answer into then.
    if (res.destroyed) {
      log(`turn #${turn} model=${body.model} -> ${act.kind}, but the requester is gone (server killed?)`);
      return;
    }
    log(`turn #${turn} model=${body.model} -> ${act.kind}${act.kind === "tools" ? `(${act.calls.length})` : ""}`);

    const content = [{ type: "text", text: act.kind === "end" ? act.text : "Working on it." }];
    if (act.kind === "tools") {
      act.calls.forEach((c, i) =>
        content.push({ type: "tool_use", id: `toolu_${turn}_${i}`, name: c.name, input: c.input }),
      );
    }
    const stop = act.kind === "tools" ? "tool_use" : "end_turn";

    if (!body.stream) {
      const raw = JSON.stringify({
        id: `msg_${turn}`,
        type: "message",
        role: "assistant",
        model: body.model || "qa-model-a",
        content,
        stop_reason: stop,
        usage: { input_tokens: 10, output_tokens: 10 },
      });
      res.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(raw) });
      res.end(raw);
      return;
    }

    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
    const w = (p) => res.write(sse(p));
    w({
      type: "message_start",
      message: {
        id: `msg_${turn}`,
        type: "message",
        role: "assistant",
        model: body.model || "qa-model-a",
        content: [],
        stop_reason: null,
        stop_sequence: null,
        usage: { input_tokens: 10, output_tokens: 1 },
      },
    });
    w({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } });
    w({
      type: "content_block_delta",
      index: 0,
      delta: { type: "text_delta", text: act.kind === "end" ? act.text : "Working on it." },
    });
    w({ type: "content_block_stop", index: 0 });
    if (act.kind === "tools") {
      act.calls.forEach((c, i) => {
        const idx = i + 1;
        w({
          type: "content_block_start",
          index: idx,
          content_block: { type: "tool_use", id: `toolu_${turn}_${i}`, name: c.name, input: {} },
        });
        w({
          type: "content_block_delta",
          index: idx,
          delta: { type: "input_json_delta", partial_json: JSON.stringify(c.input) },
        });
        w({ type: "content_block_stop", index: idx });
      });
    }
    w({ type: "message_delta", delta: { stop_reason: stop, stop_sequence: null }, usage: { output_tokens: 12 } });
    w({ type: "message_stop" });
    res.end();
  });
  // A killed server drops the connection mid-stream. That is the fixture
  // working, not a mock bug.
  req.on("error", (e) => log("request stream error (server killed?):", e.message));
  res.on("error", (e) => log("response stream error (server killed?):", e.message));
});

server.listen(PORT, "127.0.0.1", () =>
  log(
    `listening on 127.0.0.1:${PORT}, burst size ${BURST}, embedding stall ${EMBED_STALL_MS}ms, ` +
      `slow end ${SLOW_MS}ms, child hold ${CHILD_HOLD_MS}ms`,
  ),
);
