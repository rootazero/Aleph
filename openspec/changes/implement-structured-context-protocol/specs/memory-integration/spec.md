# memory-integration

## SUMMARY

Integration of Memory module into the routing layer, enabling automatic retrieval of relevant conversation history.

## ADDED Requirements

### Requirement: Memory Capability Execution

The system MUST execute Memory capability when configured in routing rules, retrieving similar conversations and populating `payload.context.memory_snippets`.

#### Scenario: Memory retrieval on matching rule

```rust
// config.toml
[[rules]]
regex = ".*"
provider = "openai"
capabilities = ["memory"]  # Enable Memory

// Router execution
let payload = router.route("继续之前的话题", &context)?;

// Memory should be retrieved
assert!(payload.context.memory_snippets.is_some());
let memories = payload.context.memory_snippets.unwrap();
assert!(memories.len() <= 5);  // Respects max_context_items
assert!(memories.iter().all(|m| m.similarity_score.unwrap() >= 0.7));  // Above threshold
```

**Validation**: Memory entries are retrieved when capability is enabled.

---

### Requirement: Configurable Retrieval Parameters

The system MUST respect `max_context_items` and `similarity_threshold` from memory configuration.

#### Scenario: Limiting retrieval count

```toml
[memory]
enabled = true
max_context_items = 3
similarity_threshold = 0.8
```

```rust
let payload = router.route("test input", &context)?;
let memories = payload.context.memory_snippets.unwrap();

assert!(memories.len() <= 3);
assert!(memories.iter().all(|m| m.similarity_score.unwrap() >= 0.8));
```

**Validation**: Retrieval respects configured limits and thresholds.

---

### Requirement: Memory Entry Enrichment

The system MUST populate `similarity_score` field for each retrieved memory entry.

#### Scenario: Similarity scores are present

```rust
let payload = router.route("翻译为英文", &context)?;
let memories = payload.context.memory_snippets.unwrap();

for memory in memories {
    assert!(memory.similarity_score.is_some());
    let score = memory.similarity_score.unwrap();
    assert!(score >= 0.0 && score <= 1.0);
}
```

**Validation**: Every memory entry has a valid similarity score.

---

### Requirement: Graceful Degradation on Memory Failure

The system MUST continue processing requests when Memory retrieval fails, logging warnings but not propagating errors.

#### Scenario: Memory DB unavailable

```rust
// Simulate Memory DB failure
memory_store.close();

// Request should still succeed
let result = router.route("test", &context);
assert!(result.is_ok());

let payload = result.unwrap();
assert!(payload.context.memory_snippets.is_none());

// Warning should be logged
assert_logs_contain("Memory retrieval failed");
```

**Validation**: Memory failures do not block requests.

---

### Requirement: Memory Disable Switch

The system MUST skip Memory retrieval when `memory.enabled = false` in configuration.

#### Scenario: Disabled memory is not retrieved

```toml
[memory]
enabled = false
```

```rust
let payload = router.route("test", &context)?;
assert!(payload.context.memory_snippets.is_none());
```

**Validation**: Memory is skipped when disabled.

## MODIFIED Requirements

None.

## REMOVED Requirements

None.

## RENAMED Requirements

None.
