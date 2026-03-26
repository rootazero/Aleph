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
    StreamingSink (real-time chunk forwarding to client)
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

**`AcpHarness` trait** — Ensure both methods are required (not defaulted to error):
- `execute_oneshot(prompt, cwd) -> Result<String>`
- `spawn_session(cwd) -> Result<AcpSession>`

**Per-harness dual paths**:

| Harness | Oneshot | Native ACP |
|---------|---------|------------|
| ClaudeCodeHarness | `claude --print --output-format json -p "<prompt>"` (existing) | `claude --acp` (new) |
| CodexHarness | `codex exec "<prompt>"` (existing) | `codex --acp` (new, verify CLI support, fallback to oneshot if unsupported) |
| GeminiHarness | TBD — verify `gemini` oneshot CLI flags (new) | `gemini --acp` (existing) |

**Config layer** — `AcpHarnessEntry.mode` renamed to `default_mode`. Preset factories updated accordingly.

**Pre-requisite: Bug fix** — Investigate and fix Panel displaying all harnesses as "Native ACP" regardless of actual configuration.

## Section 2: Manager Session Pool Upgrade

### Current State

`AcpHarnessManager` has `sessions: HashMap<String, AcpSession>` — one session per harness, no cwd distinction.

### Target State

Smart session pool indexed by `(harness_id, cwd)`, with automatic lifecycle management.

### Changes

**Session pool structure**:
```rust
// Old
sessions: RwLock<HashMap<String, AcpSession>>

// New
sessions: RwLock<HashMap<(String, String), Vec<AcpSession>>>
//                       (harness_id, cwd)   active sessions
```

**`manager.prompt()` interface extension**:
```rust
pub async fn prompt(
    &self,
    harness_id: &str,
    text: &str,
    cwd: &str,
    mode: Option<HarnessMode>,       // None = use harness default_mode
    reuse_session: bool,              // true = find/reuse, false = force new
    sink: Option<&StreamingSink>,     // for streaming passthrough
) -> Result<String>
```

Behavior:
- `mode: None` → use `harness.default_mode()`
- `mode: Some(Oneshot)` → always `execute_oneshot()`, ignore `reuse_session`
- `mode: Some(NativeAcp)` + `reuse_session: true` → find alive session for `(harness_id, cwd)`, reuse if found, else create new
- `mode: Some(NativeAcp)` + `reuse_session: false` → always create new session

**Session lifecycle**:
- Dead sessions (process exited, timeout) auto-cleaned on next access
- No background GC thread — lazy cleanup on access is sufficient

**Concurrency safety**:
- `RwLock` on session pool — read lock for lookup, write lock for create/remove
- Different sessions are fully independent, parallel tool calls safe

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

**`session.prompt()` extension**:
```rust
pub async fn prompt(
    &self,
    text: &str,
    cwd: &str,
    timeout: Duration,
    sink: Option<&StreamingSink>,  // new
) -> Result<(String, Vec<ContentBlock>)>
```

When `sink` is `Some`:
- Each `agent_message_chunk` notification is forwarded via `sink` in real-time
- Text is still aggregated for the final return value
- Client receives intermediate output as tool execution progress

When `sink` is `None`:
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
- **Reuse sessions for continuity**: Use reuse_session=true when follow-up
  prompts need prior context (e.g., "now add error handling to what you wrote").
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
| Harness dual-mode | `core/src/acp/harnesses/claude_code.rs` | Add native_acp path (`claude --acp`) |
| | `core/src/acp/harnesses/codex.rs` | Add native_acp path (`codex --acp`) |
| | `core/src/acp/harnesses/gemini.rs` | Add oneshot path |
| | `core/src/acp/harness.rs` | Ensure trait requires both mode methods |
| Config | `core/src/config/types/acp.rs` | `mode` → `default_mode`, update presets |
| Manager | `core/src/acp/manager.rs` | Session pool `(harness_id, cwd)`, `prompt()` extension, auto-cleanup |
| Tools | `core/src/builtin_tools/acp_tools.rs` | `AcpDelegateArgs` + mode/reuse_session, pass sink |
| Streaming | `core/src/acp/session.rs` | `prompt()` accepts `Option<StreamingSink>`, chunk forwarding |
| Prompt | System prompt template | Add orchestration strategy section |
| Panel | `interfaces/webchat/src/views/settings/acp_harnesses.rs` | Display/toggle default_mode, fix mode display bug |
| Tests | `core/tests/acp_probe/` | Dual-mode tests, session pool tests, parallel tests |
| Bug fix | Pre-requisite | Panel mode display vs actual config mismatch |

### What Does NOT Change

- Agent loop — untouched
- StreamingSink interface — unchanged, only new call sites
- Other tools — unaffected
- Channel layer — untouched
- Gateway RPC handlers — minor parameter additions only

## Estimated Scope

- ~800 LOC implementation (excluding tests)
- ~400 LOC tests
- Risk: low — all changes are additive, existing oneshot paths preserved
