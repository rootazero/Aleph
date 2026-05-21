# routing-enhancement

## SUMMARY

Enhancement to routing system to support structured payload building, capability execution, and intent inference from configuration.

## ADDED Requirements

### Requirement: Payload Construction in Router

The system MUST construct `AgentPayload` in `Router::route()` using information from routing decision and captured context.

#### Scenario: Router builds payload from decision

```rust
let decision = RoutingDecision {
    provider_name: "openai".to_string(),
    intent: Intent::Custom("translation".to_string()),
    capabilities: vec![Capability::Memory],
    context_format: ContextFormat::Markdown,
    processed_input: "Hello".to_string(),
    ..Default::default()
};

let context = CapturedContext {
    app_bundle_id: "com.apple.Notes".to_string(),
    window_title: Some("Doc.txt".to_string()),
};

let (provider, payload) = router.route_with_decision(decision, &context)?;

assert_eq!(payload.meta.intent, Intent::Custom("translation".to_string()));
assert_eq!(payload.config.provider_name, "openai");
assert_eq!(payload.config.capabilities, vec![Capability::Memory]);
assert_eq!(payload.user_input, "Hello");
```

**Validation**: Payload fields match routing decision values.

---

### Requirement: Capability Execution Orchestration

The system MUST execute capabilities in sorted order (Memory → Search → MCP), populating `payload.context` fields.

#### Scenario: Executing multiple capabilities

```rust
let mut payload = AgentPayload::builder()
    .config("openai".to_string(), vec![Capability::Search, Capability::Memory], ContextFormat::Markdown)
    .build()?;

router.execute_capabilities(&mut payload)?;

// Memory executed first (higher priority)
assert!(payload.context.memory_snippets.is_some());

// Search executed second
// (Currently returns None in MVP, but called)
```

**Validation**: Capabilities execute in priority order.

---

### Requirement: RoutingDecision Extension

The system MUST extend `RoutingDecision` with fields: `intent`, `capabilities`, `context_format`.

#### Scenario: Decision includes new fields

```rust
let decision = router.make_decision("/search AI news")?;

assert!(matches!(decision.intent, Intent::BuiltinSearch));
assert!(decision.capabilities.contains(&Capability::Search));
assert_eq!(decision.context_format, ContextFormat::Markdown);
```

**Validation**: Routing decision contains payload construction data.

---

### Requirement: Intent Inference from Config

The system MUST infer `Intent` from `RoutingRuleConfig.intent_type` field.

#### Scenario: Built-in feature intent

```toml
[[rules]]
regex = "^/search"
provider = "openai"
intent_type = "search"  # Maps to Intent::BuiltinSearch
```

```rust
let rule = load_rule_from_config();
let intent = Intent::from_rule(&rule);

assert_eq!(intent, Intent::BuiltinSearch);
```

**Validation**: Config intent_type correctly maps to Intent enum.

#### Scenario: Custom intent

```toml
[[rules]]
regex = "^/translate"
provider = "openai"
intent_type = "translation"
```

```rust
let intent = Intent::from_rule(&rule);
assert_eq!(intent, Intent::Custom("translation".to_string()));
```

**Validation**: Non-builtin intent_type creates Custom variant.

---

### Requirement: Capability Parsing from Config

The system MUST parse `capabilities` array from config into `Vec<Capability>`, skipping invalid entries.

#### Scenario: Parsing valid capabilities

```toml
[[rules]]
regex = ".*"
provider = "openai"
capabilities = ["memory", "search"]
```

```rust
let rule = load_rule_from_config();
let caps = parse_capabilities(&rule.capabilities.unwrap());

assert_eq!(caps, vec![Capability::Memory, Capability::Search]);
```

**Validation**: String array converts to capability list.

#### Scenario: Invalid capability is skipped

```toml
capabilities = ["memory", "invalid", "search"]
```

```rust
let caps = parse_capabilities(&capabilities);

assert_eq!(caps.len(), 2);  // "invalid" skipped
assert!(caps.contains(&Capability::Memory));
assert!(caps.contains(&Capability::Search));

// Warning logged
assert_logs_contain("Unknown capability: invalid");
```

**Validation**: Invalid capabilities are logged and skipped, not erroring.

---

### Requirement: Context Format Parsing

The system MUST parse `context_format` from config, defaulting to Markdown if not specified.

#### Scenario: Explicit format

```toml
[[rules]]
regex = ".*"
provider = "claude"
context_format = "xml"
```

```rust
let decision = router.make_decision("test")?;
assert_eq!(decision.context_format, ContextFormat::Xml);
```

**Validation**: Configured format is used.

#### Scenario: Default format

```toml
[[rules]]
regex = ".*"
provider = "openai"
# No context_format specified
```

```rust
let decision = router.make_decision("test")?;
assert_eq!(decision.context_format, ContextFormat::Markdown);
```

**Validation**: Markdown is default when not specified.

## MODIFIED Requirements

### Requirement: Router::route() Return Type

The system MUST return `AgentPayload` alongside the provider, instead of returning raw system prompt string.

**Change**: Updated return type from `(Arc<dyn AiProvider>, Option<&str>)` to `(Arc<dyn AiProvider>, AgentPayload)`

#### Scenario: Updated return signature

```rust
let (provider, payload) = router.route("test input", &context)?;

assert_eq!(provider.name(), "openai");
assert_eq!(payload.user_input, "test input");
```

**Validation**: Router returns payload instead of raw strings.

## REMOVED Requirements

None.

## RENAMED Requirements

None.
