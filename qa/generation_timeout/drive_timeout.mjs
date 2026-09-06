// Real-machine driver for the generation request-timeout round.
//
//   drive_timeout.mjs <gateway-port> <mock-port> <mock-log> <phase>
//
// Phases: cap | auto | deploy | precedence   (see run.sh for what each proves)
//
// ## The oracle is the mock's connection, not the tool's error string
//
// `tools.invoke{image_generate}` reaches the SAME `GenerationProviderRegistry`
// the agent loop uses, built at boot by `create_provider` — which is the one
// place `request_timeout_secs()` is read. But the tool coming back with an
// error in ~2 s has at least four causes that are indistinguishable from the
// client: the request timeout fired, no provider registered, the key was
// rejected, the URL did not resolve. Only one of them is this round's claim.
//
// So every assertion here is made against `mock_stall.mjs`: either a settled
// line saying the server hung up (`aborted:true`) at roughly the configured
// second, or a live `/inflight` reading saying the request is STILL open past
// the moment a cap would have fired. Both are observations of the connection
// the timeout acts on.
//
// A run that never reaches the mock at all is reported as HARNESS_*, never as
// a failed assertion — "I could not ask" must not render as "the answer is no".
import fs from "node:fs";

const [portArg, mockPortArg, MOCK_LOG, PHASE = "cap"] = process.argv.slice(2);
const PORT = Number(portArg);
const MOCK_PORT = Number(mockPortArg);
if (!PORT || !MOCK_PORT || !MOCK_LOG) {
  console.error("usage: drive_timeout.mjs <gateway-port> <mock-port> <mock-log> <phase>");
  process.exit(2);
}

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
// Gateway connection — the minimum needed for one RPC.
// ---------------------------------------------------------------------------

class Conn {
  constructor() {
    this.pending = new Map();
    this.nextId = 1;
  }

  async open() {
    this.ws = new WebSocket(`ws://127.0.0.1:${PORT}/ws`);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("connect timeout")), 30_000);
      this.ws.addEventListener("open", () => {
        clearTimeout(timer);
        resolve();
      });
      this.ws.addEventListener("error", () => {
        clearTimeout(timer);
        reject(new Error("websocket error"));
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
      }
    });
    return this.rpc("connect", { client_type: "cli" });
  }

  rpc(method, params = {}, budget = 90_000) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`no reply to ${method} within ${budget}ms`));
      }, budget);
      this.pending.set(id, (msg) => {
        clearTimeout(timer);
        resolve(msg);
      });
      this.ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    });
  }

  close() {
    try {
      this.ws?.close();
    } catch {
      /* teardown */
    }
  }
}

// ---------------------------------------------------------------------------
// Mock observations
// ---------------------------------------------------------------------------

/** Settled requests, newest last. */
const settled = () => {
  if (!fs.existsSync(MOCK_LOG)) return [];
  return fs
    .readFileSync(MOCK_LOG, "utf8")
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

const inflight = async () => {
  const r = await fetch(`http://127.0.0.1:${MOCK_PORT}/inflight`);
  return (await r.json()).held || [];
};

/** Poll until `fn` returns something truthy, or give up. */
const until = async (fn, budget, every = 200) => {
  const end = Date.now() + budget;
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() >= end) return null;
    await sleep(every);
  }
};

/** Did the generation request ever reach the mock at all? */
const reachedMock = async (budget = 20_000) =>
  until(
    async () => {
      const held = await inflight().catch(() => []);
      if (held.length > 0) return true;
      return settled().some((r) => r.path.includes("/images/generations")) || false;
    },
    budget,
    150,
  );

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

// Each phase names the cap it expects to act, in seconds. `null` = no cap
// should fire within the observation window.
const EXPECTED_CAP_SECS = { cap: 2, deploy: 2, auto: null, precedence: null };
if (!(PHASE in EXPECTED_CAP_SECS)) {
  console.error(`unknown phase: ${PHASE}`);
  process.exit(64);
}
const expected = EXPECTED_CAP_SECS[PHASE];

/** How long a no-cap phase watches before concluding nothing cut the request. */
const NO_CAP_WATCH_MS = 8_000;

const main = async () => {
  const conn = new Conn();
  const hello = await conn.open();
  if (hello.error) {
    console.log(`HARNESS_CONNECT_REFUSED ${JSON.stringify(hello.error)}`);
    process.exit(3);
  }

  // Fire and do not await: on a no-cap phase the tool holds for the provider's
  // own default, which is minutes. The claim is about the connection, and the
  // connection is observable while the call is still in flight.
  const started = Date.now();
  const invocation = conn
    .rpc(
      "tools.invoke",
      { tool_name: "image_generate", arguments: { prompt: "qa stall probe" } },
      // Long enough to carry a 2 s cap plus the provider's retry ladder.
      60_000,
    )
    .catch((e) => ({ error: { message: e.message } }));

  if (!(await reachedMock())) {
    const reply = await Promise.race([invocation, sleep(2000).then(() => null)]);
    console.log("HARNESS_REQUEST_NEVER_REACHED_PROVIDER");
    console.log(`  the tool call answered: ${JSON.stringify(reply)}`);
    console.log("  (no provider registered / key gate / wrong base_url -- not a timeout verdict)");
    conn.close();
    process.exit(3);
  }
  log("the generation request reached the mock");

  if (expected === null) {
    // Negative arm. Watch past the second a cap WOULD have fired and require
    // the request to still be open — positive evidence, not an absent line.
    await sleep(NO_CAP_WATCH_MS);
    const held = await inflight().catch(() => []);
    const cut = settled().filter((r) => r.aborted);
    check(
      cut.length === 0,
      `no cap fires within ${NO_CAP_WATCH_MS / 1000}s`,
      `aborted requests: ${JSON.stringify(cut)}`,
    );
    check(
      held.length === 1 && held[0].held_ms >= NO_CAP_WATCH_MS - 1500,
      `the request is still open after ${NO_CAP_WATCH_MS / 1000}s`,
      `inflight: ${JSON.stringify(held)}`,
    );
  } else {
    // Positive arm. Wait for the abort, then check WHEN it happened: an abort
    // is only evidence for this cap if it landed near the configured second.
    const hit = await until(
      async () => settled().find((r) => r.aborted) || null,
      // The provider retries transient errors, and a timeout is transient, so
      // the FIRST abort is the one that dates the cap.
      expected * 1000 + 15_000,
      100,
    );
    const low = expected * 1000 - 700;
    const high = expected * 1000 + 2500;
    if (!check(Boolean(hit), `the ${expected}s cap cut the request`, "no aborted request logged")) {
      console.log(`  settled so far: ${JSON.stringify(settled())}`);
      // An empty ledger has TWO readings and they need different fixes: the
      // server never hung up, or it hung up and the mock failed to notice.
      // `/inflight` separates them -- it reports what the mock still holds
      // open, so a request that is STILL listed seconds after the cap should
      // have fired is positive evidence that nothing was cut (判据 §8: "no
      // record" is only entitled to say "I don't know").
      const timeline = [];
      for (let i = 0; i < 12; i += 1) {
        // eslint-disable-next-line no-await-in-loop
        const open = await inflight().catch(() => null);
        timeline.push(`${(i * 0.5).toFixed(1)}s:${open === null ? "?" : open.length}`);
        // eslint-disable-next-line no-await-in-loop
        await sleep(500);
      }
      console.log(`  /inflight over 6s (count of still-open requests): ${timeline.join(" ")}`);
      console.log(
        `  a flat non-zero line means the ${expected}s cap never reached the socket;\n` +
          "  a drop to 0 with an empty ledger means the MOCK missed the close.",
      );
    } else {
      check(
        hit.held_ms >= low && hit.held_ms <= high,
        `it cut at ~${expected}s, not at some other bound (held ${hit.held_ms}ms)`,
        `a cut far from ${expected}s means a DIFFERENT timeout acted -- connect timeout is ${8}s, ` +
          `the provider's own default is minutes`,
      );
    }
    // The elapsed wall time of the tool call is a second, independent witness
    // that nothing downstream simply swallowed the request.
    const reply = await Promise.race([invocation, sleep(20_000).then(() => "TIMED_OUT_WAITING")]);
    const elapsed = Date.now() - started;
    log(`tool call settled after ${elapsed}ms`);
    // Always show what came back, not only on the branch that asserts about it.
    // When the cap does NOT fire, this reply is the single most informative
    // artifact in the run -- it names which error the provider raised, and the
    // failing run that motivated this line had no other way to say so.
    log(`  reply: ${JSON.stringify(reply).slice(0, 600)}`);
    if (reply !== "TIMED_OUT_WAITING") {
      check(
        Boolean(reply.error) || reply.result?.ok === false,
        "the capped call surfaces as an error rather than a silent success",
        JSON.stringify(reply).slice(0, 400),
      );
    }

    // Every attempt, not just the first one that dated the cap.
    //
    // `timeout_seconds` is a PER-ATTEMPT cap. The provider retries a timeout,
    // so an operator who writes 2 waits about 7 s: measured here as three
    // attempts of ~2 s each. That is worth pinning, because "the first abort
    // landed at 2 s" is also true in a world where the cap governs only the
    // first attempt and a later one runs unbounded — the exact failure this
    // knob exists to prevent.
    //
    // The assertion is on the SHAPE of every attempt, never on the retry
    // count: the count is a provider policy that may legitimately change, while
    // "each attempt is bounded by the configured second" is the contract.
    //
    // Let the ledger go quiet before reading it. The mock records an attempt
    // when its socket CLOSES, which happens slightly after the tool call
    // resolves, so sampling the moment the reply lands can miss the final
    // retry: the first version of this line printed "2 attempts" on one run and
    // "3" on the next with the same ~7 s wall time. A count that moves between
    // runs is not a measurement, and it was being printed inside a PASS
    // (判据 §18 — instruments lie, and your own is the one to distrust first).
    let attempts = settled();
    for (let quiet = 0; quiet < 6; quiet += 1) {
      await sleep(400);
      const again = settled();
      if (again.length === attempts.length) break;
      attempts = again;
    }
    const strays = attempts.filter(
      (r) => !r.aborted || r.held_ms < low || r.held_ms > high,
    );
    check(
      attempts.length >= 1 && strays.length === 0,
      `every attempt was cut at ~${expected}s -- ${attempts.length} attempt(s), ` +
        `held ${attempts.map((r) => r.held_ms).join("/")}ms, tool call ${elapsed}ms wall`,
      `attempts NOT governed by the cap: ${JSON.stringify(strays)}`,
    );
  }

  conn.close();
  console.log(`\n${PASS} passed, ${FAIL} failed  (phase ${PHASE})`);
  process.exit(FAIL === 0 ? 0 : 1);
};

main().catch((e) => {
  console.log(`HARNESS_DRIVER_CRASHED ${e.stack || e}`);
  process.exit(3);
});
