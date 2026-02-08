# Aleph Design Patterns

> Core design patterns and architectural decisions in Aleph

---

## Overview

This document describes the key design patterns used throughout the Aleph codebase to ensure maintainability, type safety, and API ergonomics.

---

## Table of Contents

- [Context Pattern](#context-pattern)
- [Newtype Pattern](#newtype-pattern)
- [FromStr Trait Pattern](#fromstr-trait-pattern)
- [Builder Pattern](#builder-pattern)

---

## Context Pattern

### Motivation

As APIs evolve, function signatures can accumulate many parameters, making them:
- Hard to read and understand
- Difficult to extend without breaking changes
- Error-prone when parameters have similar types
- Cumbersome to use with optional parameters

### Solution

The **Context Pattern** groups related parameters into a dedicated struct, reducing parameter count and improving API ergonomics.

### Implementation: RunContext

**Before (7 parameters):**
```rust
pub async fn run(
    &self,
    request: String,
    context: RequestContext,
    tools: Vec<UnifiedTool>,
    identity: IdentityContext,
    callback: impl LoopCallback,
    abort_signal: Option<watch::Receiver<bool>>,
    initial_history: Option<String>,
) -> LoopResult
```

**After (2 parameters + Context):**
```rust
pub async fn run(
    &self,
    run_context: RunContext,
    callback: impl LoopCallback,
) -> LoopResult
```

**Context Structure:**
```rust
#[derive(Clone)]
pub struct RunContext {
    pub request: String,
    pub context: RequestContext,
    pub tools: Vec<UnifiedTool>,
    pub identity: IdentityContext,
    pub abort_signal: Option<watch::Receiver<bool>>,
    pub initial_history: Option<String>,
}

impl RunContext {
    pub fn new(
        request: impl Into<String>,
        context: RequestContext,
        tools: Vec<UnifiedTool>,
        identity: IdentityContext,
    ) -> Self {
        Self {
            request: request.into(),
            context,
            tools,
            identity,
            abort_signal: None,
            initial_history: None,
        }
    }

    pub fn with_abort_signal(mut self, signal: watch::Receiver<bool>) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    pub fn with_initial_history(mut self, history: impl Into<String>) -> Self {
        self.initial_history = Some(history.into());
        self
    }
}
```

### Usage Example

```rust
// Create context with required parameters
let run_context = RunContext::new(
    request,
    RequestContext::empty(),
    tools,
    identity,
);

// Add optional parameters using builder pattern
let run_context = run_context
    .with_abort_signal(abort_rx)
    .with_initial_history(history);

// Call with clean API
let result = agent_loop.run(run_context, callback).await;
```

### Benefits

1. **Extensibility**: Add new parameters without breaking existing code
2. **Readability**: Clear parameter grouping at call sites
3. **Type Safety**: Compile-time validation of required parameters
4. **Ergonomics**: Optional parameters via builder pattern
5. **Documentation**: Self-documenting parameter relationships

### When to Use

Apply the Context Pattern when:
- Function has 5+ parameters
- Multiple parameters are optional
- Parameters form a logical group
- API is likely to evolve with new parameters
- Function is called from many locations

### Locations in Codebase

- `agent_loop::RunContext` - Agent loop execution parameters
- Future candidates: Tool execution contexts, configuration contexts

---

## Newtype Pattern

### Motivation

Primitive types (String, u64, etc.) lack semantic meaning and can be easily confused:
```rust
fn assign(experiment_id: String, variant_id: String) { ... }
// Easy to swap arguments accidentally!
assign(variant_id, experiment_id); // Compiles but wrong!
```

### Solution

The **Newtype Pattern** wraps primitive types in distinct structs, providing:
- Type safety (prevents mixing different IDs)
- Self-documentation (clear semantic meaning)
- Encapsulation (controlled access to inner value)
- Extension points (add methods without modifying primitives)

### Implementation Examples

#### ID Types

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExperimentId(String);

impl ExperimentId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for ExperimentId {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for ExperimentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for ExperimentId {
    fn from(s: String) -> Self {
        Self(s)
    }
}
```

#### Collection Types

```rust
#[derive(Debug, Clone)]
pub struct Ruleset(Vec<PermissionRule>);

impl Ruleset {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn add(&mut self, rule: PermissionRule) {
        self.0.push(rule);
    }

    pub fn rules(&self) -> &[PermissionRule] {
        &self.0
    }
}

impl FromIterator<PermissionRule> for Ruleset {
    fn from_iter<T: IntoIterator<Item = PermissionRule>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}
```

#### Value Types

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer(Vec<String>);

impl Answer {
    pub fn new(selections: Vec<String>) -> Self {
        Self(selections)
    }

    pub fn single(selection: impl Into<String>) -> Self {
        Self(vec![selection.into()])
    }

    pub fn selections(&self) -> &[String] {
        &self.0
    }
}
```

### Usage Example

```rust
// Type-safe IDs prevent confusion
let exp_id = ExperimentId::new("exp-001");
let var_id = VariantId::new("control");

// Compiler catches type mismatches
engine.assign(exp_id, var_id); // ✓ Correct
engine.assign(var_id, exp_id); // ✗ Compile error!

// Transparent access via Deref
if exp_id.starts_with("exp-") { ... } // Works via Deref

// Explicit conversion when needed
let outcome = ExperimentOutcome::new(
    exp_id.as_str(),  // Explicit conversion
    var_id.as_str(),
    request_id,
    model,
);
```

### Standard Trait Implementations

All Newtypes should implement:

**Required:**
- `Debug` - For debugging output
- `Clone` - For value copying
- `PartialEq`, `Eq` - For comparisons (if applicable)
- `Hash` - For use in HashMap/HashSet (if applicable)

**Recommended:**
- `Display` - For user-facing output
- `From<T>` - For ergonomic construction
- `Deref` - For transparent access to inner type
- `Serialize`, `Deserialize` - For JSON/TOML support

**Optional:**
- `FromStr` - For parsing from strings
- `FromIterator` - For collection types
- `Default` - If there's a sensible default

### Newtype Catalog

| Type | Inner Type | Purpose | Location |
|------|-----------|---------|----------|
| **IDs** |
| `ExperimentId` | `String` | AB testing experiment identifier | `dispatcher/model_router/advanced/ab_testing/types.rs` |
| `VariantId` | `String` | AB testing variant identifier | `dispatcher/model_router/advanced/ab_testing/types.rs` |
| `ContextId` | `String` | Browser context identifier | `browser/context_registry.rs` |
| `TaskId` | `String` | Browser task identifier | `browser/context_registry.rs` |
| `SubscriptionId` | `String` | Event bus subscription identifier | `event/global_bus.rs` |
| **Collections** |
| `Ruleset` | `Vec<PermissionRule>` | Permission rule collection | `permission/rule.rs` |
| **Values** |
| `Answer` | `Vec<String>` | User question answer selections | `event/question.rs` |

### When to Use

Apply the Newtype Pattern for:
- **Identifiers**: User IDs, session IDs, resource IDs
- **Domain Values**: Email addresses, phone numbers, URLs
- **Collections**: When the collection has domain-specific operations
- **Units**: Measurements, currencies, time durations
- **Validated Data**: Types that require validation on construction

Avoid for:
- Simple data transfer objects (DTOs)
- Internal implementation details
- Types that don't benefit from additional type safety

---

## FromStr Trait Pattern

### Motivation

Consistent parsing interface across the codebase:
- Uniform error handling
- Integration with standard library (`str::parse()`)
- Enables generic parsing code

### Implementation

```rust
impl FromStr for TaskStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("Invalid TaskStatus: {}", s)),
        }
    }
}
```

### Usage

```rust
// Direct parsing
let status: TaskStatus = "pending".parse()?;

// Generic parsing
fn parse_config<T: FromStr>(value: &str) -> Result<T, T::Err> {
    value.parse()
}

// Configuration loading
let status = config.get("status")?.parse::<TaskStatus>()?;
```

### Types with FromStr

- `FactType`, `FactSpecificity`, `TemporalScope`
- `HookKind`, `HookPriority`, `PromptScope`
- `DeviceType`, `DeviceRole`
- `TaskStatus`, `RiskLevel`, `Lane`
- `SessionStatus`, `TraceRole`
- `RuntimeKind`, `EvolutionStatus`, `EventType`

---

## Builder Pattern

### Motivation

Ergonomic construction of complex objects with optional parameters.

### Implementation

```rust
impl RunContext {
    pub fn new(/* required params */) -> Self { ... }

    pub fn with_abort_signal(mut self, signal: watch::Receiver<bool>) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    pub fn with_initial_history(mut self, history: impl Into<String>) -> Self {
        self.initial_history = Some(history.into());
        self
    }
}
```

### Usage

```rust
let context = RunContext::new(request, ctx, tools, identity)
    .with_abort_signal(abort_rx)
    .with_initial_history(history);
```

### Benefits

- Fluent API (method chaining)
- Optional parameters without Option<T> in constructor
- Self-documenting (method names describe what's being set)
- Compile-time validation of required parameters

---

## Pattern Combinations

### Context + Builder

`RunContext` combines both patterns:
- Context Pattern: Groups related parameters
- Builder Pattern: Ergonomic optional parameters

```rust
let run_context = RunContext::new(required_params)
    .with_optional_param1(value1)
    .with_optional_param2(value2);
```

### Newtype + FromStr

Many Newtypes implement `FromStr` for parsing:
```rust
let status: TaskStatus = "pending".parse()?;
let device: DeviceType = config.get("device")?.parse()?;
```

### Newtype + Deref

Newtypes with `Deref` allow transparent access:
```rust
let id = ExperimentId::new("exp-001");
if id.starts_with("exp-") { ... } // Works via Deref to str
```

---

## Migration Guide

### Adding Context Pattern

1. **Identify candidate function** (5+ parameters, multiple optional)
2. **Create Context struct** with required + optional fields
3. **Implement constructor** for required parameters
4. **Add builder methods** for optional parameters
5. **Update function signature** to accept Context
6. **Update all call sites** to use Context
7. **Export Context** in module's public API

### Adding Newtype

1. **Identify primitive type** needing semantic meaning
2. **Create Newtype struct** wrapping the primitive
3. **Implement standard traits** (Debug, Clone, PartialEq, etc.)
4. **Add constructor** and accessor methods
5. **Implement Deref** for transparent access (if appropriate)
6. **Update all usage sites** to use Newtype
7. **Add to Newtype catalog** in this document

---

## References

- [Rust API Guidelines - Newtype Pattern](https://rust-lang.github.io/api-guidelines/type-safety.html#c-newtype)
- [Rust Design Patterns - Builder](https://rust-unofficial.github.io/patterns/patterns/creational/builder.html)
- [Effective Rust - Item 6: Use newtypes for type safety](https://www.lurklurk.org/effective-rust/newtype.html)

---

## Changelog

| Date | Change | Author |
|------|--------|--------|
| 2026-02-09 | Initial document with Context and Newtype patterns | Architecture Team |
