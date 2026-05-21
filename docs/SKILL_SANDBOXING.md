# Skill Sandboxing System

> OS-native sandboxing for evolved skills with three-layer security model

---

## Overview

The Skill Sandboxing System provides **OS-native security isolation** for dynamically evolved skills in Aleph. It implements a three-layer security model that balances flexibility with safety:

1. **Static Declaration** - Skills declare required capabilities upfront
2. **Parameter Binding** - Runtime parameters map to sandbox constraints
3. **Dynamic Execution** - OS-native sandbox enforces restrictions

### Key Features

- **Preset-based Templates** - Common capability patterns (file_processor, web_scraper, etc.)
- **Override Merging** - Fine-grained capability customization
- **Parameter Binding** - Dynamic path/domain restrictions from runtime values
- **Enhanced Audit Logging** - Full traceability of capability resolution
- **Immutability Enforcement** - Presets cannot be modified at runtime

---

## Architecture

### Three-Layer Security Model

```
┌─────────────────────────────────────────────────────────────┐
│                    Layer 1: Static Declaration               │
│  ┌────────────┐     ┌────────────┐     ┌────────────┐      │
│  │   Preset   │ +   │ Overrides  │  =  │  Required  │      │
│  │ (Template) │     │ (Custom)   │     │Capabilities│      │
│  └────────────┘     └────────────┘     └────────────┘      │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│                  Layer 2: Parameter Binding                  │
│  ┌────────────┐     ┌────────────┐     ┌────────────┐      │
│  │  Runtime   │ +   │ Validation │  =  │   Bound    │      │
│  │ Parameters │     │   Rules    │     │Capabilities│      │
│  └────────────┘     └────────────┘     └────────────┘      │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│                 Layer 3: Dynamic Execution                   │
│  ┌────────────┐     ┌────────────┐     ┌────────────┐      │
│  │ OS-Native  │ +   │   Audit    │  =  │   Secure   │      │
│  │  Sandbox   │     │    Log     │     │ Execution  │      │
│  └────────────┘     └────────────┘     └────────────┘      │
└─────────────────────────────────────────────────────────────┘
```

### Module Structure

```
src/
├── exec/sandbox/
│   ├── presets.rs              # PresetRegistry + 4 core presets
│   ├── parameter_binding.rs    # ParameterBinding + ValidationRule
│   ├── capability_resolver.rs  # apply_overrides() + bind_parameters()
│   ├── adapter.rs              # SandboxAdapter (macOS sandbox-exec)
│   ├── audit.rs                # Enhanced audit with ToolExecutionContext
│   └── mod.rs
└── skill_evolution/
    ├── sandbox_integration.rs  # resolve_tool_capabilities()
    ├── sandboxed_executor.rs   # SandboxedToolExecutor
    ├── tool_generator.rs       # GeneratedToolDefinition + required_capabilities
    ├── integration_tests.rs    # End-to-end tests
    └── mod.rs
```

---

## Core Concepts

### 1. Presets

**Presets** are immutable capability templates for common use cases.

#### Available Presets

| Preset | Purpose | Capabilities |
|--------|---------|--------------|
| `file_processor` | File operations | read/write specific paths |
| `web_scraper` | HTTP requests | network access to domains |
| `code_analyzer` | Read-only code analysis | read-only file access |
| `data_transformer` | In-memory processing | no file/network access |

#### Example: File Processor Preset

```rust
Capabilities {
    file_read: vec!["/tmp/*".to_string()],
    file_write: vec!["/tmp/*".to_string()],
    network_access: vec![],
    allow_exec: false,
}
```

### 2. Overrides

**Overrides** allow fine-grained customization of preset capabilities.

#### Override Rules

- **Additive**: `file_read`, `file_write`, `network_access` append to preset
- **Replacement**: `allow_exec` replaces preset value
- **Validation**: Paths must be absolute, domains must be valid

#### Example: Custom File Processor

```rust
RequiredCapabilities {
    preset: Some("file_processor".to_string()),
    overrides: Some(Capabilities {
        file_read: vec!["/data/input/*".to_string()],  // Added to preset
        file_write: vec!["/data/output/*".to_string()], // Added to preset
        network_access: vec![],
        allow_exec: false,
    }),
    parameter_bindings: None,
}
```

**Result**: Can read/write `/tmp/*` (from preset) + `/data/input/*` + `/data/output/*`

### 3. Parameter Binding

**Parameter Binding** maps runtime parameters to sandbox constraints.

#### Binding Types

| Type | Purpose | Example |
|------|---------|---------|
| `PathParameter` | Map parameter to file path | `input_file` → `/data/file.txt` |
| `DomainParameter` | Map parameter to domain | `api_url` → `api.example.com` |

#### Validation Rules

| Rule | Description | Example |
|------|-------------|---------|
| `MustBeAbsolute` | Path must start with `/` | `/data/file.txt` ✓ |
| `MustBeSubpathOf(path)` | Path must be under directory | `/data/sub/file.txt` ✓ |
| `MustMatchPattern(regex)` | Path must match regex | `.*\.json$` |

#### Example: Dynamic Path Binding

```rust
ParameterBinding {
    parameter_name: "input_file".to_string(),
    mapping_type: MappingType::PathParameter {
        capability_type: "file_read".to_string(),
    },
    validation_rules: vec![
        ValidationRule::MustBeAbsolute,
        ValidationRule::MustBeSubpathOf("/data/input".to_string()),
    ],
}
```

**Runtime**: `input_file = "/data/input/report.json"` → adds `/data/input/report.json` to `file_read`

---

## Capability Resolution Flow

### Step-by-Step Process

```
1. Tool Definition
   ↓
   GeneratedToolDefinition {
       required_capabilities: Some(RequiredCapabilities {
           preset: Some("file_processor"),
           overrides: Some(...),
           parameter_bindings: Some(...),
       })
   }

2. Resolve Capabilities (sandbox_integration.rs)
   ↓
   resolve_tool_capabilities(tool_def, runtime_params)
   ├─ Load preset from PresetRegistry
   ├─ Apply overrides (additive merge)
   └─ Bind parameters (validate + add paths/domains)

3. Create Sandbox Command (sandboxed_executor.rs)
   ↓
   SandboxAdapter::create_command(capabilities)
   └─ Generate macOS sandbox profile

4. Execute with Audit (sandboxed_executor.rs)
   ↓
   SandboxedToolExecutor::execute_tool(...)
   ├─ Log ToolExecutionContext (tool_name, capabilities, resolution_steps)
   ├─ Execute in sandbox
   └─ Log result
```

### Resolution Example

**Input**:
```rust
RequiredCapabilities {
    preset: Some("file_processor"),
    overrides: Some(Capabilities {
        file_read: vec!["/data/input/*"],
        ..Default::default()
    }),
    parameter_bindings: Some(vec![
        ParameterBinding {
            parameter_name: "output_file",
            mapping_type: MappingType::PathParameter {
                capability_type: "file_write",
            },
            validation_rules: vec![
                ValidationRule::MustBeSubpathOf("/data/output"),
            ],
        }
    ]),
}

Runtime Parameters:
{
    "output_file": "/data/output/result.json"
}
```

**Resolution Steps**:
1. Load `file_processor` preset: `file_read=["/tmp/*"], file_write=["/tmp/*"]`
2. Apply overrides: `file_read=["/tmp/*", "/data/input/*"]`
3. Bind `output_file`: `file_write=["/tmp/*", "/data/output/result.json"]`

**Final Capabilities**:
```rust
Capabilities {
    file_read: vec!["/tmp/*", "/data/input/*"],
    file_write: vec!["/tmp/*", "/data/output/result.json"],
    network_access: vec![],
    allow_exec: false,
}
```

---

## Enhanced Audit Logging

### ToolExecutionContext

Every tool execution logs full context:

```rust
pub struct ToolExecutionContext {
    pub tool_name: String,
    pub capabilities: Capabilities,
    pub resolution_steps: Vec<ResolutionStep>,
}

pub enum ResolutionStep {
    PresetLoaded { preset_name: String },
    OverrideApplied { field: String, values: Vec<String> },
    ParameterBound { parameter: String, bound_value: String },
}
```

### Audit Log Entry

```json
{
  "timestamp": "2026-02-09T10:30:00Z",
  "event_type": "tool_execution",
  "tool_context": {
    "tool_name": "process_data",
    "capabilities": {
      "file_read": ["/tmp/*", "/data/input/*"],
      "file_write": ["/tmp/*", "/data/output/result.json"],
      "network_access": [],
      "allow_exec": false
    },
    "resolution_steps": [
      {"PresetLoaded": {"preset_name": "file_processor"}},
      {"OverrideApplied": {"field": "file_read", "values": ["/data/input/*"]}},
      {"ParameterBound": {"parameter": "output_file", "bound_value": "/data/output/result.json"}}
    ]
  },
  "result": "success",
  "exit_code": 0
}
```

---

## Integration with Skill Evolution

### Tool Generator Integration

`ToolGenerator` automatically infers required capabilities from tool purpose:

```rust
impl ToolGenerator {
    fn generate_required_capabilities(suggestion: &ToolSuggestion) -> RequiredCapabilities {
        let preset = infer_preset_from_purpose(&suggestion.purpose);

        RequiredCapabilities {
            preset: Some(preset),
            overrides: None,
            parameter_bindings: None,
        }
    }
}
```

**Inference Rules**:
- Purpose contains "file", "read", "write" → `file_processor`
- Purpose contains "http", "api", "fetch" → `web_scraper`
- Purpose contains "analyze", "inspect" → `code_analyzer`
- Default → `data_transformer`

### Sandboxed Execution

`SandboxedToolExecutor` wraps tool execution with capability resolution:

```rust
impl SandboxedToolExecutor {
    pub async fn execute_tool(
        tool_def: &GeneratedToolDefinition,
        parameters: &HashMap<String, String>,
    ) -> Result<ToolOutput> {
        // 1. Resolve capabilities
        let capabilities = resolve_tool_capabilities(tool_def, parameters)?;

        // 2. Create sandbox command
        let sandbox_cmd = SandboxAdapter::create_command(&capabilities)?;

        // 3. Execute with audit
        let context = ToolExecutionContext { ... };
        audit_log.log_tool_execution(&context);

        let output = sandbox_cmd.execute().await?;

        audit_log.log_tool_result(&context, &output);
        Ok(output)
    }
}
```

---

## Security Guarantees

### Immutability

- **Presets are immutable**: Cannot be modified at runtime
- **Validation enforced**: All paths/domains validated before binding
- **Audit trail**: Full traceability of capability resolution

### Least Privilege

- **Preset-based defaults**: Start with minimal capabilities
- **Explicit overrides**: Additional capabilities must be declared
- **Parameter validation**: Runtime values validated against rules

### Defense in Depth

1. **Static Analysis**: Capabilities declared in tool definition
2. **Runtime Validation**: Parameters validated before binding
3. **OS-Level Enforcement**: macOS sandbox enforces restrictions
4. **Audit Logging**: Full traceability for security review

---

## Usage Examples

### Example 1: File Processor with Custom Paths

```rust
let tool_def = GeneratedToolDefinition {
    name: "process_logs".to_string(),
    required_capabilities: Some(RequiredCapabilities {
        preset: Some("file_processor".to_string()),
        overrides: Some(Capabilities {
            file_read: vec!["/var/log/*".to_string()],
            file_write: vec!["/data/processed/*".to_string()],
            ..Default::default()
        }),
        parameter_bindings: Some(vec![
            ParameterBinding {
                parameter_name: "output_file",
                mapping_type: MappingType::PathParameter {
                    capability_type: "file_write".to_string(),
                },
                validation_rules: vec![
                    ValidationRule::MustBeSubpathOf("/data/processed".to_string()),
                ],
            }
        ]),
    }),
    ..Default::default()
};

let mut params = HashMap::new();
params.insert("output_file".to_string(), "/data/processed/summary.json".to_string());

let executor = SandboxedToolExecutor::new();
let output = executor.execute_tool(&tool_def, &params).await?;
```

### Example 2: Web Scraper with Domain Restrictions

```rust
let tool_def = GeneratedToolDefinition {
    name: "fetch_api_data".to_string(),
    required_capabilities: Some(RequiredCapabilities {
        preset: Some("web_scraper".to_string()),
        overrides: Some(Capabilities {
            network_access: vec!["api.example.com".to_string()],
            ..Default::default()
        }),
        parameter_bindings: Some(vec![
            ParameterBinding {
                parameter_name: "api_url",
                mapping_type: MappingType::DomainParameter {
                    capability_type: "network_access".to_string(),
                },
                validation_rules: vec![],
            }
        ]),
    }),
    ..Default::default()
};

let mut params = HashMap::new();
params.insert("api_url".to_string(), "https://api.example.com/data".to_string());

let executor = SandboxedToolExecutor::new();
let output = executor.execute_tool(&tool_def, &params).await?;
```

### Example 3: Code Analyzer (Read-Only)

```rust
let tool_def = GeneratedToolDefinition {
    name: "analyze_codebase".to_string(),
    required_capabilities: Some(RequiredCapabilities {
        preset: Some("code_analyzer".to_string()),
        overrides: Some(Capabilities {
            file_read: vec!["/workspace/src/*".to_string()],
            ..Default::default()
        }),
        parameter_bindings: None,
    }),
    ..Default::default()
};

let params = HashMap::new();

let executor = SandboxedToolExecutor::new();
let output = executor.execute_tool(&tool_def, &params).await?;
```

---

## Testing

### Integration Tests

Located in `src/skill_evolution/integration_tests.rs`:

#### Test 1: End-to-End Capability Resolution

```rust
#[tokio::test]
async fn test_end_to_end_capability_resolution() {
    // Tests full flow: preset → overrides → parameter binding
    // Validates final capabilities match expected values
}
```

#### Test 2: Preset Immutability Enforcement

```rust
#[tokio::test]
async fn test_preset_immutability_enforcement() {
    // Verifies presets cannot be modified at runtime
    // Ensures override merging doesn't mutate preset
}
```

### Running Tests

```bash
cd core
cargo test --package alephcore --lib skill_evolution::integration_tests
```

---

## Future Enhancements

### Phase 3: User Approval Workflow

- **Permission Prompts**: User approval for capability requests
- **Capability Profiles**: Save approved capability sets
- **Audit Dashboard**: UI for reviewing tool executions

### Phase 4: Cross-Platform Support

- **Linux**: seccomp-bpf + AppArmor/SELinux
- **Windows**: Job Objects + AppContainer
- **Unified API**: Platform-agnostic capability model

### Phase 5: Advanced Features

- **Capability Inference**: ML-based capability prediction
- **Dynamic Adjustment**: Runtime capability refinement
- **Anomaly Detection**: Flag unusual capability usage

---

## References

- **Design Document**: `docs/plans/2026-02-09-skill-sandboxing-phase2-design.md`
- **Implementation Plan**: `docs/plans/2026-02-09-skill-sandboxing-phase2-implementation.md`
- **Security System**: `docs/SECURITY.md`
- **Exec System**: `src/exec/`
- **Skill Evolution**: `src/skill_evolution/`

---

## Glossary

| Term | Definition |
|------|------------|
| **Preset** | Immutable capability template for common use cases |
| **Override** | Custom capability additions/replacements |
| **Parameter Binding** | Mapping runtime parameters to sandbox constraints |
| **Capability** | Permission to access resources (files, network, exec) |
| **Validation Rule** | Constraint on parameter values (path format, domain) |
| **Resolution** | Process of computing final capabilities from preset + overrides + bindings |
| **Audit Context** | Full traceability record of capability resolution |
