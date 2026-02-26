# Prompt System Enhancement Design

> Enhancing Aleph's prompt system with environmental awareness, protocol tokens,
> self-management guidance, and memory citation standards.

**Date**: 2026-02-26
**Status**: Approved
**Scope**: 4 focused enhancements to the existing PromptBuilder
**Approach**: Method A — Inline extension of existing architecture

---

## Background

Comparative audit of Aleph vs OpenClaw prompt systems revealed 4 capability gaps:

1. **Micro-Environmental Awareness** — LLM lacks OS/arch/shell/repo context
2. **Protocol Tokens** — No structured silent response protocol
3. **Self-Management** — No operational awareness guidance
4. **Memory Citation** — No enforced source attribution for recalled knowledge

Aleph's existing architecture (PromptBuilder, EnvironmentContract, SilentReply capability,
Hydration system) provides strong foundations. This design adds 4 targeted sections without
architectural disruption.

### Design Principles

- Minimal invasion: extend existing patterns, don't refactor
- Each enhancement is an independent section method in PromptBuilder
- Content generation logic lives in dedicated files, builder only calls
- Total token overhead: ~280-410 tokens/request (conditional injection reduces this)

---

## 1. RuntimeContext Injection

### Purpose

Provide LLM with accurate physical execution environment information to prevent
cross-platform script generation hallucinations.

### Data Structure

```rust
// core/src/thinker/runtime_context.rs (new file)

/// Runtime micro-environment snapshot, collected at prompt assembly time
pub struct RuntimeContext {
    pub os: String,              // e.g., "macOS 15.3 (Darwin 25.3.0)"
    pub arch: String,            // e.g., "aarch64"
    pub shell: String,           // e.g., "zsh"
    pub working_dir: PathBuf,    // Current working directory
    pub repo_root: Option<PathBuf>,  // Git repository root (if in a git repo)
    pub current_model: String,   // Current LLM model identifier
    pub hostname: String,        // Machine hostname
}

impl RuntimeContext {
    /// Collect runtime context from the current environment.
    /// All operations are zero-cost or cached (no subprocess spawning).
    pub fn collect(current_model: &str) -> Self { ... }

    /// Format as a compact prompt section
    pub fn to_prompt_section(&self) -> String { ... }
}
```

### Collection Method

| Field | Source | Cost |
|-------|--------|------|
| `os` | `std::env::consts::OS` + `uname -r` (cached) | Zero |
| `arch` | `std::env::consts::ARCH` | Zero |
| `shell` | `$SHELL` env var | Zero |
| `working_dir` | `std::env::current_dir()` | Zero |
| `repo_root` | `git rev-parse --show-toplevel` (cached) | One-time |
| `current_model` | Passed from session context | Zero |
| `hostname` | `gethostname()` (cached) | Zero |

### Prompt Output

```markdown
## Runtime Environment
os=macOS 15.3 (Darwin 25.3.0) | arch=aarch64 | shell=zsh | cwd=/path/to/workspace | repo=/path/to/repo | model=claude-opus-4-6 | host=MacBook-Pro
```

### Integration

- `PromptBuilder::append_runtime_context(&RuntimeContext)` — new method
- Injected after role definition, before tools section
- **Always injected** — environmental awareness is universally valuable
- Token cost: ~40-60 tokens

---

## 2. AlephProtocolTokens

### Purpose

Define structured protocol tokens for LLM responses in background/automated scenarios,
enabling Agent Loop interception to eliminate wasteful token generation.

### Token Definitions

```rust
// core/src/thinker/protocol_tokens.rs (new file)

/// Protocol tokens for structured LLM-to-system communication
pub enum ProtocolToken {
    /// Heartbeat check: nothing to report
    HeartbeatOk,
    /// Background task completed successfully, no user notification needed
    SilentComplete,
    /// No meaningful response to give
    NoReply,
    /// Something requires user attention (with brief description)
    NeedsAttention(String),
}

impl ProtocolToken {
    pub const HEARTBEAT_OK: &'static str = "ALEPH_HEARTBEAT_OK";
    pub const SILENT_COMPLETE: &'static str = "ALEPH_SILENT_COMPLETE";
    pub const NO_REPLY: &'static str = "ALEPH_NO_REPLY";
    pub const NEEDS_ATTENTION_PREFIX: &'static str = "ALEPH_NEEDS_ATTENTION:";

    /// Parse raw LLM output into a protocol token
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        match trimmed {
            Self::HEARTBEAT_OK => Some(Self::HeartbeatOk),
            Self::SILENT_COMPLETE => Some(Self::SilentComplete),
            Self::NO_REPLY => Some(Self::NoReply),
            s if s.starts_with(Self::NEEDS_ATTENTION_PREFIX) => {
                let msg = s[Self::NEEDS_ATTENTION_PREFIX.len()..].trim().to_string();
                Some(Self::NeedsAttention(msg))
            }
            _ => None,
        }
    }
}
```

### Prompt Injection (Background mode only)

```markdown
## Response Protocol Tokens
When operating in background mode, use these exact tokens as your ENTIRE response:
- `ALEPH_HEARTBEAT_OK` — Heartbeat check found nothing to report.
- `ALEPH_SILENT_COMPLETE` — Background task completed successfully, no user notification needed.
- `ALEPH_NO_REPLY` — No meaningful response to give.
- `ALEPH_NEEDS_ATTENTION: <brief description>` — Something requires user attention.

Rules:
- Token must be the ENTIRE message. Never mix with normal text.
- Use ALEPH_HEARTBEAT_OK for routine heartbeat checks with no findings.
- Use ALEPH_NEEDS_ATTENTION only when there is genuinely actionable information.
```

### Agent Loop Interception

In `agent_loop/feedback.rs`, at the feedback processing stage:

1. Check if LLM output matches `ProtocolToken::parse()`
2. If matched:
   - `HeartbeatOk` / `SilentComplete` / `NoReply` → Silently consume, do not forward to user
   - `NeedsAttention(msg)` → Route to notification channel (Halo/push notification)
3. If not matched → Normal processing flow

### Relationship to Existing SilentReply

- `Capability::SilentReply` retained as **activation condition**
- `ProtocolToken` is the **concrete implementation** — upgrades from advisory guidance to parseable protocol
- Token cost: ~80-120 tokens (injected only in Background mode)

---

## 3. System Operational Guidelines

### Purpose

Give LLM operational awareness to detect and report system issues proactively.
Advisory only — no autonomous remediation.

### Prompt Content

```markdown
## System Operational Awareness
You are aware of your own runtime environment and can monitor it proactively.

### Diagnostic Capabilities (read-only, always allowed)
- Check disk space: `df -h`
- Check memory usage: `vm_stat` / `free -h`
- Check running Aleph processes: `ps aux | grep aleph`
- Check configuration validity: read config files and validate structure
- Check Desktop Bridge status: query UDS socket availability
- Check LanceDB health: verify database file accessibility

### When You Detect Issues
If you notice any of the following during normal operation:
- Configuration conflicts or invalid settings
- Database files inaccessible or corrupted
- Desktop Bridge disconnected
- Abnormal memory/disk usage patterns
- Runtime capability degradation (e.g., a runtime disappeared from ledger)

**Action**: Report to the user with:
1. What you observed (specific evidence)
2. Potential impact
3. Suggested remediation steps
4. Do NOT execute remediation without explicit user approval

### What You Must NEVER Do Autonomously
- Restart Aleph services
- Modify configuration files
- Delete or compact databases
- Kill processes
- Change system settings
```

### Integration

- `PromptBuilder::append_operational_guidelines()` — new method
- **Conditional injection**: Only when `InteractionParadigm` is `Background` or `CLI`
  (not in Messaging/Embedded to save tokens)
- Pure prompt-driven — no new Rust structures needed
- Aligns with R1 (no platform-specific APIs), R3 (core minimalism), R6 (no intrusion)
- Token cost: ~100-150 tokens (Background/CLI only)

---

## 4. Memory Citation Standards

### Purpose

Enforce source attribution for knowledge recalled from vector/long-term memory,
improving trustworthiness and verifiability of AI responses.

### Memory Result Format Enhancement

When injecting memory retrieval results into prompts, prefix with source metadata:

**Before:**
```
A discussion about X from a previous session...
```

**After:**
```
[Source: memory/facts/2026-02-20.lance#42] A discussion about X from a previous session...
```

### Prompt Rules

```markdown
## Citation Standards
When referencing information from memory or knowledge base:
- Include source reference in format: `[Source: <path>#<id>]` or `[Source: <path>#L<line>]`
- Sources are provided in the context metadata — do not fabricate source paths
- If multiple sources support a claim, cite the most specific one
- For real-time observations (current tool output, live data), no citation needed
- For recalled facts, prior decisions, or historical context, citation is mandatory
```

### Integration

- Ensure memory store retrieval results include `source_path` and `record_id` fields
- Format memory facts with `[Source: ...]` prefix when serializing into prompt text
- `PromptBuilder::append_citation_standards()` — new method
- **Always injected** — citation standards are valuable in all interaction modes
- Token cost: ~60-80 tokens

---

## File Change Summary

### New Files

| File | Purpose |
|------|---------|
| `core/src/thinker/runtime_context.rs` | RuntimeContext struct + collection + formatting |
| `core/src/thinker/protocol_tokens.rs` | ProtocolToken enum + constants + parser |

### Modified Files

| File | Changes |
|------|---------|
| `core/src/thinker/prompt_builder.rs` | Add 4 `append_*` methods, wire into `build_system_prompt*` |
| `core/src/thinker/mod.rs` | Re-export new modules |
| `core/src/thinker/context.rs` | Add `RuntimeContext` to `ResolvedContext` (optional) |
| `core/src/agent_loop/feedback.rs` | Add `ProtocolToken::parse()` interception |
| `core/src/memory/store/*.rs` | Ensure retrieval results carry source metadata |

### Token Budget

| Section | Condition | Tokens |
|---------|-----------|--------|
| RuntimeContext | Always | 40-60 |
| ProtocolTokens | Background mode only | 80-120 |
| Operational Guidelines | Background/CLI only | 100-150 |
| Citation Standards | Always | 60-80 |
| **Total (Interactive)** | | **100-140** |
| **Total (Background)** | | **280-410** |

---

## Non-Goals

These are explicitly out of scope for this design:

- **Shadow Prompting** — Real-time prompt updates via SSB (future enhancement)
- **Reflexive Learning** — L2 auto-crystallization of LLM corrections (future)
- **Collaborative Skill Evolution** — Skill System v2 architecture promotion (future)
- **PromptPipeline refactoring** — Layer-based prompt assembly (future, when builder grows further)
- **Deep environmental sensing** — Network/disk/load monitoring in RuntimeContext
- **Autonomous self-healing** — LLM executing remediation without approval
