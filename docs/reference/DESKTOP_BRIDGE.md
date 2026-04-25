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

Methods are grouped into five namespaces. All parameter and result types are
defined in `shared/protocol/src/desktop_bridge/methods/`.

### bridge.* — lifecycle

| Method | Purpose |
|---|---|
| `bridge.handshake` | Version negotiation and capability advertisement |
| `bridge.ping` | Liveness check; reply carries `{ "pong": true }` |
| `bridge.shutdown` | Request graceful exit; helper flushes pending work and exits |

### media.* — camera, audio, speech

| Method | Permission required | Notes |
|---|---|---|
| `media.camera.snap` | Camera (TCC) | Returns a base64-encoded PNG |
| `media.camera.clip` | Camera + optionally Microphone | Returns a base64-encoded MP4 |
| `media.audio.list_devices` | None | Enumerate available input devices |
| `media.audio.record` | Microphone (TCC) | Record from default mic; returns base64 WAV |
| `media.speech.transcribe_file` | Speech Recognition + Microphone (TCC) | Offline on-device STT via SFSpeechRecognizer |

### screen.* — OCR

| Method | Purpose |
|---|---|
| `screen.ocr` | Run Vision text recognition on a PNG buffer; returns a list of recognized strings |

### ax.* — Accessibility tree

| Method | Permission required | Purpose |
|---|---|---|
| `ax.query_focused` | Accessibility (TCC) | Element currently holding keyboard focus + its ancestors |
| `ax.query_tree` | Accessibility (TCC) | Full subtree rooted at a given PID (defaults to frontmost app) |
| `ax.query_by_role` | Accessibility (TCC) | Collect all elements matching an AX role string |

### perm.* — Permission introspection

| Method | Purpose |
|---|---|
| `perm.check` | Return TCC authorization status for a given permission kind |
| `perm.guide` | Return a self-describing `PermissionGuide` for a kind |
| `perm.open_settings` | Deep-link to the relevant System Settings pane |

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

Standard JSON-RPC error codes plus one Aleph-specific extension:

| Code | Meaning |
|---|---|
| -32700 | Parse error — malformed JSON |
| -32600 | Invalid request — missing required fields |
| -32601 | Method not found |
| -32602 | Invalid params — schema validation failed |
| -32603 | Internal error — unexpected exception in the handler |
| **-32001** | **Permission denied** — `data` carries a `PermissionGuide` |

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

**R1 compliance (Brain–Limb separation):** The Rust core (`alephcore`,
`aleph-desktop`) must never link AVFoundation, Vision, AppKit, or Speech.
The documented exception is `desktop/macos/src/permission.rs`, which uses
`objc2-av-foundation` and `objc2-speech` solely for TCC status checks
(`AVCaptureDevice::authorizationStatus` and
`SFSpeechRecognizer::authorizationStatus`). No capture, recognition, or
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
