# Channel Capability Awareness Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement environment-contract-driven adaptive system that allows AI to sense channel capabilities and security constraints.

**Architecture:** Three new modules: `interaction` (InteractionManifest), `security` (SecurityContext), `context` (ContextAggregator). These integrate with existing Thinker to produce enhanced system prompts with Environment Contract sections.

**Tech Stack:** Rust, serde, HashSet for capabilities, BDD testing with cucumber

---

## Task 1: Define Interaction Types

**Files:**
- Create: `core/src/thinker/interaction.rs`
- Modify: `core/src/thinker/mod.rs:32` (add module)

**Step 1: Write BDD feature file**

Create `core/tests/features/thinker/interaction.feature`:

```gherkin
Feature: Interaction Manifest
  As an AI assistant
  I need to know my environment's interaction capabilities
  So I can adapt my responses appropriately

  Scenario: CLI paradigm has limited capabilities
    Given an interaction manifest with paradigm "CLI"
    When I check for capability "MultiGroupUI"
    Then the capability should be absent
    And the capability "Streaming" should be present

  Scenario: Web paradigm has rich capabilities
    Given an interaction manifest with paradigm "WebRich"
    When I check for capability "MultiGroupUI"
    Then the capability should be present

  Scenario: Messaging paradigm has inline buttons
    Given an interaction manifest with paradigm "Messaging"
    And capability "InlineButtons" is enabled
    When I check for capability "InlineButtons"
    Then the capability should be present

  Scenario: Constraints are respected
    Given an interaction manifest with paradigm "Messaging"
    And max output chars is 4096
    When I get the constraints
    Then max_output_chars should be 4096
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test cucumber -- features/thinker/interaction.feature`
Expected: FAIL with "step not found"

**Step 3: Create interaction types**

Create `core/src/thinker/interaction.rs`:

```rust
//! Interaction Manifest - Environment capability awareness
//!
//! Describes what the current channel can technically do,
//! allowing the AI to adapt its responses appropriately.

use std::collections::HashSet;
use serde::{Deserialize, Serialize};

/// Interaction paradigm - defines base behavior mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionParadigm {
    /// Pure text terminal
    CLI,
    /// Rich interactive web interface
    WebRich,
    /// Messaging channel (Telegram, Discord, etc.)
    Messaging,
    /// Background task / scheduled job
    Background,
    /// Embedded / constrained environment
    Embedded,
}

impl InteractionParadigm {
    /// Get human-readable description for prompts
    pub fn description(&self) -> &'static str {
        match self {
            Self::CLI => "CLI (text-only terminal)",
            Self::WebRich => "Web Rich Interface (supports interactive UI)",
            Self::Messaging => "Messaging Channel (chat-optimized)",
            Self::Background => "Background Task (no direct user interaction)",
            Self::Embedded => "Embedded/Constrained Environment",
        }
    }

    /// Get default capabilities for this paradigm
    pub fn default_capabilities(&self) -> HashSet<Capability> {
        match self {
            Self::CLI => [
                Capability::RichText,
                Capability::CodeHighlight,
                Capability::Streaming,
            ].into_iter().collect(),
            Self::WebRich => [
                Capability::RichText,
                Capability::CodeHighlight,
                Capability::MultiGroupUI,
                Capability::Streaming,
                Capability::MermaidCharts,
                Capability::ImageInline,
                Capability::Canvas,
            ].into_iter().collect(),
            Self::Messaging => [
                Capability::RichText,
                Capability::ImageInline,
            ].into_iter().collect(),
            Self::Background => [
                Capability::SilentReply,
            ].into_iter().collect(),
            Self::Embedded => HashSet::new(),
        }
    }
}

/// Atomic interaction capability
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Markdown/HTML rendering
    RichText,
    /// Inline buttons / quick replies
    InlineButtons,
    /// ask_user_multigroup support
    MultiGroupUI,
    /// Streaming output
    Streaming,
    /// Inline image display
    ImageInline,
    /// Mermaid diagram rendering
    MermaidCharts,
    /// Code syntax highlighting
    CodeHighlight,
    /// User can upload files
    FileUpload,
    /// Canvas / visualization component
    Canvas,
    /// Silent reply support (background tasks)
    SilentReply,
}

impl Capability {
    /// Get name and hint for prompt generation
    pub fn prompt_hint(&self) -> (&'static str, &'static str) {
        match self {
            Self::RichText => ("rich_text", "Markdown rendering supported"),
            Self::InlineButtons => ("inline_buttons", "Offer quick-reply buttons when appropriate"),
            Self::MultiGroupUI => ("multi_group_ui", "Use ask_user_multigroup for structured input"),
            Self::Streaming => ("streaming", "Your reasoning is visible in real-time"),
            Self::ImageInline => ("image_inline", "Images display inline, no download needed"),
            Self::MermaidCharts => ("mermaid", "Render diagrams with ```mermaid blocks"),
            Self::CodeHighlight => ("code_highlight", "Syntax highlighting available"),
            Self::FileUpload => ("file_upload", "User can upload files"),
            Self::Canvas => ("canvas", "Canvas/visualization component available"),
            Self::SilentReply => ("silent_reply", "Use silent/heartbeat_ok when nothing to report"),
        }
    }
}

/// Physical constraints on output
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InteractionConstraints {
    /// Maximum output length (None = unlimited)
    pub max_output_chars: Option<usize>,
    /// Whether streaming is supported
    pub supports_streaming: bool,
    /// Prefer compact output
    pub prefer_compact: bool,
}

/// Interaction Manifest - describes "what can technically be done"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionManifest {
    /// Core interaction paradigm
    pub paradigm: InteractionParadigm,
    /// Active capabilities
    pub capabilities: HashSet<Capability>,
    /// Physical constraints
    pub constraints: InteractionConstraints,
}

impl InteractionManifest {
    /// Create a new manifest with paradigm defaults
    pub fn new(paradigm: InteractionParadigm) -> Self {
        Self {
            capabilities: paradigm.default_capabilities(),
            paradigm,
            constraints: InteractionConstraints::default(),
        }
    }

    /// Create with explicit capabilities (overriding defaults)
    pub fn with_capabilities(paradigm: InteractionParadigm, capabilities: HashSet<Capability>) -> Self {
        Self {
            paradigm,
            capabilities,
            constraints: InteractionConstraints::default(),
        }
    }

    /// Add a capability
    pub fn add_capability(&mut self, cap: Capability) -> &mut Self {
        self.capabilities.insert(cap);
        self
    }

    /// Remove a capability
    pub fn remove_capability(&mut self, cap: Capability) -> &mut Self {
        self.capabilities.remove(&cap);
        self
    }

    /// Check if capability is present
    pub fn has_capability(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }

    /// Set constraints
    pub fn with_constraints(mut self, constraints: InteractionConstraints) -> Self {
        self.constraints = constraints;
        self
    }

    /// Check if a tool is supported by this interaction context
    pub fn supports_tool(&self, tool_name: &str) -> bool {
        match tool_name {
            // Canvas tool requires Canvas capability
            "canvas" => self.has_capability(Capability::Canvas),
            // Most tools are interaction-agnostic
            _ => true,
        }
    }
}

impl Default for InteractionManifest {
    fn default() -> Self {
        Self::new(InteractionParadigm::CLI)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_paradigm_defaults() {
        let manifest = InteractionManifest::new(InteractionParadigm::CLI);
        assert!(manifest.has_capability(Capability::Streaming));
        assert!(manifest.has_capability(Capability::RichText));
        assert!(!manifest.has_capability(Capability::MultiGroupUI));
        assert!(!manifest.has_capability(Capability::InlineButtons));
    }

    #[test]
    fn test_web_paradigm_defaults() {
        let manifest = InteractionManifest::new(InteractionParadigm::WebRich);
        assert!(manifest.has_capability(Capability::MultiGroupUI));
        assert!(manifest.has_capability(Capability::MermaidCharts));
        assert!(manifest.has_capability(Capability::Canvas));
    }

    #[test]
    fn test_capability_override() {
        let mut manifest = InteractionManifest::new(InteractionParadigm::Messaging);
        assert!(!manifest.has_capability(Capability::InlineButtons));

        manifest.add_capability(Capability::InlineButtons);
        assert!(manifest.has_capability(Capability::InlineButtons));
    }

    #[test]
    fn test_constraints() {
        let manifest = InteractionManifest::new(InteractionParadigm::Messaging)
            .with_constraints(InteractionConstraints {
                max_output_chars: Some(4096),
                supports_streaming: false,
                prefer_compact: true,
            });

        assert_eq!(manifest.constraints.max_output_chars, Some(4096));
        assert!(!manifest.constraints.supports_streaming);
    }
}
```

**Step 4: Add module to thinker/mod.rs**

Add after line 36 in `core/src/thinker/mod.rs`:

```rust
pub mod interaction;
```

And add to re-exports after line 54:

```rust
pub use interaction::{Capability, InteractionConstraints, InteractionManifest, InteractionParadigm};
```

**Step 5: Run unit tests**

Run: `cargo test -p alephcore interaction --lib`
Expected: PASS (4 tests)

**Step 6: Commit**

```bash
git add core/src/thinker/interaction.rs core/src/thinker/mod.rs
git commit -m "feat(thinker): add InteractionManifest for channel capability awareness"
```

---

## Task 2: Define Security Context

**Files:**
- Create: `core/src/thinker/security_context.rs`
- Modify: `core/src/thinker/mod.rs` (add module)

**Step 1: Create security context types**

Create `core/src/thinker/security_context.rs`:

```rust
//! Security Context - Policy-driven permission layer
//!
//! Orthogonal to InteractionManifest, this defines what is
//! allowed by security policy (vs what is technically possible).

use std::collections::HashSet;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

/// Sandbox isolation level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxLevel {
    /// No restrictions (trusted local user)
    #[default]
    None,
    /// Standard sandbox: limited filesystem, network
    Standard,
    /// Strict sandbox: read-only operations only
    Strict,
    /// Untrusted code: full isolation
    Untrusted,
}

impl SandboxLevel {
    /// Get description for prompt
    pub fn description(&self) -> &'static str {
        match self {
            Self::None => "No Sandbox (full access)",
            Self::Standard => "Standard Sandbox Mode",
            Self::Strict => "Strict Sandbox Mode (read-only)",
            Self::Untrusted => "Untrusted Sandbox (isolated)",
        }
    }
}

/// Tool permission result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolPermission {
    /// Tool is allowed
    Allowed,
    /// Tool is denied by policy
    Denied { reason: String },
    /// Tool requires user approval for each use
    RequiresApproval { prompt: String },
}

/// Policy for elevated/privileged execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevatedPolicy {
    /// Elevated execution disabled
    #[default]
    Off,
    /// Ask user for each elevated command
    Ask,
    /// Auto-approve commands in allowlist
    AllowList(Vec<String>),
    /// Full trust (dangerous)
    Full,
}

/// Security Context - defines "what is allowed by policy"
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityContext {
    /// Sandbox isolation level
    pub sandbox_level: SandboxLevel,
    /// Tool whitelist (None = all allowed)
    pub allowed_tools: Option<HashSet<String>>,
    /// Tool blacklist
    pub denied_tools: HashSet<String>,
    /// Filesystem access boundary
    pub filesystem_scope: Option<PathBuf>,
    /// Network access allowed
    pub network_allowed: bool,
    /// Elevated execution policy
    pub elevated_policy: ElevatedPolicy,
}

impl SecurityContext {
    /// Create a fully permissive context (for trusted local use)
    pub fn permissive() -> Self {
        Self {
            sandbox_level: SandboxLevel::None,
            allowed_tools: None,
            denied_tools: HashSet::new(),
            filesystem_scope: None,
            network_allowed: true,
            elevated_policy: ElevatedPolicy::Full,
        }
    }

    /// Create a standard sandbox context
    pub fn standard_sandbox(workspace: PathBuf) -> Self {
        Self {
            sandbox_level: SandboxLevel::Standard,
            allowed_tools: None,
            denied_tools: HashSet::new(),
            filesystem_scope: Some(workspace),
            network_allowed: true,
            elevated_policy: ElevatedPolicy::Ask,
        }
    }

    /// Create a strict read-only context
    pub fn strict_readonly(workspace: PathBuf) -> Self {
        let mut denied = HashSet::new();
        denied.insert("file_ops".to_string()); // No file modifications
        denied.insert("exec".to_string());     // No execution
        denied.insert("bash".to_string());     // No shell

        Self {
            sandbox_level: SandboxLevel::Strict,
            allowed_tools: None,
            denied_tools: denied,
            filesystem_scope: Some(workspace),
            network_allowed: false,
            elevated_policy: ElevatedPolicy::Off,
        }
    }

    /// Check tool permission
    pub fn check_tool(&self, tool_name: &str) -> ToolPermission {
        // Blacklist takes priority
        if self.denied_tools.contains(tool_name) {
            return ToolPermission::Denied {
                reason: "blocked by security policy".to_string(),
            };
        }

        // Check whitelist if set
        if let Some(ref allowed) = self.allowed_tools {
            if !allowed.contains(tool_name) {
                return ToolPermission::Denied {
                    reason: "not in allowed tools list".to_string(),
                };
            }
        }

        // Special handling for exec tools
        if tool_name == "exec" || tool_name == "bash" {
            return self.check_exec_permission();
        }

        // Network tools check
        if !self.network_allowed && is_network_tool(tool_name) {
            return ToolPermission::Denied {
                reason: "network access blocked".to_string(),
            };
        }

        ToolPermission::Allowed
    }

    /// Check exec permission based on elevated policy
    fn check_exec_permission(&self) -> ToolPermission {
        match &self.elevated_policy {
            ElevatedPolicy::Off => ToolPermission::Denied {
                reason: "execution disabled".to_string(),
            },
            ElevatedPolicy::Ask => ToolPermission::RequiresApproval {
                prompt: "Command execution requires approval".to_string(),
            },
            ElevatedPolicy::AllowList(_) => ToolPermission::RequiresApproval {
                prompt: "Command execution requires approval (allowlist active)".to_string(),
            },
            ElevatedPolicy::Full => ToolPermission::Allowed,
        }
    }

    /// Generate security notes for prompt
    pub fn security_notes(&self) -> Vec<String> {
        let mut notes = Vec::new();

        if self.sandbox_level != SandboxLevel::None {
            notes.push(format!("Running in {}", self.sandbox_level.description()));
        }

        if let Some(ref scope) = self.filesystem_scope {
            notes.push(format!("Filesystem access limited to: {}", scope.display()));
        }

        if !self.network_allowed {
            notes.push("Network access: blocked".to_string());
        }

        notes
    }
}

/// Check if a tool requires network access
fn is_network_tool(tool_name: &str) -> bool {
    matches!(tool_name, "web_search" | "web_fetch" | "http_request")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permissive_allows_all() {
        let ctx = SecurityContext::permissive();
        assert_eq!(ctx.check_tool("exec"), ToolPermission::Allowed);
        assert_eq!(ctx.check_tool("web_search"), ToolPermission::Allowed);
        assert_eq!(ctx.check_tool("file_ops"), ToolPermission::Allowed);
    }

    #[test]
    fn test_strict_denies_exec() {
        let ctx = SecurityContext::strict_readonly(PathBuf::from("/workspace"));
        assert!(matches!(ctx.check_tool("exec"), ToolPermission::Denied { .. }));
        assert!(matches!(ctx.check_tool("bash"), ToolPermission::Denied { .. }));
    }

    #[test]
    fn test_network_blocked() {
        let mut ctx = SecurityContext::permissive();
        ctx.network_allowed = false;

        assert!(matches!(ctx.check_tool("web_search"), ToolPermission::Denied { .. }));
        assert_eq!(ctx.check_tool("file_ops"), ToolPermission::Allowed);
    }

    #[test]
    fn test_standard_sandbox_requires_approval() {
        let ctx = SecurityContext::standard_sandbox(PathBuf::from("/workspace"));
        assert!(matches!(ctx.check_tool("exec"), ToolPermission::RequiresApproval { .. }));
    }

    #[test]
    fn test_blacklist_priority() {
        let mut ctx = SecurityContext::permissive();
        ctx.denied_tools.insert("dangerous_tool".to_string());

        assert!(matches!(ctx.check_tool("dangerous_tool"), ToolPermission::Denied { .. }));
    }

    #[test]
    fn test_security_notes() {
        let ctx = SecurityContext::standard_sandbox(PathBuf::from("/workspace"));
        let notes = ctx.security_notes();

        assert!(notes.iter().any(|n| n.contains("Standard Sandbox")));
        assert!(notes.iter().any(|n| n.contains("/workspace")));
    }
}
```

**Step 2: Add module to thinker/mod.rs**

Add after interaction module:

```rust
pub mod security_context;
```

And add to re-exports:

```rust
pub use security_context::{ElevatedPolicy, SandboxLevel, SecurityContext, ToolPermission};
```

**Step 3: Run tests**

Run: `cargo test -p alephcore security_context --lib`
Expected: PASS (6 tests)

**Step 4: Commit**

```bash
git add core/src/thinker/security_context.rs core/src/thinker/mod.rs
git commit -m "feat(thinker): add SecurityContext for policy-driven permissions"
```

---

## Task 3: Implement Context Aggregator

**Files:**
- Create: `core/src/thinker/context.rs`
- Modify: `core/src/thinker/mod.rs`

**Step 1: Create context aggregator**

Create `core/src/thinker/context.rs`:

```rust
//! Context Aggregator - Combines interaction and security contexts
//!
//! Performs the "reconciliation" between what is technically possible
//! (InteractionManifest) and what is allowed by policy (SecurityContext).

use super::interaction::{Capability, InteractionConstraints, InteractionManifest, InteractionParadigm};
use super::security_context::{SecurityContext, ToolPermission};
use crate::agent_loop::ToolInfo;

/// Reason why a tool was disabled
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisableReason {
    /// Channel doesn't support this tool (silent filter)
    UnsupportedByChannel,
    /// Security policy blocks this tool (transparent to AI)
    BlockedByPolicy { reason: String },
    /// Tool requires user approval for each use
    RequiresApproval { prompt: String },
}

/// A tool that was disabled with reason
#[derive(Debug, Clone)]
pub struct DisabledTool {
    pub name: String,
    pub reason: DisableReason,
}

/// Environment contract - what AI needs to know about its environment
#[derive(Debug, Clone)]
pub struct EnvironmentContract {
    pub paradigm: InteractionParadigm,
    pub active_capabilities: Vec<Capability>,
    pub constraints: InteractionConstraints,
    pub security_notes: Vec<String>,
}

/// Resolved context after aggregation
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    /// Tools available for use
    pub available_tools: Vec<ToolInfo>,
    /// Tools that were disabled (for transparency)
    pub disabled_tools: Vec<DisabledTool>,
    /// Environment contract for prompt generation
    pub environment_contract: EnvironmentContract,
}

/// Context Aggregator - reconciles interaction and security
pub struct ContextAggregator;

impl ContextAggregator {
    /// Resolve final context by combining interaction and security
    pub fn resolve(
        interaction: &InteractionManifest,
        security: &SecurityContext,
        all_tools: &[ToolInfo],
    ) -> ResolvedContext {
        let mut available = Vec::new();
        let mut disabled = Vec::new();

        for tool in all_tools {
            // Step 1: Interaction layer filter (silent)
            if !interaction.supports_tool(&tool.name) {
                disabled.push(DisabledTool {
                    name: tool.name.clone(),
                    reason: DisableReason::UnsupportedByChannel,
                });
                continue;
            }

            // Step 2: Security layer check (transparent)
            match security.check_tool(&tool.name) {
                ToolPermission::Allowed => {
                    available.push(tool.clone());
                }
                ToolPermission::Denied { reason } => {
                    disabled.push(DisabledTool {
                        name: tool.name.clone(),
                        reason: DisableReason::BlockedByPolicy { reason },
                    });
                }
                ToolPermission::RequiresApproval { prompt } => {
                    // Tool is available but needs approval
                    available.push(tool.clone());
                    disabled.push(DisabledTool {
                        name: tool.name.clone(),
                        reason: DisableReason::RequiresApproval { prompt },
                    });
                }
            }
        }

        let environment_contract = Self::build_contract(interaction, security);

        ResolvedContext {
            available_tools: available,
            disabled_tools: disabled,
            environment_contract,
        }
    }

    /// Build environment contract from contexts
    fn build_contract(
        interaction: &InteractionManifest,
        security: &SecurityContext,
    ) -> EnvironmentContract {
        let active_capabilities: Vec<Capability> =
            interaction.capabilities.iter().cloned().collect();

        EnvironmentContract {
            paradigm: interaction.paradigm,
            active_capabilities,
            constraints: interaction.constraints.clone(),
            security_notes: security.security_notes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_tool(name: &str) -> ToolInfo {
        ToolInfo {
            name: name.to_string(),
            description: format!("{} tool", name),
            parameters_schema: "{}".to_string(),
        }
    }

    #[test]
    fn test_all_tools_available_in_permissive() {
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::permissive();
        let tools = vec![
            make_tool("file_ops"),
            make_tool("web_search"),
            make_tool("exec"),
        ];

        let resolved = ContextAggregator::resolve(&interaction, &security, &tools);

        assert_eq!(resolved.available_tools.len(), 3);
        assert!(resolved.disabled_tools.iter()
            .all(|d| matches!(d.reason, DisableReason::UnsupportedByChannel)));
    }

    #[test]
    fn test_security_blocks_tool() {
        let interaction = InteractionManifest::new(InteractionParadigm::CLI);
        let security = SecurityContext::strict_readonly(PathBuf::from("/workspace"));
        let tools = vec![
            make_tool("file_ops"),
            make_tool("exec"),
            make_tool("read"),
        ];

        let resolved = ContextAggregator::resolve(&interaction, &security, &tools);

        // file_ops and exec should be blocked
        let blocked_names: Vec<_> = resolved.disabled_tools.iter()
            .filter(|d| matches!(d.reason, DisableReason::BlockedByPolicy { .. }))
            .map(|d| d.name.as_str())
            .collect();

        assert!(blocked_names.contains(&"file_ops"));
        assert!(blocked_names.contains(&"exec"));

        // read should be available
        assert!(resolved.available_tools.iter().any(|t| t.name == "read"));
    }

    #[test]
    fn test_requires_approval_shows_both() {
        let interaction = InteractionManifest::new(InteractionParadigm::CLI);
        let security = SecurityContext::standard_sandbox(PathBuf::from("/workspace"));
        let tools = vec![make_tool("exec")];

        let resolved = ContextAggregator::resolve(&interaction, &security, &tools);

        // exec should be available (but with approval marker)
        assert!(resolved.available_tools.iter().any(|t| t.name == "exec"));

        // And also in disabled with RequiresApproval
        assert!(resolved.disabled_tools.iter().any(|d|
            d.name == "exec" && matches!(d.reason, DisableReason::RequiresApproval { .. })
        ));
    }

    #[test]
    fn test_environment_contract() {
        let mut interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        interaction.constraints.max_output_chars = Some(10000);

        let security = SecurityContext::standard_sandbox(PathBuf::from("/workspace"));

        let resolved = ContextAggregator::resolve(&interaction, &security, &[]);

        assert_eq!(resolved.environment_contract.paradigm, InteractionParadigm::WebRich);
        assert!(resolved.environment_contract.active_capabilities.contains(&Capability::MultiGroupUI));
        assert_eq!(resolved.environment_contract.constraints.max_output_chars, Some(10000));
        assert!(!resolved.environment_contract.security_notes.is_empty());
    }

    #[test]
    fn test_canvas_filtered_by_interaction() {
        let interaction = InteractionManifest::new(InteractionParadigm::CLI);
        let security = SecurityContext::permissive();
        let tools = vec![make_tool("canvas"), make_tool("file_ops")];

        let resolved = ContextAggregator::resolve(&interaction, &security, &tools);

        // canvas should be filtered (CLI doesn't support it)
        assert!(!resolved.available_tools.iter().any(|t| t.name == "canvas"));
        assert!(resolved.disabled_tools.iter().any(|d|
            d.name == "canvas" && matches!(d.reason, DisableReason::UnsupportedByChannel)
        ));

        // file_ops should be available
        assert!(resolved.available_tools.iter().any(|t| t.name == "file_ops"));
    }
}
```

**Step 2: Add module to thinker/mod.rs**

Add after security_context:

```rust
pub mod context;
```

And add to re-exports:

```rust
pub use context::{ContextAggregator, DisableReason, DisabledTool, EnvironmentContract, ResolvedContext};
```

**Step 3: Run tests**

Run: `cargo test -p alephcore context --lib`
Expected: PASS (5 tests)

**Step 4: Commit**

```bash
git add core/src/thinker/context.rs core/src/thinker/mod.rs
git commit -m "feat(thinker): add ContextAggregator for environment reconciliation"
```

---

## Task 4: Enhance PromptBuilder with Environment Contract

**Files:**
- Modify: `core/src/thinker/prompt_builder.rs`

**Step 1: Add environment contract section builder**

Add new methods to `PromptBuilder` in `prompt_builder.rs` (after `append_language_setting`):

```rust
    /// Append environment contract section
    pub fn append_environment_contract(
        &self,
        prompt: &mut String,
        contract: &EnvironmentContract,
    ) {
        prompt.push_str("## Environment Contract\n\n");

        // Paradigm declaration
        prompt.push_str(&format!("**Paradigm**: {}\n\n", contract.paradigm.description()));

        // Active capabilities
        if !contract.active_capabilities.is_empty() {
            prompt.push_str("**Active Capabilities**:\n");
            for cap in &contract.active_capabilities {
                let (name, hint) = cap.prompt_hint();
                prompt.push_str(&format!("- `{}`: {}\n", name, hint));
            }
            prompt.push('\n');
        }

        // Constraints
        prompt.push_str("**Constraints**:\n");
        if let Some(max) = contract.constraints.max_output_chars {
            prompt.push_str(&format!("- Max output: {} chars\n", max));
        }
        if contract.constraints.prefer_compact {
            prompt.push_str("- Prefer concise responses\n");
        }
        if !contract.constraints.supports_streaming {
            prompt.push_str("- No streaming (batch response only)\n");
        }
        prompt.push('\n');
    }

    /// Append security constraints section
    pub fn append_security_constraints(
        &self,
        prompt: &mut String,
        disabled_tools: &[DisabledTool],
        security_notes: &[String],
    ) {
        prompt.push_str("## Security & Constraints\n\n");

        // Security notes
        for note in security_notes {
            prompt.push_str(&format!("- {}\n", note));
        }

        // Policy-blocked tools (transparent)
        let policy_blocked: Vec<_> = disabled_tools.iter()
            .filter(|t| matches!(t.reason, DisableReason::BlockedByPolicy { .. }))
            .collect();

        if !policy_blocked.is_empty() {
            prompt.push_str("\n**Disabled by Policy**:\n");
            for tool in policy_blocked {
                prompt.push_str(&format!("- `{}` — unavailable in current security context\n", tool.name));
            }
        }

        // Approval-required tools
        let requires_approval: Vec<_> = disabled_tools.iter()
            .filter(|t| matches!(t.reason, DisableReason::RequiresApproval { .. }))
            .collect();

        if !requires_approval.is_empty() {
            prompt.push_str("\n**Requires User Approval**:\n");
            for tool in requires_approval {
                prompt.push_str(&format!(
                    "- `{}` — available, but each invocation requires user confirmation\n",
                    tool.name
                ));
            }
        }

        prompt.push('\n');
    }

    /// Append silent behavior section (for Background paradigm)
    pub fn append_silent_behavior(
        &self,
        prompt: &mut String,
        contract: &EnvironmentContract,
    ) {
        use super::interaction::Capability;

        if !contract.active_capabilities.contains(&Capability::SilentReply) {
            return;
        }

        prompt.push_str("## Silent Behavior\n\n");
        prompt.push_str("In background or monitoring contexts, you may have nothing to report.\n\n");
        prompt.push_str("**When to use silent response**:\n");
        prompt.push_str("- Heartbeat poll with no pending tasks → `{\"action\": {\"type\": \"heartbeat_ok\"}}`\n");
        prompt.push_str("- Monitoring check with no anomalies → `{\"action\": {\"type\": \"silent\"}}`\n");
        prompt.push_str("- Already delivered via `message` tool → `{\"action\": {\"type\": \"silent\"}}`\n\n");
        prompt.push_str("**Never** output filler like \"Task complete, standing by\" — use silent instead.\n\n");
    }

    /// Build system prompt with resolved context (v2)
    pub fn build_system_prompt_with_context(&self, ctx: &ResolvedContext) -> String {
        let mut prompt = String::new();

        // 1. Role definition
        prompt.push_str("You are an AI assistant executing tasks step by step.\n\n");

        // 2. Core instructions
        prompt.push_str("## Your Role\n");
        prompt.push_str("- Observe the current state and history\n");
        prompt.push_str("- Decide the SINGLE next action to take\n");
        prompt.push_str("- Execute until the task is complete or you need user input\n\n");

        // 3. Environment contract (NEW)
        self.append_environment_contract(&mut prompt, &ctx.environment_contract);

        // 4. Runtime capabilities
        self.append_runtime_capabilities(&mut prompt);

        // 5. Tools (filtered)
        self.append_tools(&mut prompt, &ctx.available_tools);

        // 6. Security constraints (NEW)
        self.append_security_constraints(
            &mut prompt,
            &ctx.disabled_tools,
            &ctx.environment_contract.security_notes,
        );

        // 7. Silent behavior (NEW - if applicable)
        self.append_silent_behavior(&mut prompt, &ctx.environment_contract);

        // 8. Generation models
        self.append_generation_models(&mut prompt);

        // 9. Special actions
        self.append_special_actions(&mut prompt);

        // 10. Response format
        self.append_response_format(&mut prompt);

        // 11. Guidelines
        self.append_guidelines(&mut prompt);

        // 12. Skill mode
        self.append_skill_mode(&mut prompt);

        // 13. Custom instructions
        self.append_custom_instructions(&mut prompt);

        // 14. Language setting
        self.append_language_setting(&mut prompt);

        prompt
    }
```

**Step 2: Add imports at top of prompt_builder.rs**

```rust
use super::context::{DisableReason, DisabledTool, EnvironmentContract, ResolvedContext};
```

**Step 3: Run existing tests**

Run: `cargo test -p alephcore prompt_builder --lib`
Expected: PASS (existing tests should still work)

**Step 4: Commit**

```bash
git add core/src/thinker/prompt_builder.rs
git commit -m "feat(thinker): add environment contract and security sections to PromptBuilder"
```

---

## Task 5: Add Silent/HeartbeatOk Decision Types

**Files:**
- Modify: `core/src/agent_loop/decision.rs`

**Step 1: Add new decision variants**

In `decision.rs`, add to `Decision` enum (after `Fail`):

```rust
    /// Silent response - nothing to report
    Silent,
    /// Heartbeat acknowledgment - background task alive
    HeartbeatOk,
```

Add to `LlmAction` enum:

```rust
    /// Silent - no output needed
    Silent,
    /// Heartbeat OK - background task alive
    HeartbeatOk,
```

**Step 2: Update is_terminal and decision_type**

In `Decision::is_terminal()`:

```rust
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Decision::Complete { .. }
            | Decision::Fail { .. }
            | Decision::Silent
            | Decision::HeartbeatOk
        )
    }
```

In `Decision::decision_type()`:

```rust
    pub fn decision_type(&self) -> &'static str {
        match self {
            Decision::UseTool { .. } => "tool",
            Decision::AskUser { .. } => "ask_user",
            Decision::AskUserMultigroup { .. } => "ask_user_multigroup",
            Decision::AskUserRich { .. } => "ask_user_rich",
            Decision::Complete { .. } => "complete",
            Decision::Fail { .. } => "fail",
            Decision::Silent => "silent",
            Decision::HeartbeatOk => "heartbeat_ok",
        }
    }
```

**Step 3: Update From implementations**

In `From<LlmAction> for Decision`:

```rust
    LlmAction::Silent => Decision::Silent,
    LlmAction::HeartbeatOk => Decision::HeartbeatOk,
```

**Step 4: Run tests**

Run: `cargo test -p alephcore decision --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/agent_loop/decision.rs
git commit -m "feat(agent_loop): add Silent and HeartbeatOk decision types"
```

---

## Task 6: Implement ChannelProvider Trait

**Files:**
- Modify: `core/src/gateway/channel.rs`
- Modify: `core/src/gateway/channels/cli.rs`

**Step 1: Add ChannelProvider trait to channel.rs**

Add after `Channel` trait definition:

```rust
use crate::thinker::interaction::InteractionManifest;

/// Provider of interaction manifest for a channel
///
/// Channels implement this to declare their interaction capabilities.
/// The manifest is used by ContextAggregator to filter tools and
/// generate appropriate system prompts.
pub trait ChannelProvider {
    /// Get the interaction manifest for this channel
    fn interaction_manifest(&self) -> InteractionManifest;

    /// Optional runtime capability detection
    fn detect_capabilities(&self) -> Option<std::collections::HashSet<crate::thinker::interaction::Capability>> {
        None
    }
}
```

**Step 2: Implement for CliChannel**

In `cli.rs`, add implementation:

```rust
use crate::thinker::interaction::{Capability, InteractionConstraints, InteractionManifest, InteractionParadigm};
use crate::gateway::channel::ChannelProvider;

impl ChannelProvider for CliChannel {
    fn interaction_manifest(&self) -> InteractionManifest {
        InteractionManifest::new(InteractionParadigm::CLI)
            .with_constraints(InteractionConstraints {
                max_output_chars: None,
                supports_streaming: true,
                prefer_compact: false,
            })
    }
}
```

**Step 3: Run tests**

Run: `cargo test -p alephcore cli --lib`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/gateway/channel.rs core/src/gateway/channels/cli.rs
git commit -m "feat(gateway): add ChannelProvider trait for interaction manifests"
```

---

## Task 7: Integration Test

**Files:**
- Create: `core/tests/features/thinker/context_aggregation.feature`
- Modify: `core/tests/steps/thinker_steps.rs`
- Modify: `core/tests/world/thinker_ctx.rs`

**Step 1: Create BDD feature**

Create `core/tests/features/thinker/context_aggregation.feature`:

```gherkin
Feature: Context Aggregation
  As an AI system
  I need to combine interaction and security contexts
  So I can generate appropriate system prompts

  Scenario: Web environment with standard security
    Given a web rich interaction manifest
    And a standard sandbox security context
    And tools "file_ops,exec,web_search,canvas"
    When I aggregate the context
    Then the environment contract paradigm should be "WebRich"
    And "canvas" should be available
    And "exec" should require approval

  Scenario: CLI environment with strict security
    Given a CLI interaction manifest
    And a strict readonly security context
    And tools "file_ops,exec,read"
    When I aggregate the context
    Then "file_ops" should be blocked by policy
    And "exec" should be blocked by policy
    And "read" should be available

  Scenario: Generated prompt includes environment contract
    Given a messaging interaction manifest with inline buttons
    And a permissive security context
    And tools "message,file_ops"
    When I build the system prompt with context
    Then the prompt should contain "Environment Contract"
    And the prompt should contain "Messaging Channel"
    And the prompt should contain "inline_buttons"
```

**Step 2: Implement step definitions**

Add to `thinker_steps.rs`:

```rust
use alephcore::thinker::{
    ContextAggregator, InteractionManifest, InteractionParadigm,
    SecurityContext, Capability, DisableReason,
};

#[given("a web rich interaction manifest")]
async fn given_web_manifest(world: &mut ThinkerWorld) {
    world.interaction = Some(InteractionManifest::new(InteractionParadigm::WebRich));
}

#[given("a CLI interaction manifest")]
async fn given_cli_manifest(world: &mut ThinkerWorld) {
    world.interaction = Some(InteractionManifest::new(InteractionParadigm::CLI));
}

#[given("a messaging interaction manifest with inline buttons")]
async fn given_messaging_with_buttons(world: &mut ThinkerWorld) {
    let mut manifest = InteractionManifest::new(InteractionParadigm::Messaging);
    manifest.add_capability(Capability::InlineButtons);
    world.interaction = Some(manifest);
}

#[given("a standard sandbox security context")]
async fn given_standard_security(world: &mut ThinkerWorld) {
    world.security = Some(SecurityContext::standard_sandbox(
        std::path::PathBuf::from("/workspace")
    ));
}

#[given("a strict readonly security context")]
async fn given_strict_security(world: &mut ThinkerWorld) {
    world.security = Some(SecurityContext::strict_readonly(
        std::path::PathBuf::from("/workspace")
    ));
}

#[given("a permissive security context")]
async fn given_permissive_security(world: &mut ThinkerWorld) {
    world.security = Some(SecurityContext::permissive());
}

#[when("I aggregate the context")]
async fn when_aggregate(world: &mut ThinkerWorld) {
    let interaction = world.interaction.as_ref().unwrap();
    let security = world.security.as_ref().unwrap();
    let tools = &world.tools;

    world.resolved = Some(ContextAggregator::resolve(interaction, security, tools));
}

#[then(expr = "the environment contract paradigm should be {string}")]
async fn then_paradigm(world: &mut ThinkerWorld, expected: String) {
    let resolved = world.resolved.as_ref().unwrap();
    let paradigm = format!("{:?}", resolved.environment_contract.paradigm);
    assert!(paradigm.contains(&expected));
}

#[then(expr = "{string} should be available")]
async fn then_tool_available(world: &mut ThinkerWorld, tool_name: String) {
    let resolved = world.resolved.as_ref().unwrap();
    assert!(resolved.available_tools.iter().any(|t| t.name == tool_name));
}

#[then(expr = "{string} should require approval")]
async fn then_requires_approval(world: &mut ThinkerWorld, tool_name: String) {
    let resolved = world.resolved.as_ref().unwrap();
    assert!(resolved.disabled_tools.iter().any(|d|
        d.name == tool_name && matches!(d.reason, DisableReason::RequiresApproval { .. })
    ));
}

#[then(expr = "{string} should be blocked by policy")]
async fn then_blocked(world: &mut ThinkerWorld, tool_name: String) {
    let resolved = world.resolved.as_ref().unwrap();
    assert!(resolved.disabled_tools.iter().any(|d|
        d.name == tool_name && matches!(d.reason, DisableReason::BlockedByPolicy { .. })
    ));
}
```

**Step 3: Update ThinkerWorld**

Add fields to `ThinkerWorld` in `thinker_ctx.rs`:

```rust
pub struct ThinkerWorld {
    // ... existing fields ...
    pub interaction: Option<InteractionManifest>,
    pub security: Option<SecurityContext>,
    pub resolved: Option<ResolvedContext>,
    pub tools: Vec<ToolInfo>,
}
```

**Step 4: Run integration tests**

Run: `cargo test --test cucumber -- features/thinker/context_aggregation.feature`
Expected: PASS

**Step 5: Commit**

```bash
git add core/tests/features/thinker/context_aggregation.feature \
        core/tests/steps/thinker_steps.rs \
        core/tests/world/thinker_ctx.rs
git commit -m "test(thinker): add BDD tests for context aggregation"
```

---

## Task 8: Documentation Update

**Files:**
- Modify: `docs/AGENT_SYSTEM.md`

**Step 1: Add Channel Capability Awareness section**

Add new section to AGENT_SYSTEM.md:

```markdown
## Channel Capability Awareness

Aleph's Thinker uses a two-layer context system to adapt AI behavior:

### InteractionManifest

Describes what the channel can technically do:

```rust
InteractionManifest {
    paradigm: InteractionParadigm::WebRich,
    capabilities: [MultiGroupUI, Streaming, MermaidCharts],
    constraints: { max_output_chars: None, supports_streaming: true }
}
```

**Paradigms**: CLI, WebRich, Messaging, Background, Embedded

### SecurityContext

Orthogonal layer defining what policy allows:

```rust
SecurityContext {
    sandbox_level: SandboxLevel::Standard,
    filesystem_scope: Some("/workspace"),
    elevated_policy: ElevatedPolicy::Ask,
}
```

### ContextAggregator

Reconciles the two layers:

1. **Interaction filter** (silent) - removes tools unsupported by channel
2. **Security filter** (transparent) - blocks/marks tools per policy

The result feeds into PromptBuilder, generating an "Environment Contract" section that tells the AI exactly what it can and cannot do.
```

**Step 2: Commit**

```bash
git add docs/AGENT_SYSTEM.md
git commit -m "docs: add Channel Capability Awareness section"
```

---

## Summary

| Task | Description | Files Changed |
|------|-------------|---------------|
| 1 | InteractionManifest types | interaction.rs, mod.rs |
| 2 | SecurityContext types | security_context.rs, mod.rs |
| 3 | ContextAggregator | context.rs, mod.rs |
| 4 | PromptBuilder enhancements | prompt_builder.rs |
| 5 | Silent/HeartbeatOk decisions | decision.rs |
| 6 | ChannelProvider trait | channel.rs, cli.rs |
| 7 | Integration tests | BDD features, steps |
| 8 | Documentation | AGENT_SYSTEM.md |

**Total commits**: 8
**Estimated test coverage**: Unit tests in each module + BDD integration tests

---

Plan complete and saved to `docs/plans/2026-02-05-channel-capability-awareness-impl.md`. Two execution options:

**1. Subagent-Driven (this session)** - I dispatch fresh subagent per task, review between tasks, fast iteration

**2. Parallel Session (separate)** - Open new session with executing-plans, batch execution with checkpoints

Which approach?
