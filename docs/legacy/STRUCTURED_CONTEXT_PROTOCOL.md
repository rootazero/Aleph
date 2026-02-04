# Structured Context Protocol (SCP)

## Overview

The Structured Context Protocol is Aleph's evolution from "string concatenation" to "structured context" for AI prompt assembly. This architectural upgrade provides:

- **Type Safety**: Structured data types instead of string manipulation
- **Extensibility**: Clean interfaces for adding memory, search, MCP, and other context sources
- **Maintainability**: Separation of intent, config, payload, and context
- **UniFFI Compatibility**: Native Swift/Kotlin/C# support via type-safe bindings

## Architecture

### Data Flow

```
User Input (Clipboard)
    ↓
[1. Parser] → AgentPayload (with Intent)
    ↓
[2. Middleware] → Augment Context (Memory, Search, MCP)
    ↓
[3. Assembler] → Final Prompt (System + User messages)
    ↓
[4. Provider] → AI API
```

### Core Components

#### 1. AgentIntent

Defines **what** the user wants to do:

```rust
pub enum AgentIntent {
    Translation { target_lang: String },
    WebSearch,
    CodeGeneration,
    GeneralChat,
    CustomTransform { prompt_template_id: String },
    SkillCall { tool_name: String },
}
```

#### 2. AgentConfig

Controls **how** to process the request:

```rust
pub struct AgentConfig {
    pub model_override: Option<String>,
    pub temperature: f32,
    pub system_template_id: String,
    pub tools_enabled: Vec<String>,
}
```

#### 3. AgentContext

Contains **augmentation** data:

```rust
pub struct AgentContext {
    pub search_results: Option<Vec<SearchResult>>,
    pub mcp_resources: Option<HashMap<String, Value>>,
    pub memory_snippets: Option<Vec<String>>,
    pub app_context: Option<AppContext>,
}
```

#### 4. AgentPayload

The complete structured request:

```rust
pub struct AgentPayload {
    pub meta: AgentMeta,           // Intent + timestamp
    pub config: AgentConfig,        // Model settings
    pub context: AgentContext,      // Augmented data
    pub user_input: String,         // Original content
}
```

## Usage Examples

### Basic Usage (Rust)

```rust
use alephcore::prompt::{PromptAssembler, AgentIntent, AppContext};

// Create assembler
let assembler = PromptAssembler::new();

// Parse user input
let app_ctx = AppContext {
    app_bundle_id: "com.apple.Notes".to_string(),
    app_name: "Notes".to_string(),
    window_title: Some("Document.txt".to_string()),
};

let mut payload = assembler.parse_intent(
    "/en Hello world",
    Some(app_ctx)
);

// Augment with memory
let memories = vec!["User prefers formal English".to_string()];
assembler.augment_with_memory(&mut payload, memories);

// Get final prompt
let final_prompt = assembler.get_final_system_prompt(&payload);

// Build messages for AI API
let messages = payload.build_messages();
```

### Custom Commands

Users can define shortcuts that map to specific prompts:

```rust
use alephcore::config::RoutingRuleConfig;

let rule = RoutingRuleConfig {
    regex: "^/polite".to_string(),
    provider: "openai".to_string(),
    system_prompt: Some(
        "Rewrite the user's message in a polite, professional tone.".to_string()
    ),
    strip_prefix: Some(true),
};

assembler.add_custom_command_from_rule(&rule)?;
```

Now `/polite <message>` will automatically use the polite tone prompt.

## Integration with Existing System

### Backward Compatibility

The new system integrates with existing components:

```rust
// Existing router still works
let (provider, system_prompt_override) = router.route(&routing_context)?;

// New assembler enhances it
let mut payload = assembler.parse_intent(&user_input, Some(app_context));

// Merge system prompt from router (if exists)
if let Some(custom_prompt) = system_prompt_override {
    assembler.templates_mut().insert("custom_override".to_string(), custom_prompt.to_string());
    payload.config.system_template_id = "custom_override".to_string();
}

// Augment with memory
if let Some(memories) = retrieve_memories()? {
    assembler.augment_with_memory(&mut payload, memories);
}

// Generate final prompt
let final_prompt = assembler.get_final_system_prompt(&payload);
```

### Migration Path

**Phase 1: Parallel Implementation (Current)**
- New `prompt` module coexists with existing system
- Router continues to work as before
- Assembler can be used for new features

**Phase 2: Integration**
- Update `AlephCore::process_with_ai_internal()` to use PromptAssembler
- Keep existing routing logic for provider selection
- Use assembler for prompt construction

**Phase 3: Full Migration**
- Remove string-based prompt concatenation
- All prompts go through structured protocol
- Enable advanced features (MCP, chaining)

## Future Extensions

### Web Search Integration

```rust
// Step 1: Detect search intent
let payload = assembler.parse_intent("/search Rust async", None);

// Step 2: Execute search (in middleware)
let results = search_google("Rust async").await?;

// Step 3: Augment payload
assembler.augment_with_search(&mut payload, results);

// Step 4: AI uses search context automatically
let final_prompt = assembler.get_final_system_prompt(&payload);
```

### MCP Integration

```rust
// Define skill that requires MCP tool
let skill_rule = RoutingRuleConfig {
    regex: "^/weather".to_string(),
    provider: "claude".to_string(),
    system_prompt: Some("You are a weather assistant.".to_string()),
    strip_prefix: Some(true),
};

// Enable MCP tool in config
payload.config.tools_enabled = vec!["weather_api".to_string()];

// MCP middleware fetches weather data
let weather_data = mcp_client.call_tool("weather_api", location)?;
payload.context.mcp_resources = Some(
    HashMap::from([("weather".to_string(), weather_data)])
);

// AI receives weather data in system prompt automatically
```

### Command Chaining (Future)

```rust
// User input: "/search Rust async | /summarize"
let pipeline = CommandPipeline::parse("/search Rust async | /summarize")?;

let mut payload = assembler.parse_intent(&pipeline.commands[0], None);

for command in pipeline.commands {
    match command {
        Command::Search(query) => {
            let results = search(query).await?;
            assembler.augment_with_search(&mut payload, results);
        }
        Command::Summarize => {
            payload.config.system_template_id = "summarizer".to_string();
        }
    }
}
```

## Design Principles

### 1. Separation of Concerns

- **Intent** = What to do
- **Config** = How to do it
- **Context** = What data to use
- **Payload** = What user said

### 2. Zero-Cost Abstraction

Using `Option<T>` and `#[serde(skip_serializing_if = "Option::is_none")]` ensures:
- No memory overhead when context is not used
- Efficient JSON serialization
- Type-safe null handling

### 3. Middleware Pattern

Context augmentation is middleware:

```rust
// Each middleware enriches the payload
let mut payload = parser.parse(input);

payload = memory_middleware.process(payload)?;
payload = search_middleware.process(payload)?;
payload = mcp_middleware.process(payload)?;

let final_prompt = assembler.render(payload);
```

### 4. Template Library

System prompts are centralized:

```rust
templates.insert("trans_en", "You are a professional translator...");
templates.insert("code_expert", "You are a senior engineer...");
templates.insert("summarizer", "You are a concise summarizer...");
```

Users can add custom templates via settings UI.

## Testing

The new system includes comprehensive tests:

```bash
# Run all prompt module tests
cargo test --lib prompt

# Run specific test
cargo test prompt::assembler::test_parse_intent_custom_command

# Test with output
cargo test prompt -- --nocapture
```

## Performance Considerations

### Memory Usage

- `AgentPayload` is lightweight when context is empty
- `Option<Vec<T>>` is `None` = 0 bytes overhead
- Serialization only includes populated fields

### Latency

- Template lookup: O(1) HashMap access
- Intent parsing: O(n) regex matching (same as current routing)
- Context augmentation: Async, non-blocking

## Comparison: Before vs After

### Before (String Concatenation)

```rust
// Hard to extend, error-prone
let mut prompt = "You are a helpful assistant.".to_string();

if let Some(memories) = memories {
    prompt.push_str("\n\nPrevious context:\n");
    for mem in memories {
        prompt.push_str(&format!("- {}\n", mem));
    }
}

prompt.push_str(&format!("\n\nUser: {}", user_input));
```

### After (Structured Protocol)

```rust
// Type-safe, extensible, testable
let mut payload = AgentPayload::new(user_input, intent);

if let Some(memories) = memories {
    payload.context.memory_snippets = Some(memories);
}

let messages = payload.build_messages();
```

## Next Steps

1. ✅ Implement core data structures (`AgentPayload`, `AgentIntent`, etc.)
2. ✅ Implement `PromptAssembler` with template library
3. ✅ Add unit tests for all components
4. ⬜ Integrate with `AlephCore::process_with_ai_internal()`
5. ⬜ Add UniFFI bindings for Swift access
6. ⬜ Update Swift layer to use structured payloads
7. ⬜ Add settings UI for custom command management
8. ⬜ Implement web search middleware
9. ⬜ Implement MCP middleware
10. ⬜ Add command chaining support

## Contributing

When adding new features:

1. **New Intent Type**: Add to `AgentIntent` enum
2. **New Context Source**: Add to `AgentContext` struct
3. **New Template**: Add to `TemplateLibrary::default()`
4. **New Middleware**: Implement `augment_with_*()` method

Example:

```rust
// 1. Add new intent
pub enum AgentIntent {
    // ...existing...
    ImageGeneration { style: String },
}

// 2. Add new context
pub struct AgentContext {
    // ...existing...
    pub image_references: Option<Vec<ImageRef>>,
}

// 3. Add new middleware
impl PromptAssembler {
    pub fn augment_with_images(
        &self,
        payload: &mut AgentPayload,
        images: Vec<ImageRef>
    ) {
        payload.context.image_references = Some(images);
    }
}
```

## References

- [agentstructure.md](../agentstructure.md) - Original design document
- [CLAUDE.md](../CLAUDE.md) - Project architecture guidelines
- [router/mod.rs](../Aleph/core/src/router/mod.rs) - Existing routing system
- [providers/mod.rs](../Aleph/core/src/providers/mod.rs) - AI provider abstraction
