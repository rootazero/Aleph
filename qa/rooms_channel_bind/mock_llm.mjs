// Content-driven Anthropic-protocol stub, plus the webhook channel's outbound
// sink, in one process.
//
// Two servers would be two more things that can fail to come up; the callback
// is three lines of routing on a listener this fixture already needs. `POST
// /outbound` is what `[channels.webhook] callback_url` points at, so every
// reply the agent sends back into the group lands in OUTBOUND_LOG — which is
// the only oracle for "what did the group actually see".
//
// It decides on the LAST LABELLED LINE of the last user message — `[Ada]: ...`,
// the shape `nudges::speaker_prefixed` and `format_transcript` both produce —
// falling back to the whole text. A room's transcript is cumulative, so keying
// on "does the conversation mention this marker anywhere" would re-fire a
// marker answered three turns ago.
//
//   qa-note:<id>       -> tool_use note_manage(create, filename=<id>)
//   qa-delegate:<id>   -> tool_use subagent(run, task="qa-child:<id> …")
//   qa-bindws:<pid>|<p> -> tool_use project_manage(bind_workspace, …)
//   anything else      -> end_turn
//
// `note_manage` is the fixture's partition probe. It resolves its storage
// partition through `project_scope::session_write_id`, i.e. off the run's
// ambient `ScopeAttribution` — which is precisely the value every scenario
// here is making a claim about. One tool call per turn with a unique filename
// turns "whose scope did this turn run under" into a row in `notes_index` with
// an `agent_id` column, readable from disk without asking the server anything.
//
// Every request body is appended to REQUEST_LOG as one JSON object per line.
// Turn N+1 carries turn N's tool_result verbatim, so that file is the only
// oracle for what a tool handed back to the model — which is how addendum A
// reads the tier refusal out of `project_manage`.
//
// Usage: mock_llm.mjs <port> <request-log> <outbound-log>
import http from "node:http";
import fs from "node:fs";

const PORT = Number(process.argv[2] || 18923);
const REQUEST_LOG = process.argv[3] || "";
const OUTBOUND_LOG = process.argv[4] || "";

const T0 = Date.now();
const log = (...a) =>
  console.log(`${((Date.now() - T0) / 1000).toFixed(2)}s [mock]`, ...a);

let turns = 0;
/** Markers already delegated, so a child's own turn cannot re-delegate. */
const delegated = new Set();

/** Flatten one message's content into plain text. */
const textOf = (content) => {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .filter((b) => b && typeof b === "object" && b.type === "text")
    .map((b) => b.text || "")
    .join(" ");
};

/**
 * The `<system-reminder>` fence is NOT a reliable "this is scaffolding" test:
 * `user_interjection_note` wraps a REAL user message in the same fence when it
 * is steered into a live run, and `user_turn_text` also applies it to any user
 * message that merely FOLLOWS an assistant turn in the replayed log.
 */
const INTERJECTION_MARK = "The user sent the following message:";

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

/**
 * Every `qa-<verb>:<marker>` the user side of this request mentions, in order.
 *
 * The FIRST version of this keyed on `currentLine` — the last `[speaker]:`
 * labelled line, the shape a room transcript emits. That degraded silently as a
 * session grew: the prompt embeds a rendered transcript whose own lines are
 * `[user]:` / `[assistant]:`, so once a conversation was a few turns old the
 * "last labelled line" was a piece of scaffolding rather than the newest human
 * message, and the mock answered a marker three turns stale or none at all.
 *
 * Scanning for the markers directly, and answering each exactly once per
 * process, is order-independent — which is the property this fixture actually
 * needs, because the server (not the fixture) decides what order four different
 * conversations' runs reach the provider in.
 */
const MARKER = /qa-(note|delegate|bindws):([^\s"'\\]+)/g;

/** Markers already answered with a tool call. One answer each, ever. */
const answered = new Set();

/** What this turn emits: a tool call, or a final answer. */
const decide = (body) => {
  const msgs = body.messages || [];
  // A request with no tool surface is a SIDE CHANNEL — strategy synthesis,
  // topic naming, compaction, the working-memory assembler. Those carry the
  // conversation's text (markers and all) but cannot execute anything, so
  // answering one with a tool call is meaningless AND would burn the marker.
  if (!Array.isArray(body.tools) || body.tools.length === 0) {
    return { kind: "end", text: "QA side-channel answer." };
  }
  const userSide = (msgs || [])
    .filter((m) => m.role === "user")
    .map((m) => textOf(m.content))
    .join("\n");

  const hits = [...userSide.matchAll(MARKER)];
  const pending = hits.filter(([, , marker]) => !answered.has(marker));
  const [, verb, marker] = pending.length > 0 ? pending[pending.length - 1] : [];

  if (marker) {
    answered.add(marker);
    if (verb === "bindws") {
      const [projectId, wsPath] = marker.split("|");
      return {
        kind: "tool",
        name: "project_manage",
        input: {
          action: "bind_workspace",
          project_id: projectId,
          path: (wsPath || "").replace(/\+/g, " "),
        },
      };
    }
    if (verb === "delegate") {
      delegated.add(marker);
      return {
        kind: "tool",
        name: "subagent",
        input: {
          action: "run",
          // The child's task carries a DIFFERENT verb, so the child's own turn
          // cannot re-delegate even if it somehow saw the parent's text.
          task: `qa-child:${marker} — reply with one short sentence, then stop.`,
          context: "isolated",
          timeout_secs: 120,
        },
      };
    }
    return {
      kind: "tool",
      name: "note_manage",
      input: {
        action: "create",
        category: "project",
        filename: marker,
        title: marker,
        content: `Partition probe written by the QA fixture for marker ${marker}.`,
        tags: ["qa"],
      },
    };
  }

  const { label } = currentLine(msgs);
  return {
    kind: "end",
    text: label ? `Hi ${label}, noted.` : "Hi there, noted.",
  };
};

const sse = (payload) =>
  Buffer.from(`event: ${payload.type}\ndata: ${JSON.stringify(payload)}\n\n`);

const server = http.createServer((req, res) => {
  const url = req.url || "/";

  // The webhook channel's outbound leg. Recorded, never interpreted here —
  // the driver decides what a reply proves.
  if (req.method === "POST" && url.startsWith("/outbound")) {
    const chunks = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => {
      const raw = Buffer.concat(chunks).toString("utf8");
      if (OUTBOUND_LOG) {
        try {
          fs.appendFileSync(OUTBOUND_LOG, raw.replace(/\n/g, " ") + "\n");
        } catch (e) {
          log("could not append to the outbound log:", e.message);
        }
      }
      log(`outbound ${raw.slice(0, 160)}`);
      const body = JSON.stringify({ ok: true });
      res.writeHead(200, {
        "content-type": "application/json",
        "content-length": Buffer.byteLength(body),
      });
      res.end(body);
    });
    return;
  }

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
      `turn #${turn} speaker=${JSON.stringify(label)} ` +
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
  log(`listening on 127.0.0.1:${PORT} (provider + /outbound sink)`),
);
