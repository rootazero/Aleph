# Prompt System Enhancement Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add 4 prompt system enhancements: RuntimeContext injection, AlephProtocolTokens, System Operational Guidelines, and Memory Citation Standards.

**Architecture:** Inline extension of existing PromptBuilder (Method A). Two new files (`runtime_context.rs`, `protocol_tokens.rs`) for content generation logic, with 4 new `append_*` methods wired into all `build_system_prompt*` variants. Protocol token interception added to DecisionParser before JSON parsing.

**Tech Stack:** Rust, existing thinker/prompt_builder module, DecisionParser, MemoryFact augmentation.

**Design Doc:** `docs/plans/2026-02-26-prompt-system-enhancement-design.md`

---

### Task 1: RuntimeContext — Create struct and collection

**Files:**
- Create: `core/src/thinker/runtime_context.rs`

**Step 1: Write the failing test**

At the bottom of the new file, add a test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_returns_valid_context() {
        let ctx = RuntimeContext::collect("claude-opus-4-6");
        assert!(!ctx.os.is_empty());
        assert!(!ctx.arch.is_empty());
        assert!(!ctx.hostname.is_empty());
        assert_eq!(ctx.current_model, "claude-opus-4-6");
    }

    #[test]
    fn test_to_prompt_section_format() {
        let ctx = RuntimeContext {
            os: "macOS 15.3 (Darwin 25.3.0)".to_string(),
            arch: "aarch64".to_string(),
            shell: "zsh".to_string(),
            working_dir: PathBuf::from("/workspace"),
            repo_root: Some(PathBuf::from("/workspace")),
            current_model: "claude-opus-4-6".to_string(),
            hostname: "test-host".to_string(),
        };
        let section = ctx.to_prompt_section();
        assert!(section.contains("## Runtime Environment"));
        assert!(section.contains("os=macOS 15.3"));
        assert!(section.contains("arch=aarch64"));
        assert!(section.contains("shell=zsh"));
        assert!(section.contains("model=claude-opus-4-6"));
        assert!(section.contains("host=test-host"));
        assert!(section.contains("repo=/workspace"));
    }

    #[test]
    fn test_to_prompt_section_no_repo() {
        let ctx = RuntimeContext {
            os: "Linux".to_string(),
            arch: "x86_64".to_string(),
            shell: "bash".to_string(),
            working_dir: PathBuf::from("/tmp"),
            repo_root: None,
            current_model: "gpt-4".to_string(),
            hostname: "server".to_string(),
        };
        let section = ctx.to_prompt_section();
        assert!(!section.contains("repo="));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::runtime_context -- --nocapture 2>&1 | head -30`
Expected: FAIL — module doesn't exist yet

**Step 3: Write minimal implementation**

```rust
//! Runtime Context for micro-environmental awareness
//!
//! Provides LLM with accurate physical execution environment information
//! to prevent cross-platform script generation hallucinations.

use std::path::PathBuf;

/// Runtime micro-environment snapshot, collected at prompt assembly time.
///
/// All collection operations are zero-cost or cached (no subprocess spawning
/// on the hot path). This struct is created once per prompt assembly.
#[derive(Debug, Clone)]
pub struct RuntimeContext {
    /// Operating system with version, e.g. "macOS 15.3 (Darwin 25.3.0)"
    pub os: String,
    /// CPU architecture, e.g. "aarch64"
    pub arch: String,
    /// User's default shell, e.g. "zsh"
    pub shell: String,
    /// Current working directory
    pub working_dir: PathBuf,
    /// Git repository root (if in a git repo)
    pub repo_root: Option<PathBuf>,
    /// Current LLM model identifier
    pub current_model: String,
    /// Machine hostname
    pub hostname: String,
}

impl RuntimeContext {
    /// Collect runtime context from the current environment.
    ///
    /// Uses `std::env::consts` for zero-cost OS/arch info,
    /// environment variables for shell, and `gethostname` for hostname.
    /// `repo_root` must be provided by the caller (from cached git info).
    pub fn collect(current_model: &str) -> Self {
        let os = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
        let arch = std::env::consts::ARCH.to_string();
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        Self {
            os,
            arch,
            shell,
            working_dir,
            repo_root: None, // Caller sets this from cached git info
            current_model: current_model.to_string(),
            hostname,
        }
    }

    /// Format as a compact prompt section (single-line key=value pairs).
    pub fn to_prompt_section(&self) -> String {
        let mut parts = vec![
            format!("os={}", self.os),
            format!("arch={}", self.arch),
            format!("shell={}", self.shell),
            format!("cwd={}", self.working_dir.display()),
        ];

        if let Some(ref repo) = self.repo_root {
            parts.push(format!("repo={}", repo.display()));
        }

        parts.push(format!("model={}", self.current_model));
        parts.push(format!("host={}", self.hostname));

        format!("## Runtime Environment\n{}\n\n", parts.join(" | "))
    }
}
```

**Step 4: Register module in mod.rs**

In `core/src/thinker/mod.rs`, add after `pub mod soul;`:
```rust
pub mod runtime_context;
```

And add to exports:
```rust
pub use runtime_context::RuntimeContext;
```

**Step 5: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::runtime_context -- --nocapture 2>&1 | tail -20`
Expected: 3 tests PASS

**Step 6: Commit**

```bash
git add core/src/thinker/runtime_context.rs core/src/thinker/mod.rs
git commit -m "thinker: add RuntimeContext for micro-environmental awareness"
```

---

### Task 2: RuntimeContext — Wire into PromptBuilder

**Files:**
- Modify: `core/src/thinker/prompt_builder.rs`

**Step 1: Write the failing test**

Add to the existing test module at the bottom of `prompt_builder.rs`:

```rust
#[test]
fn test_append_runtime_context() {
    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();

    let ctx = super::runtime_context::RuntimeContext {
        os: "macOS 15.3".to_string(),
        arch: "aarch64".to_string(),
        shell: "zsh".to_string(),
        working_dir: std::path::PathBuf::from("/workspace"),
        repo_root: Some(std::path::PathBuf::from("/workspace")),
        current_model: "claude-opus-4-6".to_string(),
        hostname: "test-host".to_string(),
    };

    builder.append_runtime_context_section(&mut prompt, &ctx);

    assert!(prompt.contains("## Runtime Environment"));
    assert!(prompt.contains("os=macOS 15.3"));
    assert!(prompt.contains("model=claude-opus-4-6"));
}

#[test]
fn test_build_system_prompt_includes_runtime_context() {
    let config = PromptConfig::default();
    let builder = PromptBuilder::new(config);

    let ctx = super::runtime_context::RuntimeContext {
        os: "Linux".to_string(),
        arch: "x86_64".to_string(),
        shell: "bash".to_string(),
        working_dir: std::path::PathBuf::from("/tmp"),
        repo_root: None,
        current_model: "test-model".to_string(),
        hostname: "server".to_string(),
    };

    let prompt = builder.build_system_prompt_with_runtime(&[], Some(&ctx));
    assert!(prompt.contains("## Runtime Environment"));
    assert!(prompt.contains("os=Linux"));
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::prompt_builder::tests::test_append_runtime_context -- --nocapture 2>&1 | tail -10`
Expected: FAIL — method doesn't exist

**Step 3: Implement the append method and wiring**

Add to `PromptBuilder` impl block (after `append_runtime_capabilities`):

```rust
/// Append runtime context section (micro-environmental awareness)
pub fn append_runtime_context_section(
    &self,
    prompt: &mut String,
    runtime_ctx: &super::runtime_context::RuntimeContext,
) {
    prompt.push_str(&runtime_ctx.to_prompt_section());
}
```

Add a new convenience method that accepts optional RuntimeContext:

```rust
/// Build system prompt with optional RuntimeContext injection.
///
/// This is a wrapper over `build_system_prompt` that injects
/// runtime environment context after the role definition.
pub fn build_system_prompt_with_runtime(
    &self,
    tools: &[ToolInfo],
    runtime_ctx: Option<&super::runtime_context::RuntimeContext>,
) -> String {
    let mut prompt = String::new();

    // Role definition
    prompt.push_str("You are an AI assistant executing tasks step by step.\n\n");
    prompt.push_str("## Your Role\n");
    prompt.push_str("- Observe the current state and history\n");
    prompt.push_str("- Decide the SINGLE next action to take\n");
    prompt.push_str("- Execute until the task is complete or you need user input\n\n");

    // Runtime context (NEW — injected right after role)
    if let Some(ctx) = runtime_ctx {
        self.append_runtime_context_section(&mut prompt, ctx);
    }

    // Standard sections
    self.append_runtime_capabilities(&mut prompt);
    self.append_tools(&mut prompt, tools);
    self.append_generation_models(&mut prompt);
    self.append_skill_instructions(&mut prompt);
    self.append_special_actions(&mut prompt);
    self.append_response_format(&mut prompt);
    self.append_guidelines(&mut prompt);
    self.append_thinking_guidance(&mut prompt);
    self.append_skill_mode(&mut prompt);
    self.append_custom_instructions(&mut prompt);
    self.append_language_setting(&mut prompt);

    prompt
}
```

**Step 4: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::prompt_builder::tests::test_append_runtime_context -- --nocapture 2>&1 | tail -10`
Expected: PASS

**Step 5: Also wire RuntimeContext into `build_system_prompt_with_context`**

In `build_system_prompt_with_context`, add RuntimeContext field to `ResolvedContext`:

Modify `core/src/thinker/context.rs`:
```rust
pub struct ResolvedContext {
    pub available_tools: Vec<ToolInfo>,
    pub disabled_tools: Vec<DisabledTool>,
    pub environment_contract: EnvironmentContract,
    /// Optional runtime context for micro-environmental awareness
    pub runtime_context: Option<super::runtime_context::RuntimeContext>,
}
```

In `ContextAggregator::resolve()`, set `runtime_context: None` (caller provides it).

In `build_system_prompt_with_context()`, after the role definition block, add:
```rust
// Runtime context (micro-environmental awareness)
if let Some(ref runtime_ctx) = ctx.runtime_context {
    self.append_runtime_context_section(&mut prompt, runtime_ctx);
}
```

**Step 6: Run full test suite**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker -- --nocapture 2>&1 | tail -30`
Expected: All thinker tests PASS

**Step 7: Commit**

```bash
git add core/src/thinker/prompt_builder.rs core/src/thinker/context.rs
git commit -m "thinker: wire RuntimeContext into PromptBuilder"
```

---

### Task 3: ProtocolTokens — Create token definitions and parser

**Files:**
- Create: `core/src/thinker/protocol_tokens.rs`

**Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_heartbeat_ok() {
        let result = ProtocolToken::parse("ALEPH_HEARTBEAT_OK");
        assert!(matches!(result, Some(ProtocolToken::HeartbeatOk)));
    }

    #[test]
    fn test_parse_heartbeat_ok_with_whitespace() {
        let result = ProtocolToken::parse("  ALEPH_HEARTBEAT_OK  \n");
        assert!(matches!(result, Some(ProtocolToken::HeartbeatOk)));
    }

    #[test]
    fn test_parse_silent_complete() {
        let result = ProtocolToken::parse("ALEPH_SILENT_COMPLETE");
        assert!(matches!(result, Some(ProtocolToken::SilentComplete)));
    }

    #[test]
    fn test_parse_no_reply() {
        let result = ProtocolToken::parse("ALEPH_NO_REPLY");
        assert!(matches!(result, Some(ProtocolToken::NoReply)));
    }

    #[test]
    fn test_parse_needs_attention() {
        let result = ProtocolToken::parse("ALEPH_NEEDS_ATTENTION: Database disk usage at 95%");
        assert!(matches!(result, Some(ProtocolToken::NeedsAttention(msg)) if msg == "Database disk usage at 95%"));
    }

    #[test]
    fn test_parse_normal_response_returns_none() {
        let result = ProtocolToken::parse(r#"{"reasoning": "hello", "action": {"type": "complete"}}"#);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_mixed_content_returns_none() {
        // Token mixed with other text should NOT match
        let result = ProtocolToken::parse("All good. ALEPH_HEARTBEAT_OK");
        assert!(result.is_none());
    }

    #[test]
    fn test_to_prompt_section_contains_all_tokens() {
        let section = ProtocolToken::to_prompt_section();
        assert!(section.contains("ALEPH_HEARTBEAT_OK"));
        assert!(section.contains("ALEPH_SILENT_COMPLETE"));
        assert!(section.contains("ALEPH_NO_REPLY"));
        assert!(section.contains("ALEPH_NEEDS_ATTENTION"));
        assert!(section.contains("## Response Protocol Tokens"));
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::protocol_tokens -- --nocapture 2>&1 | head -20`
Expected: FAIL — module doesn't exist

**Step 3: Write implementation**

```rust
//! Aleph Protocol Tokens for structured LLM-to-system communication
//!
//! Defines protocol tokens that LLM returns as its ENTIRE response in
//! background/automated scenarios. These are intercepted by the DecisionParser
//! before JSON parsing, enabling minimal-cost responses.

/// Protocol tokens for structured LLM-to-system communication.
///
/// When the LLM returns one of these tokens as its entire response,
/// the system intercepts it and converts to the appropriate Decision
/// variant without requiring JSON parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolToken {
    /// Heartbeat check: nothing to report
    HeartbeatOk,
    /// Background task completed, no user notification needed
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

    /// Parse raw LLM output into a protocol token.
    ///
    /// Returns `Some(token)` if the entire (trimmed) response is a valid
    /// protocol token. Returns `None` for normal responses containing
    /// JSON or mixed content.
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        match trimmed {
            Self::HEARTBEAT_OK => Some(Self::HeartbeatOk),
            Self::SILENT_COMPLETE => Some(Self::SilentComplete),
            Self::NO_REPLY => Some(Self::NoReply),
            s if s.starts_with(Self::NEEDS_ATTENTION_PREFIX) => {
                let msg = s[Self::NEEDS_ATTENTION_PREFIX.len()..].trim().to_string();
                if msg.is_empty() {
                    None // NEEDS_ATTENTION requires a message
                } else {
                    Some(Self::NeedsAttention(msg))
                }
            }
            _ => None,
        }
    }

    /// Generate the prompt section that teaches LLM about protocol tokens.
    ///
    /// This should be injected when `Capability::SilentReply` is active.
    pub fn to_prompt_section() -> String {
        let mut s = String::new();
        s.push_str("## Response Protocol Tokens\n\n");
        s.push_str("When operating in background mode, use these exact tokens as your ENTIRE response:\n\n");
        s.push_str(&format!("- `{}` — Heartbeat check found nothing to report.\n", Self::HEARTBEAT_OK));
        s.push_str(&format!("- `{}` — Background task completed successfully, no user notification needed.\n", Self::SILENT_COMPLETE));
        s.push_str(&format!("- `{}` — No meaningful response to give.\n", Self::NO_REPLY));
        s.push_str(&format!("- `{} <brief description>` — Something requires user attention.\n\n", Self::NEEDS_ATTENTION_PREFIX));
        s.push_str("Rules:\n");
        s.push_str("- Token must be the ENTIRE message. Never mix with normal text.\n");
        s.push_str("- Use ALEPH_HEARTBEAT_OK for routine heartbeat checks with no findings.\n");
        s.push_str("- Use ALEPH_NEEDS_ATTENTION only when there is genuinely actionable information.\n\n");
        s
    }
}
```

**Step 4: Register module in mod.rs**

In `core/src/thinker/mod.rs`, add:
```rust
pub mod protocol_tokens;
```
And export:
```rust
pub use protocol_tokens::ProtocolToken;
```

**Step 5: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::protocol_tokens -- --nocapture 2>&1 | tail -20`
Expected: All 8 tests PASS

**Step 6: Commit**

```bash
git add core/src/thinker/protocol_tokens.rs core/src/thinker/mod.rs
git commit -m "thinker: add AlephProtocolTokens for silent response protocol"
```

---

### Task 4: ProtocolTokens — Wire into PromptBuilder and DecisionParser

**Files:**
- Modify: `core/src/thinker/prompt_builder.rs`
- Modify: `core/src/thinker/decision_parser.rs`

**Step 1: Write the failing test for PromptBuilder**

Add to prompt_builder.rs tests:

```rust
#[test]
fn test_append_protocol_tokens_with_silent_reply() {
    use super::context::EnvironmentContract;
    use super::interaction::{Capability, InteractionConstraints, InteractionParadigm};

    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();

    let contract = EnvironmentContract {
        paradigm: InteractionParadigm::Background,
        active_capabilities: vec![Capability::SilentReply],
        constraints: InteractionConstraints::default(),
        security_notes: vec![],
    };

    builder.append_protocol_tokens(&mut prompt, &contract);

    assert!(prompt.contains("ALEPH_HEARTBEAT_OK"));
    assert!(prompt.contains("ALEPH_SILENT_COMPLETE"));
    assert!(prompt.contains("Response Protocol Tokens"));
}

#[test]
fn test_append_protocol_tokens_without_silent_reply() {
    use super::context::EnvironmentContract;
    use super::interaction::{InteractionConstraints, InteractionParadigm};

    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();

    let contract = EnvironmentContract {
        paradigm: InteractionParadigm::CLI,
        active_capabilities: vec![],
        constraints: InteractionConstraints::default(),
        security_notes: vec![],
    };

    builder.append_protocol_tokens(&mut prompt, &contract);

    // Should NOT inject protocol tokens without SilentReply
    assert!(!prompt.contains("ALEPH_HEARTBEAT_OK"));
}
```

**Step 2: Run to verify failure**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::prompt_builder::tests::test_append_protocol_tokens -- --nocapture 2>&1 | tail -10`
Expected: FAIL

**Step 3: Implement PromptBuilder integration**

Add to `PromptBuilder` impl block:

```rust
/// Append protocol tokens section (replaces append_silent_behavior for protocol-aware mode)
pub fn append_protocol_tokens(
    &self,
    prompt: &mut String,
    contract: &super::context::EnvironmentContract,
) {
    if !contract.active_capabilities.contains(&Capability::SilentReply) {
        return;
    }
    prompt.push_str(&super::protocol_tokens::ProtocolToken::to_prompt_section());
}
```

In `build_system_prompt_with_context()`, replace the `append_silent_behavior` call (line ~839) with:

```rust
// 7. Protocol tokens (replaces basic silent behavior with structured protocol)
self.append_protocol_tokens(&mut prompt, &ctx.environment_contract);
```

**Step 4: Write DecisionParser interception test**

Add to `decision_parser.rs` tests (or create new test):

```rust
#[test]
fn test_parse_protocol_token_heartbeat() {
    let parser = DecisionParser::new();
    let result = parser.parse_with_fallback("ALEPH_HEARTBEAT_OK");
    assert!(result.is_ok());
    let thinking = result.unwrap();
    assert!(matches!(thinking.decision, Decision::HeartbeatOk));
}

#[test]
fn test_parse_protocol_token_no_reply() {
    let parser = DecisionParser::new();
    let result = parser.parse_with_fallback("ALEPH_NO_REPLY");
    assert!(result.is_ok());
    let thinking = result.unwrap();
    assert!(matches!(thinking.decision, Decision::Silent));
}
```

**Step 5: Implement DecisionParser interception**

In `decision_parser.rs`, at the top of `parse()` and `parse_with_fallback()`, add:

```rust
// Check for protocol tokens before JSON parsing
if let Some(token) = super::protocol_tokens::ProtocolToken::parse(response) {
    let decision = match token {
        super::protocol_tokens::ProtocolToken::HeartbeatOk => Decision::HeartbeatOk,
        super::protocol_tokens::ProtocolToken::SilentComplete => Decision::Silent,
        super::protocol_tokens::ProtocolToken::NoReply => Decision::Silent,
        super::protocol_tokens::ProtocolToken::NeedsAttention(msg) => {
            Decision::Complete { summary: format!("⚠️ Needs Attention: {}", msg) }
        }
    };
    return Ok(Thinking {
        reasoning: Some("Protocol token response".to_string()),
        decision,
        structured: None,
    });
}
```

**Note:** Check the actual `Decision` enum variants in `core/src/agent_loop/decision.rs` and adjust accordingly. The existing `Decision::HeartbeatOk` and `Decision::Silent` variants should map directly.

**Step 6: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker -- --nocapture 2>&1 | tail -30`
Expected: All tests PASS

**Step 7: Commit**

```bash
git add core/src/thinker/prompt_builder.rs core/src/thinker/decision_parser.rs
git commit -m "thinker: wire ProtocolTokens into PromptBuilder and DecisionParser"
```

---

### Task 5: Operational Guidelines — Add prompt section

**Files:**
- Modify: `core/src/thinker/prompt_builder.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_append_operational_guidelines_background() {
    use super::interaction::InteractionParadigm;

    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();

    builder.append_operational_guidelines(&mut prompt, InteractionParadigm::Background);

    assert!(prompt.contains("System Operational Awareness"));
    assert!(prompt.contains("Diagnostic Capabilities"));
    assert!(prompt.contains("NEVER Do Autonomously"));
}

#[test]
fn test_append_operational_guidelines_cli() {
    use super::interaction::InteractionParadigm;

    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();

    builder.append_operational_guidelines(&mut prompt, InteractionParadigm::CLI);

    // CLI should also get operational guidelines
    assert!(prompt.contains("System Operational Awareness"));
}

#[test]
fn test_append_operational_guidelines_messaging_skipped() {
    use super::interaction::InteractionParadigm;

    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();

    builder.append_operational_guidelines(&mut prompt, InteractionParadigm::Messaging);

    // Messaging should NOT get operational guidelines (save tokens)
    assert!(!prompt.contains("System Operational Awareness"));
}
```

**Step 2: Run to verify failure**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::prompt_builder::tests::test_append_operational_guidelines -- --nocapture 2>&1 | tail -10`
Expected: FAIL

**Step 3: Implement**

Add to `PromptBuilder` impl block:

```rust
/// Append system operational awareness guidelines.
///
/// Only injected for Background and CLI paradigms where the LLM
/// may need to detect and report system issues proactively.
pub fn append_operational_guidelines(
    &self,
    prompt: &mut String,
    paradigm: InteractionParadigm,
) {
    match paradigm {
        InteractionParadigm::Background | InteractionParadigm::CLI => {}
        _ => return, // Skip for Messaging, WebRich, Embedded
    }

    prompt.push_str("## System Operational Awareness\n\n");
    prompt.push_str("You are aware of your own runtime environment and can monitor it proactively.\n\n");

    prompt.push_str("### Diagnostic Capabilities (read-only, always allowed)\n");
    prompt.push_str("- Check disk space: `df -h`\n");
    prompt.push_str("- Check memory usage: `vm_stat` / `free -h`\n");
    prompt.push_str("- Check running Aleph processes: `ps aux | grep aleph`\n");
    prompt.push_str("- Check configuration validity: read config files and validate structure\n");
    prompt.push_str("- Check Desktop Bridge status: query UDS socket availability\n");
    prompt.push_str("- Check LanceDB health: verify database file accessibility\n\n");

    prompt.push_str("### When You Detect Issues\n");
    prompt.push_str("If you notice configuration conflicts, database issues, disconnected bridges,\n");
    prompt.push_str("abnormal resource usage, or runtime capability degradation:\n\n");
    prompt.push_str("**Action**: Report to the user with:\n");
    prompt.push_str("1. What you observed (specific evidence)\n");
    prompt.push_str("2. Potential impact\n");
    prompt.push_str("3. Suggested remediation steps\n");
    prompt.push_str("4. Do NOT execute remediation without explicit user approval\n\n");

    prompt.push_str("### What You Must NEVER Do Autonomously\n");
    prompt.push_str("- Restart Aleph services\n");
    prompt.push_str("- Modify configuration files\n");
    prompt.push_str("- Delete or compact databases\n");
    prompt.push_str("- Kill processes\n");
    prompt.push_str("- Change system settings\n\n");
}
```

**Step 4: Wire into `build_system_prompt_with_context`**

In `build_system_prompt_with_context()`, after the protocol tokens section, add:

```rust
// 8. Operational guidelines (Background/CLI only)
self.append_operational_guidelines(&mut prompt, ctx.environment_contract.paradigm);
```

**Step 5: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::prompt_builder::tests::test_append_operational_guidelines -- --nocapture 2>&1 | tail -15`
Expected: 3 tests PASS

**Step 6: Commit**

```bash
git add core/src/thinker/prompt_builder.rs
git commit -m "thinker: add System Operational Guidelines prompt section"
```

---

### Task 6: Citation Standards — Add prompt section

**Files:**
- Modify: `core/src/thinker/prompt_builder.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_append_citation_standards() {
    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();

    builder.append_citation_standards(&mut prompt);

    assert!(prompt.contains("## Citation Standards"));
    assert!(prompt.contains("[Source: <path>#<id>]"));
    assert!(prompt.contains("citation is mandatory"));
}
```

**Step 2: Run to verify failure**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::prompt_builder::tests::test_append_citation_standards -- --nocapture 2>&1 | tail -10`
Expected: FAIL

**Step 3: Implement**

Add to `PromptBuilder` impl block:

```rust
/// Append memory citation standards.
///
/// Always injected — citation standards are valuable in all interaction modes.
pub fn append_citation_standards(&self, prompt: &mut String) {
    prompt.push_str("## Citation Standards\n\n");
    prompt.push_str("When referencing information from memory or knowledge base:\n");
    prompt.push_str("- Include source reference in format: `[Source: <path>#<id>]` or `[Source: <path>#L<line>]`\n");
    prompt.push_str("- Sources are provided in the context metadata — do not fabricate source paths\n");
    prompt.push_str("- If multiple sources support a claim, cite the most specific one\n");
    prompt.push_str("- For real-time observations (current tool output, live data), no citation needed\n");
    prompt.push_str("- For recalled facts, prior decisions, or historical context, citation is mandatory\n\n");
}
```

**Step 4: Wire into all build methods**

In `build_system_prompt_with_context()`, after operational guidelines:
```rust
// 9. Citation standards (always injected)
self.append_citation_standards(&mut prompt);
```

Also add to `build_system_prompt_with_runtime()` and `build_system_prompt_with_soul()`:
```rust
// Citation standards
self.append_citation_standards(&mut prompt);
```

**Step 5: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::prompt_builder -- --nocapture 2>&1 | tail -20`
Expected: All tests PASS

**Step 6: Commit**

```bash
git add core/src/thinker/prompt_builder.rs
git commit -m "thinker: add Memory Citation Standards prompt section"
```

---

### Task 7: Memory Augmentation — Add source metadata to formatted output

**Files:**
- Modify: `core/src/memory/augmentation.rs`

**Step 1: Read the current augmentation.rs to understand exact formatting**

Read: `core/src/memory/augmentation.rs`

This step requires reading the actual code first to determine the exact integration point. The goal is to prefix memory facts with `[Source: path#id]` when formatting for prompt injection.

**Step 2: Write the failing test**

Add a test that checks memory formatting includes source metadata. The exact test depends on the current `format_memories` signature. Example:

```rust
#[test]
fn test_format_memory_fact_includes_source() {
    let fact = MemoryFact {
        id: "fact-123".to_string(),
        path: "aleph://user/preferences/coding".to_string(),
        content: "User prefers Rust for systems programming".to_string(),
        // ... other fields with defaults
    };

    let formatted = format_fact_with_source(&fact);
    assert!(formatted.contains("[Source: aleph://user/preferences/coding#fact-123]"));
    assert!(formatted.contains("User prefers Rust for systems programming"));
}
```

**Step 3: Implement source metadata formatting**

Add a helper function for formatting facts with source:

```rust
/// Format a memory fact with source attribution for LLM citation.
pub fn format_fact_with_source(fact: &MemoryFact) -> String {
    format!("[Source: {}#{}] {}", fact.path, fact.id, fact.content)
}
```

Update the existing memory-to-prompt formatting code to use this helper when injecting facts into prompts.

**Step 4: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib memory -- --nocapture 2>&1 | tail -20`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/augmentation.rs
git commit -m "memory: add source metadata to fact formatting for citation"
```

---

### Task 8: Integration test — Verify full prompt assembly

**Files:**
- Modify: `core/src/thinker/prompt_builder.rs` (test section)

**Step 1: Write integration test**

```rust
#[test]
fn test_full_prompt_with_all_enhancements() {
    use super::context::{ContextAggregator, EnvironmentContract, ResolvedContext};
    use super::interaction::{Capability, InteractionConstraints, InteractionManifest, InteractionParadigm};
    use super::runtime_context::RuntimeContext;
    use super::security_context::SecurityContext;

    let config = PromptConfig::default();
    let builder = PromptBuilder::new(config);

    // Build a Background-mode context (should trigger all 4 enhancements)
    let interaction = InteractionManifest::new(InteractionParadigm::Background);
    let security = SecurityContext::permissive();
    let mut resolved = ContextAggregator::resolve(&interaction, &security, &[]);

    // Add RuntimeContext
    resolved.runtime_context = Some(RuntimeContext {
        os: "macOS 15.3".to_string(),
        arch: "aarch64".to_string(),
        shell: "zsh".to_string(),
        working_dir: std::path::PathBuf::from("/workspace"),
        repo_root: Some(std::path::PathBuf::from("/workspace")),
        current_model: "claude-opus-4-6".to_string(),
        hostname: "test-host".to_string(),
    });

    let prompt = builder.build_system_prompt_with_context(&resolved);

    // 1. RuntimeContext should be present
    assert!(prompt.contains("## Runtime Environment"), "Missing RuntimeContext");
    assert!(prompt.contains("os=macOS 15.3"), "Missing OS info");

    // 2. Protocol tokens should be present (Background mode has SilentReply)
    assert!(prompt.contains("ALEPH_HEARTBEAT_OK"), "Missing protocol tokens");

    // 3. Operational guidelines should be present (Background mode)
    assert!(prompt.contains("System Operational Awareness"), "Missing operational guidelines");

    // 4. Citation standards should be present (always)
    assert!(prompt.contains("Citation Standards"), "Missing citation standards");

    // Standard sections should still be present
    assert!(prompt.contains("Your Role"), "Missing role section");
    assert!(prompt.contains("Response Format"), "Missing response format");
}

#[test]
fn test_interactive_prompt_minimal_token_overhead() {
    use super::context::{ContextAggregator, ResolvedContext};
    use super::interaction::{InteractionManifest, InteractionParadigm};
    use super::security_context::SecurityContext;

    let builder = PromptBuilder::new(PromptConfig::default());

    // WebRich mode (interactive) — should NOT include protocol tokens or operational guidelines
    let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
    let security = SecurityContext::permissive();
    let resolved = ContextAggregator::resolve(&interaction, &security, &[]);

    let prompt = builder.build_system_prompt_with_context(&resolved);

    // Should NOT have Background-only sections
    assert!(!prompt.contains("ALEPH_HEARTBEAT_OK"), "Protocol tokens in interactive mode");
    assert!(!prompt.contains("System Operational Awareness"), "Operational guidelines in interactive mode");

    // Should still have always-on sections
    assert!(prompt.contains("Citation Standards"), "Missing citation standards");
}
```

**Step 2: Run integration tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::prompt_builder::tests::test_full_prompt_with_all_enhancements -- --nocapture 2>&1 | tail -20`
Expected: PASS

**Step 3: Run full test suite**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore 2>&1 | tail -30`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/thinker/prompt_builder.rs
git commit -m "thinker: add integration tests for prompt system enhancements"
```

---

### Task 9: Final verification and cleanup

**Step 1: Run cargo clippy**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo clippy -p alephcore 2>&1 | tail -20`
Expected: No new warnings from our changes

**Step 2: Run cargo doc**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo doc -p alephcore --no-deps 2>&1 | tail -10`
Expected: Documentation builds without warnings

**Step 3: Fix any issues and commit**

If clippy/doc issues found, fix and commit:
```bash
git add -A && git commit -m "thinker: fix clippy warnings from prompt enhancements"
```

**Step 4: Final summary commit**

If no issues:
```bash
echo "All 4 prompt system enhancements implemented and tested."
```

---

## Summary of Changes

| Task | Files | Description |
|------|-------|-------------|
| 1-2 | `runtime_context.rs`, `prompt_builder.rs`, `context.rs`, `mod.rs` | RuntimeContext struct + collection + wiring |
| 3-4 | `protocol_tokens.rs`, `prompt_builder.rs`, `decision_parser.rs`, `mod.rs` | ProtocolToken enum + parser + interception |
| 5 | `prompt_builder.rs` | Operational Guidelines prompt section |
| 6 | `prompt_builder.rs` | Citation Standards prompt section |
| 7 | `augmentation.rs` | Memory fact source metadata formatting |
| 8 | `prompt_builder.rs` | Integration tests |
| 9 | Various | Clippy + doc + cleanup |

## Dependencies

- `hostname` crate — for `gethostname()` in RuntimeContext. Check if already in `Cargo.toml`; if not, add `hostname = "0.4"` to `[dependencies]` in `core/Cargo.toml`.
- No other new dependencies required.
