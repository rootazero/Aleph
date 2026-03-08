# Inline Directives & Legacy Code Cleanup Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add inline directive extraction (`/think`, `/model`, `/verbose`, etc.) and delete all legacy intent code (`ExecutionMode`, `ExecutionIntentDecider`, old `IntentClassifier`, etc.) in one pass.

**Architecture:** A `DirectiveParser` pre-processes user input before the `UnifiedIntentClassifier` pipeline. Legacy callers (`inbound_router.rs`) are migrated to `IntentResult` first, then all old types and files are deleted.

**Tech Stack:** Rust, Tokio, serde_json, regex

---

### Task 1: Create DirectiveParser with tests

**Files:**
- Create: `core/src/intent/detection/directive.rs`
- Modify: `core/src/intent/detection/mod.rs`

**Step 1: Write failing tests**

Add the following to `core/src/intent/detection/directive.rs`:

```rust
//! Inline directive extraction.
//!
//! Pre-processes user input to extract directives like `/think high`, `/model claude`,
//! `/verbose` before the intent classification pipeline. Unregistered `/xxx` tokens
//! (paths, unknown commands) are left in the text.

use std::collections::HashMap;

/// Definition of a registered directive.
#[derive(Debug, Clone)]
pub struct DirectiveDefinition {
    /// Directive name (e.g. "think", "model")
    pub name: String,
    /// Whether this directive accepts a value (e.g. /think high → true, /verbose → false)
    pub accepts_value: bool,
}

/// A single extracted directive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    /// Directive name
    pub name: String,
    /// Optional value
    pub value: Option<String>,
}

/// Result of pre-processing user input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedInput {
    /// Text with directives removed
    pub cleaned_text: String,
    /// Extracted directives
    pub directives: Vec<Directive>,
}

impl ParsedInput {
    /// Check if a directive with the given name was extracted.
    pub fn has_directive(&self, name: &str) -> bool {
        self.directives.iter().any(|d| d.name == name)
    }

    /// Get the value of a directive by name.
    pub fn directive_value(&self, name: &str) -> Option<&str> {
        self.directives
            .iter()
            .find(|d| d.name == name)
            .and_then(|d| d.value.as_deref())
    }
}

/// Extensible directive parser with a registry of known directives.
pub struct DirectiveParser {
    registry: HashMap<String, DirectiveDefinition>,
}

impl DirectiveParser {
    /// Create an empty parser with no registered directives.
    pub fn new() -> Self {
        Self {
            registry: HashMap::new(),
        }
    }

    /// Create a parser with the built-in directives pre-registered.
    pub fn with_builtins() -> Self {
        let mut parser = Self::new();
        parser.register("think", true);
        parser.register("model", true);
        parser.register("verbose", false);
        parser.register("brief", false);
        parser.register("notools", false);
        parser
    }

    /// Register a directive.
    pub fn register(&mut self, name: &str, accepts_value: bool) {
        self.registry.insert(
            name.to_lowercase(),
            DirectiveDefinition {
                name: name.to_lowercase(),
                accepts_value,
            },
        );
    }

    /// Parse input, extracting registered directives and returning cleaned text.
    pub fn parse(&self, input: &str) -> ParsedInput {
        // TODO: implement
        ParsedInput {
            cleaned_text: input.to_string(),
            directives: vec![],
        }
    }
}

impl Default for DirectiveParser {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser() -> DirectiveParser {
        DirectiveParser::with_builtins()
    }

    #[test]
    fn extract_single_directive_with_value() {
        let result = parser().parse("/think high help me code");
        assert_eq!(result.cleaned_text, "help me code");
        assert_eq!(result.directives.len(), 1);
        assert_eq!(result.directives[0].name, "think");
        assert_eq!(result.directives[0].value.as_deref(), Some("high"));
    }

    #[test]
    fn extract_multiple_directives() {
        let result = parser().parse("translate this /model claude /verbose");
        assert_eq!(result.cleaned_text, "translate this");
        assert_eq!(result.directives.len(), 2);
        assert!(result.has_directive("model"));
        assert!(result.has_directive("verbose"));
        assert_eq!(result.directive_value("model"), Some("claude"));
        assert_eq!(result.directive_value("verbose"), None);
    }

    #[test]
    fn unregistered_directive_preserved() {
        let result = parser().parse("read /etc/hosts");
        assert_eq!(result.cleaned_text, "read /etc/hosts");
        assert!(result.directives.is_empty());
    }

    #[test]
    fn slash_command_preserved() {
        let result = parser().parse("/search rust async");
        assert_eq!(result.cleaned_text, "/search rust async");
        assert!(result.directives.is_empty());
    }

    #[test]
    fn boolean_directive() {
        let result = parser().parse("/verbose what is the weather");
        assert_eq!(result.cleaned_text, "what is the weather");
        assert_eq!(result.directives.len(), 1);
        assert_eq!(result.directives[0].name, "verbose");
        assert_eq!(result.directives[0].value, None);
    }

    #[test]
    fn directive_only_no_text() {
        let result = parser().parse("/think high");
        assert_eq!(result.cleaned_text, "");
        assert_eq!(result.directives.len(), 1);
        assert_eq!(result.directive_value("think"), Some("high"));
    }

    #[test]
    fn directive_at_start_with_slash_command() {
        let result = parser().parse("/think high /search query");
        assert_eq!(result.cleaned_text, "/search query");
        assert_eq!(result.directives.len(), 1);
        assert_eq!(result.directive_value("think"), Some("high"));
    }

    #[test]
    fn case_insensitive_directive() {
        let result = parser().parse("/Think High some text");
        assert_eq!(result.cleaned_text, "some text");
        assert_eq!(result.directive_value("think"), Some("High"));
    }

    #[test]
    fn empty_input() {
        let result = parser().parse("");
        assert_eq!(result.cleaned_text, "");
        assert!(result.directives.is_empty());
    }

    #[test]
    fn no_directives_in_plain_text() {
        let result = parser().parse("hello world how are you");
        assert_eq!(result.cleaned_text, "hello world how are you");
        assert!(result.directives.is_empty());
    }

    #[test]
    fn directive_between_text() {
        let result = parser().parse("help me /think high with coding");
        assert_eq!(result.cleaned_text, "help me with coding");
        assert_eq!(result.directive_value("think"), Some("high"));
    }

    #[test]
    fn multiple_spaces_collapsed() {
        let result = parser().parse("hello  /verbose  world");
        // After removing /verbose, extra spaces should be collapsed
        let cleaned = result.cleaned_text.trim().to_string();
        assert!(!cleaned.contains("  "), "should not have double spaces: '{}'", cleaned);
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib intent::detection::directive -- --nocapture 2>&1 | head -40`

Expected: Most tests FAIL because `parse()` is a stub.

**Step 3: Implement DirectiveParser::parse()**

Replace the `parse()` method stub with:

```rust
    /// Parse input, extracting registered directives and returning cleaned text.
    pub fn parse(&self, input: &str) -> ParsedInput {
        if input.is_empty() {
            return ParsedInput {
                cleaned_text: String::new(),
                directives: vec![],
            };
        }

        let mut directives = Vec::new();
        let mut parts: Vec<&str> = Vec::new();
        let mut chars = input.char_indices().peekable();
        let mut i = 0;

        while i < input.len() {
            // Look for '/' that could start a directive
            if input.as_bytes().get(i) == Some(&b'/') {
                // Extract the token starting at '/'
                let start = i;
                i += 1; // skip '/'

                // Read the name (alphanumeric + underscore)
                let name_start = i;
                while i < input.len() && (input.as_bytes()[i].is_ascii_alphanumeric() || input.as_bytes()[i] == b'_') {
                    i += 1;
                }
                let name = &input[name_start..i];

                if name.is_empty() {
                    // Just a '/' with no name — keep it
                    parts.push(&input[start..i]);
                    continue;
                }

                let name_lower = name.to_lowercase();

                if let Some(def) = self.registry.get(&name_lower) {
                    // It's a registered directive
                    let value = if def.accepts_value {
                        // Skip whitespace to find the value
                        while i < input.len() && input.as_bytes()[i] == b' ' {
                            i += 1;
                        }
                        // Read value until next whitespace or next '/'
                        if i < input.len() && input.as_bytes()[i] != b'/' {
                            let val_start = i;
                            while i < input.len() && input.as_bytes()[i] != b' ' && input.as_bytes()[i] != b'/' {
                                // Handle multi-byte chars safely
                                let c = input[i..].chars().next().unwrap();
                                i += c.len_utf8();
                            }
                            Some(input[val_start..i].to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    directives.push(Directive {
                        name: name_lower,
                        value,
                    });
                } else {
                    // Not a registered directive — check if it looks like a path
                    // (contains '/' after the name portion, like /etc/hosts)
                    // Keep the entire token including what follows
                    // Rewind: keep /name as part of text
                    parts.push(&input[start..i]);
                }
            } else {
                // Regular character — find the end of this text segment
                let start = i;
                while i < input.len() && input.as_bytes()[i] != b'/' {
                    let c = input[i..].chars().next().unwrap();
                    i += c.len_utf8();
                }
                parts.push(&input[start..i]);
            }
        }

        // Join parts and collapse whitespace
        let joined = parts.join("");
        let cleaned_text = joined
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        ParsedInput {
            cleaned_text,
            directives,
        }
    }
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib intent::detection::directive -- --nocapture`

Expected: All tests PASS.

**Step 5: Register the module**

In `core/src/intent/detection/mod.rs`, add `pub mod directive;` and re-export:

Current (line 8-13):
```rust
mod abort;
mod ai_binary;
pub mod ai_detector;
mod classifier;
pub mod keyword;
mod structural;
```

Change to:
```rust
mod abort;
mod ai_binary;
pub mod ai_detector;
mod classifier;
pub mod directive;
pub mod keyword;
mod structural;
```

Add re-export after line 23:
```rust
pub use directive::{Directive, DirectiveDefinition, DirectiveParser, ParsedInput};
```

**Step 6: Verify compilation**

Run: `cargo check -p alephcore`

Expected: Compiles without errors.

**Step 7: Commit**

```bash
git add core/src/intent/detection/directive.rs core/src/intent/detection/mod.rs
git commit -m "intent: add DirectiveParser for inline directive extraction"
```

---

### Task 2: Integrate DirectiveParser into IntentAnalyzer

**Files:**
- Modify: `core/src/components/intent_analyzer.rs`
- Modify: `core/src/intent/mod.rs`

**Step 1: Add re-export in intent/mod.rs**

Add to the detection re-exports section (after line 58):
```rust
pub use detection::{Directive, DirectiveDefinition, DirectiveParser, ParsedInput};
```

**Step 2: Add DirectiveParser field to IntentAnalyzer**

In `core/src/components/intent_analyzer.rs`, add import and field:

Add to imports (line 17-19):
```rust
use crate::intent::{
    DirectiveParser, IntentContext, IntentResult, ParsedInput, StructuralContext,
    UnifiedIntentClassifier,
};
```

Add field to struct (line 112-115):
```rust
pub struct IntentAnalyzer {
    /// Unified intent classifier (v3 pipeline)
    classifier: UnifiedIntentClassifier,
    /// Inline directive parser
    directive_parser: DirectiveParser,
}
```

Update constructors:

```rust
impl IntentAnalyzer {
    pub fn new() -> Self {
        Self {
            classifier: UnifiedIntentClassifier::new(),
            directive_parser: DirectiveParser::default(),
        }
    }

    pub fn with_unified_classifier(classifier: UnifiedIntentClassifier) -> Self {
        Self {
            classifier,
            directive_parser: DirectiveParser::default(),
        }
    }
}
```

**Step 3: Add pre-processing before classification**

Find the call to `self.classifier.classify()` (around line 315). Before it, add directive extraction:

```rust
// Pre-process: extract inline directives
let parsed = self.directive_parser.parse(&input.text);
let classify_text = if parsed.cleaned_text.is_empty() {
    &input.text  // No text remaining, use original (pure directive)
} else {
    parsed.cleaned_text.as_str()
};

// Classify the cleaned text
let result = self.classifier.classify(classify_text, &intent_ctx).await;
```

The `parsed.directives` are available for downstream consumption. For v1, log them:

```rust
if !parsed.directives.is_empty() {
    tracing::info!(
        "[IntentAnalyzer] Extracted directives: {:?}",
        parsed.directives.iter().map(|d| &d.name).collect::<Vec<_>>()
    );
}
```

**Step 4: Verify compilation and tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib components::intent_analyzer`

Expected: Compiles and existing tests pass.

**Step 5: Commit**

```bash
git add core/src/components/intent_analyzer.rs core/src/intent/mod.rs
git commit -m "intent: integrate DirectiveParser into IntentAnalyzer"
```

---

### Task 3: Migrate inbound_router.rs from ExecutionMode to IntentResult

**Files:**
- Modify: `core/src/gateway/inbound_router.rs`

**Context:** `inbound_router.rs` currently uses `ExecutionMode` via `parsed_command_to_mode()` and `serialize_execution_mode()`. It already has `serialize_intent_result()` (marked `#[allow(dead_code)]`). We need to:
1. Replace `parsed_command_to_mode()` with a function that returns `IntentResult`
2. Replace `serialize_execution_mode()` calls with `serialize_intent_result()`
3. Remove the `ExecutionMode` import

**Step 1: Replace parsed_command_to_mode with parsed_command_to_intent_result**

In `core/src/gateway/inbound_router.rs`, replace the function at lines 749-800:

Old:
```rust
    /// Convert a ParsedCommand to ExecutionMode
    fn parsed_command_to_mode(&self, cmd: crate::command::ParsedCommand) -> ExecutionMode {
```

New:
```rust
    /// Convert a ParsedCommand to IntentResult
    fn parsed_command_to_intent_result(&self, cmd: crate::command::ParsedCommand) -> IntentResult {
        use crate::command::CommandContext;
        use crate::intent::DirectToolSource;

        let args = cmd.arguments.clone();

        let (tool_id, source) = match cmd.context {
            CommandContext::Builtin { tool_name } => (tool_name, DirectToolSource::SlashCommand),
            CommandContext::Skill { skill_id, .. } => (skill_id, DirectToolSource::Skill),
            CommandContext::Mcp { server_name, tool_name, .. } => {
                let id = tool_name.unwrap_or(server_name);
                (id, DirectToolSource::Mcp)
            }
            CommandContext::Custom { .. } => (cmd.command_name.clone(), DirectToolSource::Custom),
            CommandContext::None => (cmd.command_name.clone(), DirectToolSource::SlashCommand),
        };

        IntentResult::DirectTool { tool_id, args, source }
    }
```

**Step 2: Update the call site**

At line 624-625, change:
```rust
                    let mode = self.parsed_command_to_mode(parsed);
                    if let Some(mode_json) = serialize_execution_mode(&mode) {
```
To:
```rust
                    let result = self.parsed_command_to_intent_result(parsed);
                    if let Some(mode_json) = serialize_intent_result(&result) {
```

**Step 3: Remove dead_code annotation from serialize_intent_result**

At line 236, remove `#[allow(dead_code)]`.

**Step 4: Delete serialize_execution_mode and parsed_command_to_mode**

Delete the `serialize_execution_mode()` function (lines 185-230) and the old `parsed_command_to_mode()` function (lines 749-800).

**Step 5: Remove ExecutionMode import**

At line 31, delete:
```rust
use crate::intent::ExecutionMode;
```

Also remove `CustomInvocation, McpInvocation, SkillInvocation, ToolInvocation` from the import at lines 752-754 (they were inside the deleted function).

**Step 6: Verify compilation**

Run: `cargo check -p alephcore`

Expected: Compiles without errors.

**Step 7: Commit**

```bash
git add core/src/gateway/inbound_router.rs
git commit -m "gateway: migrate inbound_router from ExecutionMode to IntentResult"
```

---

### Task 4: Delete old decision layer

**Files:**
- Delete: `core/src/intent/decision/execution_decider.rs`
- Delete: `core/src/intent/decision/router.rs`
- Delete: `core/src/intent/decision/aggregator.rs`
- Modify: `core/src/intent/decision/mod.rs`

**Step 1: Update decision/mod.rs**

Replace the entire file content with:

```rust
//! Decision layer for intent routing.
//!
//! Provides confidence calibration for the unified intent pipeline.

pub mod calibrator;

pub use calibrator::{
    CalibratedSignal, CalibrationHistory, CalibratorConfig, ConfidenceCalibrator, IntentSignal,
    RoutingLayer,
};
```

**Step 2: Delete old files**

```bash
rm core/src/intent/decision/execution_decider.rs
rm core/src/intent/decision/router.rs
rm core/src/intent/decision/aggregator.rs
```

**Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -50`

Expected: Likely compile errors from `intent/mod.rs` re-exporting deleted types. Fix in next step.

**Step 4: Update intent/mod.rs re-exports**

Remove lines 72-80 (old decision re-exports):

```rust
// Re-export from decision: execution mode types (used by gateway/inbound_router)
pub use decision::{
    CustomInvocation, ExecutionMode, McpInvocation, SkillInvocation, ToolInvocation,
};

// Re-export from decision: calibrator (used by unified classifier)
pub use decision::{
    CalibratedSignal, CalibrationHistory, CalibratorConfig, ConfidenceCalibrator, IntentSignal,
    RoutingLayer,
};
```

Replace with just the calibrator re-exports:

```rust
// Re-export from decision: calibrator (used by unified classifier)
pub use decision::{
    CalibratedSignal, CalibrationHistory, CalibratorConfig, ConfidenceCalibrator, IntentSignal,
    RoutingLayer,
};
```

**Step 5: Fix any remaining compile errors**

Run: `cargo check -p alephcore 2>&1 | head -80`

Fix any files still importing deleted types. Common fixes:
- `unified.rs` line 19: If it imports from `crate::intent::decision::ConfidenceCalibrator`, this should still work since calibrator is kept.
- `config/types/policies/experimental.rs`: `use_unified_intent_decider` flag — keep the field but update its doc comment.

**Step 6: Verify compilation**

Run: `cargo check -p alephcore`

Expected: Compiles without errors.

**Step 7: Commit**

```bash
git add -A core/src/intent/decision/
git add core/src/intent/mod.rs
git commit -m "intent: delete old decision layer (ExecutionMode, ExecutionIntentDecider, router, aggregator)"
```

---

### Task 5: Delete old classifier files

**Files:**
- Delete: `core/src/intent/detection/classifier/l1_regex.rs`
- Delete: `core/src/intent/detection/classifier/l2_keywords.rs`
- Delete: `core/src/intent/detection/classifier/keywords.rs`
- Delete: `core/src/intent/detection/classifier/l3_ai.rs`
- Delete: `core/src/intent/detection/classifier/core.rs`
- Delete: `core/src/intent/detection/classifier/types.rs`
- Modify: `core/src/intent/detection/classifier/mod.rs`

**Step 1: Rewrite classifier/mod.rs**

Replace the entire file with:

```rust
//! Intent classifier module.
//!
//! Provides the unified intent classification pipeline (v3).

mod unified;

pub use unified::{
    IntentConfig, IntentContext, UnifiedIntentClassifier, UnifiedIntentClassifierBuilder,
};
```

**Step 2: Delete old files**

```bash
rm core/src/intent/detection/classifier/core.rs
rm core/src/intent/detection/classifier/types.rs
rm core/src/intent/detection/classifier/keywords.rs
rm core/src/intent/detection/classifier/l1_regex.rs
rm core/src/intent/detection/classifier/l2_keywords.rs
rm core/src/intent/detection/classifier/l3_ai.rs
```

**Step 3: Update detection/mod.rs**

Remove old re-exports. Replace the file with:

```rust
//! Intent detection layers.
//!
//! Provides abort detection, structural detection, AI binary classification,
//! keyword matching, inline directive extraction, and the unified classifier pipeline.

mod abort;
mod ai_binary;
mod classifier;
pub mod directive;
pub mod keyword;
mod structural;

pub use abort::AbortDetector;
pub use ai_binary::{AiBinaryClassifier, AiBinaryConfig};
pub use classifier::{
    IntentConfig, IntentContext, UnifiedIntentClassifier, UnifiedIntentClassifierBuilder,
};
pub use directive::{Directive, DirectiveDefinition, DirectiveParser, ParsedInput};
pub use keyword::{KeywordIndex, KeywordMatch, KeywordMatchMode, KeywordRule};
pub use structural::{StructuralContext, StructuralDetector};
```

**Step 4: Fix unified.rs imports if needed**

Check `core/src/intent/detection/classifier/unified.rs` — it may import from deleted modules via `super::`. The file currently uses:
- `crate::intent::decision::ConfidenceCalibrator` (still exists)
- `crate::intent::detection::abort::AbortDetector` (still exists)
- `crate::intent::detection::ai_binary::AiBinaryClassifier` (still exists)
- `crate::intent::detection::keyword::KeywordIndex` (still exists)
- `crate::intent::detection::structural::*` (still exists)
- `crate::intent::support::IntentCache` (still exists)
- `crate::intent::types::*` (still exists)

These should all be fine.

**Step 5: Update intent/mod.rs**

Remove the old detection re-exports (lines 60-63):
```rust
// Re-export from detection: legacy types (still used internally by parameters module)
pub use detection::{ExecutableTask, ExecutionIntent, IntentClassifier};
// Re-export from detection: AI detector (used internally by old classifier)
pub use detection::{AiIntentDetector, AiIntentResult};
```

These types no longer exist. Keep only the unified pipeline re-exports.

**Step 6: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -80`

Expected: Likely errors from `parameters/defaults.rs` and `parameters/presets.rs` which import `ExecutableTask`. Fix in Task 6.

**Step 7: Commit (if compiles) or continue to Task 6**

If there are compile errors from parameters module, continue to Task 6 before committing. If clean:

```bash
git add -A core/src/intent/detection/
git add core/src/intent/mod.rs
git commit -m "intent: delete old classifier (IntentClassifier, l1_regex, l2_keywords, keywords, l3_ai)"
```

---

### Task 6: Delete ai_detector.rs and simplify parameters module

**Files:**
- Delete: `core/src/intent/detection/ai_detector.rs`
- Delete: `core/src/intent/parameters/presets.rs`
- Modify: `core/src/intent/parameters/defaults.rs`
- Modify: `core/src/intent/parameters/mod.rs`
- Modify: `core/src/intent/support/mod.rs`

**Step 1: Delete ai_detector.rs**

The `AiIntentDetector` and `AiIntentResult` types from `ai_detector.rs` have been replaced by `AiBinaryClassifier` in `ai_binary.rs`. Delete:

```bash
rm core/src/intent/detection/ai_detector.rs
```

Note: `detection/mod.rs` was already updated in Task 5 to not reference this file.

**Step 2: Delete presets.rs**

`presets.rs` contains hardcoded Chinese keyword presets that depend on `ExecutableTask`. This is exactly what the language-agnostic redesign removes.

```bash
rm core/src/intent/parameters/presets.rs
```

**Step 3: Rewrite defaults.rs**

The `DefaultsResolver` depends on `ExecutableTask` and `PresetRegistry`. Since both are deleted, simplify it to a stub that returns defaults:

```rust
//! DefaultsResolver for smart parameter resolution.
//!
//! Resolves default parameters. Currently returns defaults directly;
//! preset matching was removed as part of the language-agnostic redesign.

use super::types::{ParameterSource, TaskParameters};

/// Resolves default parameters for tasks.
pub struct DefaultsResolver;

impl DefaultsResolver {
    /// Create a new defaults resolver.
    pub fn new() -> Self {
        Self
    }

    /// Resolve parameters (currently returns inference defaults).
    pub fn resolve(&self) -> TaskParameters {
        TaskParameters::default().with_source(ParameterSource::Inference)
    }
}

impl Default for DefaultsResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_resolver() {
        let resolver = DefaultsResolver::new();
        let params = resolver.resolve();
        assert_eq!(params.source, ParameterSource::Inference);
    }
}
```

**Step 4: Update parameters/mod.rs**

Remove `PresetRegistry` and `ScenarioPreset` from re-exports:

```rust
//! Parameter management for intent classification.

pub mod context;
pub mod defaults;
pub mod types;

pub use context::{
    AppContext, ConversationContext, InputFeatures, MatchingContext, MatchingContextBuilder,
    PendingParam, TimeContext,
};
pub use defaults::DefaultsResolver;
pub use types::{ConflictResolution, OrganizeMethod, ParameterSource, TaskParameters};
```

**Step 5: Update intent/mod.rs parameters re-exports**

Remove `PresetRegistry` and `ScenarioPreset` from lines 83-87:

Old:
```rust
pub use parameters::{
    AppContext, ConflictResolution, ConversationContext, DefaultsResolver, InputFeatures,
    MatchingContext, MatchingContextBuilder, OrganizeMethod, ParameterSource, PendingParam,
    PresetRegistry, ScenarioPreset, TaskParameters, TimeContext,
};
```

New:
```rust
pub use parameters::{
    AppContext, ConflictResolution, ConversationContext, DefaultsResolver, InputFeatures,
    MatchingContext, MatchingContextBuilder, OrganizeMethod, ParameterSource, PendingParam,
    TaskParameters, TimeContext,
};
```

**Step 6: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -80`

Fix any remaining errors. Common issues:
- Files importing `PresetRegistry` or `ScenarioPreset` — search with `grep -r "PresetRegistry\|ScenarioPreset" core/src/`
- Files importing `AiIntentDetector` or `AiIntentResult` — search with `grep -r "AiIntentDetector\|AiIntentResult" core/src/`

**Step 7: Commit**

```bash
git add -A core/src/intent/
git commit -m "intent: delete ai_detector, presets, simplify defaults"
```

---

### Task 7: Clean up all re-exports and experimental config

**Files:**
- Modify: `core/src/intent/mod.rs` (final cleanup)
- Modify: `core/src/config/types/policies/experimental.rs`
- Modify: `core/src/dispatcher/model_router/core/intent.rs` (doc update)

**Step 1: Final intent/mod.rs cleanup**

The file should now contain only valid re-exports. Verify and rewrite to:

```rust
//! Intent detection module for AI-powered conversation flow.
//!
//! This module provides a unified intent classification pipeline that determines
//! whether user input should be aborted, routed to a direct tool, executed as
//! a task, or handled conversationally.
//!
//! # Unified Pipeline Architecture (v3)
//!
//! ```text
//! User Input
//!     ↓
//! ┌─────────────────────────────────────────────────────────────┐
//! │ Pre-process: DirectiveParser                                  │
//! │     - Extract /think, /model, /verbose from text              │
//! │     - Return (cleaned_text, directives)                       │
//! └─────────────────────────────────────────────────────────────┘
//!     ↓ cleaned_text
//! ┌─────────────────────────────────────────────────────────────┐
//! │ L0: Abort Detection (<1ms)                                   │
//! │     - Exact-match stop words (multilingual)                  │
//! └─────────────────────────────────────────────────────────────┘
//!     ↓ (not aborted)
//! ┌─────────────────────────────────────────────────────────────┐
//! │ L1: Slash Command Detection (<1ms)                           │
//! │     - Built-in commands (/screenshot, /ocr, /search, etc.)   │
//! └─────────────────────────────────────────────────────────────┘
//!     ↓ (no match)
//! ┌─────────────────────────────────────────────────────────────┐
//! │ L2: Structural Detection (<5ms)                              │
//! │     - Paths, URLs, context signals                           │
//! └─────────────────────────────────────────────────────────────┘
//!     ↓ (no match)
//! ┌─────────────────────────────────────────────────────────────┐
//! │ L3: Keyword Matching (<20ms)                                 │
//! │     - KeywordIndex with weighted scoring                     │
//! │     - Supports CJK character tokenization                    │
//! └─────────────────────────────────────────────────────────────┘
//!     ↓ (no match)
//! ┌─────────────────────────────────────────────────────────────┐
//! │ L4: Default Fallback                                         │
//! │     - Execute or Converse depending on configuration         │
//! └─────────────────────────────────────────────────────────────┘
//! ```

// Submodules
pub mod decision;
pub mod detection;
pub mod parameters;
pub mod support;
pub mod types;

// Re-export from detection: unified pipeline (primary API)
pub use detection::{
    IntentContext, KeywordIndex, KeywordMatch, KeywordMatchMode, KeywordRule, StructuralContext,
    UnifiedIntentClassifier, UnifiedIntentClassifierBuilder,
};

// Re-export from detection: directive parser
pub use detection::{Directive, DirectiveDefinition, DirectiveParser, ParsedInput};

// Re-export from types: pipeline result types
pub use types::{DetectionLayer, DirectToolSource, ExecuteMetadata, IntentResult};

// Re-export from types: shared
pub use types::TaskCategory;

// Re-export from decision: calibrator
pub use decision::{
    CalibratedSignal, CalibrationHistory, CalibratorConfig, ConfidenceCalibrator, IntentSignal,
    RoutingLayer,
};

// Re-export from parameters
pub use parameters::{
    AppContext, ConflictResolution, ConversationContext, DefaultsResolver, InputFeatures,
    MatchingContext, MatchingContextBuilder, OrganizeMethod, ParameterSource, PendingParam,
    TaskParameters, TimeContext,
};

// Re-export from support
pub use support::{
    AgentModePrompt, CacheConfig, CacheMetrics, CachedIntent, GenerationModelInfo, IntentCache,
    RollbackCapable, RollbackConfig, RollbackEntry, RollbackManager, RollbackResult,
    ToolDescription,
};
```

**Step 2: Update experimental.rs**

Update the doc comment for `use_unified_intent_decider` since `ExecutionIntentDecider` no longer exists:

```rust
    /// Legacy flag — no longer has any effect.
    /// The unified intent classifier is now the only classifier.
    /// Kept for backward compatibility with existing config files.
    #[serde(default)]
    pub use_unified_intent_decider: bool,
```

Also update the `verbose_decision_logging` doc:

```rust
    /// Enable verbose decision logging for debugging.
    ///
    /// When enabled, logs detailed information about
    /// intent classification decisions.
    ///
    /// Default: false
    #[serde(default)]
    pub verbose_decision_logging: bool,
```

**Step 3: Update model_router doc comment**

In `core/src/dispatcher/model_router/core/intent.rs`, update the doc comment at lines 217-221:

Old:
```rust
    /// Convert from ExecutionIntentDecider's TaskCategory
    ///
    /// This bridges the ExecutionIntentDecider (Phase 1) with the Model Router (Phase 2).
    /// The TaskCategory represents the execution intent, and this method maps it to
    /// the appropriate TaskIntent for model selection.
```

New:
```rust
    /// Convert from TaskCategory to TaskIntent for model routing.
    ///
    /// Maps the task category to the appropriate TaskIntent for model selection.
```

**Step 4: Search for any remaining references to deleted types**

Run:
```bash
grep -r "ExecutionMode\|ExecutionIntentDecider\|ExecutableTask\|ExecutionIntent\b\|IntentClassifier\b\|AiIntentDetector\|AiIntentResult\|PresetRegistry\|ScenarioPreset\|IntentRouter\|IntentAggregator\|AggregatedIntent\|intent_type_to_category" core/src/ --include="*.rs" | grep -v "test\|doc\|plan\|legacy\|archive"
```

Fix any remaining references.

**Step 5: Verify compilation and tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib 2>&1 | tail -30`

Expected: Compiles and tests pass.

**Step 6: Commit**

```bash
git add core/src/intent/mod.rs core/src/config/types/policies/experimental.rs core/src/dispatcher/model_router/core/intent.rs
git commit -m "intent: clean up re-exports and update stale doc comments"
```

---

### Task 8: Final verification

**Step 1: Full build check**

Run: `cargo check -p alephcore`

Expected: Clean compilation.

**Step 2: Run all intent-related tests**

Run: `cargo test -p alephcore --lib intent 2>&1 | tail -40`

Expected: All tests pass.

**Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -40`

Expected: All tests pass (except pre-existing `markdown_skill` failures which are unrelated).

**Step 4: Verify no dead re-exports**

Run:
```bash
grep -r "ExecutionMode\|ExecutionIntentDecider\|ExecutableTask\|ExecutionIntent\b\|IntentClassifier\b\|AiIntentDetector\|AiIntentResult\|IntentRouter\|RouteResult\|DirectMode\|ThinkingContext\|IntentAction\|MissingParameter\|DecisionResult\|DecisionMetadata\|IntentLayer\|DecisionLayer\|ContextSignals\|DeciderConfig\|SlashCommand\b" core/src/ --include="*.rs" -l
```

Expected: No hits in production code (only in docs/legacy/ or test fixtures if any).

**Step 5: Commit any final fixes**

If any fixes were needed:
```bash
git add -A core/src/
git commit -m "intent: final cleanup pass"
```
