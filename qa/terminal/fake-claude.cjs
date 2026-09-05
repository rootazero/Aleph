#!/usr/bin/env node
// A stand-in for `claude`, for `qa/terminal/run.sh`.
//
// ⚠️ The shebang is load-bearing on UNIX and inert on Windows, and it has to be
// here rather than in the installer: `run.sh` copies this file to
// `$QA_ROOT/bin/claude` and Unix stages exec that path directly (the `cwd`
// stage spawns it as the pty child; `identify` types `claude` into `sh`).
// Without it the kernel answers ENOEXEC. Node ignores a shebang in any file it
// runs, so the Windows shim (`node "%~dp0claude"`) does not care.
//
// `run.sh` copies this file to `$QA_ROOT/bin/claude` — WITHOUT an extension,
// and the name is the point twice over:
//
//   * `agent_detect::lookup_agent` resolves a program to an agent by basename
//     (`crates/agent-detect/src/engine.rs`), so a file called `fake-claude`
//     identifies as nothing at all;
//   * the extension has to be absent on BOTH platforms. The probe reads the
//     runtime's argv, so the token it identifies is this file's path:
//     `claude.js` would normalise to `claude` for `agent` (the `.js` suffix is
//     stripped) but `normalized_program_name` returns the token AS INVOKED, so
//     `program` would read `claude.js` on Windows and `claude` on Unix — the
//     same fixture asserting two different wires. Node runs an extensionless
//     file as CommonJS, which is why this file is `.cjs` in the tree and has
//     no extension once installed.
//
// Nothing else about this file is claude-shaped: it paints three screens and
// sleeps.
//
// ## Where its screen text comes from
//
// NOT from here. `chrome.json`, generated beside this file by
// `derive_chrome.mjs`, carries the idle / working / blocked screens, each
// built from a named rule in `crates/agent-detect/src/manifests/claude.toml`
// and checked against that rule's own regex before it is written. A manifest
// whose wording moved fails there, loudly, instead of leaving this script
// painting chrome no rule matches — which on the wire is indistinguishable
// from detection being broken.
//
// ## Why Node and not bash
//
// The bash original could not run on Windows at all: there is no shebang
// support, so `PtySession::spawn` cannot execute an extensionless script, and
// the four automated stages all depend on this program starting. Node is the
// interpreter both platforms have here — this Windows host has no Python
// installed at all (`python3` on PATH is a stub that exits 49; `uv` is present
// but manages no interpreter yet). `run.sh`'s "Platform" section carries the
// full version.
//
// ## Knobs (all optional)
//
//   PHASE_SECS     seconds each screen stays up before the next (default 2)
//   QUIET=1        after the working screen, emit NOTHING for QUIET_SECS
//                  before moving on — the 30 s `quiet_since` clock in
//                  `gateway::runtime::QUIET_AFTER_MS` needs a real silence,
//                  and 35 s is that clock plus margin
//   QUIET_SECS     length of that silence (default 35)
//   QA_FAKE_CD     `chdir` here first, so the foreground probe's cwd is a
//                  directory neither the spawn dir nor the OSC 7 one
//   QA_FAKE_OSC7   emit `OSC 7` naming this directory
//   QA_CHROME      path to chrome.json (default: beside this file)
//
// The screen is cleared between phases (ED 2 + ED 3 + CUP home). Without that
// the earlier chrome stays visible and the HIGHEST-priority rule still on the
// grid wins: `live_turn_working` (970) would outrank the `live_prompt_box`
// (950) painted after it, and the fixture would assert a state the screen no
// longer shows.
"use strict";

const fs = require("node:fs");
const path = require("node:path");

const chromePath = process.env.QA_CHROME || path.join(__dirname, "chrome.json");
if (!fs.existsSync(chromePath)) {
  process.stderr.write(`fake-claude: no chrome at ${chromePath} (run derive_chrome.mjs first)\n`);
  process.exit(1);
}
const chrome = JSON.parse(fs.readFileSync(chromePath, "utf8"));

const phaseSecs = Number(process.env.PHASE_SECS || 2);
const quietSecs = Number(process.env.QUIET_SECS || 35);

// ED 2 (whole screen) + ED 3 (scrollback) + cursor home, then the chrome.
const paint = (text) => process.stdout.write(`\x1b[2J\x1b[3J\x1b[H${text}`);
const sleep = (secs) => new Promise((r) => setTimeout(r, secs * 1000));

async function main() {
  if (process.env.QA_FAKE_CD) {
    try {
      process.chdir(process.env.QA_FAKE_CD);
    } catch (e) {
      process.stderr.write(`fake-claude: cannot chdir to ${process.env.QA_FAKE_CD}: ${e}\n`);
      process.exit(1);
    }
  }

  if (process.env.QA_FAKE_OSC7) {
    // `file://` + an EMPTY-or-`localhost` host is the only form
    // `screen::perform::osc::parse_osc7_cwd` accepts; anything else names
    // another machine's filesystem and is dropped.
    //
    // The leading `/` is RFC 8089 and is what a Windows path needs: the drive
    // letter lives INSIDE the path component, so a correct emitter sends
    // `file:///C:/Users/x`. Without it the host/path split finds no `/` at all
    // and the whole payload is rejected — which reads on the wire as "this
    // session emitted no OSC 7", i.e. exactly the tier this stage is trying to
    // tell apart.
    const dir = process.env.QA_FAKE_OSC7;
    process.stdout.write(`\x1b]7;file://localhost${dir.startsWith("/") ? "" : "/"}${dir}\x07`);
  }

  paint(chrome.screens.idle.text);
  await sleep(phaseSecs);

  paint(chrome.screens.working.text);
  await sleep(phaseSecs);

  if (process.env.QUIET === "1") {
    // Not a `paint` — the point is that NOTHING reaches the screen, so the
    // session's last frame recedes past QUIET_AFTER_MS while its state stays
    // exactly where the working screen put it (spec R2-3).
    await sleep(quietSecs);
  }

  paint(chrome.screens.blocked.text);

  // Hold. An exited child is reported `visible_idle` by
  // `detection_update_for_publish_with_osc`, which would release the
  // working -> idle hold and turn the last screen into an Idle row — so the
  // process has to outlive every stage that reads it.
  await sleep(86400);
}

main();
