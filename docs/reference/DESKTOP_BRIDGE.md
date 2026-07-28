# Desktop Bridge (Swift Helper Process)

> Long-lived `AlephBridge` Swift subprocess that proxies macOS native APIs
> (AVFoundation, Vision, Accessibility, IOKit) over JSON-RPC 2.0 stdio.

## 1. Overview

Aleph uses a three-tier process model to keep the Rust core free of framework
linkage while still giving the LLM access to rich native macOS capabilities.

1. **aleph-server (Rust)** — owns the agent loop, vault, sessions, and all
   business logic. It knows nothing about `AVCaptureSession` or
   `VNRecognizeTextRequest`. It communicates with the helper by writing
   JSON-RPC requests to the child's stdin and reading responses from stdout.
2. **AlephBridge (Swift)** — a long-lived child process spawned by aleph-server
   on the first desktop capability call. It owns the `*.framework` linkage
   (AVFoundation, Vision, Speech, Accessibility) and executes native API calls
   on behalf of the Rust core. It never touches the vault or session storage.
3. **Caller (LLM)** — invokes `desktop.*` tools through the normal tool-use
   loop. The tool handlers in alephcore are thin wrappers that forward each
   call to the `SwiftBridge` client, await the response, and surface errors as
   structured `DesktopError` variants.

### Process topology

```
aleph-server (Rust)
  │
  │  spawn (tokio::process::Command)
  ▼
AlephBridge (Swift)          ← long-lived child process
  │  stdin  ← JSON-RPC requests  (line-delimited)
  │  stdout → JSON-RPC responses (line-delimited)
  │  stderr → tracing logs (forwarded to Rust tracing subscriber)
  │
  ├── AVFoundation  (camera, audio, speech)
  ├── Vision        (OCR)
  ├── Accessibility (AX tree queries)
  └── IOKit         (sleep assertions — owned by Rust via FFI, NOT Swift)
```

### Spawn lifecycle

The `SwiftBridge` client (`desktop/shared/src/bridge/client.rs`) spawns the
helper lazily on the first call. A `Supervisor` (`bridge/supervisor.rs`)
monitors the child process and respawns it with exponential backoff (1 s, 2 s,
4 s, 8 s, 16 s, capped at 30 s) when it exits unexpectedly. After five
restarts within a ten-minute window the bridge enters **disabled mode**: all
subsequent calls return `DesktopError::BridgeDisabled` immediately, and no
further respawns are attempted until the server restarts.

The Swift helper installs a parent-death watchdog: it polls `getppid()` and
exits cleanly if the parent PID changes, preventing zombie helper processes
when the server crashes.

### Per-call RPC timeout

Crash recovery only fires when the helper closes stdout (EOF). A helper that
accepts a request and then *hangs* — stuck in a native API, deadlocked —
keeps stdout open, so the reader loop never observes EOF. To stop such a
helper from wedging an agent turn indefinitely, every RPC is bounded by a
per-call deadline:

- `SwiftBridge::call` spends **the deadline the protocol declares for that
  method** (`methods::suggested_timeout_ms` → `bridge::client::rpc_timeout_for`).
  Resolution is: an exact per-method override, else the namespace default, else
  the client's `DEFAULT_RPC_TIMEOUT` (60 s) for a method outside every known
  namespace.
- `SwiftBridge::call_with_timeout` takes an explicit deadline, and is for the
  operations whose length is a function of their *arguments*: `camera.clip` and
  `audio.record` pass `requested_duration + 30 s`; `speech.transcribe_file`
  passes a flat 300 s.

The deadlines live next to the method constants they belong to
(`shared/protocol/src/desktop_bridge/methods/*.rs`, `DEFAULT_TIMEOUT_MS` +
`TIMEOUT_OVERRIDES_MS`). Current values:

| namespace | default | overrides |
|---|---|---|
| `ax.*` | 15 s | `query_focused` 3 s |
| `bridge.*` | 5 s | `ping` 2 s |
| `input.*` | 2 s | `click` / `double_click` 5 s |
| `media.*` | 60 s | `camera.snap` 10 s, `audio.list_devices` 5 s, `audio.mic_meter` 2 s, `audio.record_stop` 15 s |
| `perm.*` | 10 s | — |
| `pim.*` | 60 s | — |
| `screen.*` | 10 s | `ocr` 20 s, `list_displays` 5 s |

> ⚠️ **Declaring a deadline is not the same as spending one.** These numbers
> existed for a long time as ten free-floating `SUGGESTED_TIMEOUT_MS*` constants
> with **zero consumers**: every call rode the 60 s catch-all instead. The two
> that hurt were `ax.query_focused` (the `type_text` focus gate issues it before
> every keystroke batch — 3 s intended, 60 s actual) and `screen.capture`, which
> has an xcap fallback, so the deadline was exactly how long a wedged helper
> delayed a capture that would have succeeded instantly on the other transport.
> The namespace fallback exists so a method added later inherits a sane budget
> rather than silently reverting to a minute.

On timeout the caller receives `DesktopError::BridgeTimeout`, the in-flight
slot is dropped (no leak), and the helper is **left running** — only that one
call fails. A late reply from a merely-slow helper is discarded by the reader
loop as an unknown id. Timeouts do not count toward the restart window.

A client-side deadline bounds *the call*, not the helper's work. For AX that is
not enough on its own: `AxQuerier` is an actor, so every accessibility operation
is serialised behind whichever one is currently blocked on an unresponsive
application. `AXUIElementSetMessagingTimeout` (2 s) is applied at every point a
handle enters a walk — the application element, the system-wide element, and each
child, because the setting is per-element and is **not** inherited by elements
copied out of one.

## 2. Protocol

### Transport

All IPC uses **line-delimited JSON over stdio** — one JSON object per line,
`\n` terminated. No sockets, no shared memory, no HTTP. This choice:

- Eliminates port collisions and socket-file permission issues.
- Lets the OS detect parent death automatically (read returns EOF when the
  parent closes the pipe).
- Makes the protocol trivially scriptable for debugging (see §6).

The codec lives in `desktop/shared/src/bridge/codec.rs` (Rust) and
`desktop/macos/bridge/Sources/AlephBridge/RPC/Codec.swift` (Swift). Both
sides frame messages identically.

### JSON-RPC 2.0

The protocol follows JSON-RPC 2.0 with `u64` request IDs (the legacy
`String` ID from Stage 0 was removed in T0.3). Types are defined in
`shared/protocol/src/desktop_bridge/envelope.rs`:

```rust
pub struct Request   { jsonrpc: String, id: u64, method: String, params: Option<Value> }
pub struct Response  { jsonrpc: String, id: u64, result: Value }
pub struct ErrorResponse { jsonrpc: String, id: Option<u64>, error: RpcError }
pub struct RpcError  { code: i32, message: String, data: Option<Value> }
pub struct Notification { jsonrpc: String, method: String, params: Option<Value> }
```

The `Message` enum (untagged `serde`) parses any inbound frame:

```rust
pub enum Message {
    Error(ErrorResponse),
    Response(Response),
    Notification(Notification),
}
```

### Handshake

On startup the Rust client sends `bridge.handshake` immediately after spawn.
The helper replies with its protocol version and the list of methods it
supports. If the handshake fails (version mismatch, timeout, or the helper
exits before replying) the bridge enters disabled mode and returns
`DesktopError::BridgeDisabled` on all subsequent calls.

The `supported_methods` list in the handshake reply allows the client to
know at runtime which optional capabilities the installed helper binary
provides, without relying on version-number comparisons.

## 3. Methods

Methods are grouped into seven namespaces. Parameter and result types are
defined in `shared/protocol/src/desktop_bridge/methods/`; the handlers that
serve them are registered in `desktop/macos/bridge/Sources/AlephBridge/RPC/`.
The two sides are kept honest by the golden-fixture test (§7).

**The tables below are the registered surface — 55 methods.** `bridge.handshake`
advertises exactly this list as `supported_methods`, so a method absent here is
a method the Rust client will not attempt. There are no longer any declared-but-
unserved constants: `input.clipboard_read` / `input.clipboard_write` had neither
a handler nor a caller and were removed (clipboard access is done in-process by
the limb — `desktop/macos/src/system/clipboard.rs`, `NSPasteboard` — never over
the bridge). **Do not re-add a constant ahead of its handler**: a published
method with nothing behind it answers `-32601` at runtime, which reads to the
model as "this feature is broken" rather than "this feature does not exist".

There is no `window.*` namespace. Window listing, focusing, moving and app
launch/quit run in-process in the limb (`desktop/shared/src/action/window.rs`),
not over IPC. `screen.capture`'s `window_id` refers to the ids that path
returns.

### bridge.* — lifecycle

| Method | Purpose |
|---|---|
| `bridge.handshake` | Version negotiation; reply carries `swift_version`, `protocol_version` and the `supported_methods` list |
| `bridge.ping` | Liveness check; reply carries `{ "pong": true }` |

Shutdown is **not** an RPC method: the helper exits on stdin EOF (`Server.run`)
or when its parent dies (`ParentWatch`).

### screen.* — capture and OCR

| Method | Permission required | Purpose |
|---|---|---|
| `screen.capture` | Screen Recording (TCC) | Capture a display (`display_id`, optional `region`) or ONE window (`window_id`) cropped to its frame, even when covered or not frontmost. `show_cursor` controls whether the pointer is drawn. Result carries `png_base64` + pixel `width`/`height`, plus `window_bounds` and `scale` on a window capture — those two are load-bearing: without them the crop's pixels cannot be mapped back to global click coordinates |
| `screen.ocr` | None | Run Vision text recognition over a supplied `image_base64`; returns recognized lines with bounding boxes and confidences |
| `screen.list_displays` | None | Enumerate displays with resolution, scale and origin |

### input.* — synthetic input (two delivery rails)

Requires Accessibility (TCC). Every params struct takes an optional `pid`, and
every result carries `delivery`:

- `delivery: "targeted"` — the event was posted straight into that process's
  own event queue (`CGEvent.postToPid`). The user's cursor never moves and the
  app need not be frontmost.
- `delivery: "global"` — the event went to the global HID tap: it physically
  moves the user's cursor and lands wherever focus already is.

`pid` is a *request* for the targeted rail, never a guarantee. The client must
read `delivery` back rather than assume — see `ensure_targeted` in
`desktop/macos/src/screen.rs`.

| Method | Purpose |
|---|---|
| `input.click` | Click at a point |
| `input.double_click` | Double-click at a point (one event stream with an incrementing click-state, not two clicks) |
| `input.type_text` | Type a string at the current keyboard focus |
| `input.key_combo` | Press and release a chord atomically |
| `input.key_button` | Hold a chord down, or let it back up (`action`: press / release / click). The one input that outlives its call, so its release must reach the same pid — the helper tracks which modifiers it is holding per process and stamps them onto later key events (a bare `key_combo` sent while ⌘ is held is still ⌘-something) |
| `input.scroll` | Scroll by a pixel delta, quantized to wheel clicks |
| `input.drag` | Press, move, release between two points |
| `input.hover` | Move the pointer without clicking |
| `input.mouse_button` | Press / release / click a button without auto-release |
| `input.cursor_position` | Read the current pointer position |

### ax.* — Accessibility tree

| Method | Permission required | Purpose |
|---|---|---|
| `ax.query_focused` | Accessibility (TCC) | The focused element — **of a given `pid`**, or of the system when none is given |
| `ax.query_tree` | Accessibility (TCC) | Budgeted subtree rooted at a given PID (defaults to frontmost app) |
| `ax.query_by_role` | Accessibility (TCC) | Collect all elements matching an AX role string |
| `ax.set_value` | Accessibility (TCC) | Locate an element by stateless locator (role/title/center scoring) and write its `AXValue`, reading it back for verification |
| `ax.perform_action` | Accessibility (TCC) | Locate an element the same way and perform a native AX action (`AXPress`, `AXShowMenu`, …) |

Elements report their own `actions` list and an `enabled` flag, so a caller
never has to guess an action name. Values of secure (password) fields are
redacted at the handler, never crossing the IPC boundary.

**`query_focused`'s `pid` is not a filter, it is the question.** With a pid the
helper asks *that application* for its own `AXFocusedUIElement`; without one it
reads the system-wide focus. The distinction is what the targeted input rail runs
on: that rail delivers keystrokes into a named process without bringing it
forward, so the system-focused element usually belongs to a different app
entirely. Reading it there is how the `type_text` focus gate — password-field
refusal included — came to inspect a window the keystrokes were never going to
reach. Contract for every limb: a `Some(pid)` answer **belongs to that process**,
and "some other app holds focus" is `None`, never that app's element. A platform
that can only answer system-wide honours this by filtering.

**Walks are budgeted, and say so.** `max_nodes` (default
`ax::DEFAULT_MAX_NODES` = 1500, ceiling 10 000) bounds a walk; `QueryResult` /
`QueryListResult` carry `node_count` and `truncated`. Depth alone does not bound
a tree — a browser or a long document is wide, not deep — and every node costs
several round trips into the target app on the way out plus a few hundred bytes
of model context on the way in.

> ⚠️ There used to be three of these numbers and none of them was visible:
> the macOS helper stopped at 10 000 nodes, Windows UI Automation at 4 000, the
> Linux AT-SPI walk at 1 500, each cutting the tree **silently**. A model handed a
> clipped subtree with no marker concludes the control it is hunting for does not
> exist and goes off to do something else. How much of an app one query may return
> is a property of the protocol, not of whichever limb answers.

### perm.* — Permission introspection

| Method | Purpose |
|---|---|
| `perm.check` | Return TCC authorization status for a given permission kind |
| `perm.guide` | Return a self-describing `PermissionGuide` for a kind |
| `perm.open_settings` | Deep-link to the relevant System Settings pane |

### media.* — camera, audio, speech

| Method | Permission required | Notes |
|---|---|---|
| `media.camera.snap` | Camera (TCC) | Still frame. Returns `image_base64` — a base64-encoded **JPEG** (`quality` 0.05–1.0) — plus `width`/`height` |
| `media.camera.clip` | Camera + Microphone if `with_audio` | Records `duration_secs`. Returns a **`file_path`** to an MP4/MOV on disk, not bytes |
| `media.audio.list_devices` | None | Enumerate input devices (`uid`, `name`, `is_input`, `is_default`) |
| `media.audio.record` | Microphone (TCC) | Fixed-duration record from the default mic. Returns a **`file_path`** (typically `.m4a`) + actual `duration_secs` + `format`. It does **not** return audio bytes |
| `media.audio.record_start` | Microphone (TCC) | Open-ended push-to-talk: start recording now, stop on a later call. Backs the Panel mic button (`WKWebView`'s `getUserMedia` is blocked on unsigned macOS builds, so capture happens natively) |
| `media.audio.record_stop` | Microphone (TCC) | Stop the active push-to-talk recording; result mirrors `media.audio.record` |
| `media.audio.mic_meter` | Microphone (TCC) | Poll the live input level. First call lazily installs an `AVAudioEngine` tap; the helper tears it down after an idle timeout |
| `media.speech.transcribe_file` | Speech Recognition + Microphone (TCC) | Offline on-device STT via `SFSpeechRecognizer` (Apple's hard ~60s budget) |

### pim.* — Personal information (Notes, Calendar, Reminders, Contacts)

Each group is served by the matching `*Commands.swift` type (AppleScript for
Notes, EventKit for Calendar/Reminders, the Contacts framework for Contacts),
hopped onto a serial `pimQueue`.

| Group | Methods | Permission required |
|---|---|---|
| Notes | `pim.notes.list` · `.get` · `.create` · `.update` · `.delete` · `.folders` | Automation (AppleScript → Notes) |
| Calendar | `pim.calendar.events` · `.get` · `.create` · `.update` · `.delete` · `.lists` | Calendars (TCC) |
| Reminders | `pim.reminders.list` · `.get` · `.create` · `.complete` · `.delete` · `.lists` | Reminders (TCC) |
| Contacts | `pim.contacts.search` · `.get` · `.groups` | Contacts (TCC) |
| Mail | `pim.mail.search` · `.get` · `.folders` | — **registered but not implemented**: all three return `-32002` with an explicit message. There is no Mail command type. Read-only by design; do not add write methods without an approval gate |

Contacts is **read-only**: there is no `pim.contacts.create` / `.update` /
`.delete` on either side of the wire.

## 4. Error envelope

The helper always responds with a well-formed JSON-RPC error object:

```json
{
  "jsonrpc": "2.0",
  "id": 42,
  "error": {
    "code": -32001,
    "message": "permission denied: accessibility",
    "data": {
      "kind": "Accessibility",
      "status": "Denied",
      "deep_link": "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
      "human_readable_steps": ["Open System Settings → Privacy & Security → Accessibility", "Enable AlephBridge in the list"],
      "rationale": "Accessibility permission is required to read the focused UI element."
    }
  }
}
```

Standard JSON-RPC error codes plus the Aleph-specific extensions. All are
defined in `shared/protocol/src/desktop_bridge/errors.rs` — that file is the
source of truth:

| Code | Meaning |
|---|---|
| -32700 | Parse error — malformed JSON |
| -32600 | Invalid request — missing required fields |
| -32601 | Method not found — the helper has no handler under that name |
| -32602 | Invalid params — schema validation failed |
| -32603 | Internal error — unexpected exception in the handler |
| **-32001** | **Permission denied** — `data` carries a `PermissionGuide` |
| -32002 | Not implemented — the method is registered but this platform cannot serve it (e.g. `pim.mail.*`). Distinct from -32601: the method exists, the capability does not |
| -32003 | Platform error — a native API returned a failure |
| -32004 | Timeout — the operation exceeded its deadline |
| -32005 | Helper crashed — the child exited mid-call |
| -32006 | Bridge disabled — restart budget exhausted; no further respawns |

## 5. PermissionGuide (self-describing errors)

Any method that requires a TCC grant returns a `-32001` error when the
permission has not been granted. The `data` field of the error carries a
`PermissionGuide` structure:

```rust
pub struct PermissionGuide {
    pub kind: PermissionKind,
    pub status: PermissionStatus,
    pub deep_link: String,
    pub human_readable_steps: Vec<String>,
    pub rationale: String,
}
```

The Rust client's `From<JsonRpcError> for DesktopError` implementation
automatically lifts a `-32001` error into
`DesktopError::PermissionDenied { kind, guide }`. The tool-use layer
surfaces `deep_link` and `human_readable_steps` to the LLM verbatim so it
can produce actionable guidance for the user without any additional
out-of-band metadata lookups.

The LLM is expected to say something like: *"Aleph needs Accessibility access.
Open System Settings → Privacy & Security → Accessibility and enable
AlephBridge."* — copying the text directly from `human_readable_steps`.

## 6. Debugging

```bash
# Tail bridge logs (written to stderr, forwarded by the Rust tracing subscriber)
tail -f ~/.aleph/logs/aleph-server.log | grep -i bridge

# One-shot manual RPC against the compiled helper binary
echo '{"jsonrpc":"2.0","id":1,"method":"bridge.ping","params":{}}' \
  | desktop/macos/bridge/.build/release/AlephBridge

# Check for active sleep assertions while an agent turn is in flight
pmset -g assertions | grep "Aleph agent loop"

# Verify the helper is running and attached to the server
pgrep -la AlephBridge
```

## 7. Development

```bash
# Build the Swift helper binary
just swift-bridge

# Regenerate the JSON schema golden fixture (must pass CI)
just bridge-schema

# Run Swift unit tests inside the helper package
just bridge-test

# Rust ↔ Swift round-trip end-to-end test (requires compiled helper)
cargo test -p aleph-desktop-macos --test bridge_e2e -- --ignored --nocapture

# Camera end-to-end (requires Camera TCC grant)
cargo test -p aleph-desktop-macos --test bridge_e2e camera -- --ignored --nocapture

# OCR end-to-end
cargo test -p aleph-desktop-macos --test bridge_e2e ocr -- --ignored --nocapture
```

### Schema ownership

The canonical types live in `shared/protocol/src/desktop_bridge/`. The
`just bridge-schema` target serializes them to a JSON Schema golden fixture
checked into the repository. The CI job fails if the fixture drifts from the
Rust types, preventing silent protocol mismatches between the Rust client and
the Swift helper.

## 8. Architectural invariants

These rules must be preserved by every change to the bridge subsystem.

**R1 compliance (Brain–Limb separation):** The **brain** — `alephcore` (`src/`)
— must never link AVFoundation, Vision, AppKit, CoreGraphics, or Speech; it
reaches every platform capability through the `DesktopPlatform` trait only.

`aleph-desktop` (`desktop/shared/`) is *not* pure brain: alongside the contracts
(traits + types + IPC client) it hosts the cross-platform in-process limb paths
that deliberately do **not** go over IPC — clipboard (`NSPasteboard`), app launch
(`NSWorkspace`/`NSRunningApplication`), window enumeration (CoreGraphics), and
screen recording (`objc2-screen-capture-kit`), all under per-OS `cfg`. These are
by design (see FEATURE_LOCATOR §7.1: `window.*`/app/clipboard/`screen_record` run
in-process, not as bridge methods), so `aleph-desktop` legitimately links native
frameworks. The heavyweight capture/recognition stack (camera, audio, Vision
OCR, Speech STT, EventKit, Contacts) still lives only behind the Swift helper.

The one native-framework use in a per-OS **limb** crate worth calling out is
`desktop/macos/src/permission.rs`, which links `objc2-av-foundation` and
`objc2-speech` solely for TCC status checks (`AVCaptureDevice::authorizationStatus`
and `SFSpeechRecognizer::authorizationStatus`) — no capture, recognition, or
rendering code lives on the Rust side.

**No vault access from Swift:** `AlephBridge` must never read or write
`~/.aleph/data/`, the `.shared_token` file, or any other vault path. The
vault's file lock is owned exclusively by the Rust core; concurrent writes
from a second writer silently corrupt the encrypted vault, destroying stored
API keys, OAuth tokens, and embedding keys (see CLAUDE.md, `.shared_token`
incident).

**Stdio only:** The helper process must not open any TCP or Unix domain
socket. All IPC goes through the inherited stdio pipes. This constraint
keeps the helper sandbox-friendly and eliminates port-collision and
socket-permission edge cases.

**Crash isolation:** Helper crashes are invisible to the agent loop — the
`Supervisor` respawns the helper and the next call retries transparently.
After the restart budget is exhausted the bridge degrades gracefully to
`BridgeDisabled` errors rather than propagating panics or blocking
indefinitely.

**IOKit stays in Rust:** The sleep inhibitor (`IOPMAssertion`) is driven by
Rust FFI in `desktop/macos/src/sleep_inhibitor.rs`, not by the Swift helper.
This keeps the inhibitor in the same process as the agent loop so the
assertion is automatically released if the server exits without calling
`Drop`.

## 9. Adding a typed desktop capability (playbook)

Desktop reach grows by **typed intents added one at a time**, never by a
speculative "run any desktop command" framework. The latter would duplicate the
sandboxed `bash` / `code_exec` tools and violate R3 / R10 / P6 (zero-consumer
abstractions get withdrawn, not kept "for the future"). Arbitrary command
execution intentionally stays sandboxed in `bash` / `code_exec`; that posture
does not change when a new typed intent lands.

**Before adding one, pass the "3 questions" gate (CLAUDE.md R10):** is there a
real consumer *today* (not a hypothetical)? Is it scaffolding rather than
reasoning the model should do itself (R7)? Will a stronger model still need it?
A "no" means don't add it yet.

**Each intent is the same three pieces:**

1. **Capability method** on the relevant trait in `desktop/shared/src/traits/`
   (e.g. `SystemCapability`, `ScreenCapability`). If the behavior is uniform
   across platforms, give it a **default** that delegates to a cross-platform
   helper in `desktop/shared/src/action/` (cfg-gated per OS) — every platform
   impl and test double then inherits it from one source. Only when an OS needs
   genuinely different native code do you override in `desktop/{macos,linux,windows}`.
2. **Gated tool action** on the `system` / `desktop` tool in
   `src/builtin_tools/` — a new `action` arm plus any argument field, routed
   through the existing approval gate (reuse an `ActionType` such as
   `DesktopLaunchApp`; the permissive default keeps it byte-identical until a
   policy tightens it). Document it in the tool `DESCRIPTION` with an example so
   the model can discover it.
3. **A unit test** asserting the tool forwards the right argument to the
   capability and rejects missing/empty input with a friendly message.

**Worked example — `system.open_path`** (open a file/URL with the OS default
app, the `open` / `xdg-open` / `start` primitive): default
`SystemCapability::open_path` → `action::open` (one cfg-gated helper);
`open_path` action on the `system` tool gated under `DesktopLaunchApp`; three
`system_tool` unit tests. No per-platform override, no new framework.
