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
//   the repair text ("OUTCOME UNKNOWN" / "NOT EXECUTED") -> end_turn, so the
//                 resumed run FINISHES and the session's own `last_run` face
//                 can be observed settling to `clean`
//   anything else -> end_turn
//
// Every request body is appended to the request log as one JSON object per
// line. That file is the only oracle for "what was actually put in front of
// the model", which is the question every producer-side unit test in
// `session::boundary_repair` cannot answer.
//
// usage: mock_r2.mjs <port> <request-log>
import http from "node:http";
import fs from "node:fs";

const PORT = Number(process.argv[2] || 18932);
const REQUEST_LOG = process.argv[3] || "";
const BURST = Number(process.env.QA_BURST || 40);

const T0 = Date.now();
const log = (...a) => console.log(`${((Date.now() - T0) / 1000).toFixed(2)}s [mock]`, ...a);

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

const MARKER = /qa-(dangle|burst)(?::([\w-]+))?/g;

const decide = (body) => {
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

  // The boundary repair reached this turn: answer it and let the run END, so
  // the session's `last_run` can be watched settling to `clean`.
  if (userSide.includes("OUTCOME UNKNOWN") || userSide.includes("NOT EXECUTED")) {
    return { kind: "end", text: "QA: I see the previous call's outcome. Stopping here." };
  }

  const hits = [...userSide.matchAll(MARKER)];
  const pending = hits.filter((h) => !answered.has(h[0]));
  if (pending.length === 0) return { kind: "end", text: "QA: nothing to do." };
  const [whole, verb] = pending[pending.length - 1];
  answered.add(whole);

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

const server = http.createServer((req, res) => {
  if (req.method !== "POST") {
    const raw = JSON.stringify({ data: [{ id: "qa-model-a", type: "model" }] });
    res.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(raw) });
    res.end(raw);
    return;
  }
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
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
    const act = decide(body);
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

server.listen(PORT, "127.0.0.1", () => log(`listening on 127.0.0.1:${PORT}, burst size ${BURST}`));
