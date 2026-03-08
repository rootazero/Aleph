# Inline Directives & Legacy Code Cleanup Design

> Date: 2026-03-08
> Status: Approved
> Scope: `core/src/intent/`, `core/src/gateway/inbound_router.rs`, `core/src/components/intent_analyzer.rs`

## Problem

1. **No inline directives**: Aleph has no mechanism for users to embed control parameters (`/think high`, `/model claude`, `/verbose`) within natural language messages. Users must rely on separate UI controls or API parameters.

2. **Legacy code**: The previous intent detection redesign left behind dead code (`ExecutionMode`, `ExecutableTask`, `ExecutionIntent`, old `IntentClassifier`, `ExecutionIntentDecider`, etc.) that is still wired into `inbound_router.rs` and `model_router`.

## Design Philosophy

**Directives are pre-processing, not classification.** Extracting `/think high` from a message is a syntactic operation, not a semantic one. It belongs before the intent pipeline, not inside it.

**One pass, full cleanup.** Migrate all remaining callers of old types to `IntentResult`, then delete everything in one pass. No more "retained for backward compatibility".

## Architecture

### DirectiveParser — Pre-processing Layer

```rust
/// Registered directive definition
pub struct DirectiveDefinition {
    pub name: String,                        // "think", "model", "verbose"
    pub accepts_value: bool,                 // /think high → true, /verbose → false
    pub validate: Option<fn(&str) -> bool>,  // optional value validator
}

/// Extracted directive
pub struct Directive {
    pub name: String,
    pub value: Option<String>,
}

/// Pre-processing result
pub struct ParsedInput {
    pub cleaned_text: String,       // text with directives removed
    pub directives: Vec<Directive>, // extracted directives
}

/// Extensible directive parser with registry
pub struct DirectiveParser {
    registry: HashMap<String, DirectiveDefinition>,
}

impl DirectiveParser {
    pub fn parse(&self, input: &str) -> ParsedInput;
}
```

### Parsing Rules

- Scan all `/name` or `/name value` tokens in the input
- Only extract tokens whose name matches a registered directive
- Unregistered `/xxx` tokens are left in the text (paths, unknown commands, etc.)
- Value boundary: whitespace-delimited, up to the next token
- After extraction, collapse extra whitespace in cleaned text

### Built-in Directives

| Name | Accepts Value | Effect |
|------|---------------|--------|
| `think` | `low\|medium\|high` | Controls thinking level |
| `model` | model name string | Overrides model routing |
| `verbose` | no | Verbose output |
| `brief` | no | Concise output |
| `notools` | no | Disable tool invocation |

### Directive Scope

- **v1: Single-message** — directives only affect the current message processing
- **Future: Session-sticky** — `/model claude` persists until overridden (deferred)

### Pipeline Integration

```
User Input
    ↓
┌──────────────────────────────────┐
│ DirectiveParser.parse(input)      │
│ → ParsedInput { cleaned_text,     │
│                  directives }     │
└──────────────────────────────────┘
    ↓ cleaned_text
┌──────────────────────────────────┐
│ UnifiedIntentClassifier.classify()│
│ Abort → L0 → L1 → L2 → L3 → L4  │
│ → IntentResult                    │
└──────────────────────────────────┘
    ↓ IntentResult + directives (independent channel)
┌──────────────────────────────────┐
│ Downstream consumers              │
│ - Thinker: think level, model     │
│ - Agent Loop: notools, verbose    │
│ - InboundRouter: routing          │
└──────────────────────────────────┘
```

**Key principle:** Directives flow independently from IntentResult. The classifier receives cleaned text and knows nothing about directives. Downstream consumers receive both IntentResult and directives separately.

### Edge Cases

| Input | Directives | Cleaned Text | Behavior |
|-------|-----------|--------------|----------|
| `/search rust async` | [] | `/search rust async` | L0 detects slash command |
| `/think high help me code` | [think=high] | `help me code` | Normal classification |
| `read /etc/hosts /verbose` | [verbose] | `read /etc/hosts` | `/etc` not registered, preserved |
| `/think high` | [think=high] | `` | Empty text, directives apply to empty input |
| `translate /model claude /verbose` | [model=claude, verbose] | `translate` | Multiple directives extracted |

### Slash Command Compatibility

The DirectiveParser runs first. If `cleaned_text` still starts with `/`, the L0 slash command layer handles it normally. This preserves full backward compatibility:

- `/search query` → no directive match → cleaned_text = `/search query` → L0 handles it
- `/think high /search query` → think extracted → cleaned_text = `/search query` → L0 handles it

## Legacy Code Cleanup

### Delete List

| File | Reason |
|------|--------|
| `detection/classifier/l1_regex.rs` | Replaced by StructuralDetector |
| `detection/classifier/l2_keywords.rs` | Replaced by KeywordIndex |
| `detection/classifier/keywords.rs` | Hardcoded keyword sets |
| `detection/ai_detector.rs` | Replaced by AiBinaryClassifier |
| `detection/classifier/core.rs` (IntentClassifier) | Replaced by UnifiedIntentClassifier |
| `detection/classifier/types.rs` (ExecutableTask, ExecutionIntent) | Replaced by IntentResult |
| `decision/execution_decider.rs` | Merged into UnifiedIntentClassifier |
| `decision/router.rs` | IntentRouter no longer needed |
| `decision/aggregator.rs` | First-match-wins pipeline, no aggregation |
| `parameters/presets.rs` | Hardcoded Chinese presets |
| `support/agent_prompt.rs` | Chinese capability labels |

### Migration List (migrate callers before deletion)

| Caller | Current Dependency | Migration Target |
|--------|-------------------|-----------------|
| `gateway/inbound_router.rs` | `ExecutionMode`, `ExecutionIntentDecider` | `IntentResult` + `DirectiveParser` |
| `dispatcher/model_router/core/intent.rs` | `ExecutionIntentDecider` | `IntentResult` |
| `components/intent_analyzer.rs` | Old classifier references | Pure `UnifiedIntentClassifier` |
| `config/types/policies/experimental.rs` | Old classifier config flags | Remove or adapt |
| `parameters/defaults.rs` | `ExecutableTask` import | Remove dependency |
| `prompt/builder.rs`, `prompt/executor.rs` | Doc references to old types | Update docs |

### Deletion Order

1. Migrate `inbound_router.rs` → `IntentResult`
2. Migrate `model_router` → `IntentResult`
3. Delete `ExecutionMode`, `ExecutionIntentDecider`, `router.rs`, `aggregator.rs`
4. Delete old classifier files (`core.rs` old code, `l1_regex.rs`, `l2_keywords.rs`, `keywords.rs`, old types in `types.rs`)
5. Delete `ai_detector.rs`, `presets.rs`, `agent_prompt.rs`
6. Clean up all old re-exports in `detection/mod.rs` and `intent/mod.rs`

## Testing Strategy

### DirectiveParser Unit Tests

```rust
// Basic extraction
"/think high help me code" → directives=[think=high], cleaned="help me code"

// Multiple directives
"translate this /model claude /verbose" → directives=[model=claude, verbose], cleaned="translate this"

// Unregistered directive preserved
"read /etc/hosts" → directives=[], cleaned="read /etc/hosts"

// Pure slash command (no directive match)
"/search rust async" → directives=[], cleaned="/search rust async"

// Boolean directive
"/verbose what is the weather" → directives=[verbose], cleaned="what is the weather"

// Directive with no remaining text
"/think high" → directives=[think=high], cleaned=""
```

### Pipeline Integration Tests

- Directive extraction → classifier receives cleaned_text → correct IntentResult
- Directives independently forwarded to Thinker configuration
- All existing `UnifiedIntentClassifier` tests pass unchanged

### Regression Tests

- All existing slash command behavior preserved after cleanup
- `inbound_router` routing behavior unchanged after migration
- No broken imports or dead re-exports

## File Changes

### Create

| File | Content |
|------|---------|
| `intent/detection/directive.rs` | DirectiveParser, DirectiveDefinition, Directive, ParsedInput |

### Modify

| File | Changes |
|------|---------|
| `intent/detection/mod.rs` | Add `pub mod directive`, update re-exports |
| `intent/mod.rs` | Update re-exports, remove old types |
| `gateway/inbound_router.rs` | Migrate to IntentResult + DirectiveParser |
| `dispatcher/model_router/core/intent.rs` | Migrate to IntentResult |
| `components/intent_analyzer.rs` | Integrate DirectiveParser, clean old refs |
| `config/types/policies/experimental.rs` | Remove old classifier flags |
| `parameters/defaults.rs` | Remove ExecutableTask dependency |
| `intent/detection/classifier/mod.rs` | Remove old tests and re-exports |

### Delete

| File |
|------|
| `intent/detection/classifier/l1_regex.rs` |
| `intent/detection/classifier/l2_keywords.rs` |
| `intent/detection/classifier/keywords.rs` |
| `intent/detection/ai_detector.rs` |
| `intent/decision/execution_decider.rs` |
| `intent/decision/router.rs` |
| `intent/decision/aggregator.rs` |
| `intent/parameters/presets.rs` |
| `intent/support/agent_prompt.rs` |
