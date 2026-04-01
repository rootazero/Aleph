# ACP Coding Orchestrator Design

## Summary

Upgrade Aleph's ACP infrastructure from "single-shot delegation" to "multi-step orchestration" — Aleph acts as a tech lead, autonomously directing professional coding CLI tools (Claude Code, Codex, Gemini) through plan → code → review → test workflows, with real-time progress streaming to the user.

## Design Decisions

| Dimension | Decision | Rationale |
|-----------|----------|-----------|
| Orchestration granularity | LLM autonomous (B) | R8 LLM Sovereignty — no hardcoded flows |
| User participation | Real-time streaming + intervention (C) | R6 AI Comes to You — early error detection |
| Session strategy | LLM decides reuse/new (C) | R8 — model judges context continuity |
| Parallelism | Allowed via parallel tool call (B) | Free — agent loop already supports it |
| Dual mode | Each harness supports both oneshot + native_acp (A) | Minimal tool count, low LLM cognitive load |
| Prompt strategy | Static orchestration prompt in system prompt (B) | R10 Intelligence Lives in the Prompt |
| Failure handling | Fully LLM-driven (A) | R8 — model judges retry/switch/escalate |
| ACP positioning | Tool, not Channel or Provider | Clear architectural boundary |

## Architecture

```
User Request
    ↓
Aleph LLM (with orchestration prompt)
    ↓ (autonomous multi-step decisions)
    ├─ claude_code(prompt, mode?, reuse_session?)  ──→ ClaudeCodeHarness
    ├─ codex(prompt, mode?, reuse_session?)         ──→ CodexHarness
    ├─ gemini_cli(prompt, mode?, reuse_session?)    ──→ GeminiHarness
    │   (parallel tool calls supported)
    ↓
AcpHarnessManager (session pool, mode routing)
    ├─ Oneshot: spawn → execute → exit
    └─ NativeAcp: session pool → reuse/create → streaming
         ↓
    AcpChunkCallback → EventEmitter (real-time chunk forwarding to client)
    ↓
User (sees step-level + streaming progress, can intervene)
```

## Section 1: Dual-Mode Harness Unification

### Current State

- `ClaudeCodeHarness`: oneshot only (`claude --print -p "<prompt>"`)
- `CodexHarness`: oneshot only (`codex exec "<prompt>"`)
- `GeminiHarness`: native_acp only (`gemini --acp`)

### Target State

Every harness implements both `execute_oneshot()` and `spawn_session()`.

### Changes

**`AcpHarness` trait** — Keep default implementations for both methods (returning "unsupported mode" error), preserving backward compatibility for `CustomHarness` and future third-party implementations. Add a new method:
- `fn supported_modes(&self) -> Vec<HarnessMode>` — declares which modes a harness actually supports
- Manager validates at runtime: if LLM requests an unsupported mode, return a clear error

Each harness overrides the methods it supports:
- `execute_oneshot(prompt, cwd) -> Result<String>` (default: error)
- `spawn_session(cwd) -> Result<AcpSession>` (default: error)

**Per-harness dual paths**:

| Harness | Oneshot | Native ACP |
|---------|---------|------------|
| ClaudeCodeHarness | `claude --print --output-format json -p "<prompt>"` (existing) | `claude --acp` (new) |
| CodexHarness | `codex exec "<prompt>"` (existing) | `codex --acp` (new — detect at harness registration via `codex --help` or version check; if unsupported, `supported_modes()` returns `[Oneshot]` only and Manager auto-falls back) |
| GeminiHarness | TBD — verify `gemini` oneshot CLI flags (new) | `gemini --acp` (existing) |

**Config layer** — `AcpHarnessEntry.mode` renamed to `default_mode`, with serde backward compatibility:
```rust
#[serde(default, alias = "mode")]
pub default_mode: HarnessModeSerde,
```
Preset factories updated accordingly.

**Pre-requisite: Bug fix — Root cause analysis**

The Panel shows all harnesses as "Native ACP" because all three presets in `config/types/acp.rs` set `mode: HarnessModeSerde::NativeAcp` (lines 162, 178, 189). Meanwhile, each harness struct hard-codes its own `mode()` return value and completely ignores the config's `mode` field. This is a config-vs-runtime disconnect.

**Fix**: After renaming to `default_mode`, the harness constructors must accept and store the config value as a field:
```rust
struct ClaudeCodeHarness {
    default_mode: HarnessMode,  // from config, not hard-coded
    // ...
}
```
The `mode()` trait method returns `self.default_mode`. Preset factories set correct defaults (ClaudeCode → Oneshot, Codex → Oneshot, Gemini → NativeAcp).

## Section 2: Manager Session Pool Upgrade

### Current State

`AcpHarnessManager` has `sessions: HashMap<String, AcpSession>` — one session per harness, no cwd distinction.

### Target State

Smart session pool indexed by `(harness_id, cwd)`, with automatic lifecycle management.

### Changes

**Session pool key type**:
```rust
/// Canonicalized session pool key — prevents duplicate sessions for equivalent paths
#[derive(Clone, Hash, Eq, PartialEq)]
struct SessionKey {
    harness_id: String,
    cwd: PathBuf,  // always canonicalized via std::fs::canonicalize() at construction
}

impl SessionKey {
    fn new(harness_id: &str, cwd: &str) -> Self {
        Self {
            harness_id: harness_id.to_string(),
            cwd: std::fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd)),
        }
    }
}
```

**Session pool structure**:
```rust
// Old
sessions: RwLock<HashMap<String, AcpSession>>

// New — keyed by canonicalized SessionKey, single session per key (most recent)
sessions: RwLock<HashMap<SessionKey, AcpSession>>
```

Selection strategy: one active session per `(harness_id, cwd)` pair. When `reuse_session: true`, reuse it if alive; if dead, replace with a new one. When `reuse_session: false`, drop existing session and create fresh. No `Vec` — one session per key is sufficient for the orchestration use case.

**`manager.prompt()` interface extension**:
```rust
pub async fn prompt(
    &self,
    harness_id: &str,
    text: &str,
    cwd: &str,
    mode: Option<HarnessMode>,       // None = use harness default_mode
    reuse_session: bool,              // true = find/reuse, false = force new
    on_chunk: Option<AcpChunkCallback>,  // for streaming passthrough (see Section 4)
) -> Result<String>
```

Behavior:
- `mode: None` → use `harness.default_mode()`
- `mode: Some(Oneshot)` → always `execute_oneshot()`, ignore `reuse_session`
- `mode: Some(NativeAcp)` + `reuse_session: true` → find alive session for key, reuse if found, else create new
- `mode: Some(NativeAcp)` + `reuse_session: false` → drop existing session, create new

**Session lifecycle**:
- Dead sessions (process exited, timeout) auto-cleaned on next access
- No background GC thread — lazy cleanup on access is sufficient

**Concurrency safety and lock ordering**:

Lock ordering (preserving existing convention): `sessions → harnesses → configs`

Critical: the write lock on `sessions` must NOT be held during `session.prompt()` (which can block for minutes). Pattern:
```rust
// 1. Acquire write lock briefly to extract/create session
let session = {
    let mut pool = self.sessions.write().await;
    pool.remove(&key)  // take ownership
    // or create new session
};
// 2. Lock released here

// 3. Use session without holding pool lock
let result = session.prompt(text, cwd, timeout, on_chunk).await;

// 4. Re-insert session into pool
self.sessions.write().await.insert(key, session);
```

This allows parallel tool calls to access different sessions without blocking.

## Section 3: Tool Layer Changes

### Current State

Three tools (`ClaudeCodeTool`, `CodexTool`, `GeminiCliTool`) with `AcpDelegateArgs { prompt, cwd }`.

### Target State

Extended parameters for mode selection and session reuse.

### Changes

**`AcpDelegateArgs` extension**:
```rust
pub struct AcpDelegateArgs {
    pub prompt: String,
    pub cwd: Option<String>,
    pub mode: Option<String>,           // "oneshot" | "native_acp"
    pub reuse_session: Option<bool>,    // default: true for native_acp
}
```

**Tool description enhancement** — Each tool's JSON Schema description updated to explain:
- Two modes available and when to use each
- `reuse_session` semantics for multi-step continuity
- Parallel invocation is safe

**Keep three separate tools** — `claude_code`, `codex`, `gemini_cli` remain distinct. LLM has natural semantic understanding of tool names. Better than a generic `acp_delegate(harness="...")`.

**`acp_switch` tool retained** — For runtime default harness switching.

## Section 4: Streaming Passthrough

### Current State

- Step-level: `notify_tool_start` / `notify_tool_result` via StreamingSink (working)
- Native ACP streaming: `agent_message_chunk` collected internally in `session.prompt()`, not exposed

### Target State

Two-level real-time reporting: step notifications + native ACP chunk forwarding.

### Changes

**Streaming abstraction — `AcpChunkCallback`**

The existing `StreamingDeltaSink` is designed for LLM provider deltas (`ProviderDelta`), not ACP chunks. Instead of coupling ACP to the provider streaming type, define a lightweight callback:

```rust
/// Callback for real-time ACP streaming chunks
/// Receives the chunk text as it arrives from the external tool
pub type AcpChunkCallback = Arc<dyn Fn(&str) + Send + Sync>;
```

At the tool call site, the callback is constructed to bridge into the existing notification system (e.g., `EventEmitter::emit_tool_progress` or a new `emit_acp_chunk` event that clients render as tool intermediate output).

**`session.prompt()` extension**:
```rust
pub async fn prompt(
    &self,
    text: &str,
    cwd: &str,
    timeout: Duration,
    on_chunk: Option<AcpChunkCallback>,  // new — lightweight callback, not StreamingSink
) -> Result<(String, Vec<ContentBlock>)>
```

When `on_chunk` is `Some`:
- Each `agent_message_chunk` notification calls `on_chunk(chunk_text)` in real-time
- Text is still aggregated for the final return value
- The callback bridges into EventEmitter for client delivery

When `on_chunk` is `None`:
- Existing behavior — internal aggregation only

**Oneshot mode**: No streaming data to forward. Step-level notifications (`notify_tool_start/result`) are sufficient.

**User intervention**: When user sends a new message during orchestration:
- Agent loop's existing cancellation mechanism interrupts tool execution
- Native ACP mode: send `session/cancel` to the external tool subprocess
- Oneshot mode: kill the subprocess

## Section 5: Orchestration Prompt

Static addition to system prompt template (~200 words):

```
## Code Task Orchestration

When the user requests coding work, you have professional coding CLI tools
at your disposal (claude_code, codex, gemini_cli). Use them like a tech lead
directing engineers:

- **Plan before code**: For non-trivial tasks, first ask a tool to analyze
  and propose a plan. Review the plan, then proceed.
- **Review after code**: After code is written, consider asking the same or
  a different tool to review it.
- **Parallel when independent**: If tasks are independent (e.g., code + tests),
  dispatch multiple tools simultaneously.
- **Reuse sessions for continuity**: When follow-up prompts need prior
  context (e.g., "now add error handling to what you wrote"), reuse the
  session so the tool retains conversation history.
- **Switch tools strategically**: Different tools have different strengths.
  You may use one for planning and another for implementation.
- **Report progress**: The user sees your tool calls in real-time.
  Briefly explain what each step is doing and why before invoking.
- **Handle failures**: If a tool fails or produces poor results, retry,
  try a different tool, or ask the user — use your judgment.
```

Injected as a fixed section in the system prompt template, alongside existing tool descriptions.

## Section 6: File Change Manifest

| Layer | File | Change |
|-------|------|--------|
| Harness dual-mode | `src/acp/harnesses/claude_code.rs` | Add native_acp path (`claude --acp`), accept `default_mode` from config |
| | `src/acp/harnesses/codex.rs` | Add native_acp path (`codex --acp`), detect support at registration |
| | `src/acp/harnesses/gemini.rs` | Add oneshot path, accept `default_mode` from config |
| | `src/acp/harness.rs` | Add `supported_modes()` method, keep default impls for oneshot/session |
| Config | `src/config/types/acp.rs` | `mode` → `default_mode` with `#[serde(alias = "mode")]`, fix preset defaults |
| Manager | `src/acp/manager.rs` | `SessionKey` newtype, session pool with extract-use-reinsert pattern, `prompt()` extension |
| Tools | `src/builtin_tools/acp_tools.rs` | `AcpDelegateArgs` + mode/reuse_session, construct `AcpChunkCallback` |
| Streaming | `src/acp/session.rs` | `AcpChunkCallback` type, `prompt()` accepts `Option<AcpChunkCallback>` |
| Prompt | System prompt template | Add orchestration strategy section (~200 words, intent-level not parameter-level) |
| Panel | `interfaces/webchat/src/views/settings/acp_harnesses.rs` | Display/toggle default_mode, fix mode display bug |
| Tests | `tests/acp_probe/` | Dual-mode tests, session pool tests, parallel tests |
| Bug fix | Pre-requisite | Config-vs-harness mode disconnect (root cause: presets all set NativeAcp, harness structs ignore config) |

### What Does NOT Change

- Agent loop — untouched
- `StreamingDeltaSink` — unchanged, ACP uses its own `AcpChunkCallback`
- Other tools — unaffected
- Channel layer — untouched
- Gateway RPC handlers — minor parameter additions only
- `AcpHarness` trait backward compatibility — default impls preserved, `CustomHarness` unaffected

## Estimated Scope

- ~1000 LOC implementation (excluding tests)
- ~500 LOC tests
- Risk: low — all changes are additive, existing oneshot paths preserved, trait backward-compatible
