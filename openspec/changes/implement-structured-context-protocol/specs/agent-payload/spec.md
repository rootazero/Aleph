# agent-payload

## SUMMARY

Structured data format for internal agent processing, replacing string concatenation with typed payload objects.

## ADDED Requirements

### Requirement: Core Data Structure Definition

The system MUST define `AgentPayload` as the unified data format for agent request processing, containing metadata, configuration, context, and user input.

#### Scenario: Building a basic payload

```rust
use aethecore::payload::*;

let anchor = ContextAnchor::new(
    "com.apple.Notes".to_string(),
    "Notes".to_string(),
    Some("Document.txt".to_string()),
);

let payload = PayloadBuilder::new()
    .meta(Intent::GeneralChat, 1234567890, anchor)
    .config("openai".to_string(), vec![], ContextFormat::Markdown)
    .user_input("Hello world".to_string())
    .build()
    .unwrap();

assert_eq!(payload.meta.intent, Intent::GeneralChat);
assert_eq!(payload.config.provider_name, "openai");
assert_eq!(payload.user_input, "Hello world");
```

**Validation**: Payload construction succeeds with all required fields.

---

### Requirement: Intent Classification

The system MUST support Intent enum with variants: `BuiltinSearch`, `BuiltinMcp`, `Skills(String)`, `Custom(String)`, and `GeneralChat`.

#### Scenario: Parsing intent from routing rule config

```rust
use aethecore::payload::Intent;
use aethecore::config::RoutingRuleConfig;

// Built-in feature
let rule = RoutingRuleConfig {
    intent_type: Some("search".to_string()),
    ..Default::default()
};
assert_eq!(Intent::from_rule(&rule), Intent::BuiltinSearch);

// Custom command
let rule = RoutingRuleConfig {
    intent_type: Some("translation".to_string()),
    ..Default::default()
};
assert_eq!(Intent::from_rule(&rule), Intent::Custom("translation".to_string()));

// Skills workflow
let rule = RoutingRuleConfig {
    intent_type: Some("skills:pdf".to_string()),
    ..Default::default()
};
assert!(Intent::from_rule(&rule).is_skills());
```

**Validation**: Different intent_type values map to correct Intent variants.

---

### Requirement: Capability Enumeration

The system MUST define `Capability` enum with fixed execution order: `Memory(0)`, `Search(1)`, `Mcp(2)`.

#### Scenario: Sorting capabilities by priority

```rust
use aethecore::payload::Capability;

let caps = vec![Capability::Mcp, Capability::Memory, Capability::Search];
let sorted = Capability::sort_by_priority(caps);

assert_eq!(sorted, vec![
    Capability::Memory,
    Capability::Search,
    Capability::Mcp
]);
```

**Validation**: Capabilities are always executed in the order Memory → Search → MCP.

---

### Requirement: Context Data Container

The system MUST define `AgentContext` with optional fields: `memory_snippets`, `search_results`, `mcp_resources`, `workflow_state`.

#### Scenario: Default context has no data

```rust
use aethecore::payload::AgentContext;

let context = AgentContext::default();

assert!(context.memory_snippets.is_none());
assert!(context.search_results.is_none());
assert!(context.mcp_resources.is_none());
assert!(context.workflow_state.is_none());
```

**Validation**: Default context is empty, avoiding unnecessary memory allocation.

---

### Requirement: Payload Builder Pattern

The system MUST provide `PayloadBuilder` with fluent API for constructing payloads, validating required fields on `.build()`.

#### Scenario: Builder validation catches missing fields

```rust
use aethecore::payload::PayloadBuilder;

// Missing meta
let result = PayloadBuilder::new()
    .config("openai".to_string(), vec![], ContextFormat::Markdown)
    .user_input("Test".to_string())
    .build();

assert!(result.is_err());
assert_eq!(result.unwrap_err(), "Missing meta");
```

**Validation**: Builder returns error when required fields are not set.

---

### Requirement: Context Anchor

The system MUST capture application context at the moment of request, storing `app_bundle_id`, `app_name`, and `window_title`.

#### Scenario: Creating context anchor from captured context

```rust
use aethecore::payload::ContextAnchor;
use aethecore::core::CapturedContext;

let captured = CapturedContext {
    app_bundle_id: "com.apple.Notes".to_string(),
    window_title: Some("Document.txt".to_string()),
};

let anchor = ContextAnchor::from_captured_context(&captured);

assert_eq!(anchor.app_bundle_id, "com.apple.Notes");
assert_eq!(anchor.app_name, "Notes");
assert_eq!(anchor.window_title, Some("Document.txt".to_string()));
```

**Validation**: Context anchor correctly extracts app information.

## MODIFIED Requirements

None.

## REMOVED Requirements

None.

## RENAMED Requirements

None.
