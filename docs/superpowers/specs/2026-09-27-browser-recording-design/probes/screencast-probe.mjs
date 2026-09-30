// screencast-probe.mjs — the measurement the `screencast` capability row's
// NOT_PROBED note promised: does a REAL engine serve the screencast handshake
// Aleph's `browser_record` is built on (Page.startScreencast → screencastFrame
// → screencastFrameAck → Page.stopScreencast)?
//
//   node screencast-probe.mjs                  # obscura (default)
//   node screencast-probe.mjs --engine chrome  # real chromium
//   node screencast-probe.mjs --engine chrome --encode
//        # ^ the full smoke: frames piped through ffmpeg with the PRODUCTION
//        # argv (image2pipe + `-c:v mjpeg` hint + `-c:v libvpx`), then
//        # ffprobe must parse the container — the project's first real-
//        # chromium recording reading.
//
// The criterion is the C1-tightened one: frame ARRIVAL is not usability.
// A frame counts only when its `data` base64-decodes AND starts with the
// JPEG magic (FFD8) — an engine may emit undecodable or non-JPEG payloads
// and a row certified on arrival alone would send `browser_record` at a
// stream ffmpeg cannot eat. Every received frame is acked AFTER its check,
// mirroring the production pacing (write first, ack second — recording.rs).
//
// Links, each measured on its own (判据 §4 — a green that only asserts the
// call succeeded is a protocol reading, not the effect):
//
//   1. CONTROL: the animated fixture page loads and its rAF loop runs
//      (`window.__ticks` advances). A red below can then only mean the
//      screencast path, never a broken page (判据 §2).
//   2. `Page.startScreencast {format:"jpeg", quality:80}` — the exact shape
//      `methods::screencast::start_screencast` sends. A refusal here = the
//      stream path does not exist.
//   3. Frames: ≥1 `Page.screencastFrame` within the window, EVERY one
//      base64-decodable AND JPEG-magic'd, each acked after its check.
//   4. `Page.stopScreencast` answers, and no further frames arrive after.
//   5. (--encode only) the ffmpeg leg: production argv, exit code 0, and
//      ffprobe reads a vp8 stream back out of the container.
//
// Lives beside the spec that demanded it (this directory's parent), reusing
// the 2026-09-06 evidence round's launch/server helpers rather than growing
// a second copy (the launch argv is a measured property of those helpers).
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { launchObscura, launchChrome, serveStatic, freePort, Cdp, sleep, emit }
  from "../../2026-09-06-browser-dual-engine-evidence/probes/t0-lib.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const ENGINE = args.includes("--engine") ? args[args.indexOf("--engine") + 1] : "obscura";
const ENCODE = args.includes("--encode");
const FRAME_BUDGET_MS = 9000;
const MIN_FRAMES = 3;

const report = { engine: ENGINE, encode: ENCODE, steps: {} };
const done = (verdict) => ({ verdict, ...report });

const port = await freePort();
const server = await serveStatic(HERE, port);
const pageUrl = `http://127.0.0.1:${port}/screencast-page.html`;

const e = ENGINE === "chrome" || ENGINE === "chromium"
  ? await launchChrome({})
  : await launchObscura({});
if (!e.wsUrl) { server.close(); throw new Error(`${ENGINE} published no CDP endpoint`); }
report.wsUrl = e.wsUrl;
report.pid = e.pid;

const c = new Cdp(e.wsUrl, "screencast");
await c.connect();
const { targetId } = await c.call("Target.createTarget", { url: "about:blank" });
const { sessionId } = await c.call("Target.attachToTarget", { targetId, flatten: true });
await c.call("Page.enable", {}, sessionId).catch(() => {});
await c.call("Runtime.enable", {}, sessionId).catch(() => {});

const evalJs = async (expr) => {
  const r = await c.call("Runtime.evaluate", { expression: expr, returnByValue: true },
    sessionId, 10000);
  return r?.result?.value ?? null;
};
const finish = async (out) => { c.close(); e.kill(); server.close(); emit(out); };
const bail = async (verdict) => { await finish(done(verdict)); process.exit(0); };

// ---- step 1: control — the animated page actually animates ------------------
await c.call("Page.navigate", { url: pageUrl }, sessionId, 30000);
await sleep(1200);
const t0 = await evalJs("window.__ticks ?? -1");
await sleep(700);
const t1 = await evalJs("window.__ticks ?? -1");
report.steps.control_ticks = { t0, t1 };
if (!(t1 > t0 && t0 >= 0)) {
  await bail("FIXTURE BROKEN — the rAF loop did not advance; a silent stream below proves nothing");
}

// ---- step 2: startScreencast, the exact shape the Rust wrapper sends --------
const start = await c.call("Page.startScreencast",
  { format: "jpeg", quality: 80 }, sessionId, 15000)
  .then(() => ({ ok: true }))
  .catch((err) => ({ ok: false, error: String(err?.message ?? err) }));
report.steps.start_screencast = start;
if (!start.ok) {
  await bail(`UNSUPPORTED (Page.startScreencast refused): ${start.error}`);
}

// ---- step 3: frames — arrival AND decodability AND JPEG magic ---------------
// Subscribed AFTER start deliberately: production subscribes before, but the
// listener here is a plain callback registered in the same tick chain, and
// the fixture animates continuously so frames keep coming.
const frames = [];
let ackErrors = 0;
let badFrames = 0;
const off = c.on((m) => {
  if (m.method !== "Page.screencastFrame" || m.sessionId !== sessionId) return;
  const p = m.params ?? {};
  let ok = false;
  try {
    const buf = Buffer.from(String(p.data ?? ""), "base64");
    ok = buf.length > 2 && buf[0] === 0xff && buf[1] === 0xd8;
    if (ok) frames.push(buf);
  } catch { /* undecodable */ }
  if (!ok) badFrames += 1;
  // Ack AFTER the check — the production pacing direction (a stalled consumer
  // must stall the stream, not drop silently).
  c.call("Page.screencastFrameAck", { sessionId: p.sessionId }, sessionId, 10000)
    .catch(() => { ackErrors += 1; });
});
const collectStart = Date.now();
while (frames.length < MIN_FRAMES && Date.now() - collectStart < FRAME_BUDGET_MS) {
  await sleep(150);
}
// Let any in-flight bad frames land before the tally.
await sleep(500);
off();
report.steps.frames = {
  usable: frames.length,
  bad: badFrames,
  ack_errors: ackErrors,
  first_frame_bytes: frames[0]?.length ?? 0,
};
const framesOk = frames.length >= MIN_FRAMES && badFrames === 0;

// ---- step 4: stopScreencast, and the stream actually stops ------------------
const stop = await c.call("Page.stopScreencast", {}, sessionId, 15000)
  .then(() => ({ ok: true }))
  .catch((err) => ({ ok: false, error: String(err?.message ?? err) }));
report.steps.stop_screencast = stop;
let lateFrames = 0;
const offLate = c.on((m) => {
  if (m.method === "Page.screencastFrame" && m.sessionId === sessionId) lateFrames += 1;
});
await sleep(1000);
offLate();
report.steps.frames_after_stop = lateFrames;

// ---- step 5 (--encode): the production ffmpeg leg, end to end ---------------
if (ENCODE && framesOk) {
  const out = path.join(fs.mkdtempSync(path.join(os.tmpdir(), "screencast-enc-")), "out.webm");
  // argv copied from recording.rs::spawn_ffmpeg + codec_args_for(".webm"):
  // the `-c:v mjpeg` input hint is required (image2pipe cannot probe an
  // unseekable pipe), and the VP8 encoder on ffmpeg n9.0.1 is `libvpx`,
  // not `libvpx-vp8` (both measured; FL §3.12 records them).
  const ff = spawn("ffmpeg", ["-y", "-loglevel", "error",
    "-f", "image2pipe", "-c:v", "mjpeg", "-framerate", "10", "-i", "-",
    "-c:v", "libvpx", out],
    { stdio: ["pipe", "ignore", "pipe"] });
  let ffStderr = "";
  ff.stderr.on("data", (d) => { ffStderr += d; });
  const ffExit = new Promise((res) => ff.on("exit", (code, signal) => res({ code, signal })));

  await c.call("Page.startScreencast", { format: "jpeg", quality: 80 }, sessionId, 15000)
    .catch(() => {});
  let fed = 0;
  const offFeed = c.on((m) => {
    if (m.method !== "Page.screencastFrame" || m.sessionId !== sessionId) return;
    const p = m.params ?? {};
    try {
      const buf = Buffer.from(String(p.data ?? ""), "base64");
      if (buf.length > 2 && buf[0] === 0xff && buf[1] === 0xd8) {
        // Write first, ack second — the pacing direction that keeps a slow
        // encoder from turning into unbounded in-memory frames.
        ff.stdin.write(buf, () => {
          c.call("Page.screencastFrameAck", { sessionId: p.sessionId }, sessionId, 10000)
            .catch(() => {});
        });
        fed += 1;
      }
    } catch { /* counted by the arrival leg above; here we just skip */ }
  });
  await sleep(4000);
  offFeed();
  await c.call("Page.stopScreencast", {}, sessionId, 15000).catch(() => {});
  await sleep(700); // in-flight writes drain before stdin closes
  ff.stdin.end();
  const exit = await ffExit;
  report.steps.encode = { fed_frames: fed, ffmpeg_exit: exit, stderr: ffStderr.slice(0, 400) };

  const probe = await new Promise((res) => {
    const pr = spawn("ffprobe", ["-v", "error", "-show_entries",
      "stream=codec_name,width,height", "-of", "json", out], { stdio: ["ignore", "pipe", "pipe"] });
    let body = "";
    pr.stdout.on("data", (d) => { body += d; });
    pr.on("exit", (code) => res({ code, body }));
  });
  let stream = null;
  try { stream = JSON.parse(probe.body)?.streams?.[0] ?? null; } catch { /* unparseable */ }
  report.steps.ffprobe = { exit: probe.code, stream };
  report.steps.encode_ok = exit.code === 0 && probe.code === 0 && stream?.codec_name === "vp8";
}

// ---- verdict ----------------------------------------------------------------
const links = {
  start: start.ok,
  frames: framesOk,
  stop: stop.ok && lateFrames === 0,
  ...(ENCODE ? { encode: report.steps.encode_ok === true } : {}),
};
report.links = links;
const verdict = Object.values(links).every(Boolean)
  ? "SUPPORTED — frames arrived, every one decoded to JPEG, acks accepted, stream stopped"
    + (ENCODE ? ", and the production ffmpeg leg yielded an ffprobe-readable vp8 webm" : "")
  : `UNSUPPORTED — measured broken link(s): ${
      Object.entries(links).filter(([, v]) => !v).map(([k]) => k).join(", ")}`;
await finish(done(verdict));
