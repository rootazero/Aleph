// A deliberately slow OpenAI-images endpoint, and the oracle for this fixture.
//
//   mock_stall.mjs <port> <stall-ms> <request-log>
//
// It answers `POST /v1/images/generations` after `stall-ms`, which is longer
// than any timeout the scenarios configure. The interesting part is not the
// response — no scenario ever waits for it — it is what the server does to the
// connection while it waits.
//
// ## Why the log records the ABORT and not the reply
//
// "The tool call came back in 2.1 s with an error" has two causes that read
// identically from the client: the request timeout fired, or something else
// upstream refused before the request was ever made (no provider registered, a
// bad key, a URL that does not resolve). One of those proves the knob is wired
// and the other proves nothing at all.
//
// So every request writes one JSON line when it SETTLES, carrying whether the
// client hung up and how long it had been connected:
//
//   {"seq":1,"method":"POST","path":"/v1/images/generations",
//    "aborted":true,"held_ms":2013}
//
// `aborted:true` means the HTTP client dropped the connection mid-flight — the
// only thing a request-timeout actually does at this layer. A phase asserting
// "the cap fired" checks that line, not the tool's error string; a phase
// asserting "nothing capped it" checks that the request is still being held
// when the phase ends, which is a different observation from "no error came
// back".
import fs from "node:fs";
import http from "node:http";

const [portArg, stallArg, REQUEST_LOG] = process.argv.slice(2);
const PORT = Number(portArg);
const STALL_MS = Number(stallArg || 30_000);
if (!PORT || !REQUEST_LOG) {
  console.error("usage: mock_stall.mjs <port> <stall-ms> <request-log>");
  process.exit(2);
}

const T0 = Date.now();
const log = (...a) => console.log(`${((Date.now() - T0) / 1000).toFixed(2)}s [stall]`, ...a);

let seq = 0;
/** Requests currently being held open, so `inflight` can be reported on exit. */
const inflight = new Map();

const record = (entry) => {
  fs.appendFileSync(REQUEST_LOG, JSON.stringify(entry) + "\n");
  log(
    `#${entry.seq} ${entry.path} settled: ${entry.aborted ? "ABORTED BY CLIENT" : "answered"} after ${entry.held_ms}ms`,
  );
};

const server = http.createServer((req, res) => {
  // Readiness probe. Answered immediately — a fixture that cannot tell "the
  // mock is up" from "the mock is stalling" would report every startup race as
  // a timeout defect.
  if (req.method === "GET" && req.url === "/health") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end('{"ok":true}');
    return;
  }

  // Introspection: what is being held RIGHT NOW.
  //
  // A phase whose claim is "nothing cut this request" cannot assert on the
  // absence of a settled line — absence is also what "the request never
  // arrived" looks like, and those are opposite verdicts (判据 §8: an empty
  // answer only says "I do not know"). This endpoint turns that claim into a
  // positive one: the request exists, and it has been open for N ms.
  if (req.method === "GET" && req.url === "/inflight") {
    const now = Date.now();
    const held = [...inflight.entries()].map(([n, v]) => ({
      seq: n,
      path: v.path,
      held_ms: now - v.started,
    }));
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ held }));
    return;
  }

  const n = ++seq;
  const started = Date.now();
  const path = req.url || "";
  let settled = false;

  const settle = (aborted) => {
    if (settled) return;
    settled = true;
    inflight.delete(n);
    clearTimeout(timer);
    record({
      seq: n,
      method: req.method,
      path,
      aborted,
      held_ms: Date.now() - started,
    });
  };

  inflight.set(n, { started, path });
  log(`#${n} ${req.method} ${path} -- holding for ${STALL_MS}ms`);

  // Drain the body: an un-consumed request body can keep the socket from
  // reporting a close on some Node versions, which would make every phase look
  // like "the client never hung up".
  req.resume();

  // `close` fires for both outcomes; `writableEnded` is what separates them.
  // There is no "the client gave up" event that is not also fired on a normal
  // finish, so the discriminator has to be read, not listened for.
  res.on("close", () => settle(!res.writableEnded));

  const timer = setTimeout(() => {
    if (settled) return;
    res.writeHead(200, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        created: Math.floor(Date.now() / 1000),
        data: [{ url: `http://127.0.0.1:${PORT}/qa-image.png`, revised_prompt: "qa" }],
      }),
    );
  }, STALL_MS);
});

// Report what is still being held when the fixture tears the mock down. A
// phase whose claim is "nothing cut this request" needs positive evidence that
// the request existed and was still open, not merely the absence of a line.
const dumpInflight = () => {
  for (const [n, v] of inflight) {
    fs.appendFileSync(
      REQUEST_LOG,
      JSON.stringify({
        seq: n,
        method: "POST",
        path: v.path,
        aborted: false,
        still_open_at_shutdown: true,
        held_ms: Date.now() - v.started,
      }) + "\n",
    );
  }
};
for (const sig of ["SIGTERM", "SIGINT"]) {
  process.on(sig, () => {
    dumpInflight();
    process.exit(0);
  });
}

server.listen(PORT, "127.0.0.1", () => log(`listening on ${PORT}, stall=${STALL_MS}ms`));
