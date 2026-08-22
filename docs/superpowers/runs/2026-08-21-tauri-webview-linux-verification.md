# Linux verification — Tauri cross-platform WebView resource control (G1–G5)

**Date:** 2026-08-22
**Machine:** Ubuntu 26.04 LTS, x86_64, 10 cores / 15 GiB RAM
**Mode:** read-only diagnostic. No source file was modified, nothing committed, nothing pushed.
**Verifier's note:** every value below is the value actually read, not a restatement of the
expectation. Where I could not verify something, it is named in §8 rather than omitted.

---

## 0. Code under test

| | |
|---|---|
| `git rev-parse HEAD` | `064d036fcbbca9e29ab31e598852a3cb2fd9f31f` |
| Expected by runbook | `791e2b9534e6d8df40011c00a98b6c125b8eda4c` |
| Relationship | **`791e2b953` is an ancestor of HEAD** (`git merge-base --is-ancestor` → true) |
| `git status --short` | clean |

HEAD is a `Merge remote-tracking branch 'origin/main'` sitting on top of the webview work,
which pulled in ~280 unrelated files (per-principal spend budget, `/btw` side questions,
memory curated round-2, …).

**I did not stop, and here is the check that justifies continuing:**

```
git diff 791e2b953..HEAD -- qa/webview_compat/ desktop/shell/   →  0 lines
```

The verification surface is byte-identical. The only webview-adjacent change is that
`interfaces/webchat/dist/` was rebuilt by the merged Panel work, which moves the G2 numbers
(§2). Everything else in G1–G5 is untouched, so the run is valid for the change it targets.

⚠️ One consequence worth flagging to the main machine: **the runbook's stated byte sizes are
already stale.** It says `22,177,008 → 3,396,020`; the tree now holds `22,193,722 → 3,400,989`.
Nothing is wrong — the Panel was rebuilt — but a fixture that hard-codes those numbers would
now be red for a reason that has nothing to do with brotli.

---

## 1. Environment facts

| Fact | Value |
|---|---|
| Distro | `Ubuntu 26.04 LTS (Resolute Raccoon)` |
| Kernel | `7.0.0-30-generic` |
| **WebKitGTK (`webkit2gtk-4.1`)** | **`2.52.3`** — floor is 2.42, so **10 minor versions above the G1 minimum** |
| `webkit2gtk-4.0` | not present (4.1 only) |
| `XDG_SESSION_TYPE` | `tty` — **no graphical session in this shell** |
| Actual display used | `:10`, a real X11 server (`Xorg :10 … xrdp/xorg.conf`, X.Org 21.1.22) |
| GPU | none usable — `libEGL: failed to open /dev/dri/renderD128: 权限不够`; software rendering throughout |
| GStreamer | `1.28.2`, 109 plugins / 618 features |
| Aleph `VERSION` | `26.7.31` |
| rustc | `1.96.0 (ac68faa20 2026-05-25)` (matches pinned toolchain) |

Because WebKitGTK is 2.52.3, **the G1 "too old" path is structurally unreachable on this box.**
Nothing here proves the fallback page renders correctly on a <2.42 machine; see §8.

### GStreamer decoder matrix (`gst-inspect-1.0 --exists`)

| Element | Result |
|---|---|
| `mpg123audiodec` | **present** |
| `avdec_mp3` | MISSING |
| `avdec_aac` | MISSING |
| `faad` | MISSING |
| `opusdec` | **present** |
| `vp8dec` | **present** |
| `vp9dec` | **present** |

Installed plugin packages: `gstreamer1.0-{0,alsa,gl,plugins-base,plugins-good,tools,x}`.
Neither `-bad`, `-ugly`, nor `-libav` is installed.

**This box is a genuinely useful G4 fixture**: it is neither all-green nor all-missing. MP3 has
a decoder, AAC has none. That exercises the middle (`Missing`) verdict rather than the trivial
one — which is what makes §4's consistency check meaningful.

---

## 2. G2 — brotli precompression (observed)

| Asset | raw | `.br` | ratio |
|---|---:|---:|---:|
| `aleph_panel_bg.wasm` | 22,193,722 | 3,400,989 | 6.5× |
| `aleph_panel.js` | 110,252 | 13,377 | 8.2× |
| `tailwind.css` | 145,380 | 19,477 | 7.5× |
| `index.html` | 6,925 | 2,411 | 2.9× |

`.br` siblings are committed build products, so no wasm toolchain was needed, as the runbook says.

---

## 3. Step 3 — `qa/webview_compat/run.sh linux`

### 3a. Baseline (no `ARTIFACT_URL`)

```
== webview_compat (linux) against http://127.0.0.1:18790 ==
  PASS  br-negotiation: content-encoding
  PASS  br-negotiation: body under 4 MiB
  SKIP  br-negotiation: sha comparison
        reason: python3 with the 'brotli' module not available
  PASS  br-negotiation: identity is honoured
  PASS  br-negotiation: an explicit br;q=0 refusal is honoured
  SKIP  range-206 / range-416
        reason: set ARTIFACT_URL to a capability URL for an artifact of >=200 bytes
  PASS  gst-codecs: MP3 decoder present

== 5 passed, 0 failed, 2 skipped ==
EXIT=0
```

### 3b. Final run, with `ARTIFACT_URL` supplied

```
== webview_compat (linux) against http://127.0.0.1:18790 ==
  PASS  br-negotiation: content-encoding
  PASS  br-negotiation: body under 4 MiB
  SKIP  br-negotiation: sha comparison
        reason: python3 with the 'brotli' module not available
  PASS  br-negotiation: identity is honoured
  PASS  br-negotiation: an explicit br;q=0 refusal is honoured
  PASS  range-206: status
  PASS  range-206: exactly 100 bytes
  PASS  range-206: content-range
  PASS  range-416: status
  PASS  range-416: content-range
  PASS  gst-codecs: MP3 decoder present

== 10 passed, 0 failed, 1 skipped ==
EXIT=0
```

**10 passed, 0 failed, 1 skipped — and the one SKIP is closed below by other means.**

### 3c. How the Range assertions were unlocked without an LLM turn

The runbook suggests having an agent publish an artifact. This box has no provider
credentials, so instead I used **`session.export_html`**, which `method_census.rs` lists as
`Class::Open` and which returns `{url, filename, size}` directly:

```
session.create {title}            → session_key "agent:main:session_1787400153006"
session.export_html {session_key} → 8,379-byte HTML + capability URL
```

Driven over the gateway WebSocket by Node 24's **built-in** `WebSocket` (loopback connect is
unconditionally operator). No pip, no npm, no system packages installed — this box has neither
`pip` nor the `websockets` module, and I did not add them.

### 3d. Closing the brotli SKIP with a different tool

The SKIP is an artifact of this machine (no python `brotli` module), not of the code. Node ships
brotli in its stdlib, so the same claim was checked with the same bytes:

```
dist_wasm_bytes      = 22193722
downloaded_br_bytes  = 3400989
decompressed_bytes   = 22193722
dist_sha256          = 6079753907ddc90f274a8dbd42e96e59c3f6dc1d797095165b80abf740b697ed
decompressed_sha256  = 6079753907ddc90f274a8dbd42e96e59c3f6dc1d797095165b80abf740b697ed
RESULT: same
```

**Byte-identical.** The served `.br` really is the dist wasm.

---

## 4. Step 4 — `media/codecs` doctor check vs. raw `gst-inspect`

**Prediction made *before* running doctor**, from the §1 matrix: MP3 satisfied by
`mpg123audiodec`; AAC has neither `avdec_aac` nor `faad`; Opus and VP8/VP9 satisfied ⇒ expect
`Missing(["AAC"])`.

Actual output:

```
[warn] media/codecs Missing media decoders
    GStreamer has no decoder for: AAC. Voice replies and media attachments in these formats
    will fail to play in the Panel, and the failure is silent at the WebKitGTK layer.
    fix: AAC: install gstreamer1.0-plugins-bad (or gstreamer1.0-libav)
```

**Consistency: exact.** Severity `warn` (not ok, not error), the detail names the missing format,
and the fix hint names an installable package. The three-state discriminator behaves as
designed for the state this machine is in. This is the first time G4 has produced output.

The `[info] … unknown` state was **not** exercised — `gst-inspect-1.0` is installed here (§8).

---

## 5. Step 5 — desktop shell and the flat-mode degradation

### 5a. First-ever Linux compile of the shell

`just` is not installed on this box; the `_stage-shell-placeholders` recipe was replicated by
hand (`touch desktop/shell/binaries/aleph-server-x86_64-unknown-linux-gnu`; that path is
gitignored via `desktop/shell/.gitignore:6`, so the tree stayed clean).

```
cargo check -p aleph-desktop-shell
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m 21s
```

**It compiles. No errors, no warnings.** The `SHELL_MARKER_JS` arm used here is
`#[cfg(not(any(target_os = "macos", target_os = "windows")))]` — a catch-all, structurally
identical to the other two — so the "never compiled" arm is now compiled and clean.

### 5b. Instrumented WebKitGTK instead of a manual inspector

`just shell-build` (a full `.deb`/AppImage) was not run — see §8. Instead I drove **the same
engine the shell embeds** (`webkit2gtk-4.1` 2.52.3) via PyGObject, loading the real Panel from
the running server and evaluating the runbook's expressions. This is the same engine and the
same page; what it does *not* exercise is the Tauri `initialization_script`, discussed below.

**Values actually returned, against `http://127.0.0.1:18790/` (live server, real Panel):**

| Expression | Value read |
|---|---|
| `document.documentElement.dataset.platform` | `"linux"` |
| `document.documentElement.dataset.flat` | `"1"` |
| `document.documentElement.dataset.shell` | `undefined` |
| `getAttribute('data-webview-unsupported')` | `null` |
| `document.querySelectorAll('.glass').length` | `1` |
| `getComputedStyle(.glass).backdropFilter` | `"none"` |
| `CSS.supports('color','oklch(0 0 0)')` | `true` |
| `CSS.supports('color','color-mix(in oklab, red, red)')` | `true` |
| `typeof CSS.registerProperty === 'function'` | `true` |
| `typeof WebAssembly === 'object'` | `true` |
| `getComputedStyle(document.body).color` | `"oklch(0.97 0.005 220)"` |
| `navigator.userAgent` | `Mozilla/5.0 (X11; Ubuntu; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/60.5 Safari/605.1.15` |

The Panel's WASM booted and rendered real UI (body text contained
`团队群聊 / 项目管理 / 我们从哪里开始？`), so these are computed styles of a live Panel, not of a
blank document.

Two things worth stating precisely:

- **`data-webview-unsupported` is `null` and the palette resolved to `oklch(...)`.** The G1
  fallback page did not fire and the ~328 oklch tokens did not collapse to `initial`. That is
  the failure mode G1 exists to prevent, and it is absent here.
- **`data-shell` is `undefined`, and that is correct for this harness, not a defect.** Per
  `baseline-probe.js`'s own header, the probe *owns* the resolution of `data-platform` and
  `data-flat`; `SHELL_MARKER_JS` only pre-declares them, and a plain browser "never gets the
  marker at all." So `data-platform="linux"` here was produced by the probe's **UA fallback**,
  not by the shell's initialization script. Both writers are supposed to yield `linux` on this
  machine, and `HostPlatform::from_attribute` maps a missing attribute to `Linux` anyway — but
  **the shell-declared path specifically is not what this run exercised** (§8).

### 5c. `.aleph-todo-wrap` — the hole this round closed

The todo panel only exists while a plan is live, so the element is not present on an idle
Panel (`document.querySelectorAll('.aleph-todo-wrap').length` → `0`). Asserting "none" against
a non-existent element would have been vacuous, so I tested the claim itself.

The shipped stylesheet does contain the rule:

```css
html[data-flat="1"] .aleph-todo-wrap{-webkit-backdrop-filter:none!important}
```

and the blur it must beat is injected by a Rust `const`
(`todo_panel.rs:129-131`, `backdrop-filter:blur(8px);-webkit-backdrop-filter:blur(8px)`, no
`!important`). I injected that exact rule verbatim, mounted the element, and read the computed
value **with a control**:

```json
{
  "flat_on":          { "flat_attr": "1",         "backdropFilter": "none",      "webkitBackdropFilter": "none" },
  "flat_off_control": { "flat_attr": "undefined", "backdropFilter": "blur(8px)", "webkitBackdropFilter": "blur(8px)" }
}
```

The control is the point: it proves the blur rule was **live** and that `none` is the override
winning — not the rule having silently failed to apply. A guard that cannot tell those apart
proves nothing.

### 5d. GUI-free completeness guard

`no_backdrop_filter_survives_flat_mode` derives the required set from the stylesheet **and**
from every `.rs` file declaring a backdrop filter, so it catches a blur added anywhere:

```
test appearance::tests::no_backdrop_filter_survives_flat_mode ... ok
```

Full Panel suite (mandated by CLAUDE.md whenever `interfaces/webchat/` changes, and `dist/`
did change in the merge):

```
test result: ok. 1058 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

---

## 6. Step 6 — TTS

**Outcome: none of the three. I could not trigger a spoken reply — there is no TTS provider
configured in the isolated `ALEPH_HOME` and no API credentials on this box.** Stating that
plainly, because "no sound and no warning bar" is exactly the defect this change targets and I
must not let an untested path read as a passing one.

What I *could* test is the predicate the code actually branches on. `voice_playback.rs` treats
an empty `canPlayType` as the engine's definite "no" and raises the bar; `"maybe"`/`"probably"`
fall through and play. Measured in the real engine:

| MIME | `canPlayType` |
|---|---|
| `audio/mpeg` | `maybe` |
| `audio/mp3` | `maybe` |
| `audio/aac` | **`""` (definite no)** |
| `audio/mp4; codecs="mp4a.40.2"` | **`""` (definite no)** |
| `audio/wav` | `maybe` |
| `audio/ogg; codecs="opus"` | `probably` |
| `audio/webm; codecs="opus"` | `probably` |
| `video/webm; codecs="vp8"` | `probably` |
| `video/webm; codecs="vp9"` | `probably` |
| `video/mp4; codecs="avc1.42E01E"` | **`""` (definite no)** |

**This resolves an open question the source itself records as unverified.** The comment at
`voice_playback.rs:73-78` says the pre-check leg is not trusted "because whether WebKitGTK's
`canPlayType` consults the GStreamer registry is unverified." **It does.** The answers track
§1's matrix exactly, element for element: MP3 present → `maybe`; AAC absent → empty; Opus
present → `probably`; VP8/VP9 present → `probably`; H.264 absent (no `-libav`) → empty.

Consequences, stated as inference and not as an observed reply:

- The default TTS mime is `audio/mpeg` (`unwrap_or("audio/mpeg")`). On this box that is
  `maybe` ⇒ leg 1 does not fire, and MP3 genuinely decodes ⇒ **the expected outcome here is
  success with no warning bar.**
- Had the provider returned AAC, `canPlayType` is empty ⇒ leg 1 fires ⇒ the warning bar with
  the `gstreamer1.0-plugins-*` text. **The failure branch is reachable and correctly gated on
  this machine** — it is one missing decoder away, not dead code.

Leg 1 being load-bearing on WebKitGTK also means the round's "two legs, neither load-bearing
alone" design is stronger here than the comment assumed.

---

## 7. Step 7 — media seek (the real purpose of G3)

### 7a. Server half — Aleph's byte route, on the wire

Against a real capability URL (8,379-byte artifact). Beyond the five scripted assertions I
tested the edge cases the module doc commits to:

```
Range: bytes=100-199      → HTTP/1.1 206   content-range: bytes 100-199/8379    content-length: 100   (100 bytes)
Range: bytes=999999999-   → HTTP/1.1 416   content-range: bytes */8379
Range: bytes=-50          → HTTP/1.1 206   content-range: bytes 8329-8378/8379  (50 bytes)
Range: bytes=0-9,20-29    → HTTP/1.1 200   content-length: 8379                 (whole resource, never 416)
Range: bytes=4096-        → HTTP/1.1 206   content-range: bytes 4096-8378/8379  (4283 bytes)
Range: bytes=8000-        → HTTP/1.1 206   content-range: bytes 8000-8378/8379  (379 bytes)
Range: bytes=-0           → HTTP/1.1 416   content-range: bytes */8379
```

`accept-ranges: bytes` is advertised on the 200 and 206 responses — that header is what makes a
media element attempt seeking at all. Multi-range answered whole rather than 416, and the
zero-length suffix answered 416, both exactly as `byte_range.rs` documents.

### 7b. Client half — does WebKitGTK/GStreamer actually seek?

This is the half no server-side test can answer. I served a **6,721,244-byte / 420-second MP3**
from a Range-capable server that logs every `Range` header, loaded it in the same WebKitGTK
engine, and seeked to t=300.

Element state after the seek:

```json
{ "duration": "420.048979591", "currentTime": "305.776353",
  "readyState": 2, "networkState": 2,
  "seekable": [[0, 420.048979591]], "buffered": [[0, 420.048979591]],
  "error": null,
  "events": ["progress","durationchange","loadedmetadata","canplay",
             "progress","durationchange","loadedmetadata","canplay",
             "seeking","seeked","progress","stalled"] }
```

Requests the engine actually issued:

```
GET /media.mp3 range=<none>
GET /media.mp3 range=bytes=6721116-    -> 206 bytes 6721116-6721243/6721244
GET /media.mp3 range=bytes=4161536-    -> 206 bytes 4161536-6721243/6721244
GET /media.mp3 range=<none>
GET /media.mp3 range=bytes=6721116-    -> 206 bytes 6721116-6721243/6721244
GET /media.mp3 range=bytes=1294336-    -> 206 bytes 1294336-6721243/6721244
```

**Seeking works, and Range is demonstrably the mechanism.** `seekable` covers the whole file
(an empty `seekable` is the "scrub bar does nothing" symptom), `seeking`→`seeked` fired, and
`currentTime` landed at 305.8 after a seek to 300.

Note the **form** GStreamer emits: a tail probe (`bytes=6721116-`, the last 128 bytes — MP3
metadata) followed by open-ended byte-offset ranges. That is why §7a specifically re-tested
`bytes=N-` against Aleph's own route: it is the shape that actually matters here, and Aleph
answers it correctly.

**Honest composition caveat:** the media was served by my logging server, not by Aleph's byte
route, because I had no >5 MB media artifact on Aleph (`session.export_html` yields an 8 KB
HTML, and minting a canvas asset needs a capability token I did not derive). So this is two
independently proven halves — Aleph answers every range form correctly (§7a, including the
exact forms observed in §7b), and WebKitGTK issues those forms and seeks (§7b) — rather than
one continuous end-to-end trace. I judge the composition sound, but it is a composition.

---

## 8. What I could NOT verify — named, with reasons

This section carries the same weight as the passes.

1. **A real spoken TTS reply (Step 6's actual three-way outcome).** No TTS provider and no API
   credentials in the isolated `ALEPH_HOME`. I verified the deciding predicate (§6) but never
   heard audio and never saw the warning bar render. **The bar's rendering, its text, and its
   ✕ dismiss control are all unobserved.**
2. **The `SHELL_MARKER_JS` path specifically.** It compiles (§5a), but `data-platform` in §5b
   was written by `baseline-probe.js`'s UA fallback, not by Tauri's `initialization_script`. A
   bug that affected only the shell-declared path would not have been caught. `data-shell`
   was `undefined` throughout, confirming no shell was in the loop.
3. **`just shell-build` / the packaged `.deb` or AppImage.** Not run: `just` is absent, the
   session is `XDG_SESSION_TYPE=tty` with only an xrdp X server and **no usable GPU**
   (`/dev/dri` permission denied, software rendering). Installing and launching a packaged
   desktop app was out of reach; I substituted the instrumented engine (§5b).
4. **G1's fallback page.** WebKitGTK here is 2.52.3, ten minors above the 2.42 floor, so the
   "too old" branch is unreachable. **Nothing in this run proves the fallback page renders
   correctly on a genuinely old WebView** — only that it correctly does *not* fire on a new one.
5. **The `[info] … unknown` codec state.** `gst-inspect-1.0` is installed, so the
   tool-absent branch never ran. I read the code path and it looks right, but I did not
   execute it. (Uninstalling `gstreamer1.0-tools` to force it would have changed the very
   matrix §4's consistency check depends on.)
6. **`.aleph-todo-wrap` with a genuinely live todo panel.** Requires an agent run producing an
   execution plan, i.e. an LLM turn. §5c tests the CSS claim with a control instead, which I
   consider stronger than one screenshot, but it is a reconstruction, not the live panel.
7. **macOS anything.** Out of scope for this machine.
8. **Whether an exported artifact URL should survive a server restart.** A URL minted before a
   restart 404'd afterwards, and `/tmp/aleph-verify/data/artifacts` holds no session exports.
   That is consistent with exports being deliberately transient, and I did not read the storage
   design, so **I am recording this as an observation, not a finding.** Worth a glance by
   someone who knows the intended lifetime.

---

## 9. Findings outside the webview scope

Both are real, both are in the current tree, and neither belongs to G1–G5. Reporting only —
nothing was fixed here.

### 9a. Two un-awaited `end_session` futures — group chat sessions are never ended

`cargo build --release` emits, and these are the **only** three warnings in the whole build:

```
warning: unused implementer of `futures_util::Future` that must be used
   --> src/gateway/handlers/group_chat.rs:261:13
    |
261 |             orch_guard.end_session(&session_id);

warning: unused implementer of `futures_util::Future` that must be used
   --> src/gateway/inbound_router/group_chat_handler.rs:346:17
    |
346 |                 orch_guard.end_session(session_id);
```

`GroupChatOrchestrator::end_session` became `async` in **`5e7dd2c98` "review: migrate sync fn
locks to async (Risk 4 part 5)"**; both call sites predate it (`2ae5a7038`, 2026-05-22) and were
not updated. The future is constructed and dropped, so **the session is never ended** at either
site — including the round-limit path in `group_chat.rs`, which then returns an error to the
caller having left the orchestrator entry live.

This is precisely the trap CLAUDE.md §10 documents ("making a function `async` turns its call
sites into un-awaited futures — Rust reports a WARNING, not an error"). `cargo check`,
`cargo test --no-run`, and CI are all green.

### 9b. `core/duplicate-instance` is a permanent false positive on Linux

**Reproduced with zero `aleph-server` processes running:**

```
$ pgrep -c aleph-server
0
$ aleph-server doctor
[warn] core/duplicate-instance Multiple aleph-server processes running
    15 other aleph-server process(es) detected. Multiple daemons racing the same vault
    cause HMAC failure and vault data loss.
```

`count_other_instances()` (`src/diagnostics/checks/duplicate_instance.rs:29`) excludes only
`std::process::id()` — the PID — but **`sysinfo` on Linux enumerates threads/tasks as
processes**, each with a distinct TID and the same `exe` name. So the doctor process counts its
own sibling threads, plus every thread of any real server.

The arithmetic confirms the mechanism exactly:

| Situation | server threads | reported "others" |
|---|---:|---:|
| no server running | 0 | **15** |
| one server running (21 tasks) | 21 | **36** = 15 + 21 |

(An earlier sample read 35, drifting with tokio worker spawn timing — which is also why two
consecutive doctor runs reported 35 then 36.)

Impact: the check **can never report ok on Linux** and fires on every clean single-instance
install. A warning that is always wrong trains the operator to ignore it — and this particular
warning guards *vault data loss*, so that is an expensive thing to train away. It also means
the check cannot do its actual job on Linux: it cannot distinguish one real duplicate from
zero. Windows/macOS are unaffected (`sysinfo` does not surface threads as processes there),
which is why it survived until a Linux run.

---

## 10. Verdict

| Goal | Status on Linux |
|---|---|
| **G1** WebView floor declared & enforced | **Verified for the supported case.** All four capability probes `true` in WebKitGTK 2.52.3; fallback correctly did not fire; oklch palette resolved. The *unsupported* branch is untestable here (§8.4). |
| **G2** build-time brotli precompression | **Verified.** Negotiation works in both directions, and the served `.br` decompresses to a byte-identical wasm (sha256 match). |
| **G3** Range/206 on both byte routes | **Verified, both halves.** Aleph answers 206/416/suffix/open-ended/multi-range exactly as documented; WebKitGTK/GStreamer issues those ranges and seeks a 420 s file successfully. Composition caveat in §7b. |
| **G4** Linux decoder diagnostics | **Verified for `ok` and `warn`.** Doctor's verdict matches the raw registry element-for-element. `unknown` state unexercised (§8.5). User-visible bar **not observed** (§8.1), though its deciding predicate is verified and both branches are correctly gated. |
| **G5** unconditional flat degradation | **Verified.** `data-flat="1"`, `.glass` → `none` on a live Panel, and `.aleph-todo-wrap`'s override proven to beat its blur *with a control*. Source-derived completeness guard green; 1058/1058 Panel tests pass. |

No failure was found in G1–G5. Two defects were found outside them (§9), one of which
(§9b) is Linux-specific and could only have surfaced on a run like this.

---

## 11. Reproduction notes

Helper scripts were written to `/tmp`, deliberately **not** into `qa/`, to keep this run
read-only: `/tmp/aleph-webkit-probe.py`, `/tmp/aleph-todowrap-probe.py`,
`/tmp/aleph-canplay-probe.py`, `/tmp/aleph-seek-probe.py`, `/tmp/aleph-mint-artifact.mjs`,
`/tmp/aleph-br-check.mjs`, `/tmp/range-log-server.mjs`.

No system package was installed and no existing data was touched; the server ran under
`ALEPH_HOME=/tmp/aleph-verify` throughout. Several of these are worth promoting into `qa/` on
the main machine — particularly the WebKitGTK harness, which turns four of the runbook's
`MANUAL` steps into assertions, and the Range-logging seek probe, which is the only thing here
that tests the client half of G3.
