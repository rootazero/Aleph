// Content-driven Anthropic-protocol stub for the project-room QA.
//
// The busy-input mock (`qa/busy_input/mock_anthropic.py`) answers from a
// scripted PLAN keyed on a global turn counter, because its scenarios are about
// timing. This one answers from the CONTENT of the turn it was handed, because
// these scenarios put four different runs in flight (a room chat, two team
// leader runs and a member run) whose order is decided by the server, not by
// the fixture. A global counter would hand the card-raising turn to whichever
// run happened to arrive third.
//
// It decides on the LAST LABELLED LINE of the last user message — `[Ada]: ...`,
// the shape `nudges::speaker_prefixed` and `format_transcript` both produce —
// falling back to the whole text. That matters because a team transcript is
// cumulative: keying on "does the request mention QA-CARD anywhere" would make
// every later member turn raise another card.
//
//   QA-TEAMCREATE  -> tool_use team_create     (room-scoped team, S1/S3 setup)
//   QA-NOTE        -> tool_use note_manage     (writes into the room partition)
//   QA-CARD        -> tool_use file_ops:delete (destructive => approval card)
//   anything else  -> end_turn, "Hi <label>, noted."
//
// The last arm is not filler: it is the only end-to-end evidence that the
// speaker labelling reached the prompt at all. A model that echoes the name it
// was shown proves the projection; a fixed string would prove nothing.
//
// Every request body is appended to REQUEST_LOG as one JSON object per line.
// Turn N+1 carries turn N's tool_result verbatim, so that file is the only
// oracle for what a tool actually handed the model — and, for this round, for
// what the `<room_context>` layer actually emitted.
//
// Usage: mock_llm.mjs <port> <request-log> <delete-path>
import http from "node:http";
import fs from "node:fs";

const PORT = Number(process.argv[2] || 18913);
const REQUEST_LOG = process.argv[3] || "";
const DELETE_PATH = process.argv[4] || "";

const T0 = Date.now();
const log = (...a) =>
  console.log(`${((Date.now() - T0) / 1000).toFixed(2)}s [mock]`, ...a);

let turns = 0;

/** Flatten one message's content into plain text. */
const textOf = (content) => {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .filter((b) => b && typeof b === "object" && b.type === "text")
    .map((b) => b.text || "")
    .join(" ");
};

const blocksOf = (msg) => (Array.isArray(msg?.content) ? msg.content : []);
const hasToolResult = (msg) =>
  blocksOf(msg).some((b) => b && typeof b === "object" && b.type === "tool_result");
const hasToolUse = (msg) =>
  blocksOf(msg).some((b) => b && typeof b === "object" && b.type === "tool_use");

/**
 * The index of the last message a HUMAN turn actually put there.
 *
 * The `<system-reminder>` fence is NOT a reliable "this is scaffolding" test,
 * and the repo's own criteria say so: `user_interjection_note` wraps a REAL
 * user message in the same fence when it is steered into a live run. This
 * fixture sends its note request while the room's previous run is still in
 * flight, so the note arrived exactly that way — and treating the fence as
 * scaffolding made the mock walk back to turn 1, see the `team_create` call
 * that had already been answered, and skip the note entirely. The failure
 * looked like "the note never landed in the room partition".
 */
const INTERJECTION_MARK = "The user sent the following message:";

const humanTurnIndex = (msgs) => {
  for (let i = msgs.length - 1; i >= 0; i -= 1) {
    const m = msgs[i];
    if (m?.role !== "user" || hasToolResult(m)) continue;
    const t = textOf(m.content).trimStart();
    if (t.startsWith("<system-reminder>") && !t.includes(INTERJECTION_MARK)) continue;
    return i;
  }
  return -1;
};

/**
 * Whether this turn's tool call has already been made.
 *
 * Two wrong versions preceded this one, and the difference is worth keeping:
 *
 * - "is the LAST message a tool_result" misses, because the harness appends
 *   `<system-reminder>` user messages after the result — so the mock re-issued
 *   the same call forever.
 * - "has ANY tool call in this conversation come back" over-fires, because a
 *   room session is long-lived: the note the user asks for on turn 9 was
 *   silently skipped on the grounds that `team_create` had answered on turn 2.
 *
 * The question is per-TURN: has an assistant tool call happened since the last
 * thing a human said?
 */
const actedOnThisTurn = (msgs) =>
  msgs
    .slice(humanTurnIndex(msgs) + 1)
    .some((m) => m?.role === "assistant" && hasToolUse(m));

/**
 * The line this turn is actually answering, plus the speaker it names.
 *
 * `[label]: text` is what both the room projection and the team transcript
 * emit. Taking the LAST one across ALL user messages is what keeps a
 * cumulative transcript from re-triggering a marker answered three turns ago —
 * and reading only the last user message would find no label at all, because
 * the harness appends `<system-reminder>` blocks as user messages of their own.
 */
const LABELLED_LINE = /^\[([^\]\n]{1,80})\]:[ \t]*(.*)$/gm;

const currentLine = (msgs) => {
  const users = msgs.filter((m) => m.role === "user");
  const joined = users.map((m) => textOf(m.content)).join("\n");
  const matches = [...joined.matchAll(LABELLED_LINE)];
  if (matches.length > 0) {
    const last = matches[matches.length - 1];
    return { label: last[1], text: last[2] };
  }
  const plain = [...users].reverse().find((m) => {
    const t = textOf(m.content).trimStart();
    return !t.startsWith("<system-reminder>") || t.includes(INTERJECTION_MARK);
  });
  return { label: null, text: textOf(plain?.content) };
};

/** What this turn emits: a tool call, or a final answer. */
const decide = (body) => {
  const msgs = body.messages || [];
  if (actedOnThisTurn(msgs)) {
    return { kind: "end", text: "Tool finished; nothing further." };
  }
  // A request with no tool surface is a SIDE CHANNEL — strategy synthesis,
  // topic naming, compaction. Those carry the conversation's text (markers and
  // all) but cannot execute anything, so answering one with a tool call is
  // both meaningless and, for the auto-namer, actively confusing: it made the
  // room's first run look like it had emitted `team_create` twice.
  if (!Array.isArray(body.tools) || body.tools.length === 0) {
    return { kind: "end", text: "QA side-channel answer." };
  }
  const { label, text } = currentLine(msgs);
  if (text.includes("QA-TEAMCREATE")) {
    return {
      kind: "tool",
      name: "team_create",
      input: {
        name: "QA Room Team",
        description: "created inside a project room by the QA fixture",
        members: [{ agent_id: "coder", role: "coder" }],
      },
    };
  }
  if (text.includes("QA-NOTE")) {
    return {
      kind: "tool",
      name: "note_manage",
      input: {
        action: "create",
        category: "project",
        filename: "qa-room-note",
        title: "QA Room Note",
        content: "The room partition took this note during the QA run.",
        tags: ["qa"],
      },
    };
  }
  if (text.includes("QA-CARD")) {
    return {
      kind: "tool",
      name: "file_ops",
      input: { operation: "delete", path: DELETE_PATH },
    };
  }
  return {
    kind: "end",
    text: label ? `Hi ${label}, noted.` : "Hi there, noted.",
  };
};

const sse = (payload) =>
  Buffer.from(`event: ${payload.type}\ndata: ${JSON.stringify(payload)}\n\n`);

const server = http.createServer((req, res) => {
  if (req.method !== "POST") {
    const raw = JSON.stringify({ data: [{ id: "qa-mock-model", type: "model" }] });
    res.writeHead(200, {
      "content-type": "application/json",
      "content-length": Buffer.byteLength(raw),
    });
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
    const { label, text } = currentLine(body.messages || []);
    log(
      `turn #${turn} model=${body.model} speaker=${JSON.stringify(label)} ` +
        `line=${JSON.stringify(text.slice(0, 90))} -> ${act.kind}` +
        (act.kind === "tool" ? `:${act.name}` : ""),
    );

    if (!body.stream) {
      const content = [
        { type: "text", text: act.kind === "end" ? act.text : "Working on it." },
      ];
      if (act.kind === "tool") {
        content.push({
          type: "tool_use",
          id: `toolu_${turn}`,
          name: act.name,
          input: act.input,
        });
      }
      const raw = JSON.stringify({
        id: `msg_${turn}`,
        type: "message",
        role: "assistant",
        model: body.model || "qa-mock-model",
        content,
        stop_reason: act.kind === "tool" ? "tool_use" : "end_turn",
        usage: { input_tokens: 10, output_tokens: 10 },
      });
      res.writeHead(200, {
        "content-type": "application/json",
        "content-length": Buffer.byteLength(raw),
      });
      res.end(raw);
      return;
    }

    res.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });
    const w = (p) => res.write(sse(p));
    w({
      type: "message_start",
      message: {
        id: `msg_${turn}`,
        type: "message",
        role: "assistant",
        model: body.model || "qa-mock-model",
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
      delta: {
        type: "text_delta",
        text: act.kind === "end" ? act.text : "Working on it.",
      },
    });
    w({ type: "content_block_stop", index: 0 });
    if (act.kind === "tool") {
      w({
        type: "content_block_start",
        index: 1,
        content_block: { type: "tool_use", id: `toolu_${turn}`, name: act.name, input: {} },
      });
      w({
        type: "content_block_delta",
        index: 1,
        delta: { type: "input_json_delta", partial_json: JSON.stringify(act.input) },
      });
      w({ type: "content_block_stop", index: 1 });
    }
    w({
      type: "message_delta",
      delta: {
        stop_reason: act.kind === "tool" ? "tool_use" : "end_turn",
        stop_sequence: null,
      },
      usage: { output_tokens: 12 },
    });
    w({ type: "message_stop" });
    res.end();
  });
  // A cancelled run drops the connection mid-stream. That is evidence the
  // cancellation reached the in-flight provider call, not a fixture bug.
  req.on("error", (e) => log("request stream error (run cancelled?):", e.message));
  res.on("error", (e) => log("response stream error (run cancelled?):", e.message));
});

server.listen(PORT, "127.0.0.1", () =>
  log(`listening on 127.0.0.1:${PORT} (delete-path ${DELETE_PATH || "<unset>"})`),
);
