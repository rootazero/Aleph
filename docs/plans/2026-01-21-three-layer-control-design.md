# Three-Layer Control Architecture Design

> Date: 2026-01-21
> Status: Design Complete, Ready for Implementation
> Supersedes: 2026-01-21-agent-loop-design.md (single pipeline approach)

## Overview

This document describes the Three-Layer Control architecture for Aether, addressing the critical risks of the single-pipeline Agent Loop approach:

- **Cost Risk**: Unlimited loop could cost $100+ overnight
- **Rabbit Hole Risk**: Agent may deviate from main task (e.g., spending 30 minutes on image recognition instead of buying tickets)

The Three-Layer Control provides a balanced approach: flexible top-level orchestration, stable middle-layer skill execution, and secure bottom-layer tool access.

## Design Decisions

| Item | Decision |
|------|----------|
| Architecture | Three-Layer Control (Orchestrator / Skill-DAG / Tools) |
| Top Layer | FSM-based Orchestrator with hard constraints |
| Middle Layer | Skill DAG - stable, testable, reusable workflows |
| Bottom Layer | Capability-based tools with sandbox + audit |
| Simple Requests | IntentRouter fast path (L0-L2) |
| Complex Requests | Full FSM state machine |
| Skill Definition | Hybrid: Rust (builtin) + YAML (custom) |
| Migration | Config switch, default false, deprecated old path |

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     User Request                                 │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                    IntentRouter (L0-L2)                          │
│         Simple → Fast Action       Complex → Orchestrator        │
└─────────────────────────────────────────────────────────────────┘
                                │
        ┌───────────────────────┴───────────────────────┐
        ▼                                               ▼
┌───────────────┐                       ┌─────────────────────────┐
│  Fast Action  │                       │  Top: Orchestrator      │
│  Direct Return│                       │  (FSM State Machine)    │
└───────────────┘                       │  Clarify → Plan →       │
                                        │  Execute → Evaluate →   │
                                        │  Reflect → Stop         │
                                        └───────────┬─────────────┘
                                                    │
                                                    ▼
                                        ┌─────────────────────────┐
                                        │  Middle: Skill DAG      │
                                        │  (Stable & Testable)    │
                                        │  Research, Analyze,     │
                                        │  Verify...              │
                                        └───────────┬─────────────┘
                                                    │
                                                    ▼
                                        ┌─────────────────────────┐
                                        │  Bottom: Tools          │
                                        │  (Capability-based)     │
                                        │  MCP + Rust atomic ops  │
                                        │  + Safety Guards        │
                                        └─────────────────────────┘
```

**Core Principles**:
- **Top Layer Flexible**: Orchestrator handles high-level decisions (which direction to go next)
- **Middle Layer Stable**: Skill DAG provides testable, reusable standardized workflows
- **Bottom Layer Secure**: Tools layer has strict capability limits and sandbox

---

## Top Layer: Orchestrator (FSM State Machine)

### State Definition

```rust
pub enum OrchestratorState {
    /// Clarify problem definition, constraints, evaluation criteria
    Clarify,
    /// Produce executable plan (which Skills to invoke)
    Plan,
    /// Invoke Skill DAG for execution
    Execute,
    /// Check if goals are met (evidence, test results, etc.)
    Evaluate,
    /// On failure, identify cause, adjust plan or gather more info
    Reflect,
    /// Exit when stop conditions are met
    Stop,
}
```

### State Transition Diagram

```
                    ┌─────────────────────────────────┐
                    │           Clarify               │
                    │  - Parse user intent            │
                    │  - Identify missing info        │
                    │  - Ask user if needed           │
                    └───────────────┬─────────────────┘
                                    │ Info sufficient
                                    ▼
                    ┌─────────────────────────────────┐
                    │            Plan                 │
                    │  - Select Skills to invoke      │
                    │  - Determine execution order    │
                    │  - Estimate resource usage      │
                    └───────────────┬─────────────────┘
                                    │
                                    ▼
                    ┌─────────────────────────────────┐
        ┌──────────│          Execute                 │
        │          │  - Invoke Skill DAG             │
        │          │  - Collect execution results    │
        │          └───────────────┬─────────────────┘
        │                          │
        │                          ▼
        │          ┌─────────────────────────────────┐
        │          │          Evaluate                │
        │  Retry   │  - Check success criteria       │
        │          │  - Validate output quality      │
        │          │  - Compare with expected        │
        │          └───────────────┬─────────────────┘
        │                          │
        │               ┌──────────┴──────────┐
        │               ▼                     ▼
        │    ┌─────────────────┐    ┌─────────────────┐
        │    │    Reflect      │    │      Stop       │
        │    │  - Analyze why  │    │  - Return result│
        │    │  - Adjust plan  │    │  - Cleanup      │
        │    └────────┬────────┘    └─────────────────┘
        │             │ Recoverable
        └─────────────┘
```

### Hard Constraints (Guards)

```rust
pub struct OrchestratorGuards {
    pub max_rounds: u32,           // Default: 12
    pub max_tool_calls: u32,       // Default: 30
    pub max_tokens: u64,           // Default: 100k
    pub timeout: Duration,         // Default: 10 minutes
    pub no_progress_threshold: u32, // Default: 2 rounds
}

pub enum GuardViolation {
    MaxRoundsExceeded,
    MaxToolCallsExceeded,
    TokenBudgetExhausted,
    Timeout,
    NoProgress { rounds_without_progress: u32 },
}
```

**No Progress Detection**: Each round records "new evidence/metric changes". If no changes for 2 consecutive rounds, triggers `NoProgress` - can choose to degrade or stop.

**UI Configurable**: All guard parameters are configurable via Swift UI settings.

---

## Middle Layer: Skill DAG

### Skill Definition Structure

```rust
/// Core Skills implemented in Rust
pub struct SkillDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Input/output schema (JSON Schema)
    pub input_schema: Value,
    pub output_schema: Value,
    /// Required capabilities
    pub required_capabilities: Vec<Capability>,
    /// Cost estimate
    pub cost_estimate: CostEstimate,
    /// Retry policy on failure
    pub retry_policy: RetryPolicy,
    /// DAG node definitions
    pub nodes: Vec<SkillNode>,
    pub edges: Vec<(String, String)>,  // (from, to)
}

pub struct SkillNode {
    pub id: String,
    pub node_type: SkillNodeType,
}

pub enum SkillNodeType {
    /// Invoke bottom-layer MCP tool
    Tool { tool_id: String, args_template: Value },
    /// Invoke another Skill (nested)
    Skill { skill_id: String },
    /// LLM processing node
    LlmProcess { prompt_template: String },
    /// Conditional branch
    Condition { expression: String },
    /// Parallel fan-out
    Parallel { branches: Vec<String> },
    /// Aggregate fan-in
    Aggregate { strategy: AggregateStrategy },
}
```

### Builtin Skill Example: Research and Collect

```
┌─────────────────────────────────────────────────────────────────┐
│                 Skill: research_and_collect                      │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   ┌──────────┐     ┌──────────────┐     ┌──────────────┐        │
│   │  query   │────▶│   parallel   │────▶│   dedupe     │        │
│   │ generate │     │   search     │     │   & merge    │        │
│   └──────────┘     └──────────────┘     └──────┬───────┘        │
│        │                 │                      │                │
│   LlmProcess        ┌────┴────┐                 ▼                │
│                     ▼         ▼          ┌──────────────┐        │
│                  [web]     [mcp:        │   score &    │        │
│                  search    arxiv]        │   rank       │        │
│                                          └──────┬───────┘        │
│                                                 │                │
│                                                 ▼                │
│                                          ┌──────────────┐        │
│                                          │   summarize  │        │
│                                          └──────────────┘        │
│                                                                  │
│   Capabilities: [web_search, mcp:arxiv, llm_call]               │
│   Cost Estimate: ~5k tokens, 3-5 tool calls                     │
└─────────────────────────────────────────────────────────────────┘
```

### YAML Definition Format (User Custom Skills)

```yaml
# ~/.config/aether/skills/my_research.yaml
id: my_research
name: "My Research Skill"
description: "Custom research workflow"

input_schema:
  type: object
  properties:
    query: { type: string }
  required: [query]

output_schema:
  type: object
  properties:
    summary: { type: string }
    sources: { type: array }

capabilities:
  - web_search
  - file:read

cost_estimate:
  max_tokens: 10000
  max_tool_calls: 10

nodes:
  - id: search
    type: tool
    tool_id: web_search
    args:
      query: "{{ input.query }}"

  - id: summarize
    type: llm
    prompt: |
      Summarize the following search results:
      {{ nodes.search.output }}

edges:
  - [search, summarize]
```

### Skill Registry

```rust
pub struct SkillRegistry {
    /// Builtin Rust Skills
    builtin: HashMap<String, Arc<dyn SkillExecutor>>,
    /// User YAML Skills
    custom: HashMap<String, SkillDefinition>,
}

impl SkillRegistry {
    pub fn get(&self, id: &str) -> Option<&SkillDefinition>;
    pub fn list_by_capability(&self, cap: &Capability) -> Vec<&SkillDefinition>;
    pub fn reload_custom(&mut self) -> Result<()>;  // Hot reload YAML
}
```

---

## Bottom Layer: Tools + Safety Guards

### Capability System

```rust
/// Capability definition (principle of least privilege)
#[derive(Clone, Hash, Eq, PartialEq)]
pub enum Capability {
    // ===== File System =====
    FileRead,              // Read files
    FileList,              // List directories
    FileWrite,             // Write files (requires explicit grant)
    FileDelete,            // Delete files (dangerous)

    // ===== Network =====
    WebSearch,             // Web search
    WebFetch,              // Fetch URL content

    // ===== MCP =====
    Mcp { server: String }, // Specific MCP server

    // ===== LLM =====
    LlmCall,               // Call LLM

    // ===== System =====
    ShellExec,             // Execute shell (dangerous)
    ProcessSpawn,          // Spawn process (dangerous)
}

/// Capability level
pub enum CapabilityLevel {
    Safe,           // No confirmation needed
    Confirmation,   // Requires user confirmation
    Blocked,        // Blocked by default
}
```

### Path Sandbox (P0)

```rust
pub struct PathSandbox {
    /// Allowed root directories
    allowed_roots: Vec<PathBuf>,
    /// Explicitly denied path patterns
    denied_patterns: Vec<Regex>,
}

impl PathSandbox {
    pub fn validate(&self, path: &Path) -> Result<PathBuf, SandboxViolation> {
        // 1. Canonicalize to resolve symlinks
        let canonical = path.canonicalize()?;

        // 2. Check if within allowed roots
        let in_allowed = self.allowed_roots.iter()
            .any(|root| canonical.starts_with(root));
        if !in_allowed {
            return Err(SandboxViolation::OutsideAllowedRoots);
        }

        // 3. Check denied patterns (e.g., .git, .env, credentials)
        for pattern in &self.denied_patterns {
            if pattern.is_match(canonical.to_str().unwrap_or("")) {
                return Err(SandboxViolation::DeniedPattern);
            }
        }

        Ok(canonical)
    }
}

pub enum SandboxViolation {
    OutsideAllowedRoots,
    DeniedPattern,
    SymlinkEscape,
    PathTraversal,  // Detected ..
}
```

### Operation Whitelist (P0)

```rust
pub struct CapabilityGate {
    /// Capabilities granted to Skill
    granted: HashSet<Capability>,
}

impl CapabilityGate {
    pub fn check(&self, required: &Capability) -> Result<(), CapabilityDenied> {
        if self.granted.contains(required) {
            Ok(())
        } else {
            Err(CapabilityDenied {
                required: required.clone(),
                granted: self.granted.clone(),
            })
        }
    }
}

/// Tool Router - unified interception
pub struct ToolRouter {
    sandbox: PathSandbox,
    registry: ToolRegistry,
}

impl ToolRouter {
    pub async fn execute(
        &self,
        tool_id: &str,
        args: Value,
        gate: &CapabilityGate,
    ) -> Result<ToolOutput, ToolError> {
        let tool = self.registry.get(tool_id)?;

        // 1. Capability check
        gate.check(&tool.required_capability)?;

        // 2. Path sandbox check (if file-related)
        if let Some(path) = args.get("path") {
            self.sandbox.validate(Path::new(path.as_str()?))?;
        }

        // 3. Argument validation (JSON Schema)
        tool.validate_args(&args)?;

        // 4. Execute
        tool.execute(args).await
    }
}
```

### Resource Quota (P1)

```rust
pub struct ResourceQuota {
    pub max_file_size: u64,           // Single file max 10MB
    pub max_total_read: u64,          // Total read 100MB
    pub max_total_write: u64,         // Total write 50MB
    pub max_file_count: u32,          // Max file count 1000
    pub operation_timeout: Duration,   // Single operation timeout 30s
}

pub struct QuotaTracker {
    quota: ResourceQuota,
    used_read: AtomicU64,
    used_write: AtomicU64,
    file_count: AtomicU32,
}

impl QuotaTracker {
    pub fn check_read(&self, size: u64) -> Result<(), QuotaExceeded>;
    pub fn check_write(&self, size: u64) -> Result<(), QuotaExceeded>;
    pub fn record_read(&self, size: u64);
    pub fn record_write(&self, size: u64);
}
```

### Dangerous Operation Confirmation (P1)

```rust
pub struct ConfirmationPolicy {
    /// Operation types requiring confirmation
    requires_confirmation: HashSet<Capability>,
}

impl ConfirmationPolicy {
    pub fn default() -> Self {
        Self {
            requires_confirmation: hashset![
                Capability::FileWrite,
                Capability::FileDelete,
                Capability::ShellExec,
                Capability::ProcessSpawn,
            ],
        }
    }

    pub async fn request_confirmation(
        &self,
        operation: &str,
        details: &str,
        callback: &dyn LoopCallback,
    ) -> bool {
        callback.on_confirmation_required(operation, details).await
    }
}
```

### Audit Log (P2)

```rust
pub struct AuditLog {
    /// Record of each tool invocation
    pub entries: Vec<AuditEntry>,
}

pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub skill_id: String,
    pub tool_id: String,
    pub arguments: Value,           // Input arguments
    pub affected_files: Vec<PathBuf>, // Affected files
    pub diff: Option<String>,       // File change diff
    pub duration_ms: u64,
    pub result: AuditResult,
}

pub enum AuditResult {
    Success { output_size: usize },
    Denied { reason: String },
    Failed { error: String },
}
```

### Safety Priority

| Priority | Capability | Description |
|----------|------------|-------------|
| **P0** | Path Sandbox + Capability Gate | Must have - prevents destructive operations |
| **P1** | Resource Quota + Confirmation | Should have - cost control |
| **P2** | Audit Log + Replay | Nice to have - observability |

---

## Migration Strategy

### Configuration Switch

```toml
# config.toml
[orchestrator]
# Use new three-layer control architecture
use_three_layer_control = false  # Default false, change to true when stable

[orchestrator.guards]
max_rounds = 12
max_tool_calls = 30
max_tokens = 100000
timeout_seconds = 600
no_progress_threshold = 2
```

### Code Transition

```rust
// core/src/orchestrator/mod.rs

#[deprecated(
    since = "0.10.0",
    note = "Use ThreeLayerOrchestrator instead. Enable via config: use_three_layer_control = true"
)]
pub struct RequestOrchestrator { /* existing implementation */ }

// core/src/three_layer/mod.rs (new module)
pub struct ThreeLayerOrchestrator { /* new implementation */ }
```

### Unified FFI Entry

```rust
// core/src/ffi/processing.rs

impl AetherCore {
    pub async fn process(&self, input: String, options: ProcessOptions) -> ProcessResult {
        if self.config.orchestrator.use_three_layer_control {
            // New path
            self.three_layer_orchestrator.process(input, options).await
        } else {
            // Old path (deprecated)
            self.request_orchestrator.process(input, options).await
        }
    }
}
```

### Implementation Phases

```
┌─────────────────────────────────────────────────────────────────┐
│  Phase 1: Infrastructure                                         │
├─────────────────────────────────────────────────────────────────┤
│  □ Create three_layer/ module structure                          │
│  □ Implement Capability system + PathSandbox (P0)                │
│  □ Implement ToolRouter + CapabilityGate                         │
│  □ Mark existing orchestrator as deprecated                      │
│  □ Add configuration switch                                      │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│  Phase 2: Core Components                                        │
├─────────────────────────────────────────────────────────────────┤
│  □ Implement Orchestrator FSM (state machine)                    │
│  □ Implement OrchestratorGuards (hard constraints)               │
│  □ Implement SkillDefinition + SkillRegistry                     │
│  □ Implement SkillExecutor (DAG executor)                        │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│  Phase 3: Builtin Skills                                         │
├─────────────────────────────────────────────────────────────────┤
│  □ Implement research_and_collect skill                          │
│  □ Implement code_verification skill                             │
│  □ Implement file_analysis skill                                 │
│  □ YAML parser + hot reload                                      │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│  Phase 4: P1 Safety + UI                                         │
├─────────────────────────────────────────────────────────────────┤
│  □ Implement ResourceQuota + QuotaTracker                        │
│  □ Implement ConfirmationPolicy                                  │
│  □ Swift UI settings (Guards configurable)                       │
│  □ FFI event updates                                             │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│  Phase 5: Testing + Switchover                                   │
├─────────────────────────────────────────────────────────────────┤
│  □ Unit tests + integration tests                                │
│  □ A/B testing (compare both paths)                              │
│  □ Change default to use_three_layer_control = true              │
│  □ Documentation updates                                         │
└─────────────────────────────────────────────────────────────────┘
```

### New Module Structure

```
core/src/
├── three_layer/               # [NEW] Three-layer control
│   ├── mod.rs                 # ThreeLayerOrchestrator
│   ├── orchestrator/          # Top layer FSM
│   │   ├── mod.rs
│   │   ├── states.rs          # OrchestratorState
│   │   ├── guards.rs          # OrchestratorGuards
│   │   └── transitions.rs     # State transition logic
│   ├── skill/                 # Middle layer Skill DAG
│   │   ├── mod.rs
│   │   ├── definition.rs      # SkillDefinition
│   │   ├── registry.rs        # SkillRegistry
│   │   ├── executor.rs        # DAG executor
│   │   ├── builtin/           # Builtin Skills
│   │   │   ├── research.rs
│   │   │   ├── code_verify.rs
│   │   │   └── file_analysis.rs
│   │   └── yaml_parser.rs     # YAML Skill parsing
│   └── safety/                # Bottom layer safety
│       ├── mod.rs
│       ├── capability.rs      # Capability definitions
│       ├── sandbox.rs         # PathSandbox
│       ├── gate.rs            # CapabilityGate
│       ├── quota.rs           # ResourceQuota
│       ├── router.rs          # ToolRouter
│       └── audit.rs           # AuditLog (P2)
│
├── orchestrator/              # [KEEP deprecated]
│   └── ...
```

---

## Relationship with Existing Modules

| Existing Module | Relationship | Notes |
|-----------------|--------------|-------|
| `orchestrator/` | Deprecated, keep both paths | Config switch to select |
| `agent_loop/` | Reuse guards/callback concepts | May be integrated or replaced |
| `dispatcher/scheduler/dag.rs` | Reuse DAG scheduler | Used by Skill executor |
| `intent/` | Keep IntentRouter (L0-L2) | Fast path unchanged |
| `tools/` | Bottom layer uses ToolRegistry | Add CapabilityGate wrapper |
| `mcp/` | Bottom layer MCP tools | Wrapped by ToolRouter |

---

## Appendix: Design Rationale

### Why Three Layers?

| Single Pipeline Issues | Three-Layer Solutions |
|------------------------|----------------------|
| LLM decides everything, unpredictable cost | Guard constraints + Skill cost estimates |
| May go down rabbit holes | Skill DAG keeps execution on track |
| Hard to test end-to-end | Each Skill node can be unit tested |
| Unsafe file/system operations | Capability-based + Sandbox |

### When to Use DAG vs Loop

| Scenario | Recommended Approach |
|----------|---------------------|
| High frequency, standardizable, stability-sensitive | DAG Skill (research, code test) |
| Low frequency, high uncertainty, exploratory | Let Orchestrator decide |
| Any file write/execution | DAG Skill + strong constraints |

### Key Insight

> A "research-type agent" requires a constrained agent loop at the top (for adaptability).
> But for controllability, testability, and maintainability, core capabilities should sink into DAG-based skills.
> Meanwhile, since file system capabilities exist, the tool layer must be capability-based + sandboxed + audited.
