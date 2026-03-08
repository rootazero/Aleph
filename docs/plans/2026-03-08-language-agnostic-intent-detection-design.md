# Language-Agnostic Intent Detection Design

> Date: 2026-03-08
> Status: Approved
> Scope: `core/src/intent/`

## Problem

Aleph's intent detection is tightly bound to Chinese and English through hardcoded keywords, regex patterns, and preset mappings. This makes it impossible to detect intent in other languages. The root cause: using regex/keyword matching for natural language understanding is a dead end — every new language requires a new set of maintenance-heavy rules.

Additionally, two parallel classifier trees (`IntentClassifier` and `ExecutionIntentDecider`) exist independently with overlapping but inconsistent pattern sets.

## Design Philosophy

**Structural detection is deterministic; semantic classification belongs to the LLM.**

- Slash commands, paths, URLs, context signals — these are language-agnostic structural patterns. Keep them in deterministic code.
- "Does the user want to execute an action or have a conversation?" — this is a semantic question. Let the LLM answer it.
- User-defined keyword rules remain available as a power-user escape hatch, but ship with an empty default rule set.

Reference: OpenClaw's architecture validates this approach — it uses purely deterministic detection for commands and delegates all NLU to the LLM.

## Architecture

### Unified Pipeline

Replaces both `IntentClassifier` and `ExecutionIntentDecider` with a single first-match-wins pipeline:

```
Input
  │
  ├─[Fast Abort]─→ AbortDetector (multilingual stop words) → Abort signal
  │
  ▼
[L0] SlashCommandDetector
     - "/" prefix → CommandParser (skills, MCP, built-in)
     - Returns: IntentResult::DirectTool(tool_id, args)
  │ (no "/" prefix)
  ▼
[L1] StructuralDetector
     - Path extraction (Unix/Windows paths)
     - URL detection
     - Context signals (selected_file, clipboard_type)
     - Returns: IntentResult::Execute with structural metadata
  │ (no structural signal)
  ▼
[L2] UserKeywordMatcher (optional, config-driven)
     - KeywordPolicy from aleph.toml (default: empty rule set)
     - Reuses existing KeywordIndex engine + CJK tokenizer
     - Returns: IntentResult::Execute with optional tag hint
  │ (no match or disabled)
  ▼
[L3] AiBinaryClassifier
     - LLM prompt: "execute (needs tools) or converse (pure chat)?"
     - Simple, fast, language-agnostic
     - Returns: IntentResult::Execute or IntentResult::Converse
  │ (AI unavailable or skipped)
  ▼
[L4] Default → IntentResult::Execute (bias toward action, configurable)
```

### Core Types

```rust
/// Unified output of the intent detection pipeline
pub enum IntentResult {
    /// Slash command or direct tool invocation
    DirectTool {
        tool_id: String,
        args: Option<String>,
        source: DirectToolSource, // SlashCommand | Skill | Mcp
    },
    /// Needs Agent Loop execution (tools, multi-step tasks)
    Execute {
        confidence: f32,
        metadata: ExecuteMetadata,
    },
    /// Pure conversation, no tool needed
    Converse {
        confidence: f32,
    },
    /// User wants to abort the current task
    Abort,
}

/// Metadata attached to Execute intent
pub struct ExecuteMetadata {
    pub detected_path: Option<String>,
    pub detected_url: Option<String>,
    pub context_hint: Option<String>,   // from selected_file, clipboard
    pub keyword_tag: Option<String>,    // from user-defined L2 rules
    pub layer: DetectionLayer,
}

pub enum DetectionLayer { L0, L1, L2, L3, L4Default }
```

### AbortDetector

Fast-path abort detection checked before any classification. Multilingual stop words as a finite set (not NLU — this is enumeration):

- English: stop, abort, halt, cancel, quit
- Chinese: 停, 停止, 取消, 中止
- Japanese: やめて, 止めて, 中止
- Korean: 중지, 멈춰
- Russian: стоп, остановись
- German: stopp, anhalten
- French: arrête, arrete
- Spanish: para, detente
- Portuguese: pare
- Arabic: توقف
- Hindi: रुको

Matching rules:
- Entire message (after normalization) must exactly match a trigger — no substring matching
- Normalization: strip trailing punctuation, collapse whitespace, lowercase

Scope: Agent Loop abort only. Does not cancel tool execution or MCP calls (deferred to future work).

### AiBinaryClassifier

Replaces the current multi-class L3 AI detector with a binary classifier:

| Dimension | Current L3 | New L3 |
|-----------|-----------|--------|
| Granularity | Multi-class (search/video/file_organize/...) | Binary (execute/converse) |
| Language | Chinese examples in prompt | Language-agnostic |
| Output | intent + confidence + params + missing | intent + confidence only |
| Parameter extraction | Attempts location/query/url | None — Dispatcher's job |

System prompt:

```text
You are an intent classifier. Given a user message, determine if it requires
tool execution or is pure conversation.

Respond with JSON only:
{"intent": "execute" | "converse", "confidence": 0.0-1.0}

Guidelines:
- "execute": user wants to perform an action (file operations, code execution,
  web search, image generation, system commands, downloads, etc.)
- "converse": user wants information, explanation, analysis, creative writing,
  translation, or general chat

Examples:
- "organize my downloads folder" → execute
- "what is quantum computing" → converse
- "run the test suite" → execute
- "explain this error message" → converse
- "search for flights to Tokyo" → execute
- "write me a poem about rain" → converse
```

Configuration:

```rust
pub struct AiClassifierConfig {
    pub min_input_length: usize,        // default: 8
    pub timeout: Duration,              // default: 3s
    pub confidence_threshold: f32,      // default: 0.6
}
```

### Unified IntentClassifier

```rust
pub struct IntentClassifier {
    abort_detector: AbortDetector,
    command_parser: Option<CommandParser>,
    structural_detector: StructuralDetector,
    keyword_index: Option<KeywordIndex>,
    ai_classifier: Option<AiBinaryClassifier>,
    cache: Option<IntentCache>,
    calibrator: Option<ConfidenceCalibrator>,
    config: IntentConfig,
}
```

Single entry point `classify(&self, input, context) -> IntentResult` replaces both `IntentClassifier::classify()` and `ExecutionIntentDecider::decide()`.

## File Changes

### Delete

| File | Reason |
|------|--------|
| `detection/classifier/l1_regex.rs` | Hardcoded Chinese/English regex → replaced by StructuralDetector |
| `detection/classifier/l2_keywords.rs` | Hardcoded keyword matching → L2 becomes pure config-driven |
| `detection/classifier/keywords.rs` | EXCLUSION_VERBS + KEYWORD_SETS → deleted |
| `detection/ai_detector.rs` | Replaced by AiBinaryClassifier |
| `decision/execution_decider.rs` | Merged into unified pipeline |
| `decision/router.rs` | IntentRouter facade no longer needed |
| `decision/aggregator.rs` | First-match-wins pipeline, no multi-signal aggregation |
| `parameters/presets.rs` | Hardcoded Chinese presets → deleted |
| `support/agent_prompt.rs` | Chinese capability labels → deleted |

### Modify

| File | Changes |
|------|---------|
| `detection/classifier/core.rs` | Rewrite as unified IntentClassifier |
| `detection/keyword.rs` | Keep KeywordIndex + CJK tokenizer, remove built-in rules |
| `decision/calibrator.rs` | Adapt to IntentResult type |
| `support/cache.rs` | Cache value type → IntentResult |
| `types/task_category.rs` | Delete (20 variants → replaced by IntentResult enum) |
| `parameters/defaults.rs` | Simplify, no TaskCategory mapping |
| `mod.rs` | Update re-exports and module docs |

### Create

| File | Content |
|------|---------|
| `detection/abort.rs` | AbortDetector |
| `detection/structural.rs` | StructuralDetector (paths, URLs, context signals) |
| `detection/ai_binary.rs` | AiBinaryClassifier |
| `types/intent_result.rs` | IntentResult + ExecuteMetadata + DetectionLayer |

### Downstream Adapters

| Caller | Before | After |
|--------|--------|-------|
| Agent Loop | `IntentClassifier::classify()` → `ExecutionIntent` | `IntentClassifier::classify()` → `IntentResult` |
| Agent Loop | `IntentRouter::route()` → `RouteResult` | Use `IntentResult` directly |
| Gateway `intent_detector.rs` | `ExecutionIntentDecider::decide()` | Unified `IntentClassifier::classify()` |
| `config/.../keyword.rs` | `KeywordPolicy::with_builtin_rules()` (3 built-in rules) | `KeywordPolicy::default()` (empty) |

## Testing Strategy

### Unit Tests

- **AbortDetector**: multilingual coverage, punctuation normalization, no substring false positives
- **StructuralDetector**: Unix/Windows paths, URLs, context signals, no-match passthrough
- **AiBinaryClassifier**: mock provider for execute/converse/timeout/too-short cases
- **IntentClassifier pipeline**: layer priority (abort > L0 > L1 > L2 > L3 > L4), cache hits

### Integration Tests — Language Agnosticism

```rust
// Same semantic intent in different languages → same IntentResult variant
let execute_inputs = ["organize my files", "整理我的文件", "ファイルを整理して"];
let converse_inputs = ["what is quantum computing", "量子计算是什么", "量子コンピューティングとは"];
```

### Regression

- Slash command tests from existing `execution_decider` → migrate to L0 tests
- Context signal tests → migrate to StructuralDetector tests
- KeywordIndex engine tests → keep (engine unchanged, just no built-in rules)

## Deferred Work

- **Inline directives** (`/think high` embedded in natural text) — independent feature, separate design
- **Abort scope expansion** (cancel tool execution, MCP calls) — requires Agent Loop plumbing
- **L2 built-in multilingual rules** — not planned; users configure their own via aleph.toml

## Architectural Alignment

| Principle | How This Design Aligns |
|-----------|------------------------|
| R3 Core Minimalism | LLM does NLU; core only does structural detection |
| P1 Low Coupling | Layers are independent, first-match-wins, no cross-layer dependencies |
| P2 High Cohesion | Single IntentClassifier replaces two parallel trees |
| P3 Extensibility | L2 KeywordPolicy allows user extension without code changes |
| P6 Simplicity | 20-variant TaskCategory → 4-variant IntentResult |
| P4 Dependency Inversion | AiBinaryClassifier depends on AiProvider trait, not concrete impl |
