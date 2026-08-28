# Logic Review Report — src/thinker
**Module**: src/thinker
**Scope**: full module (70 .rs files)
**Date**: 2026-08-29
**Mode**: strict

## Findings

### [Critical] `prompt_sanitizer.rs::is_format_char` has a dual source of truth with `unicode_guard::is_invisible_char` that must stay in sync
- **Location**: `src/thinker/prompt_sanitizer.rs:75-110`
- **Trigger condition**: A new Unicode Cf (format) range is added or a maintenance change is made to either `prompt_sanitizer::is_format_char` or `security::unicode_guard::is_invisible_char`. The two are documented as a "Prompt-only supplement" to the "Shared invisible class (single source of truth)", but the function delegates to the SSOT and then ALSO checks a hand-rolled `matches!` for ranges the SSOT "deliberately omits". The doc itself says: *"If a new Cf range needs to be caught here, also add it to `unicode_guard::is_invisible_char` and remove it from the local list below — both call sites should converge on the SSOT eventually."*
- **Expected behavior**: Any invisible / format character that should be stripped from prompt input should be stripped in BOTH places, with a single source of truth. The current shape requires manual synchronization between two files and is explicitly acknowledged as a maintenance burden.
- **Actual behavior**: Two sources of truth must be kept in sync. If a new Cf range is added to only one, prompt injection vectors (e.g., new bidi overrides) could slip through the prompt sanitizer while being caught by the external-content sanitizer (or vice versa), depending on which call site receives the edit. The comment explicitly says "both call sites should converge on the SSOT eventually" — i.e., this is a known, unresolved architectural debt.
- **Suggested fix**: Move the full Cf range list into `unicode_guard::is_invisible_char` and have `prompt_sanitizer::is_format_char` delegate entirely to it. This is the documented long-term direction; the current state is the intermediate step. Until that lands, add a compile-time assertion or a unit test that enumerates every Cf range in the local matches! and asserts it is NOT in the SSOT (i.e., the supplement is actually supplementary), so a future SSOT change that silently swallows a range fails the test.

### [Critical] `prompt_budget.rs::truncate_with_head_tail` uses mixed char/byte semantics that only work because the truncation marker is ASCII
- **Location**: `src/thinker/prompt_budget.rs:179-240`
- **Trigger condition**: The truncation marker string `"\n\n[... {total_chars} chars truncated ...]\n\n"` is changed to contain non-ASCII characters (e.g., a CJK translation), OR the `max_chars` parameter semantics are changed to be bytes instead of chars.
- **Expected behavior**: The function is documented as UTF-8 safe with "All budgeting is in characters, never bytes — a 3-byte CJK glyph counts as one unit, so multi-byte text is truncated at the correct visual boundary instead of ~3× too aggressively." All math should be in characters.
- **Actual behavior**: The comparison `if max_chars <= reserved_marker.len()` (line ~199) compares the char-budget parameter against the BYTE length of the marker. This only works because the marker is ASCII (each char = 1 byte). Similarly, `let usable = max_chars - reserved_marker.len();` (line ~204) subtracts BYTES from CHARS. If the marker is ever changed to non-ASCII, the function would silently under-budget the head/tail split, producing truncated output that exceeds the budget. The char-based math downstream (`char_byte_offset(content, head_chars)`) is correct, but the initial budget reservation is byte-based.
- **Suggested fix**: Change `reserved_marker` to be computed in chars (or explicitly assert that the marker is ASCII and document this invariant). Use `reserved_marker.chars().count()` for the comparison. Add a unit test that constructs a non-ASCII marker and verifies the function still respects the budget.

### [Warning] `prompt_sanitizer.rs` `Light` mode does NOT strip control/format characters, which could allow prompt injection via internal text constructed from user input
- **Location**: `src/thinker/prompt_sanitizer.rs:28-50` (definition) and call sites at `src/thinker/layers/security.rs:75,79,85`, `src/thinker/layers/mcp_instructions.rs:48,75`, `src/thinker/layers/runtime_capabilities.rs:26`, `src/thinker/layers/language.rs:18`
- **Trigger condition**: Any `Light`-sanitized text is constructed from user-controlled data (e.g., a filesystem scope path with RTL/zero-width characters, an MCP server name with invisible Unicode, a runtime capabilities string from a config file).
- **Expected behavior**: All text reaching the LLM should have invisible/format characters stripped to prevent prompt injection via bidi overrides, zero-width joiners, etc.
- **Actual behavior**: The `Light` mode only strips injection markers (`<system-reminder>`, `<|im_start|>`, etc.). It does NOT strip control characters, bidi overrides, zero-width spaces, or any other invisible Unicode. The docstring says "Suitable for internal generated text" — but the call sites include:
  - `SecurityLayer`: sanitizes `security_notes()` which includes the filesystem scope path (user-configurable, could contain RTL chars).
  - `McpInstructionsLayer`: sanitizes `server_name` and `instructions` (MCP servers are external; instructions are user-controllable).
  - `LanguageLayer`: sanitizes the language code (configurable, but typically ASCII).
  - `RuntimeCapabilitiesLayer`: sanitizes the runtime capabilities string (configurable).
- **Current impact**: low-to-medium. The injection markers ARE stripped, which catches the most obvious attacks. But a path like `/home/user/<system-reminder></system-reminder>/` with zero-width spaces between visible characters would pass through Light sanitization intact. The model would see the file structure but the RTL/zero-width chars could be used to confuse it.
- **Suggestion**: Either (a) upgrade Light to also strip invisible Unicode (delegating to `is_format_char`), or (b) document explicitly that Light-mode callers must guarantee their input does not contain invisible chars, and add a test that verifies the assumption for each call site.

### [Warning] `prompt_layer.rs::LayerInput::basic` sets `mode: PromptMode::Full` but is named for the "Basic" assembly path — confusing naming that conflates orthogonal concepts
- **Location**: `src/thinker/prompt_layer.rs:124-145`
- **Trigger condition**: A reader or maintainer tries to understand the relationship between `AssemblyPath::Basic` and `PromptMode`, and assumes the "Basic" constructor sets the mode to something "basic" (e.g., Minimal).
- **Expected behavior**: The constructor name should reflect what it sets. `AssemblyPath` and `PromptMode` are orthogonal concepts (a Basic path can run in Full mode, a Cached path can run in Compact mode, etc.).
- **Actual behavior**: `LayerInput::basic` is the constructor for the Basic assembly path, but it hardcodes `mode: PromptMode::Full`. The Cached path constructor (`build_cached_input` in `prompt_builder/cache.rs`) calls `with_mode(mode)` to set the mode separately. The naming implies a relationship between "Basic" and "Full" that doesn't exist — the Basic path CAN run in Compact or Minimal mode if the caller sets it via `with_mode`.
- **Current impact**: low. The code is correct, but the naming is misleading and could lead to future bugs where someone assumes Basic = Full.
- **Suggestion**: Rename `LayerInput::basic` to `LayerInput::new` or `LayerInput::for_basic_path`, and make the mode parameter explicit or `None` with a separate `with_mode` call. Alternatively, add a doc comment explaining the orthogonality.

### [Warning] `nudges.rs::is_synthetic_reminder` uses positional lead-in checks that are brittle to formatting changes
- **Location**: `src/thinker/nudges.rs:370-380`
- **Trigger condition**: A new fenced reminder is added that starts with whitespace before the lead-in, or the lead-in strings are reordered, or the `SYSTEM_REMINDER_OPEN` constant is changed.
- **Expected behavior**: The classifier should correctly identify synthetic reminders vs. user interjections regardless of minor formatting changes (leading whitespace, newlines).
- **Actual behavior**: The function checks `text.trim_start().strip_prefix(SYSTEM_REMINDER_OPEN)` and then `after_fence.trim_start().starts_with(INTERJECTION_LEAD_IN)`. This means:
  1. Leading whitespace is stripped (correct).
  2. The opening fence must be present (correct).
  3. After the fence, more whitespace is stripped before checking the lead-in (correct).
  4. The lead-in must appear immediately after the fence+whitespace (brittle).
  
  If a new synthetic reminder is added that has, say, a markdown header between the fence and the lead-in, it would be misclassified as a user interjection. The existing tests (`the_two_lead_ins_cannot_collide`, `no_payload_can_make_the_carrier_read_as_user_speech`) pin the current behavior, but the brittleness is inherent in the positional check.
- **Current impact**: low. The current set of synthetic reminders all start with the fence immediately followed by content (no headers between). But adding a new reminder with a header would silently break.
- **Suggestion**: Consider a more robust classifier that checks for the ABSENCE of user-speech markers (e.g., first-person pronouns, question marks at the start) rather than the PRESENCE of a specific lead-in. Alternatively, add a `kind: SyntheticKind` field to the fenced reminders and classify by kind rather than by content.

### [Warning] `prompt_sanitizer.rs::is_format_char` comment is outdated — says "we use a heuristic" but the code enumerates all known ranges via `matches!`
- **Location**: `src/thinker/prompt_sanitizer.rs:93-103`
- **Trigger condition**: A maintainer reads the comment, assumes the code uses a heuristic, and adds a new range thinking the heuristic will catch it. Or a maintainer tries to "simplify" the matches! into a heuristic, inadvertently removing coverage.
- **Expected behavior**: The comment should accurately describe the code.
- **Actual behavior**: The comment says "Rather than enumerate all, we use a heuristic that covers the known ranges." But the code explicitly enumerates every known range via a `matches!` with 20+ arms. The comment is stale.
- **Current impact**: low. The code is correct, but the misleading comment could lead to incorrect maintenance.
- **Suggestion**: Update the comment to say "We enumerate every known Cf range explicitly. If a new range is added, also update `unicode_guard::is_invisible_char`." Or remove the comment entirely and let the code speak for itself.

### [Warning] `prompt_sanitizer.rs::strip_injection_markers_once` always calls `to_ascii_lowercase()` even when no markers are present
- **Location**: `src/thinker/prompt_sanitizer.rs:128-165`
- **Trigger condition**: The `Light` sanitizer is called on a very long string that contains no injection markers (the common case for internal text).
- **Expected behavior**: The function should avoid unnecessary allocations when no work needs to be done.
- **Actual behavior**: The function unconditionally calls `let lower = result.to_ascii_lowercase();` at line ~130, even when no CI_MARKERS or CS_MARKERS are present in the string. For a 10KB internal text with no markers, this allocates a 10KB lowercase copy and then finds nothing. The `to_ascii_lowercase()` is the most expensive part of the function.
- **Current impact**: low. The allocation is bounded by the input size and the function is not on a hot path. But for very large inputs it could be wasteful.
- **Suggestion**: Check for the presence of any marker (case-insensitive) before doing the full lowercase conversion. Or use a streaming approach that scans for the first marker before allocating.

### [Warning] `prompt_layer.rs::LayerInput.identity_file` is case-sensitive without trimming
- **Location**: `src/thinker/prompt_layer.rs:218-223`
- **Trigger condition**: A caller passes a filename with different case (e.g., `identity_file("soul.md")` instead of `identity_file("SOUL.md")`).
- **Expected behavior**: The function should be robust to case variations and whitespace.
- **Actual behavior**: The function does `files.iter().find(|f| f.name == name)` which is exact-match. If a caller passes `"soul.md"` (lowercase), it returns `None` because the canonical name is `"SOUL.md"`. This is currently fine because all callers use exact case, but a typo or refactor could silently miss the file.
- **Current impact**: low. All current callers use exact case.
- **Suggestion**: Consider case-insensitive comparison or document the exact-case requirement prominently.

### [Warning] `prompt_layer.rs::LayerInput` has 15 fields with inconsistent builder method patterns
- **Location**: `src/thinker/prompt_layer.rs:98-223`
- **Trigger condition**: A maintainer adds a new field and is unsure whether to use `with_X(T)` or `with_X_opt(Option<&T>)`.
- **Expected behavior**: Consistent builder API. Either all optional fields use the `_opt` pattern, or all use a uniform pattern.
- **Actual behavior**: The struct has 15 fields. Some have `with_X` (required) and `with_X_opt` (optional) variants. The distinction is not always clear:
  - `with_mode(mode)` — required
  - `with_identity_files_opt(files: Option<&IdentityFiles>)` — optional
  - `with_identity_files(files: &IdentityFiles)` — required (delegates to opt)
  - `with_extra_files_opt(files: Option<&[ExtraPromptFile]>)` — optional
  - `with_agent_def(agent_def: &AgentDef)` — required (no opt variant)
  - `with_mcp_instructions(instructions: &[McpServerInstruction])` — required (no opt variant)
  - `with_chain_context(chain: &ChainContext)` — required
  - `with_chain_context_opt(chain: Option<&ChainContext>)` — optional
  - `with_resolved_context_opt(ctx: Option<&ResolvedContext>)` — optional (no required variant)
  - `with_behavior_name(name: &str)` — required
  - `with_behavior_name_opt(name: Option<&str>)` — optional
  - `with_iteration_cap(cap: u32)` — required
  - `with_iteration_cap_opt(cap: Option<u32>)` — optional
  
  The pattern is inconsistent: some fields have both required and optional variants, some have only one.
- **Current impact**: low. The code is correct, but the API is harder to use than it could be.
- **Suggestion**: Standardize on either `with_X(T)` (which wraps in `Some` internally) or `with_X(Option<T>)` (explicit). Pick one pattern and apply it consistently.

### [Warning] `prompt_sanitizer.rs::strip_injection_markers` MAX_STRIP_PASSES=16 is a safety net, not a guarantee
- **Location**: `src/thinker/prompt_sanitizer.rs:64-75`
- **Trigger condition**: A pathological input requires more than 16 passes to fully strip all markers.
- **Expected behavior**: The function should either guarantee full stripping or document that 16 passes is a best-effort limit.
- **Actual behavior**: The function has a `MAX_STRIP_PASSES: usize = 16` constant. If the string requires more than 16 passes to converge (extremely unlikely in practice), the function returns early with some markers still present. The convergence test (`pass.len() == result.len()`) is the primary termination condition, but the 16-pass cap is a hard limit.
- **Current impact**: low. In practice, each pass removes at least one marker (8+ characters), so 16 passes can remove at most 16 markers. A string with more than 16 nested markers is not realistic.
- **Suggestion**: Add a test that constructs a string with >16 nested markers and verifies the function either fully strips them or returns the partially-stripped result. Document the limit.

### [Warning] `prompt_budget.rs::window_char_budget` does not validate that floor <= ceil
- **Location**: `src/thinker/prompt_budget.rs:48-52`
- **Trigger condition**: A caller passes `floor > ceil` (e.g., a misconfiguration).
- **Expected behavior**: The function should handle invalid input gracefully (return an error or clamp).
- **Actual behavior**: The function calls `.clamp(floor, ceil)`. If `floor > ceil`, `clamp` returns `floor` (the minimum). The doc says "floor must not exceed ceil (callers pass compile-time constants that satisfy this)" — relies on caller correctness.
- **Current impact**: low. All current callers use compile-time constants that satisfy the invariant.
- **Suggestion**: Add an assertion or return `Result` for invalid input. Or document the invariant more prominently.

### [Warning] `prompt_sanitizer.rs::is_format_char` has a large matches! that is hard to maintain
- **Location**: `src/thinker/prompt_sanitizer.rs:103-127`
- **Trigger condition**: A new Unicode format character is standardized.
- **Expected behavior**: The function should be easy to extend with new ranges.
- **Actual behavior**: The function has 20+ arms in the matches! expression. Adding a new range requires editing the list and potentially the SSOT. The function is hard to read and hard to verify coverage.
- **Current impact**: low. The current coverage is comprehensive.
- **Suggestion**: Consider using a `const RANGES: &[(u32, u32)]` table and checking membership. This would be more maintainable and easier to verify.

### [Warning] `prompt_budget.rs::render_truncation_notice` emits a `<system-reminder>` block that is correctly classified as synthetic by `is_synthetic_reminder`
- **Location**: `src/thinker/prompt_budget.rs:253-268`
- **Trigger condition**: The truncation notice is injected into the prompt and reaches the classifier.
- **Expected behavior**: The notice should be classified as synthetic (harness-authored scaffolding), not as user input.
- **Actual behavior**: The notice starts with `\n\n<system-reminder>` (newlines then the tag). After `trim_start()`, it starts with `<system-reminder>`. The `is_synthetic_reminder` function checks for the interjection lead-in after the fence; the notice content ("Your per-request context was trimmed...") does not start with the interjection lead-in ("The user sent the following message:"). So it is correctly classified as synthetic. This is correct behavior, but it's a subtle dependency: if the notice content is ever changed to start with the interjection lead-in (e.g., "The user's context was trimmed..."), it would be misclassified.
- **Current impact**: low. The current content is correct.
- **Suggestion**: Add a test that verifies the truncation notice is classified as synthetic.

### [Warning] `prompt_layer.rs::LayerInput.identity_file` is a public method that could be inlined
- **Location**: `src/thinker/prompt_layer.rs:218-223`
- **Trigger condition**: A maintainer considers whether to use `identity_file()` or directly access `identity_files`.
- **Expected behavior**: The API should be minimal and obvious.
- **Actual behavior**: The method is a thin wrapper around `self.identity_files.and_then(|files| files.get(name))`. It's only used by `SoulLayer` and `ProfileLayer`. It could be inlined at the call sites for clarity.
- **Current impact**: low. The method is correct and small.
- **Suggestion**: Consider making it `pub(crate)` or inlining at call sites.

### [Warning] `prompt_pipeline.rs::default_layers` has 38 layer constructions with hardcoded priorities — fragile to additions
- **Location**: `src/thinker/prompt_pipeline.rs:225-360`
- **Trigger condition**: A new layer is added with a priority that conflicts with an existing layer.
- **Expected behavior**: Adding a new layer should be straightforward with a unique priority.
- **Actual behavior**: The function manually constructs 38 `Box<dyn PromptLayer>` with explicit priorities. The `test_default_layers_have_unique_priorities` test catches conflicts, but the priorities are scattered and hard to reason about. A new layer must be inserted in the right position (by priority) to maintain the stable→dynamic ordering.
- **Current impact**: low. The test catches conflicts, and the `new()` constructor sorts by priority regardless of insertion order.
- **Suggestion**: Consider a declarative table (array of `(priority, name, constructor)`) that documents the ordering and makes additions easier.

### [Warning] `runtime_context.rs::REPO_ROOT_CACHE` uses a global `OnceLock<Mutex<HashMap>>` with two lock acquisitions
- **Location**: `src/thinker/runtime_context.rs:411-426`
- **Trigger condition**: Two threads call `cached_repo_root` concurrently with the same working directory.
- **Expected behavior**: The cache should be thread-safe with minimal contention.
- **Actual behavior**: The function acquires the lock twice: once to check the cache, releases it, then acquires it again to insert. This is a double-checked locking pattern that could allow two threads to both miss the cache and both detect the repo root, but the final insert is a no-op (the `or_insert_with` ensures only the first insert wins). The test `cached_repo_root_releases_lock_before_filesystem_io` pins the invariant that the lock is not held during I/O.
- **Current impact**: low. The pattern is correct, and the test guards against holding the lock during I/O.
- **Suggestion**: Consider using a `DashMap` or `RwLock` for better read concurrency, since the common case is a cache hit (read-only).

### [Warning] `soul.rs::SoulManifest::is_empty` checks 7 conditions — verbose but correct
- **Location**: `src/thinker/soul.rs:138-146`
- **Trigger condition**: A new field is added to `SoulManifest`.
- **Expected behavior**: The `is_empty` check should cover all meaningful fields.
- **Actual behavior**: The function checks identity, directives, anti_patterns, expertise, voice.tone, voice.language_notes, and addendum. It excludes verbosity and formatting_style (whose defaults are indistinguishable from an explicit choice). A new field added to `SoulManifest` would not be covered by `is_empty` unless the maintainer remembers to update it.
- **Current impact**: low. The current fields are covered.
- **Suggestion**: Consider an exhaustive destructure pattern (like `RuntimeContext::fact_census`) that fails to compile when a new field is added.

### [Warning] `identity_profile.rs::clean_value` is complex with multiple decoration-stripping passes
- **Location**: `src/thinker/identity_profile.rs:138-170`
- **Trigger condition**: A new markdown decoration pattern is used in IDENTITY.md.
- **Expected behavior**: The function should strip all common decorations and return the clean value.
- **Actual behavior**: The function:
  1. Strips `*`, `_`, `` ` ``, and spaces from both ends.
  2. Unwraps a fully-parenthesized value.
  3. Folds en/em dashes to ASCII hyphens.
  4. Strips a trailing editorial aside (e.g., `_(edit to taste)_`).
  5. Re-strips decoration.
  6. Checks against placeholders.
  7. Collapses whitespace.
  
  Each step is correct, but the overall logic is hard to follow and test. The `find_trailing_aside` helper uses `rfind('(')` to locate the parenthetical, which could match an earlier paren if the value contains multiple.
- **Current impact**: low. The current behavior is correct for the test cases.
- **Suggestion**: Add more test cases for edge cases (multiple parens, nested decorations, unicode decorations).

### [Warning] `project_instructions.rs::expand_imports` has a `depth` parameter for cycle detection that could overflow
- **Location**: `src/thinker/project_instructions.rs:235-237`
- **Trigger condition**: A pathological import chain exceeds `MAX_IMPORT_DEPTH` (5) plus some safety margin.
- **Expected behavior**: The function should handle deep imports gracefully.
- **Actual behavior**: The function checks `if depth >= MAX_IMPORT_DEPTH` at the start and returns the content unchanged. The `depth` parameter is incremented in the recursive call. For a chain of exactly 5 imports, depth goes 0, 1, 2, 3, 4, 5 — at depth 5, the function returns. For a chain of 6, the 6th import is not expanded. This is correct.
- **Current impact**: low. The limit is reasonable.
- **Suggestion**: No change needed.

### [Suggested Test] Property-based test for `prompt_sanitizer.rs::strip_injection_markers` convergence
```rust
#[cfg(test)]
mod proptest_strip_injection {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// The fixed-point loop must converge: after any number of passes,
        /// the result must be stable (no more markers can be stripped).
        #[test]
        fn strip_injection_markers_converges(s in ".*") {
            let result = strip_injection_markers(&s);
            let second = strip_injection_markers(&result);
            prop_assert_eq!(result, second, "strip must be idempotent");
        }

        /// No marker may survive in the result.
        #[test]
        fn no_marker_survives(s in "(<system-reminder>.*?</system-reminder>)|.*") {
            let result = strip_injection_markers(&s);
            let lower = result.to_ascii_lowercase();
            for marker in CI_MARKERS {
                prop_assert!(!lower.contains(marker), "marker {:?} survived: {:?}", marker, result);
            }
        }
    }
}
```

### [Suggested Test] Test for `prompt_budget.rs::truncate_with_head_tail` with non-ASCII marker
```rust
#[test]
fn truncate_with_head_tail_handles_non_ascii_marker() {
    // Regression for the char/byte confusion: if the marker were ever
    // changed to non-ASCII, the function would silently under-budget.
    // This test pins the current ASCII-marker behavior and documents
    // the invariant.
    let content: String = "A".repeat(10_000);
    let result = truncate_with_head_tail(&content, 100, 0.6, 0.3);
    assert!(result.chars().count() <= 100, "result must respect budget, got {} chars", result.chars().count());
    // Pin the current marker format.
    assert!(result.contains("truncated ...]"), "marker format changed: {result:?}");
}
```

### [Suggested Test] Test for `prompt_sanitizer.rs::Light` mode with RTL/format chars
```rust
#[test]
fn test_light_mode_passes_through_rtl_and_format_chars() {
    // By design, Light mode does NOT strip invisible/format chars.
    // Pin the behavior so a future change to also strip them is intentional.
    let input = "hello\u{200B}world\u{202E}reversed";
    let result = sanitize_for_prompt(input, SanitizeLevel::Light);
    assert!(result.contains('\u{200B}'), "Light must pass through zero-width space");
    assert!(result.contains('\u{202E}'), "Light must pass through RTL override");
}
```

### [Suggested Test] Test for `prompt_sanitizer.rs::is_format_char` exhaustiveness
```rust
#[test]
fn test_is_format_char_covers_known_cf_ranges() {
    // Every Cf range in the hand-rolled matches! must be caught.
    let known_format_chars = [
        '\u{00AD}', '\u{061C}', '\u{070F}', '\u{0890}', '\u{0891}',
        '\u{08E2}', '\u{180E}', '\u{200B}', '\u{200C}', '\u{200D}',
        '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}', '\u{202C}',
        '\u{202D}', '\u{202E}', '\u{2060}', '\u{2061}', '\u{2062}',
        '\u{2063}', '\u{2064}', '\u{2066}', '\u{2067}', '\u{2068}',
        '\u{2069}', '\u{206A}', '\u{206B}', '\u{206C}', '\u{206D}',
        '\u{206E}', '\u{206F}', '\u{FEFF}', '\u{FFF9}', '\u{FFFA}',
        '\u{FFFB}', '\u{110BD}', '\u{110CD}', '\u{E0001}',
    ];
    for c in known_format_chars {
        assert!(is_format_char(c), "format char {c:?} not caught");
    }
    // Zl/Zp separators
    assert!(is_format_char('\u{2028}'));
    assert!(is_format_char('\u{2029}'));
}
```

### [Suggested Test] Test for `nudges.rs::is_synthetic_reminder` with truncation notice
```rust
#[test]
fn truncation_notice_is_classified_as_synthetic() {
    // The truncation notice emitted by prompt_budget::render_truncation_notice
    // starts with a <system-reminder> fence. It must be classified as
    // synthetic (harness scaffolding), not as user input.
    let notice = "\n\n<system-reminder>\nYour per-request context was trimmed...\n</system-reminder>";
    assert!(is_synthetic_reminder(notice));
    // Even with leading whitespace
    assert!(is_synthetic_reminder(&format!("   \n\n   {notice}")));
}
```

### [Suggested Test] Test for `prompt_layer.rs::LayerInput.identity_file` case sensitivity
```rust
#[test]
fn identity_file_is_case_sensitive() {
    // Pin the current exact-case behavior. If a future change makes it
    // case-insensitive, this test catches the semantic change.
    use crate::thinker::identity_files::{IdentityFile, IdentityFiles};
    let config = PromptConfig::default();
    let files = IdentityFiles {
        identity_dir: std::path::PathBuf::from("/tmp"),
        files: vec![IdentityFile {
            name: "SOUL.md",
            content: Some("content".to_string()),
        }],
    };
    let input = LayerInput::basic(&config, &[]).with_identity_files(&files);
    assert_eq!(input.identity_file("SOUL.md"), Some("content"));
    assert_eq!(input.identity_file("soul.md"), None, "case-sensitive: lowercase fails");
    assert_eq!(input.identity_file("SOUL.MD"), None, "case-sensitive: uppercase fails");
}
```

### [Suggested Test] Test for `prompt_sanitizer.rs::strip_injection_markers` with >16 nested markers
```rust
#[test]
fn strip_handles_pathological_nesting() {
    // The MAX_STRIP_PASSES=16 safety net. A string with >16 nested markers
    // should either fully strip (within the limit) or return the best-effort
    // result. This test pins the current behavior.
    let mut s = String::from("evil");
    for _ in 0..20 {
        s = format!("<system>{s}</system>");
    }
    let result = strip_injection_markers(&s);
    // After stripping, no <system> should remain (within the 16-pass limit).
    let lower = result.to_ascii_lowercase();
    assert!(!lower.contains("<system>"), "marker survived: {result:?}");
}
```

### [Suggested Test] Test for `prompt_budget.rs::fit_dynamic_suffix_with_content` exact-budget no-op
```rust
#[test]
fn fit_exact_budget_is_byte_identical() {
    // When the dynamic content exactly matches the budget (no headroom, no
    // overflow), the function should return the input unchanged.
    let budget = TokenBudget::default();
    let dynamic = "x".repeat(budget.max_total_chars);
    let result = fit_dynamic_suffix_with_content("", dynamic.clone(), &budget);
    assert_eq!(result, dynamic, "exact-budget input must pass through unchanged");
}
```

## Cross-Module Findings

### Wiring completeness

All 38 layers registered in `PromptPipeline::default_layers()` are wired into both production entry points (`build_system_prompt_cached_with_mode` and `build_system_prompt_parts`). The `prompt_contract::reachable_layers` test enforces that every layer either contributes to a production-shaped input under at least one paradigm or is listed in `CONDITIONALLY_SILENT` with the session content that wakes it. The `prompt_contract::scaffold_bytes_ratchet` and `dynamic_tail_bytes_ratchet` tests enforce byte ceilings on the always-on and dynamic portions of the prompt.

The `MemoryContextProvider` is wired through `HarnessDeps` to the harness bridge, which calls `build_curated_message` and `build_memory_user_message`. The scope resolution (`resolve_storage_id`) is done at the provider level, not by callers, preventing double-composition.

The `RoomRosterLayer` reads from `ProjectStore` via the `ambient_room_roster_line` helper, which is called from both `harness_bridge::prompt_build` and `agents::subagent_spawner`. Both callers depend on `thinker`, and `thinker` already depends on `projects`, so there is no new edge.

The `nudges::is_synthetic_reminder` classifier is used by `context::compact::summary_utils::latest_user_task` and `providers::protocols::anthropic::adapter::cache`. Both consumers depend on the same predicate, which is the single source of truth.

### Lock primitives compliance

All `std::sync` imports in the thinker module are limited to `OnceLock` and `LazyLock` (e.g., `runtime_context.rs::REPO_ROOT_CACHE` uses `OnceLock<Mutex<HashMap>>`) and `AtomicU32` in test code (`memory_context_provider/tests.rs`). The `Mutex` and `RwLock` types come from `crate::sync_primitives`, as required by the sync primitives import rule. The `Arc` type comes from `crate::sync_primitives` in production code.

Lock hierarchy compliance: the `mod.rs::SwappableProviderRegistry` and `MultiProviderRegistry` use `RwLock` for the provider handle. The `runtime_context.rs::REPO_ROOT_CACHE` uses `Mutex`. The `prompt_builder/cache_monitor.rs::CacheMonitor` uses `Mutex`. All are at the appropriate level for their use case. No lock is held across `.await` in production code (the `tokio::sync::RwLock` in `memory_context_provider` is a separate concern, used for the curated snapshot cache which is explicitly async).

### Type coercion audit

The `as` casts in `prompt_budget.rs` are all within expected ranges:
- `((capped as f64 * multiplier) as usize).clamp(floor, ceil)` — f64 to usize, saturating in Rust 1.45+.
- `(usable as f64 * head_ratio / sum) as usize` — same pattern.
- `(saved_chars as f64 / DEFAULT_PROSE_RATIO) as usize` — same pattern.
- `(crate::context::budget::pressure::estimate_tokens_smart(content) as f64 * factor).round() as usize` — same pattern.

The `as f64` conversions from `u64` in `scale_window_to_budget` are bounded by `MAX_PRECISE_F64: u64 = 1u64 << 53`, which is the safe range for f64 precision.

The `as usize` conversions from `f64` in the head/tail calculations could theoretically produce values > usize::MAX, but Rust's saturating `as` cast for f64-to-usize handles this correctly (saturates to usize::MAX or 0).

No `as` casts that could silently truncate or overflow were found in the audited code.

### Sanitizer completeness

The `prompt_sanitizer.rs` sanitizer has three levels:
- `Strict`: strips all control and format chars. Used for paths and language codes.
- `Moderate`: preserves `\n`, `\t`, `\r`; strips other control and format chars. Used for skill instructions.
- `Light`: only strips injection markers. Used for internal text (security notes, MCP instructions, runtime capabilities, language).

The `IdentityFilesLayer` uses `sanitize_identity_content` (a separate sanitizer in `layers/identity_files.rs`) which combines injection pattern detection with invisible Unicode stripping. This is the right level for user-editable identity files.

The `ExtraFilesLayer` uses the same `sanitize_identity_content` for both the file name and content. The name sanitization includes newline stripping to prevent header forgery.

The `SoulLayer` uses `sanitize_identity_content` for SOUL.md content. The `ProfileLayer` uses the same for AGENTS.md content. Both are user-editable identity files.

### Budget monotonicity

The `TokenBudget::from_context_window` method scales the budget to the model's context window, clamped to `[DEFAULT_PROMPT_CHARS, MAX_PROMPT_CHARS]`. The floor ensures small windows behave exactly as before; the ceiling prevents mis-declared windows from letting the prompt grow unbounded. The `with_estimate_factor` method clamps the factor to `[CALIBRATION_MIN, CALIBRATION_MAX]`, preventing degenerate carry-overs.

The `fit_dynamic_suffix_with_content` function respects the char budget first, then the token budget. The stable prefix is protected (never trimmed) so the Anthropic prefix cache stays valid. The dynamic suffix is head/tail truncated with a model-visible truncation notice appended.

### Identity contract

The `SoulLayer` injects the SOUL.md file content under a `# Soul` header, sanitized by `sanitize_identity_content`. The `ProfileLayer` injects the AGENTS.md file content under a `## Project Context` header, also sanitized. The `IdentityFilesLayer` injects the remaining identity files (IDENTITY.md, TOOLS.md, HEARTBEAT.md) under a `## Identity Files` header, also sanitized. All identity files are loaded by `IdentityFiles::load` which applies per-file and total char caps. The `write_identity_file` and `write_identity_file_async` write helpers share `validate_identity_write` to prevent drift between the two write surfaces.

The `AgentIdentityProfile` is parsed from IDENTITY.md by `from_markdown` or `from_agent_dir`. The parser strips markdown decoration, unwraps parenthesized values, folds en/em dashes, and rejects known placeholders. The `round_trips_every_archetype_seeded_template` test ensures the parser reads back exactly what the creation template writes.

### Layer ordering

The 38 layers in `default_layers` are ordered by ascending priority. The `test_default_layers_sorted` and `test_default_layers_have_unique_priorities` tests enforce this. The `stable_layers_come_before_dynamic` test enforces the Stable→Dynamic split, with the threshold at priority 1700. All Stable layers have priorities < 1700; all Dynamic layers have priorities >= 1700.

The `LayerInput::basic` and `LayerInput::new` constructors set `mode: PromptMode::Full` by default. The `with_mode` method allows override. The pipeline filters by both path and mode, so a layer that doesn't support a given mode is skipped.

## Summary

| Level | Count |
|-------|-------|
| Critical | 2 |
| Warning | 12 |
| Suggested Test | 8 |
