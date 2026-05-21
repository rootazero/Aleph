# Collaborative Skill Evolution

## Overview

The Collaborative Skill Evolution system enables LLM-generated skills to be constrained by both semantic contracts (SuccessManifest) and hard enforcement (Capabilities + Sandbox). This dual-layer architecture ensures that skills operate within well-defined boundaries while maintaining flexibility.

## Architecture

### Dual-Layer Constraint System

```
┌─────────────────────────────────────────────────────────────┐
│                    SOFT LAYER (Semantic)                     │
│                                                               │
│  SuccessManifest (SUCCESS_MANIFEST.md)                       │
│  - Human-readable constraints                                │
│  - Allowed/prohibited operations                             │
│  - Recommended tool chain                                    │
│  - Success criteria                                          │
└───────────────────────────┬─────────────────────────────────┘
                            │
                   ┌────────▼────────┐
                   │ ConstraintValidator│
                   │  (Validates Match) │
                   └────────┬────────┘
                            │
┌───────────────────────────▼─────────────────────────────────┐
│                    HARD LAYER (Enforcement)                  │
│                                                               │
│  Capabilities + Sandbox                                      │
│  - Runtime enforcement                                       │
│  - Filesystem, network, process restrictions                │
│  - Audit logging                                            │
└─────────────────────────────────────────────────────────────┘
```

### Key Components

#### 1. SuccessManifest

Defines semantic constraints for a skill:

```rust
pub struct SuccessManifest {
    pub metadata: SkillMetadata,
    pub goal: String,
    pub allowed_operations: AllowedOperations,
    pub prohibited_operations: ProhibitedOperations,
    pub recommended_tools: Vec<RecommendedTool>,
    pub success_criteria: Vec<String>,
    pub failure_handling: Vec<String>,
    pub security_guarantees: Vec<String>,
}
```

**Example:**

```rust
let manifest = SuccessManifest::new(
    "personal_finance_audit",
    "Audit personal finance data and generate reports"
);

// Check constraints
assert!(manifest.prohibits_network());
assert!(manifest.allows_read_from("/data/finance/*.csv"));
assert!(!manifest.allows_write_to("/data/finance/original.csv"));
```

#### 2. ConstraintValidator

Validates that soft constraints match hard constraints:

```rust
pub struct ConstraintValidator;

impl ConstraintValidator {
    pub fn validate(
        manifest: &SuccessManifest,
        capabilities: &Capabilities,
    ) -> Result<ValidationReport, ConstraintMismatch>;
}
```

**Validation Rules:**

1. **Network**: If manifest prohibits network, capabilities must deny network
2. **Filesystem Read**: Manifest-allowed paths must be granted by capabilities
3. **Filesystem Write**: Manifest-allowed write paths must be granted
4. **Unauthorized Permissions**: Capabilities shouldn't grant undeclared permissions
5. **Process**: If manifest prohibits fork, capabilities must deny process spawn

**Example:**

```rust
let manifest = SuccessManifest::new("test_skill", "Test");
let capabilities = Capabilities::default();

match ConstraintValidator::validate(&manifest, &capabilities) {
    Ok(report) => {
        if report.has_errors() {
            // Should not happen - validate returns Err if errors exist
        }
        for warning in &report.warnings {
            println!("Warning: {:?}", warning);
        }
    }
    Err(ConstraintMismatch::ValidationFailed(report)) => {
        for error in &report.errors {
            eprintln!("Error: {:?}", error);
        }
    }
}
```

#### 3. CollaborativeSolidificationPipeline

Orchestrates the collaborative evolution workflow:

```rust
pub struct CollaborativeSolidificationPipeline {
    tracker: Arc<EvolutionTracker>,
    ai_provider: Arc<dyn AiProvider>,
}
```

**Workflow:**

1. **Detect**: Identify patterns that meet solidification threshold
2. **Generate**: Create SuccessManifest and Capabilities
3. **Validate**: Check constraints match using ConstraintValidator
4. **Fix**: If validation fails, request LLM to fix mismatches (up to 3 attempts)
5. **Queue**: Queue proposals for user approval

**Example:**

```rust
let tracker = Arc::new(EvolutionTracker::new("evolution.db")?);
let pipeline = CollaborativeSolidificationPipeline::new(tracker);

let result = pipeline.run().await?;

for proposal in result.proposals {
    println!("Skill: {}", proposal.manifest.metadata.skill_id);
    println!("Validation: {} errors, {} warnings",
        proposal.validation_report.errors.len(),
        proposal.validation_report.warnings.len()
    );

    if proposal.validation_report.has_errors() {
        println!("Proposal has errors - needs fixing");
    } else {
        println!("Proposal ready for user approval");
    }
}
```

#### 4. SandboxedToolExecutor

Executes tools with pre-execution constraint validation:

```rust
pub struct SandboxedToolExecutor {
    sandbox_manager: SandboxManager,
}
```

**Execution Flow:**

1. Resolve capabilities from tool definition and parameters
2. If SuccessManifest exists, validate against capabilities
3. Block execution if validation errors found
4. Log warnings if validation warnings found
5. Execute tool in sandbox
6. Add validation results to audit log

**Example:**

```rust
let executor = SandboxedToolExecutor::new(sandbox_adapter);

match executor.execute_tool(&tool_def, parameters, package_dir).await {
    Ok((output, audit_log)) => {
        println!("Tool output: {}", output);

        // Check audit log for validation results
        if let Some(context) = audit_log.tool_context {
            for step in context.capability_resolution_log {
                if step.step == "validate_constraints" {
                    println!("Validation: {}", step.description);
                }
            }
        }
    }
    Err(e) => {
        eprintln!("Execution failed: {}", e);
        // Error message includes constraint violations if validation failed
    }
}
```

## Integration Guide

### Adding Collaborative Evolution to Your Skill

1. **Create SuccessManifest**:

```rust
let manifest = SuccessManifest {
    metadata: SkillMetadata {
        skill_id: "my_skill".to_string(),
        version: "1.0.0".to_string(),
        created_at: chrono::Utc::now().timestamp(),
        author: "llm-generated".to_string(),
    },
    goal: "Process data files".to_string(),
    allowed_operations: AllowedOperations {
        filesystem: FileSystemOperations {
            read_paths: vec!["/data/input/*".to_string()],
            write_paths: vec!["/data/output/*".to_string()],
            allow_temp_workspace: true,
        },
        script_execution: ScriptExecution {
            languages: vec!["python".to_string()],
            libraries: vec!["pandas".to_string()],
        },
        data_processing: DataProcessing {
            input_formats: vec!["csv".to_string()],
            output_formats: vec!["json".to_string()],
            operations: vec!["parse".to_string(), "transform".to_string()],
        },
    },
    prohibited_operations: ProhibitedOperations {
        network: NetworkRestrictions {
            prohibit_all: true,
            prohibited_domains: vec![],
            reason: "No network access needed for local file processing".to_string(),
        },
        filesystem: FileSystemRestrictions {
            prohibited_paths: vec!["/data/input/*".to_string()],
            prohibit_modify_originals: true,
            reason: "Preserve original data files".to_string(),
        },
        process: ProcessRestrictions {
            prohibit_fork: true,
            prohibited_commands: vec![],
            reason: "No subprocess spawning needed".to_string(),
        },
    },
    recommended_tools: vec![
        RecommendedTool {
            name: "read_csv".to_string(),
            description: "Read CSV file".to_string(),
            order: 1,
        },
        RecommendedTool {
            name: "transform_data".to_string(),
            description: "Transform data".to_string(),
            order: 2,
        },
    ],
    success_criteria: vec![
        "Output file created".to_string(),
        "All rows processed".to_string(),
    ],
    failure_handling: vec![
        "Log error details".to_string(),
        "Preserve partial results".to_string(),
    ],
    security_guarantees: vec![
        "No network access".to_string(),
        "Original files not modified".to_string(),
    ],
};
```

2. **Add to GeneratedToolDefinition**:

```rust
let tool_def = GeneratedToolDefinition {
    name: "my_skill".to_string(),
    description: "Process data files".to_string(),
    input_schema: serde_json::json!({
        "type": "object",
        "properties": {
            "input_file": {"type": "string"},
            "output_file": {"type": "string"}
        }
    }),
    runtime: "python".to_string(),
    entrypoint: "entrypoint.py".to_string(),
    self_tested: false,
    requires_confirmation: true,
    required_capabilities: Some(capabilities),
    approval_metadata: None,
    success_manifest: Some(manifest), // Add manifest here
    generated: GenerationMetadata {
        pattern_id: "my_skill".to_string(),
        confidence: 0.9,
        generated_at: chrono::Utc::now().timestamp(),
        generator_version: "1.0.0".to_string(),
    },
};
```

3. **Execute with Validation**:

```rust
let executor = SandboxedToolExecutor::new(sandbox_adapter);

// Execution will automatically validate constraints
let result = executor.execute_tool(
    &tool_def,
    serde_json::json!({
        "input_file": "/data/input/data.csv",
        "output_file": "/data/output/result.json"
    }),
    package_dir
).await;

match result {
    Ok((output, audit_log)) => {
        println!("Success: {}", output);
    }
    Err(e) => {
        // Will include constraint violation details if validation failed
        eprintln!("Failed: {}", e);
    }
}
```

## Testing

The system includes comprehensive tests:

### SuccessManifest Tests (3 tests)

- `test_success_manifest_creation`: Verify manifest creation
- `test_prohibits_network`: Test network prohibition detection
- `test_allows_read_from`: Test filesystem read permission checking

### ConstraintValidator Tests (7 tests)

- `test_network_constraint_mismatch`: Network constraint validation
- `test_filesystem_read_constraint_mismatch`: Filesystem read validation
- `test_filesystem_write_constraint_mismatch`: Filesystem write validation
- `test_unauthorized_network_permission`: Unauthorized permission detection
- `test_unauthorized_write_permission`: Unauthorized write detection
- `test_process_constraint_mismatch`: Process constraint validation
- `test_matching_constraints`: Successful validation

### CollaborativeSolidificationPipeline Tests (5 tests)

- `test_pipeline_structure`: Pipeline structure validation
- `test_generate_and_validate_proposal`: Proposal generation
- `test_fix_proposal`: Automatic fixing
- `test_run_pipeline`: Full pipeline execution
- `test_max_fix_attempts`: Fix attempt limits

### SandboxedToolExecutor Tests (3 tests)

- `test_sandboxed_executor_structure`: Executor structure
- `test_constraint_validation_blocks_execution`: Validation blocking
- `test_constraint_validation_allows_matching`: Validation passing

**Total: 18 tests, all passing**

## Error Handling

### Validation Errors

```rust
pub enum ValidationError {
    NetworkMismatch {
        manifest_rule: String,
        capabilities_rule: String,
        reason: String,
    },
    FileSystemMismatch {
        manifest_path: String,
        operation: String,
        reason: String,
    },
    UnauthorizedPermission {
        capability: String,
        reason: String,
    },
    ProcessMismatch {
        manifest_rule: String,
        capabilities_rule: String,
        reason: String,
    },
}
```

### Validation Warnings

```rust
pub enum ValidationWarning {
    CapabilitiesMoreRestrictive {
        aspect: String,
        reason: String,
    },
    UnauthorizedReadPermission {
        path: String,
        reason: String,
    },
}
```

## Best Practices

1. **Always validate constraints** before execution
2. **Use specific paths** in manifests (avoid overly broad wildcards)
3. **Document security guarantees** clearly
4. **Test both success and failure cases**
5. **Log validation results** for audit trails
6. **Fix validation errors** before user approval
7. **Keep manifests synchronized** with capabilities

## Future Enhancements

- [ ] Automatic manifest generation from execution traces
- [ ] Machine learning-based constraint inference
- [ ] User feedback integration for constraint refinement
- [ ] Constraint versioning and migration
- [ ] Cross-skill constraint inheritance
- [ ] Constraint conflict resolution strategies

## References

- [Design Document](plans/2026-02-11-collaborative-skill-evolution-design.md)
- [Implementation Plan](plans/2026-02-09-skill-sandboxing-phase3-implementation.md)
- [Skill Evolution System](SKILL_EVOLUTION.md)
- [Security System](SECURITY.md)
