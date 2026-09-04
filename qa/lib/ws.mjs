// The one reader of the gateway's WebSocket frame envelope, shared by every
// Node QA driver that taps frames.
//
// `topic` is overloaded on this wire, in THREE shapes, and reading only one of
// them makes the tap blind to exactly the frames a scenario is chasing
// (`qa/teamchat_rooms` reported "no `team.<id>.message` ever arrived" for a
// whole round because of it):
//
//   {method:"event", params:{topic, data}}  bus events, as delivered
//   {topic, data}                            the same, un-enveloped
//   {method:"stream.x", params:{…}}          JSON-RPC notifications
//
// The authority for that list is NOT this file. It is
// `src/gateway/server/handler.rs::extract_topic_and_data`, the producer-side
// owner, whose doc records the identical failure mode: "missing this branch
// reads `topic` as the literal string `event`". This function mirrors it, so
// when a fourth producer shape appears there, it is mirrored HERE — one place,
// linked to the owner, instead of one private copy per driver.
//
// Deliberately just the normaliser. The `Conn` classes around it are NOT
// shared: `drive_r2.mjs` names its pending map `this.pending` where the others
// use `this.pendingReplies`, `drive_rooms.mjs`'s `attempt()` returns
// `{result, error}` where `drive_agents_viz.mjs` returns the raw reply, and
// the sleep/until budgets differ per fixture. Lifting those would change what
// individual fixtures assert.
//
// There is deliberately no `qa/lib/ws.py`: the Python fixtures as a family
// (`spend_budget/spend_rpc.py`, `run_halt/drive_halt.py`, `btw_tui/drive_btw_*.py`)
// read the single-shape `stream.*` JSON-RPC notification family and never
// observe a bus `event` frame at all, so a future Python fixture that needs a
// topic must port `normalizeFrame` first rather than assume one shape.

/// Frames that yielded no topic at all, since this module was loaded.
let unclassified = 0;

/**
 * Read one already-parsed WebSocket message into `{ topic, data, raw }`.
 *
 * `raw` is the untouched message: `qa/agents_viz`'s assertion D1 re-reads the
 * double-nested envelope off it (`raw.method === "event" &&
 * raw.params.topic === TOPIC`), so it is part of the contract, not a
 * convenience.
 */
export function normalizeFrame(msg) {
  let topic = null;
  let data = null;
  if (msg.method === "event" && msg.params) {
    topic = msg.params.topic ?? null;
    data = msg.params.data ?? msg.params;
  } else {
    topic = msg.topic ?? msg.method ?? null;
    data = msg.data ?? msg.params ?? null;
  }
  // Counted by the RESULT, not by the branch: an `event` envelope carrying no
  // `topic` reaches a fixture as exactly the same silence as a shape none of
  // the branches recognises, so counting only the `else` case would leave the
  // guard blind to one of the two ways of producing "no frame arrived".
  if (topic === null) {
    unclassified += 1;
  }
  return { topic, data, raw: msg };
}

/**
 * How many frames yielded no topic — neither an `event` envelope carrying one
 * nor a `topic`/`method` key. Read it through `frameDigest`, which is what the
 * fixtures print.
 */
export function unclassifiedFrameCount() {
  return unclassified;
}

/**
 * The failure-detail rendering of a tap: the topics that DID arrive, plus the
 * unclassified count.
 *
 * This is the line that makes the counter worth keeping. Every fixture
 * assertion that reports a missing frame renders its tap through this, so a
 * fifth server envelope surfaces as `unclassified: N` in the fixture's own
 * output instead of as its product-shaped lie ("no frame arrived") — a number
 * nobody can read is a display value with no renderer.
 */
export function frameDigest(frames) {
  const topics = frames.map((f) => f.topic ?? "(null)").join(",");
  // The count comes FIRST: a caller that truncates this line (the fixtures
  // bound their detail output) must not be able to cut off the one part that
  // is not visible anywhere else.
  return `unclassified: ${unclassifiedFrameCount()}; topics: ${topics || "(none)"}`;
}
