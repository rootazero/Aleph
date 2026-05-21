# Skill Evolution System (Phase 10)

The Skill Evolution System (also known as the Skill Compiler) automatically detects repeated successful execution patterns and converts them into reusable skills or tool-backed automations.

## Overview

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│ EvolutionTracker│────▶│SolidificationDet│────▶│  SkillGenerator │
│  (Log Executions)│     │ (Check Thresholds)│    │ (Create SKILL.md)│
└─────────────────┘     └─────────────────┘     └────────┬────────┘
                                                          │
         ┌────────────────────────────────────────────────┼────────────┐
         ▼                                                ▼            ▼
┌─────────────────┐                              ┌─────────────────┐  ┌──────────────┐
│   SafetyGate    │                              │   GitCommitter  │  │ ToolGenerator│
│ (Validate Safety)│                              │  (Auto-commit)  │  │(Tool Package)│
└─────────────────┘                              └─────────────────┘  └──────────────┘
```

## Core Components

### EvolutionTracker

Logs skill executions and maintains metrics in SQLite:

```rust
let tracker = EvolutionTracker::new("~/.aleph/evolution.db")?;

// Log an execution
let exec = SkillExecution::success(
    "poe-refactor-code",  // pattern ID
    "session-123",
    "refactor authentication module",
    "input summary",
    1500, // duration_ms
    2000, // output_length
);
tracker.log_execution(&exec)?;

// Get metrics
let metrics = tracker.get_metrics("poe-refactor-code")?;
```

### SolidificationPipeline

Detects candidates and generates suggestions:

```rust
let pipeline = SolidificationPipeline::new(tracker.clone())
    .with_provider(ai_provider)
    .with_min_confidence(0.7);

let result = pipeline.run().await?;
for suggestion in result.suggestions {
    println!("{}: {}", suggestion.suggested_name, suggestion.confidence);
}
```

### ApprovalManager

Manages user approval workflow:

```rust
let manager = ApprovalManager::new();

// Submit for approval
let id = manager.submit(suggestion)?;

// List pending
for req in manager.list_pending()? {
    println!("{}: {}", req.id, req.suggestion.suggested_name);
}

// Approve or reject
let approved = manager.approve(&id)?;
manager.reject(&other_id, Some("Not useful"))?;
```

### SkillCompiler

Orchestrates the full workflow:

```rust
let compiler = SkillCompiler::new(tracker, registry)
    .with_provider(ai_provider)
    .with_auto_commit();

// Detect and submit suggestions
let count = compiler.detect_and_submit().await?;

// Approve and compile
let result = compiler.approve_and_compile("request-id").await?;
println!("Created skill: {}", result.skill_id);
```

## Configuration

Add to `config.toml`:

```toml
[evolution]
# Enable the skill evolution system
enabled = true

# Database path (relative to config dir)
db_path = "evolution.db"

# Solidification thresholds
[evolution.thresholds]
min_success_count = 3      # Minimum successful executions
min_success_rate = 0.8     # 80% success rate
min_age_days = 1           # Pattern must be 1+ days old
max_idle_days = 30         # Don't solidify stale patterns
min_confidence = 0.7       # Minimum suggestion confidence

# Git integration
auto_commit = false        # Auto-commit generated skills
auto_push = false          # Auto-push to remote
remote = "origin"
branch = "main"

# Tool generation (advanced)
[evolution.tool_generation]
enabled = false                    # Enable tool-backed skills
tools_output_dir = "tools/compiled"
runtime = "python"                 # python or node
require_self_test = true
require_first_run_confirmation = true
max_pending_suggestions = 10
```

## Tool-Backed Skills

For deterministic transformations, skills can be compiled into executable tools:

```rust
let generator = ToolGenerator::new();
let result = generator.generate(&suggestion)?;

// Self-test before registration
let tester = ToolTester::new(generator.clone());
let report = tester.run_self_test("text_processor").await?;

if report.passed {
    // Register in ToolServer
    let registrar = ToolRegistrar::new(generator);
    registrar.register("text_processor", &tool_server, false).await?;
}
```

Generated tool package:
```
~/.aleph/tools/compiled/<tool-name>/
├── tool_definition.json    # Tool metadata and schema
├── entrypoint.py           # Main execution script
├── requirements.txt        # Python dependencies
└── README.md               # Auto-generated docs
```

## Safety Gating

All generated tools pass through safety analysis:

```rust
let gate = SafetyGate::new();
let report = gate.analyze(&suggestion);

match report.level {
    SafetyLevel::Safe => { /* Auto-approve */ }
    SafetyLevel::Caution => { /* Needs review */ }
    SafetyLevel::Dangerous => { /* Requires explicit approval */ }
    SafetyLevel::Blocked => { /* Cannot execute */ }
}
```

### Detected Concerns

| Concern Type | Examples | Default Level |
|--------------|----------|---------------|
| DestructiveFileOp | `rm`, `delete`, `unlink` | Dangerous |
| SystemCommand | `subprocess`, `os.system` | Caution |
| ElevatedPrivilege | `sudo`, `root`, `admin` | Blocked |
| NetworkAccess | `http://`, `requests.` | Caution |
| CredentialAccess | `password`, `token`, `api_key` | Dangerous |
| CodeExecution | `eval(`, `exec(`, `compile(` | Dangerous |

## POE Integration

The crystallization system connects POE execution to skill evolution:

```rust
// Create crystallizer
let (crystallizer, worker) = ChannelCrystallizer::new();

// Spawn worker (handles database operations)
tokio::task::spawn_blocking(move || {
    let tracker = EvolutionTracker::new("evolution.db")?;
    worker.run_blocking(tracker);
});

// Attach to PoeManager
let manager = PoeManager::new(worker, validator, config)
    .with_recorder(crystallizer);

// All POE outcomes are now recorded
let outcome = manager.execute(task).await?;
```

## API Reference

### Types

- `SkillExecution` - Single execution record
- `SkillMetrics` - Aggregated metrics for a pattern
- `SolidificationSuggestion` - Proposed skill with confidence
- `SolidificationConfig` - Detection thresholds
- `ApprovalRequest` - Pending approval entry
- `GeneratedToolDefinition` - Tool package metadata
- `SafetyReport` - Safety analysis results

### Key Methods

```rust
// EvolutionTracker
tracker.log_execution(&exec)?;
tracker.get_metrics("pattern-id")?;
tracker.get_solidification_candidates(&config)?;

// SolidificationPipeline
pipeline.run().await?;
pipeline.has_candidates()?;
pipeline.status()?;

// ApprovalManager
manager.submit(suggestion)?;
manager.list_pending()?;
manager.approve("id")?;
manager.reject("id", reason)?;

// SkillCompiler
compiler.detect_and_submit().await?;
compiler.approve_and_compile("id").await?;
compiler.preview_skill("id")?;
compiler.status()?;

// ToolGenerator
generator.generate(&suggestion)?;
generator.preview(&suggestion);
generator.list_tools()?;

// SafetyGate
gate.analyze(&suggestion);
gate.validate(&suggestion)?;
gate.can_auto_approve(&suggestion);
```

## Best Practices

1. **Start with high thresholds** - Use conservative settings initially
2. **Review suggestions** - Always preview before approving
3. **Enable git integration** - Track skill evolution history
4. **Monitor safety reports** - Review flagged concerns
5. **Test tool-backed skills** - Run self-tests before registration
