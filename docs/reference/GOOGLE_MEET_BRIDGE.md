# Google Meet Bridge (Out-of-Core Transport)

> The `google_meet` builtin tool is a **thin capability contract**. Joining a
> Meet call, capturing duplex audio, and realtime transcription are platform
> automation (Chrome control, CoreAudio/BlackHole/SoX, Twilio) and therefore
> **must not** live in the Rust core (R1 brain–limb separation, R3 core
> minimalism). Core only validates the request and relays it as JSON-RPC 2.0 to
> an external transport bridge — a Skill / MCP server / native helper that the
> operator runs separately.

## 1. Why a bridge

The reference implementation (openclaw `extensions/google-meet`, ~20k LOC) is a
*plugin* for exactly these reasons: it links Chrome automation, a BlackHole/SoX
audio pipe, a Twilio dial-in path, and a realtime voice/transcription loop.
Porting that into `src` would violate R1 (platform API in core) and R3 (heavy
deps for a single non-core feature). Aleph keeps the same split as
[DESKTOP_BRIDGE.md](DESKTOP_BRIDGE.md): the Rust core is the brain, the bridge
is the limb.

- **aleph-server (Rust core)** — owns the agent loop and the `google_meet`
  tool. It knows nothing about Chrome or audio devices. It forwards each action
  as a JSON-RPC request to the bridge endpoint.
- **Bridge (out-of-core)** — owns Chrome/Twilio/audio/realtime. It is the only
  process that touches platform APIs.
- **Caller (LLM)** — invokes the `google_meet` tool through the normal tool-use
  loop. The contract mirrors openclaw's `google_meet` tool action surface.

## 2. Configuration

The bridge is opt-in via environment variables (the bridge runs out-of-band, so
its endpoint is configured out-of-band — mirroring the `TAVILY_API_KEY`
env-fallback precedent rather than carving a TOML section into the 100-field
core `Config`):

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| `ALEPH_GOOGLE_MEET_BRIDGE_URL` | yes | — | JSON-RPC 2.0 HTTP endpoint of the bridge. Unset → tool reports `bridge_not_configured`. |
| `ALEPH_GOOGLE_MEET_BRIDGE_TOKEN` | no | — | Bearer token presented to the bridge. |
| `ALEPH_GOOGLE_MEET_BRIDGE_TIMEOUT_SECS` | no | `30` | Per-request timeout. |

## 3. Wire protocol

Core POSTs a JSON-RPC 2.0 request to the bridge URL:

```json
{ "jsonrpc": "2.0", "id": 1, "method": "googlemeet.<action>", "params": { ...GoogleMeetArgs } }
```

`method` is `googlemeet.{join|create|leave|speak|status}`. `params` is the
validated tool arguments:

| Field | Type | Required for | Notes |
|-------|------|--------------|-------|
| `action` | `join \| create \| leave \| speak \| status` | always | — |
| `meeting` | string | `join` | Meet URL or meeting code. |
| `transport` | `chrome \| chrome-node \| twilio` | optional | Bridge picks its default when omitted. |
| `mode` | `agent \| bidi \| transcribe` | optional | Talk-back behaviour. |
| `text` | string | `speak` | Text to speak into the call. |

The bridge replies with a standard JSON-RPC envelope. Core maps it to the tool
output:

- **Success** — `result` may carry `meeting_url` / `url` and `detail` /
  `message`:
  ```json
  { "jsonrpc": "2.0", "id": 1, "result": { "meeting_url": "https://meet.google.com/...", "detail": "joined" } }
  ```
  → `{ ok: true, status: "ok", meeting_url, detail }`
- **Error** — JSON-RPC `error.message` →
  `{ ok: false, status: "bridge_error", detail: <message> }`.

Argument validation (e.g. `join` requires `meeting`) happens in core **before**
any network call, surfaced as a fixable validation error so the model can
correct it. The `bridge_not_configured` case is returned as structured data
(`ok: false`), never as a hard error — the loop degrades gracefully (R5).

## 4. Source

- Tool + contract: [`src/builtin_tools/google_meet.rs`](../../src/builtin_tools/google_meet.rs)
- Bridge config injection: `BuiltinToolConfig::google_meet_bridge`
- Registration: `definitions.rs` (`google_meet`), `groups.rs` (`system_config`)
