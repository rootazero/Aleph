# prompt-assembly

## SUMMARY

Structured prompt assembly system that formats context data (Memory/Search/MCP) into LLM-ready prompts using configurable formats.

## ADDED Requirements

### Requirement: PromptAssembler Initialization

The system MUST provide `PromptAssembler::new(format)` that accepts a `ContextFormat` parameter.

#### Scenario: Creating assembler with Markdown format

```rust
use aethecore::payload::{PromptAssembler, ContextFormat};

let assembler = PromptAssembler::new(ContextFormat::Markdown);
```

**Validation**: Assembler is created without errors.

---

### Requirement: System Prompt Assembly

The system MUST combine base system prompt with formatted context via `assemble_system_prompt(base, payload)`.

#### Scenario: Assembling prompt without context

```rust
let assembler = PromptAssembler::new(ContextFormat::Markdown);
let payload = create_empty_payload();

let prompt = assembler.assemble_system_prompt("You are helpful.", &payload);

assert_eq!(prompt, "You are helpful.");
```

**Validation**: When context is empty, output equals base prompt.

#### Scenario: Assembling prompt with Memory context

```rust
let assembler = PromptAssembler::new(ContextFormat::Markdown);
let payload = create_payload_with_memory();

let prompt = assembler.assemble_system_prompt("You are helpful.", &payload);

assert!(prompt.starts_with("You are helpful."));
assert!(prompt.contains("### Context Information"));
assert!(prompt.contains("**Relevant History**"));
```

**Validation**: Context is appended to base prompt with proper formatting.

---

### Requirement: Markdown Memory Formatting

The system MUST format memory entries in Markdown with timestamp, app context, user/AI content, and relevance score.

#### Scenario: Memory formatting structure

```rust
let formatted = assembler.format_memory_markdown(&memories);

assert!(formatted.contains("**Relevant History**:"));
assert!(formatted.contains("Conversation at"));
assert!(formatted.contains("App:"));
assert!(formatted.contains("Window:"));
assert!(formatted.contains("User:"));
assert!(formatted.contains("AI:"));
assert!(formatted.contains("Relevance:"));
```

**Validation**: Memory formatting includes all required fields.

---

### Requirement: Content Truncation

The system MUST truncate user/AI content in memory entries to 200 characters maximum.

#### Scenario: Long content is truncated

```rust
let memory = MemoryEntry {
    user_input: "a".repeat(300),
    ai_output: "b".repeat(300),
    ..Default::default()
};

let formatted = assembler.format_memory_markdown(&vec![memory]);

assert!(formatted.contains("aaa..."));  // Truncated at 200 chars
assert!(formatted.contains("bbb..."));
```

**Validation**: Content exceeding 200 characters is truncated with "...".

---

### Requirement: Timestamp Formatting

The system MUST format Unix timestamps as human-readable "YYYY-MM-DD HH:MM:SS UTC" strings.

#### Scenario: Timestamp conversion

```rust
let timestamp = 1609459200;  // 2021-01-01 00:00:00 UTC
let formatted = format_timestamp(timestamp);

assert_eq!(formatted, "2021-01-01 00:00:00 UTC");
```

**Validation**: Timestamps are readable and include timezone.

---

### Requirement: XML/JSON Format Reservation

The system MUST accept `ContextFormat::Xml` and `ContextFormat::Json` but return None (no formatting) in current implementation.

#### Scenario: XML format returns base prompt only

```rust
let assembler = PromptAssembler::new(ContextFormat::Xml);
let payload = create_payload_with_memory();

let prompt = assembler.assemble_system_prompt("Base.", &payload);

assert_eq!(prompt, "Base.");  // Context not formatted in XML yet
```

**Validation**: Non-Markdown formats are accepted but do not format context.

## MODIFIED Requirements

None.

## REMOVED Requirements

None.

## RENAMED Requirements

None.
