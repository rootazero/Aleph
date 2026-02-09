# Skill Sandboxing Phase 2: Integration Design

> **Date**: 2026-02-09
> **Status**: Design Complete
> **Phase**: Phase 2 - Integration to Skill Evolution System

## Executive Summary

Phase 2 integrates the OS-native sandbox infrastructure (Phase 1) into Aleph's Skill Evolution system. The design adopts a **hybrid approach** combining static capability declaration with dynamic runtime restriction, implementing the **Principle of Least Privilege** through a three-layer security model.

## Core Architecture

### Three-Layer Security Model

**L1: Static Declaration Layer**
- Tools declare `required_capabilities` in `tool_definition.json` at generation time
- Uses hierarchical declaration: `base_preset` + `overrides`
- Users review and approve permissions via ApprovalManager

**L2: Parameter Binding Layer**
- Explicit `parameter_bindings` map parameters to capabilities
- Runtime validation (`is_file`, `is_directory`)
- Support for complex types (arrays, nested objects)

**L3: Dynamic Execution Layer**
- SandboxManager generates final profile at execution time
- Formula: `Base_Profile + Context_Policy = Final_OS_Sandbox_Profile`
- All dynamic decisions logged to audit trail

### Data Flow

```
SkillGenerator → tool_definition.json (L1 + L2)
                        ↓
                 ApprovalManager (User Approval)
                        ↓
                 ToolExecutor (Extract Parameters)
                        ↓
                 SandboxManager (L3: Dynamic Synthesis)
                        ↓
                 OS Sandbox (macOS sandbox-exec)
```

## Data Structures

### Extended tool_definition.json

```json
{
  "name": "log_analyzer",
  "description": "分析日志文件并提取错误信息",
  "input_schema": { ... },
  "runtime": "python",
  "entrypoint": "entrypoint.py",

  "required_capabilities": {
    "base_preset": "file_processor",
    "description": "需要读取日志文件并写入分析结果",
    "overrides": {
      "filesystem": [
        {
          "type": "read_only",
          "path": "${PROJECT_ROOT}/logs",
          "reason": "读取项目日志目录"
        }
      ],
      "process": {
        "max_execution_time": 600
      }
    },
    "parameter_bindings": {
      "log_file": {
        "capability": "filesystem.read_only",
        "validation": "is_file"
      },
      "output_dir": {
        "capability": "filesystem.read_write",
        "validation": "is_directory"
      }
    }
  }
}
```

### Rust Type Definitions

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredCapabilities {
    pub base_preset: String,
    pub description: String,
    #[serde(default)]
    pub overrides: CapabilityOverrides,
    #[serde(default)]
    pub parameter_bindings: HashMap<String, ParameterBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterBinding {
    /// Capability string: "filesystem.read_only", "filesystem.read_write"
    pub capability: String,
    /// Validation rule: is_file, is_directory
    pub validation: ValidationRule,
    /// Mapping type: single, each_element (for arrays)
    #[serde(default)]
    pub mapping: MappingType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationRule {
    IsFile,
    IsDirectory,
    IsPath,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingType {
    Single,
    EachElement,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityOverrides {
    #[serde(default)]
    pub filesystem: Vec<FileSystemOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSystemOverride {
    #[serde(rename = "type")]
    pub fs_type: String,  // "read_only", "read_write"
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_execution_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_memory_mb: Option<u64>,
}
```

## Preset Templates

### Preset Registry

Location: `core/src/exec/sandbox/presets.rs`

```rust
pub struct PresetRegistry {
    presets: HashMap<String, PresetDefinition>,
}

pub struct PresetDefinition {
    pub name: String,
    pub description: String,
    pub capabilities: Capabilities,
    /// Fields that cannot be overridden (hard ceiling)
    pub immutable_fields: Vec<String>,
}
```

### Core Presets

**1. `file_processor`** - File Processing Tools
```rust
Capabilities {
    filesystem: vec![FileSystemCapability::TempWorkspace],
    network: NetworkCapability::Deny,
    process: ProcessCapability {
        no_fork: true,
        max_execution_time: 300,
        max_memory_mb: Some(512),
    },
    environment: EnvironmentCapability::Restricted,
}
// immutable: ["network"]
```

**2. `web_scraper`** - Web Scraping Tools
```rust
Capabilities {
    filesystem: vec![FileSystemCapability::TempWorkspace],
    network: NetworkCapability::AllowAll,
    process: ProcessCapability {
        no_fork: true,
        max_execution_time: 600,
        max_memory_mb: Some(1024),
    },
    environment: EnvironmentCapability::Restricted,
}
// immutable: ["filesystem"]
```

**3. `code_analyzer`** - Code Analysis Tools
```rust
Capabilities {
    filesystem: vec![
        FileSystemCapability::ReadOnly {
            path: PathBuf::from("${WORKSPACE}")
        }
    ],
    network: NetworkCapability::Deny,
    process: ProcessCapability {
        no_fork: true,
        max_execution_time: 900,
        max_memory_mb: Some(2048),
    },
    environment: EnvironmentCapability::Restricted,
}
// immutable: ["network", "filesystem.write"]
```

**4. `data_transformer`** - Data Transformation Tools
```rust
Capabilities {
    filesystem: vec![
        FileSystemCapability::TempWorkspace,
        FileSystemCapability::ReadOnly {
            path: PathBuf::from("${PROJECT_ROOT}/data")
        }
    ],
    network: NetworkCapability::Deny,
    process: ProcessCapability {
        no_fork: true,
        max_execution_time: 1800,
        max_memory_mb: Some(4096),
    },
    environment: EnvironmentCapability::Restricted,
}
```

### Merging Rules

1. **Deny by Default**: Presets provide a "safe subset". Overrides typically refine paths within preset scope.
2. **Hard Ceilings**: Certain presets have immutable fields that cannot be overridden.
3. **Dynamic Path Resolution**: Placeholders like `${WORKSPACE}`, `${PROJECT_ROOT}` resolved at runtime.

## Execution Flow

### Stage 1: Tool Invocation
```rust
let tool_call = ToolCall {
    tool_name: "log_analyzer",
    parameters: json!({
        "log_file": "./logs/app.log",
        "output_dir": "./reports"
    })
};
```

### Stage 2: Permission Resolution
```rust
// Load tool definition
let tool_def = load_tool_definition("log_analyzer")?;
let required_caps = &tool_def.required_capabilities;

// Load preset template
let preset = PresetRegistry::get(&required_caps.base_preset)?;
let mut base_caps = preset.capabilities.clone();

// Apply overrides (respecting immutable fields)
base_caps = apply_overrides(
    base_caps,
    &required_caps.overrides,
    &preset.immutable_fields
)?;
```

### Stage 3: Parameter Binding
```rust
let mut runtime_caps = base_caps.clone();

for (param_name, binding) in &required_caps.parameter_bindings {
    let param_value = tool_call.parameters.get(param_name)?;

    // Validate parameter type
    validate_parameter(param_value, &binding.validation)?;

    // Dynamic capability restriction
    match binding.capability.as_str() {
        "filesystem.read_only" => {
            let path = canonicalize_path(param_value)?;
            runtime_caps.filesystem.push(
                FileSystemCapability::ReadOnly { path }
            );
        }
        "filesystem.read_write" => {
            let path = canonicalize_path(param_value)?;
            runtime_caps.filesystem.push(
                FileSystemCapability::ReadWrite { path }
            );
        }
        _ => return Err(AlephError::InvalidCapability),
    }
}

// Remove generic preset permissions, keep only specific bindings
runtime_caps.filesystem.retain(|cap| {
    !matches!(cap, FileSystemCapability::ReadOnly { path }
        if is_placeholder_path(path))
});
```

### Stage 4: Sandbox Execution
```rust
// Create SandboxManager
let sandbox_adapter = Arc::new(MacOSSandbox::new());
let sandbox_manager = SandboxManager::new(sandbox_adapter)
    .with_fallback_policy(FallbackPolicy::Deny);

// Build execution command
let command = SandboxCommand {
    program: get_runtime_executable(&tool_def.runtime),
    args: vec![
        tool_def.entrypoint.clone(),
        serde_json::to_string(&tool_call.parameters)?
    ],
    working_dir: Some(tool_package_dir),
};

// Execute and get audit log
let (result, audit_log) = sandbox_manager
    .execute_sandboxed(&tool_def.name, command, runtime_caps)
    .await?;
```

### Stage 5: Result Processing
```rust
// Save audit log to database
audit_log_store.save(&audit_log).await?;

// Link to skill execution record
evolution_tracker.record_execution(
    SkillExecution::success(
        &tool_def.name,
        &session_id,
        &context,
        &input_summary,
        audit_log.execution_result.duration_ms(),
        result.stdout.len() as u32,
    ).with_sandbox_audit(audit_log.id)
).await?;

// Return result to user
Ok(ToolExecutionResult {
    output: result.stdout,
    sandboxed: result.sandboxed,
    audit_log_id: audit_log.id,
})
```

## Error Handling

### Error Categories

**1. Permission Denied**
```rust
pub enum SandboxError {
    PermissionDenied {
        attempted_action: String,
        required_capability: String,
        suggestion: String,
    }
}
```

Example error message:
```
工具试图访问 '/Users/alice/secret.txt'，但该路径未在权限声明中。

建议：在 tool_definition.json 中添加该路径到 parameter_bindings，
或在 overrides 中声明 filesystem.read_only 权限。
```

**2. Validation Failed**
```rust
ValidationError::TypeMismatch {
    parameter: "log_file",
    expected: "file",
    actual: "directory",
    suggestion: "请提供文件路径而非目录路径"
}
```

**3. Preset Not Found**
```rust
PresetError::NotFound {
    preset_name: "unknown_preset",
    available_presets: vec!["file_processor", "web_scraper", ...],
}
```

**4. Immutable Override Violation**
```rust
OverrideError::ImmutableField {
    preset: "code_analyzer",
    field: "network",
    reason: "代码分析工具不允许网络访问以防止数据泄露"
}
```

## Enhanced Audit Logging

### Extended Audit Log Structure

```rust
pub struct EnhancedSandboxAuditLog {
    // Inherit from Phase 1
    pub base: SandboxAuditLog,

    // Phase 2 additions
    pub tool_name: String,
    pub tool_version: String,
    pub base_preset: String,
    pub applied_overrides: Vec<CapabilityOverride>,
    pub parameter_bindings_used: HashMap<String, String>,
    pub dynamic_paths: Vec<PathBuf>,

    // Transparency log
    pub capability_resolution_log: Vec<ResolutionStep>,
}

pub struct ResolutionStep {
    pub step: String,
    pub timestamp: i64,
    pub description: String,
}
```

### Example Resolution Log

```rust
vec![
    ResolutionStep {
        step: "load_preset",
        description: "加载预设 'file_processor': TempWorkspace, no network"
    },
    ResolutionStep {
        step: "apply_override",
        description: "添加 ReadOnly('/logs') 从 overrides"
    },
    ResolutionStep {
        step: "bind_parameter",
        description: "绑定 log_file='./app.log' -> ReadOnly('./app.log')"
    },
    ResolutionStep {
        step: "final_profile",
        description: "最终权限: ReadOnly('./app.log'), ReadWrite('./reports'), TempWorkspace"
    }
]
```

## User Interface

### Approval Manager Display

```
工具：log_analyzer
描述：分析日志文件并提取错误信息

请求的权限（基于 'file_processor' 预设）：
✓ 读取文件：通过 log_file 参数指定
✓ 写入目录：通过 output_dir 参数指定
✓ 临时工作区：自动创建和清理
✓ 额外权限：读取 ~/project/logs 目录
✗ 网络访问：禁止
✗ 进程 fork：禁止

执行限制：
- 最大执行时间：10 分钟
- 最大内存：512 MB

[批准] [拒绝] [查看详情]
```

## Implementation Modules

### New Modules

1. **`core/src/exec/sandbox/presets.rs`**
   - PresetRegistry
   - PresetDefinition
   - Built-in presets (file_processor, web_scraper, etc.)

2. **`core/src/exec/sandbox/parameter_binding.rs`**
   - ParameterBinding types
   - Validation logic
   - Path extraction

3. **`core/src/exec/sandbox/capability_resolver.rs`**
   - Capability resolution logic
   - Override merging
   - Dynamic restriction

4. **`core/src/skill_evolution/sandbox_integration.rs`**
   - Integration with ToolGenerator
   - Integration with ToolExecutor
   - Audit log linking

### Modified Modules

1. **`core/src/skill_evolution/tool_generator.rs`**
   - Generate `required_capabilities` field
   - Infer appropriate preset from tool purpose
   - Generate `parameter_bindings`

2. **`core/src/skill_evolution/tool_testing.rs`**
   - Execute tools in sandbox during self-test
   - Validate capability declarations

3. **`core/src/skill_evolution/approval.rs`**
   - Display capability requests to user
   - Store approved capabilities

## Security Properties

### Defense in Depth

**Layer 1: Static Validation**
- Reject unreasonable capability requests at generation time
- Enforce preset constraints (immutable fields)

**Layer 2: User Approval**
- Human review of capability requests
- Semantic presentation (not raw JSON)

**Layer 3: Runtime Isolation**
- OS-level sandbox enforcement
- Dynamic path restriction based on actual parameters

### Fail-Safe Defaults

- Missing parameter bindings → execution blocked
- Sandbox unavailable → execution denied (FallbackPolicy::Deny)
- Validation failure → clear error message

### Audit Trail

- All capability resolutions logged
- All sandbox executions recorded
- Linkage between skill executions and audit logs

## Testing Strategy

### Unit Tests

1. Preset loading and merging
2. Parameter binding extraction
3. Capability resolution logic
4. Override validation (immutable fields)

### Integration Tests

1. End-to-end tool execution with sandbox
2. Parameter binding with various types
3. Error handling for permission denied
4. Audit log generation

### Security Tests

1. Attempt to override immutable fields
2. Attempt to access undeclared paths
3. Attempt to fork processes when prohibited
4. Attempt network access when denied

## Migration Path

### Phase 2.1: Foundation (Week 1-2)
- Implement preset registry
- Implement parameter binding types
- Implement capability resolver

### Phase 2.2: Integration (Week 3-4)
- Integrate with ToolGenerator
- Integrate with ToolExecutor
- Implement enhanced audit logging

### Phase 2.3: UI & Testing (Week 5-6)
- Update ApprovalManager UI
- Comprehensive testing
- Documentation

## Success Criteria

- ✅ All evolved tools execute in sandbox
- ✅ Users can review and approve capabilities
- ✅ Dynamic path restriction works correctly
- ✅ Audit logs link to skill executions
- ✅ Clear error messages for permission issues
- ✅ No regression in tool functionality

## Next Steps

After Phase 2 completion:
- **Phase 3**: Security and performance testing
- **Phase 4**: Linux and Windows platform support
- **Phase 5**: Advanced features (network domain filtering, resource monitoring)

---

**Design Status**: Complete
**Ready for Implementation**: Yes
**Estimated Effort**: 6 weeks
**Risk Level**: Medium
