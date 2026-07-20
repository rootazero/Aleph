# ACP Coding Orchestrator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade ACP from single-shot delegation to multi-step coding orchestration — each harness supports dual mode (oneshot + native_acp), Manager gets a smart session pool, tools gain mode/session parameters, and an orchestration prompt teaches the LLM to act as tech lead.

**Architecture:** Harness trait gains `supported_modes()` + `default_mode` from config. Manager session pool keyed by `SessionKey(harness_id, cwd)` with extract-use-reinsert locking. Tools pass new `mode`/`reuse_session` params. Streaming via `AcpChunkCallback` closure. Static orchestration prompt in `PromptBuilder`.

**Tech Stack:** Rust, async_trait, tokio, serde, schemars, Leptos (Panel)

**Spec:** `docs/superpowers/specs/2026-03-26-acp-coding-orchestrator-design.md`

---

### Task 0: Bug Fix — Config Preset Modes

**Files:**
- Modify: `src/config/types/acp.rs:151-195` (preset factories)
- Test: `src/config/types/acp.rs` (inline tests) + `tests/acp_probe/p1_config_and_presets.rs`

The root cause: all three preset factories set `mode: HarnessModeSerde::NativeAcp`, but Claude Code and Codex harness structs hard-code `HarnessMode::Oneshot`. Fix presets to set correct defaults.

- [ ] **Step 1: Write failing test**

In `src/config/types/acp.rs`, add at the bottom (or in existing test module):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preset_modes_match_harness_defaults() {
        let cc = AcpHarnessEntry::preset_claude_code();
        assert_eq!(cc.mode, HarnessModeSerde::Oneshot, "Claude Code preset should default to Oneshot");

        let codex = AcpHarnessEntry::preset_codex();
        assert_eq!(codex.mode, HarnessModeSerde::Oneshot, "Codex preset should default to Oneshot");

        let gemini = AcpHarnessEntry::preset_gemini();
        assert_eq!(gemini.mode, HarnessModeSerde::NativeAcp, "Gemini preset should default to NativeAcp");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib "acp::tests::test_preset_modes" -- --nocapture 2>&1 || cargo test -p alephcore --lib "test_preset_modes" -- --nocapture`
Expected: FAIL — Claude Code and Codex presets currently set `NativeAcp`

- [ ] **Step 3: Fix preset factories**

In `src/config/types/acp.rs`, change `preset_claude_code()` line 162:
```rust
// Before:
mode: HarnessModeSerde::NativeAcp,
// After:
mode: HarnessModeSerde::Oneshot,
```

Change `preset_codex()` line 177:
```rust
// Before:
mode: HarnessModeSerde::NativeAcp,
// After:
mode: HarnessModeSerde::Oneshot,
```

Leave `preset_gemini()` line 190 as `NativeAcp` (correct).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib "test_preset_modes" -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/config/types/acp.rs
git commit -m "acp: correct preset mode defaults — Claude Code and Codex should be Oneshot"
```

---

### Task 1: Config Layer — Rename `mode` to `default_mode` with Backward Compat

**Files:**
- Modify: `src/config/types/acp.rs:78-119` (AcpHarnessEntry)
- Modify: All files referencing `entry.mode` (manager, handlers, panel)

- [ ] **Step 1: Rename field with serde alias**

In `src/config/types/acp.rs`, change the `mode` field in `AcpHarnessEntry`:

```rust
// Before (line 92-93):
    #[serde(default)]
    pub mode: HarnessModeSerde,

// After:
    /// Default execution mode. LLM can override at call time.
    #[serde(default, alias = "mode")]
    pub default_mode: HarnessModeSerde,
```

Also update `Default for AcpHarnessEntry` (line 131):
```rust
// Before:
mode: HarnessModeSerde::default(),
// After:
default_mode: HarnessModeSerde::default(),
```

Update all three preset factories to use `default_mode:` instead of `mode:`.

- [ ] **Step 2: Fix all compilation errors**

Search for `.mode` on `AcpHarnessEntry` usages and replace with `.default_mode`. Key files:
- `src/acp/manager.rs` — `build_harness()` may reference `entry.mode`
- `src/acp/harnesses/custom.rs` — `CustomHarness` reads `entry.mode`
- `src/gateway/handlers/acp_config.rs` — RPC handlers serialize/deserialize entries
- `interfaces/webchat/src/views/settings/acp_harnesses.rs` — Panel reads mode

Run: `cargo check -p alephcore` and fix each error.

- [ ] **Step 3: Run all ACP tests**

Run: `cargo test -p alephcore --lib "acp" -- --nocapture`
Expected: All tests pass (serde alias preserves backward compat)

- [ ] **Step 4: Commit**

Ensure all affected files are staged, especially `src/gateway/handlers/acp_config.rs` which serializes/deserializes `AcpHarnessEntry`:

```bash
git add src/config/types/acp.rs src/acp/ src/gateway/handlers/acp_config.rs interfaces/webchat/
git commit -m "acp: rename mode to default_mode with serde backward compat"
```

---

### Task 2: Harness Trait — Add `supported_modes()` and `default_mode` Field

**Files:**
- Modify: `src/acp/harness.rs:46-127`
- Modify: `src/acp/harnesses/claude_code.rs`
- Modify: `src/acp/harnesses/codex.rs`
- Modify: `src/acp/harnesses/gemini.rs`
- Modify: `src/acp/harnesses/custom.rs`
- Modify: `src/acp/manager.rs:117-135` (build_harness)

- [ ] **Step 1: Add `supported_modes()` to trait**

In `src/acp/harness.rs`, add to the `AcpHarness` trait after `mode()`:

```rust
    /// Modes this harness supports. Used by Manager for runtime validation.
    fn supported_modes(&self) -> Vec<HarnessMode> {
        vec![self.mode()]
    }
```

- [ ] **Step 2: Add `default_mode` field to each harness struct**

Update `ClaudeCodeHarness` in `src/acp/harnesses/claude_code.rs`:

```rust
pub struct ClaudeCodeHarness {
    executable: String,
    default_mode: HarnessMode,
}

impl ClaudeCodeHarness {
    pub fn new(executable: Option<String>, default_mode: HarnessMode) -> Self {
        Self {
            executable: executable.unwrap_or_else(|| DEFAULT_EXECUTABLE.to_string()),
            default_mode,
        }
    }
}
```

Update `mode()` to return `self.default_mode`.

Update `supported_modes()`:
```rust
    fn supported_modes(&self) -> Vec<HarnessMode> {
        vec![HarnessMode::Oneshot, HarnessMode::NativeAcp]
    }
```

Apply the same pattern to `CodexHarness` and `GeminiHarness`.

For `CustomHarness`, read mode from its config entry and return `vec![mode]` for `supported_modes()` (custom harnesses support only their configured mode).

- [ ] **Step 3: Update `build_harness()` in manager**

In `src/acp/manager.rs:117-135`, pass `default_mode` from config:

```rust
fn build_harness(id: &str, entry: &AcpHarnessEntry) -> Box<dyn AcpHarness> {
    let default_mode = HarnessMode::from_serde(&entry.default_mode);
    let preset = entry.preset.as_deref().unwrap_or("");
    match preset {
        "claude-code" => {
            Box::new(ClaudeCodeHarness::new(entry.executable.clone(), default_mode))
        }
        "codex" => {
            Box::new(CodexHarness::new(entry.executable.clone(), default_mode))
        }
        "gemini" => {
            Box::new(GeminiHarness::new(entry.executable.clone(), default_mode))
        }
        _ => {
            Box::new(CustomHarness::new(id.to_string(), entry.clone()))
        }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib "acp" -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/acp/
git commit -m "acp: add supported_modes and configurable default_mode to harnesses"
```

---

### Tasks 3, 4, 5 are independent and can run in parallel.

### Task 3: Dual-Mode — Add Native ACP to Claude Code Harness

**Files:**
- Modify: `src/acp/harnesses/claude_code.rs`
- Test: `tests/acp_probe/p5_tool_execution.rs` (add dual-mode test)

- [ ] **Step 1: Write failing test**

In `tests/acp_probe/p5_tool_execution.rs`, add:

```rust
#[tokio::test]
async fn test_claude_code_harness_supports_both_modes() {
    let harness = ClaudeCodeHarness::new(None, HarnessMode::Oneshot);
    let modes = harness.supported_modes();
    assert!(modes.contains(&HarnessMode::Oneshot));
    assert!(modes.contains(&HarnessMode::NativeAcp));
}
```

- [ ] **Step 2: Add `spawn_session` override for native ACP**

In `src/acp/harnesses/claude_code.rs`, add the native ACP spawn config:

```rust
    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        match self.default_mode {
            HarnessMode::NativeAcp => HarnessConfig {
                executable: self.executable.clone(),
                args: vec!["--acp".to_string()],
                cwd: cwd.map(String::from),
                ..Default::default()
            },
            HarnessMode::Oneshot => HarnessConfig {
                executable: self.executable.clone(),
                args: vec![
                    "--print".to_string(),
                    "--output-format".to_string(),
                    "json".to_string(),
                ],
                cwd: cwd.map(String::from),
                ..Default::default()
            },
        }
    }
```

Note: `build_config` is also used by `spawn_session` (the default trait impl calls `self.build_config(cwd)` then `AcpSession::spawn`). So when mode is NativeAcp, `build_config` should return the `--acp` config. But `build_config` is called from both `spawn_session` and `is_available`, so we need a mode-aware approach. The simplest: add a second `build_config_for_mode` method, or always return the oneshot config for `build_config` (used by `is_available`) and override `spawn_session` to use `--acp` args directly.

Better approach — override `spawn_session` explicitly:

```rust
    async fn spawn_session(&self, cwd: Option<&str>) -> Result<AcpSession> {
        let config = HarnessConfig {
            executable: self.executable.clone(),
            args: vec!["--acp".to_string()],
            cwd: cwd.map(String::from),
            ..Default::default()
        };
        let timeout = config.timeout;
        let mut session = AcpSession::spawn(self.id(), &config).await?;
        session.initialize(timeout).await?;
        Ok(session)
    }
```

Keep `build_config` returning the oneshot config (used for `is_available` check).

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib "acp" -- --nocapture`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/acp/harnesses/claude_code.rs tests/acp_probe/
git commit -m "acp: add native ACP mode to Claude Code harness"
```

---

### Task 4: Dual-Mode — Add Native ACP to Codex Harness

**Files:**
- Modify: `src/acp/harnesses/codex.rs`

Same pattern as Task 3. Override `spawn_session` with `codex --acp` args. Add `supported_modes` returning both modes.

- [ ] **Step 1: Add `spawn_session` and `supported_modes` to CodexHarness**

Same pattern as Claude Code. The `spawn_session` override uses `codex --acp` args.

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib "acp" -- --nocapture`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/acp/harnesses/codex.rs
git commit -m "acp: add native ACP mode to Codex harness"
```

---

### Task 5: Dual-Mode — Add Oneshot to Gemini Harness

**Files:**
- Modify: `src/acp/harnesses/gemini.rs`

- [ ] **Step 1: Add `execute_oneshot` to GeminiHarness**

```rust
    async fn execute_oneshot(&self, prompt: &str, cwd: &str) -> Result<String> {
        use tokio::process::Command;
        use tracing::{debug, error};

        let mut cmd = Command::new(&self.executable);
        // Gemini CLI oneshot: `gemini -p "<prompt>"` or similar
        // Verify actual CLI flags — may need `-C <cwd>` for working dir
        cmd.args(["-p", prompt])
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        debug!(harness = "gemini", "Spawning oneshot Gemini process");

        let output = cmd.output().await.map_err(|e| {
            crate::error::AlephError::tool(format!(
                "Failed to execute Gemini CLI: {}. Is 'gemini' installed and in PATH?",
                e
            ))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(harness = "gemini", stderr = %stderr, "Gemini CLI failed");
            return Err(crate::error::AlephError::tool(format!(
                "Gemini CLI exited with {}: {}",
                output.status,
                stderr.chars().take(500).collect::<String>()
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn supported_modes(&self) -> Vec<HarnessMode> {
        vec![HarnessMode::NativeAcp, HarnessMode::Oneshot]
    }
```

Note: Verify exact Gemini CLI oneshot flags during implementation. If Gemini CLI doesn't support oneshot, `supported_modes` should return `[NativeAcp]` only and `execute_oneshot` returns the default error.

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib "acp" -- --nocapture`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/acp/harnesses/gemini.rs
git commit -m "acp: add oneshot mode to Gemini harness"
```

---

### Task 6: Session Pool — SessionKey and Extract-Use-Reinsert

**Files:**
- Modify: `src/acp/manager.rs`
- Modify: `src/builtin_tools/acp_tools.rs` (AcpSwitchTool)

This is the most complex task. Replace `HashMap<String, AcpSession>` with `HashMap<SessionKey, AcpSession>`.

**Note:** Task 7 (AcpChunkCallback) changes `session.prompt()` signature. In this task, use the current 3-arg signature `session.prompt(text, cwd, timeout)`. Task 7 will add the `on_chunk` parameter afterwards. The `on_chunk` parameter in `manager.prompt()` signature is included here for completeness but will be a `let _on_chunk = on_chunk;` placeholder until Task 7 wires it through.

- [ ] **Step 1: Write failing test**

Add to `src/acp/manager.rs` tests:

```rust
#[test]
fn test_session_key_canonicalization() {
    use crate::acp::manager::SessionKey;
    // Same directory, different representations should produce equal keys
    let k1 = SessionKey::new("claude-code", "/tmp");
    let k2 = SessionKey::new("claude-code", "/tmp/");
    // After canonicalization, these should be equal (if /tmp exists)
    // Note: this test relies on /tmp existing on the system
    assert_eq!(k1, k2);
}
```

- [ ] **Step 2: Define SessionKey**

At the top of `src/acp/manager.rs`:

```rust
use std::path::PathBuf;

/// Canonicalized session pool key — prevents duplicate sessions for equivalent paths.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct SessionKey {
    harness_id: String,
    cwd: PathBuf,
}

impl SessionKey {
    pub fn new(harness_id: &str, cwd: &str) -> Self {
        Self {
            harness_id: harness_id.to_string(),
            cwd: std::fs::canonicalize(cwd).unwrap_or_else(|_| {
                tracing::debug!(cwd, "SessionKey: canonicalize failed, using raw path");
                PathBuf::from(cwd)
            }),
        }
    }
}
```

- [ ] **Step 3: Update AcpHarnessManager struct**

```rust
pub struct AcpHarnessManager {
    harnesses: RwLock<HashMap<String, Box<dyn AcpHarness>>>,
    configs: RwLock<HashMap<String, AcpHarnessEntry>>,
    /// Active sessions for NativeAcp harnesses, keyed by (harness_id, canonicalized cwd).
    sessions: RwLock<HashMap<SessionKey, AcpSession>>,
}
```

- [ ] **Step 4: Update `prompt()` with new signature and extract-use-reinsert**

```rust
    /// Send a prompt to the specified harness.
    ///
    /// - `mode`: Override the harness default mode. `None` = use harness default.
    /// - `reuse_session`: For NativeAcp, reuse existing session if alive. Ignored for Oneshot.
    /// - `on_chunk`: Optional callback for streaming chunk forwarding.
    pub async fn prompt(
        &self,
        harness_id: &str,
        prompt_text: &str,
        cwd: &str,
        mode: Option<HarnessMode>,
        reuse_session: bool,
        on_chunk: Option<AcpChunkCallback>,
    ) -> Result<String> {
        let harnesses = self.harnesses.read().await;
        let harness = harnesses.get(harness_id).ok_or_else(|| {
            AlephError::tool(format!("Unknown ACP harness: '{}'", harness_id))
        })?;

        let effective_mode = mode.unwrap_or_else(|| harness.mode());

        // Validate mode is supported
        if !harness.supported_modes().contains(&effective_mode) {
            return Err(AlephError::tool(format!(
                "Harness '{}' does not support {:?} mode",
                harness_id, effective_mode
            )));
        }

        match effective_mode {
            HarnessMode::NativeAcp => {
                let timeout = harness.build_config(Some(cwd)).timeout;
                drop(harnesses); // Release read lock

                let key = SessionKey::new(harness_id, cwd);
                let _on_chunk = on_chunk; // placeholder until Task 7 wires through

                // Extract session from pool (brief write lock)
                let existing = {
                    let mut pool = self.sessions.write().await;
                    if reuse_session {
                        if let Some(mut s) = pool.remove(&key) {
                            if s.is_alive() {
                                Some(s)
                            } else {
                                warn!(harness_id, "Dead session removed from pool");
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        // Force new — kill existing if any
                        if let Some(mut s) = pool.remove(&key) {
                            s.kill().await;
                        }
                        None
                    }
                };
                // Write lock released here

                // Create new session if needed
                let mut session = if let Some(s) = existing {
                    s
                } else {
                    let harnesses = self.harnesses.read().await;
                    let harness = harnesses.get(harness_id).ok_or_else(|| {
                        AlephError::tool(format!("Harness '{}' disappeared", harness_id))
                    })?;
                    harness.spawn_session(Some(cwd)).await?
                };

                // Use session without holding pool lock
                // Note: session.prompt() uses current 3-arg signature;
                // Task 7 will add on_chunk parameter
                let result = session.prompt(prompt_text, cwd, timeout).await;

                // Re-insert session (even on error — session may recover)
                if session.is_alive() {
                    self.sessions.write().await.insert(key, session);
                }

                result.map(|(text, _)| text)
            }
            HarnessMode::Oneshot => {
                harness.execute_oneshot(prompt_text, cwd).await
            }
        }
    }
```

- [ ] **Step 5: Update `ensure_session`, `cancel`, `unregister_harness`, `update_harness`, `shutdown_all`**

These methods use `sessions` with string keys — update them to use `SessionKey`. For methods that iterate by harness_id, filter on `key.harness_id`.

**`ensure_session`** — now takes `SessionKey`:
```rust
    pub async fn ensure_session(&self, harness_id: &str, cwd: &str) -> Result<()> {
        let key = SessionKey::new(harness_id, cwd);
        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(&key) {
                if session.is_alive() {
                    return Ok(());
                }
                warn!(harness_id, "ACP session died, respawning");
                sessions.remove(&key);
            }
        }

        let harnesses = self.harnesses.read().await;
        let harness = harnesses.get(harness_id).ok_or_else(|| {
            AlephError::tool(format!("Unknown ACP harness: '{}'", harness_id))
        })?;
        let session = harness.spawn_session(Some(cwd)).await?;
        info!(harness_id, "ACP session started");
        drop(harnesses);

        self.sessions.write().await.insert(key, session);
        Ok(())
    }
```

**`cancel`** — needs harness_id + cwd to find session:
```rust
    pub async fn cancel(&self, harness_id: &str, cwd: &str) -> Result<()> {
        let key = SessionKey::new(harness_id, cwd);
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(&key).ok_or_else(|| {
            AlephError::tool(format!("No active ACP session for '{}'", harness_id))
        })?;
        session.cancel().await
    }
```

**`unregister_harness` / `update_harness`** — iterate by harness_id:
```rust
let mut sessions = self.sessions.write().await;
let keys_to_remove: Vec<SessionKey> = sessions.keys()
    .filter(|k| k.harness_id == id)
    .cloned()
    .collect();
for key in keys_to_remove {
    if let Some(mut session) = sessions.remove(&key) {
        session.kill().await;
    }
}
```

Apply similar pattern to `shutdown_all`.

- [ ] **Step 6: Update `AcpSwitchTool` for new `ensure_session` and `cancel` signatures**

In `src/builtin_tools/acp_tools.rs`, `AcpSwitchTool::call()` at line 260 calls `self.manager.ensure_session(&args.target, &cwd)` — this still works with the new signature (takes `harness_id` + `cwd` strings, internally creates `SessionKey`). No change needed for `ensure_session`.

However, verify that `harness_mode()` still returns the correct mode — after Task 2, it returns `self.default_mode` from config, which is correct.

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib "acp" -- --nocapture`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/acp/manager.rs
git commit -m "acp: SessionKey pool with extract-use-reinsert locking"
```

---

### Task 7: Streaming — AcpChunkCallback

**Files:**
- Modify: `src/acp/session.rs:171-220`
- Create: `src/acp/callback.rs` (or add to `mod.rs`)

- [ ] **Step 1: Define AcpChunkCallback type**

Add directly to `src/acp/mod.rs` (no separate file needed — it's a single type alias):

```rust
use crate::sync_primitives::Arc;

/// Callback for real-time ACP streaming chunks.
/// Receives chunk text as it arrives from the external tool.
pub type AcpChunkCallback = Arc<dyn Fn(&str) + Send + Sync>;
```

This is automatically public since it's in `mod.rs`. Other modules import via `use crate::acp::AcpChunkCallback;`.

- [ ] **Step 2: Update `session.prompt()` to accept callback**

In `src/acp/session.rs`, change signature:

```rust
    pub async fn prompt(
        &mut self,
        text: &str,
        cwd: &str,
        timeout: Duration,
        on_chunk: Option<&AcpChunkCallback>,  // new
    ) -> Result<(String, Vec<AcpResponse>)> {
```

In the streaming chunk collection loop, add callback invocation:

```rust
                for notif in &notifications {
                    if let Some(chunk) = notif.streaming_text() {
                        // Forward to callback if provided
                        if let Some(cb) = &on_chunk {
                            cb(&chunk);
                        }
                        text_parts.push(chunk);
                    }
                }
```

- [ ] **Step 3: Wire callback through manager.prompt()**

In `src/acp/manager.rs`, update the `prompt()` method's NativeAcp branch:
- Remove the `let _on_chunk = on_chunk;` placeholder added in Task 6
- Change `session.prompt(prompt_text, cwd, timeout)` to `session.prompt(prompt_text, cwd, timeout, on_chunk.as_ref())`

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib "acp" -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/acp/
git commit -m "acp: add AcpChunkCallback for streaming passthrough"
```

---

### Task 8: Tool Layer — Extended Args and Mode Routing

**Files:**
- Modify: `src/builtin_tools/acp_tools.rs`

- [ ] **Step 1: Extend AcpDelegateArgs**

```rust
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AcpDelegateArgs {
    /// The prompt / task description to send to the external CLI agent.
    pub prompt: String,
    /// Working directory for the agent session. Defaults to home directory.
    pub cwd: Option<String>,
    /// Execution mode override: "oneshot" or "native_acp". Defaults to harness config.
    pub mode: Option<String>,
    /// Whether to reuse an existing session (for multi-step continuity).
    /// Only applies to native_acp mode. Defaults to true.
    pub reuse_session: Option<bool>,
}
```

- [ ] **Step 2: Update tool `call()` methods**

Update `ClaudeCodeTool::call()`:

```rust
    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let args_summary = format!("Claude Code: {}", truncate(&args.prompt, 80));
        notify_tool_start(Self::NAME, &args_summary);

        let cwd = resolve_cwd(args.cwd.as_deref());
        let mode = args.mode.as_deref().map(parse_mode).transpose()?;
        let reuse = args.reuse_session.unwrap_or(true);

        let result = self.manager.prompt(
            "claude-code", &args.prompt, &cwd,
            mode, reuse, None, // on_chunk: None for now
        ).await;

        match result {
            Ok(text) => {
                notify_tool_result(Self::NAME, "completed", true);
                Ok(AcpDelegateOutput { harness: "claude-code".to_string(), result: text })
            }
            Err(e) => {
                notify_tool_result(Self::NAME, &e.to_string(), false);
                Err(e)
            }
        }
    }
```

Add helper:
```rust
fn parse_mode(s: &str) -> Result<HarnessMode> {
    match s {
        "oneshot" => Ok(HarnessMode::Oneshot),
        "native_acp" => Ok(HarnessMode::NativeAcp),
        _ => Err(AlephError::tool(format!(
            "Invalid mode '{}'. Use 'oneshot' or 'native_acp'.", s
        ))),
    }
}
```

Apply same changes to `CodexTool::call()` and `GeminiCliTool::call()`.

- [ ] **Step 3: Update tool descriptions**

```rust
    const DESCRIPTION: &'static str =
        "Delegate a coding task to Claude Code CLI. Supports two modes: \
         'oneshot' (fresh process per prompt, default) and 'native_acp' \
         (persistent session with context continuity). Use reuse_session=true \
         for multi-step workflows where the tool needs prior context.";
```

Apply similar updates to Codex and Gemini tool descriptions.

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib "acp" -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/acp_tools.rs
git commit -m "acp: extend tool args with mode and reuse_session parameters"
```

---

### Task 9: Orchestration Prompt

**Files:**
- Modify: `src/agent_loop/prompt_builder.rs:29-59` (BASE_BEHAVIOR constant)

- [ ] **Step 1: Add orchestration section to BASE_BEHAVIOR**

Append to the `BASE_BEHAVIOR` constant in `src/agent_loop/prompt_builder.rs`:

```rust
// Add after the last line of BASE_BEHAVIOR (before the closing ";):
- **CODE TASK ORCHESTRATION.** When the user requests coding work, you have professional coding CLI tools at your disposal (claude_code, codex, gemini_cli). Use them like a tech lead directing engineers:\n\
  - Plan before code: For non-trivial tasks, first ask a tool to analyze and propose a plan. Review the plan, then proceed.\n\
  - Review after code: After code is written, consider asking the same or a different tool to review it.\n\
  - Parallel when independent: If tasks are independent (e.g., code + tests), dispatch multiple tools simultaneously.\n\
  - Reuse sessions for continuity: When follow-up prompts need prior context (e.g., \"now add error handling to what you wrote\"), reuse the session so the tool retains conversation history.\n\
  - Switch tools strategically: Different tools have different strengths. You may use one for planning and another for implementation.\n\
  - Handle failures: If a tool fails or produces poor results, retry, try a different tool, or ask the user — use your judgment.
```

- [ ] **Step 2: Write test**

Add to tests in `src/agent_loop/prompt_builder.rs`:

```rust
    #[test]
    fn test_build_includes_orchestration_prompt() {
        let prompt = PromptBuilder::new().build(&[], None);
        assert!(prompt.contains("CODE TASK ORCHESTRATION"));
        assert!(prompt.contains("tech lead"));
    }
```

- [ ] **Step 3: Run test**

Run: `cargo test -p alephcore --lib "prompt_builder::tests::test_build_includes_orchestration" -- --nocapture`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/agent_loop/prompt_builder.rs
git commit -m "acp: add coding orchestration strategy to system prompt"
```

---

### Task 10: Panel — Fix Mode Display and Add Default Mode Toggle

**Files:**
- Modify: `interfaces/webchat/src/views/settings/acp_harnesses.rs`

- [ ] **Step 1: Update Panel to show `default_mode` correctly**

In the Panel's ACP harness settings component, find where the mode field is displayed. Update references from `.mode` to `.default_mode`. The mode display should show "Oneshot" or "Native ACP" based on the actual `default_mode` value.

Add a toggle/selector that allows users to switch `default_mode` between Oneshot and Native ACP for each harness. When changed, call `acp.update` RPC to persist.

- [ ] **Step 2: Build WASM to verify**

Run: `cd interfaces/webchat && trunk build` (or equivalent WASM build command)
Expected: Compiles without errors

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/
git commit -m "panel: show correct default_mode for ACP harnesses + add mode toggle"
```

---

### Task 11: Integration Test — Full Orchestration Flow

**Files:**
- Modify: `tests/acp_probe/p5_tool_execution.rs`

- [ ] **Step 1: Add test for mode override**

```rust
#[tokio::test]
async fn test_prompt_with_mode_override() {
    // Create manager with Claude Code default Oneshot
    let manager = AcpHarnessManager::new();
    // Verify mode validation works
    let result = manager.prompt(
        "claude-code", "test", "/tmp",
        Some(HarnessMode::NativeAcp), // override to NativeAcp
        true, None,
    ).await;
    // This will fail (no real claude binary) but should fail with
    // "Failed to execute" not "unsupported mode"
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(!err.contains("does not support"), "Mode should be supported: {}", err);
}
```

- [ ] **Step 2: Add test for session key canonicalization**

```rust
#[test]
fn test_session_key_equality() {
    let k1 = SessionKey::new("claude-code", "/tmp");
    let k2 = SessionKey::new("claude-code", "/tmp");
    assert_eq!(k1, k2);

    let k3 = SessionKey::new("codex", "/tmp");
    assert_ne!(k1, k3); // different harness
}
```

- [ ] **Step 3: Add test for reuse_session=false**

```rust
#[tokio::test]
async fn test_prompt_reuse_session_false_clears_existing() {
    let manager = AcpHarnessManager::new();
    // With reuse_session=false, should not error even without existing session
    let result = manager.prompt(
        "claude-code", "test", "/tmp",
        Some(HarnessMode::Oneshot),
        false, None,
    ).await;
    // Will fail due to no binary, but validates the code path
    assert!(result.is_err());
}
```

- [ ] **Step 4: Run all integration tests**

Run: `cargo test -p alephcore --test acp_probe -- --nocapture`
Expected: Tests that don't require real binaries pass. Tests requiring real CLIs may skip/fail — that's expected.

- [ ] **Step 5: Commit**

```bash
git add tests/acp_probe/
git commit -m "acp: add dual-mode, session pool, and orchestration integration tests"
```

---

### Task 12: Final Verification

- [ ] **Step 1: Full compile check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 2: Full test suite**

Run: `cargo test -p alephcore --lib -- --nocapture`
Expected: All tests pass (except pre-existing known failures in markdown_skill::loader)

- [ ] **Step 3: Clippy lint**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings

- [ ] **Step 4: Final commit if any cleanup needed**

```bash
git add -A
git commit -m "acp: final cleanup for coding orchestrator"
```
