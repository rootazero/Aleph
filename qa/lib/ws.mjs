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

/// Frames that matched none of the three shapes, since this module was loaded.
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
    if (topic === null) {
      unclassified += 1;
    }
  }
  return { topic, data, raw: msg };
}

/**
 * How many frames carried neither an `event` envelope nor a `topic`/`method`
 * key. A fifth server envelope shows up here as a number a fixture can print,
 * instead of as each fixture's product-shaped lie ("no frame arrived").
 */
export function unclassifiedFrameCount() {
  return unclassified;
}
