# Three-Layer Control Architecture

## Overview

The Three-Layer Control architecture provides a balanced approach to AI agent execution:

- **Top Layer (Orchestrator)**: FSM-based state machine with hard constraints
- **Middle Layer (Skill DAG)**: Stable, testable workflow pipelines
- **Bottom Layer (Tools)**: Capability-based access with sandbox

## Enabling

Add to your `config.toml`:

```toml
[orchestrator]
use_three_layer_control = true

[orchestrator.guards]
max_rounds = 12
max_tool_calls = 30
max_tokens = 100000
timeout_seconds = 600
no_progress_threshold = 2
```

## Architecture

### Top Layer: Orchestrator

The orchestrator is a finite state machine (FSM) with 6 states:

1. **Clarify**: Gather and validate requirements
2. **Plan**: Select appropriate skills to execute
3. **Execute**: Run the skill DAG
4. **Evaluate**: Check if success criteria are met
5. **Reflect**: Analyze failures and adjust plan
6. **Stop**: Return final result

#### Guard Rails

Hard constraints prevent runaway execution:

| Guard | Default | Description |
|-------|---------|-------------|
| `max_rounds` | 12 | Maximum orchestrator iterations |
| `max_tool_calls` | 30 | Maximum total tool invocations |
| `max_tokens` | 100,000 | Maximum token budget |
| `timeout_seconds` | 600 | Maximum wall-clock time (10 min) |
| `no_progress_threshold` | 2 | Stop after N rounds without progress |

### Middle Layer: Skill DAG

Skills are stable, testable workflow definitions that wrap tool operations.

#### Skill Definition

```rust
SkillDefinition {
    id: "research",
    name: "Research Skill",
    description: "Search and summarize information",
    required_capabilities: [WebSearch, LlmCall],
    nodes: [...],  // DAG nodes
    edges: [...],  // DAG edges
}
```

#### Node Types

- **Tool**: Invoke a specific tool
- **Skill**: Invoke another skill (composition)
- **LlmProcess**: LLM processing step
- **Condition**: Conditional branching
- **Parallel**: Fan-out to multiple branches
- **Aggregate**: Fan-in with merge strategy

### Bottom Layer: Safety

#### Capability System

Skills must declare required capabilities:

| Capability | Level | Description |
|------------|-------|-------------|
| `FileRead` | Safe | Read file contents |
| `FileList` | Safe | List directory contents |
| `FileWrite` | Confirmation | Write file contents |
| `FileDelete` | Blocked | Delete files |
| `WebSearch` | Safe | Web search |
| `WebFetch` | Safe | Fetch web content |
| `LlmCall` | Safe | Call LLM API |
| `ShellExec` | Blocked | Execute shell commands |
| `ProcessSpawn` | Blocked | Spawn processes |
| `Mcp { server }` | Safe | MCP server access |

#### Path Sandbox

File operations are restricted to allowed directories:

```rust
let sandbox = PathSandbox::with_defaults(vec![
    PathBuf::from("/workspace/project"),
]);
```

**Default Denied Patterns:**
- `.git/` directories
- `.env` files
- `credentials` files
- `.ssh/` directories
- Private key files (`.pem`, `.key`, `id_rsa`, `id_ed25519`)

#### Resource Quota

Limits on resource consumption:

| Resource | Default | Description |
|----------|---------|-------------|
| `max_file_size` | 10 MB | Maximum single file size |
| `max_total_read` | 100 MB | Maximum total bytes read |
| `max_total_write` | 50 MB | Maximum total bytes written |
| `max_file_count` | 1000 | Maximum files accessed |

## Migration

The Three-Layer Control is enabled via configuration. The legacy `RequestOrchestrator` remains available but is deprecated.

To migrate:

1. Set `orchestrator.use_three_layer_control = true`
2. Configure guard values as needed
3. Test with development workloads
4. Roll out to production

## API Reference

### Core Types

```rust
use aethecore::three_layer::{
    // Safety
    Capability, CapabilityLevel, CapabilityGate, CapabilityDenied,
    PathSandbox, SandboxViolation,
    ResourceQuota, QuotaTracker, QuotaExceeded,

    // Orchestrator
    OrchestratorState, GuardChecker, GuardViolation,

    // Skills
    SkillDefinition, SkillNode, SkillNodeType, SkillRegistry,
};
```

## Design Document

See `docs/plans/2026-01-21-three-layer-control-design.md` for detailed design decisions and architecture diagrams.
