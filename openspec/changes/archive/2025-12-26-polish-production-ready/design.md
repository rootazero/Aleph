# Design Document: polish-production-ready

## Overview

This document explains the architectural decisions for Phase 7: Polish & Optimization. The design balances production quality, user experience, privacy protection, and performance while maintaining Aether's core principles of minimal friction and native-first architecture.

## Architectural Principles

### 1. Privacy-First Design

**Decision**: All logging and profiling data remains local; PII is scrubbed before any storage.

**Rationale**:
- Aether's value proposition includes "zero telemetry" and "local-first"
- Users trust Aether with sensitive clipboard content (passwords, personal info, confidential documents)
- Any data leakage violates this trust and project philosophy

**Implementation**:
- Reuse `scrub_pii()` function from memory module for log scrubbing
- Apply scrubbing before writing to log files (not just before cloud API calls)
- Test coverage: Unit tests verify PII patterns are removed from logs
- User control: Logs can be cleared via Settings UI

**Trade-offs**:
- Cannot implement remote crash reporting or analytics
- Debugging production issues requires manual log export from users
- Accepted: Privacy > convenience for debugging

---

### 2. Graceful Degradation Over Hard Failures

**Decision**: When optional features fail (typewriter, image support), fall back to core functionality instead of blocking the user.

**Rationale**:
- User's primary goal is AI-assisted text transformation, not fancy animations
- System should be resilient to permission issues, resource constraints, format incompatibilities
- Errors should guide recovery, not frustrate users

**Implementation Examples**:

**Typewriter Animation Fallback:**
- If keyboard simulation fails → paste via clipboard (instant mode)
- If Escape pressed → skip animation, paste remaining text
- User sees complete AI response regardless of animation success

**Image Clipboard Fallback:**
- If image too large → error with resize suggestion, retry with text only
- If format unsupported → error with conversion suggestion
- If vision model unavailable → error with provider switch suggestion

**Memory Module Fallback:**
- If database locked → proceed without memory augmentation, warn user
- If embedding fails → store text without embedding, skip retrieval

**Benefits**:
- Increases user tolerance for edge cases
- Reduces support burden (fewer "broken" reports)
- Maintains core value proposition even when enhancements fail

---

### 3. Configuration Over Code Changes

**Decision**: Make all Phase 7 features configurable rather than hardcoded, with sensible defaults.

**Rationale**:
- Power users want control (typing speed, log retention, profiling)
- Casual users want zero-config experience
- Different use cases have different needs (fast typing for code, slow for reading)

**Configuration Schema Extensions**:

```toml
[behavior]
output_mode = "typewriter"        # instant | typewriter
typing_speed = 50                 # 10-200 chars/second
auto_compress_images = true       # Auto-resize large images

[logging]
log_level = "info"                # error | warn | info | debug
log_retention_days = 7            # 1-30 days
enable_performance_logging = false # Opt-in profiling

[images]
max_image_size_mb = 10            # Max clipboard image size
supported_formats = ["png", "jpeg", "gif"]  # Extensible
```

**Benefits**:
- Users can adapt Aether to their workflow
- A/B testing configurations without code changes
- Settings UI provides discoverability (users learn features exist)

---

### 4. Minimal Dependency Additions

**Decision**: Reuse existing crates (`arboard`, `enigo`, `tracing`) rather than adding new dependencies for Phase 7 features.

**Rationale**:
- Aether's binary size and build time should remain small
- Fewer dependencies = fewer supply chain risks
- Existing crates already provide needed functionality

**Dependency Audit**:

| Feature | Crate | Status | Justification |
|---------|-------|--------|---------------|
| Image clipboard | `arboard` | ✅ Existing | Already used for text clipboard |
| Typewriter output | `enigo` | ✅ Existing | Already used for paste simulation |
| Structured logging | `tracing` | ✅ Existing | Partially integrated in Phase 5 |
| Log rotation | `tracing-appender` | ➕ New | Official tracing crate, minimal (~50KB) |
| Image encoding | `base64` | ➕ New | Tiny crate (~20KB), no-std compatible |
| Image metadata | `image` crate | ❌ Rejected | Too heavy (2MB+), use arboard directly |

**Total New Dependencies**: 2 small crates (~70KB combined)

---

### 5. UniFFI as Single Source of Truth for Rust↔Swift Interface

**Decision**: All Phase 7 features exposed to Swift go through UniFFI definitions, no manual FFI.

**Rationale**:
- UniFFI prevents memory safety bugs at FFI boundary
- Automatic binding generation reduces boilerplate and errors
- Swift code remains idiomatic (no unsafe pointers)

**UniFFI Extensions for Phase 7**:

```idl
// Image clipboard support
dictionary ImageData {
  sequence<u8> data;
  ImageFormat format;
};

enum ImageFormat {
  "Png",
  "Jpeg",
  "Gif",
};

interface ClipboardManager {
  [Throws=AetherException]
  ImageData? read_image();

  [Throws=AetherException]
  void write_image(ImageData image);
};

// Typewriter callbacks
callback interface AetherEventHandler {
  // Existing callbacks...
  void on_typewriter_progress(f32 percent);
  void on_typewriter_cancelled();
};

// Error suggestions
callback interface AetherEventHandler {
  void on_error(string message, string? suggestion);  // Extended signature
};

// Logging controls
interface AetherCore {
  string get_log_level();
  void set_log_level(string level);
  string get_log_directory();
};
```

**Benefits**:
- Type safety across FFI boundary
- Swift autocomplete for Rust types
- Easy to mock for Swift UI tests

---

## Component Design Decisions

### Image Clipboard Architecture

**Design Choice**: Store images as raw bytes + format enum, not as platform-specific types.

**Rationale**:
- Platform-agnostic representation allows future Windows/Linux support
- Base64 encoding for APIs is uniform across platforms
- No dependency on macOS-specific NSImage

**Data Flow**:

```
macOS Clipboard (NSImage)
    ↓ arboard abstraction
Raw bytes + MIME type detection
    ↓ ImageData struct
Rust processing (size check, validation)
    ↓ Base64 encoding
AI Provider API (OpenAI Vision, Claude)
    ↓ AI Response
Clipboard (if image generation) OR Text output
```

**Alternative Considered**: Store as NSImage in Swift, convert on demand
- **Rejected**: Breaks Rust-first architecture, complicates cross-platform support

---

### Typewriter Output Architecture

**Design Choice**: Character-by-character keyboard simulation, not clipboard manipulation.

**Rationale**:
- Clipboard approach would flicker (paste → select → paste next char)
- Keyboard simulation feels natural (user can watch typing)
- Allows cancellation mid-animation (Escape key)

**Implementation Strategy**:

```rust
async fn typewriter_output(
    text: &str,
    chars_per_second: u32,
    handler: Arc<dyn AetherEventHandler>
) -> Result<()> {
    let delay_per_char = Duration::from_secs(1) / chars_per_second;
    let total_chars = text.chars().count();

    for (i, ch) in text.chars().enumerate() {
        // Check for cancellation (Escape key or new hotkey)
        if cancellation_token.is_cancelled() {
            clipboard_manager.write_text(&text[i..])?;  // Paste remaining
            simulate_paste()?;  // Cmd+V
            handler.on_typewriter_cancelled();
            return Ok(());
        }

        // Type single character
        input_simulator.type_char(ch)?;

        // Progress callback (every 10%)
        if i % (total_chars / 10) == 0 {
            handler.on_typewriter_progress(i as f32 / total_chars as f32);
        }

        tokio::time::sleep(delay_per_char).await;
    }

    handler.on_typewriter_progress(1.0);
    Ok(())
}
```

**Edge Cases**:
- **Unicode handling**: Multi-byte chars count as 1 unit for timing
- **Special characters**: Newlines → Enter key, Tabs → Tab key
- **Modifier keys**: Not supported (paste URLs/code as-is, no auto-formatting)

**Alternative Considered**: Token-by-token streaming from AI
- **Deferred to Phase 8**: Requires SSE streaming APIs, more complex error handling

---

### Structured Logging Architecture

**Design Choice**: Use `tracing` crate with custom layer for PII scrubbing.

**Rationale**:
- `tracing` is Rust ecosystem standard (async-first, structured)
- Allows filtering by target/level without code changes (RUST_LOG env var)
- Composable layers (stdout + file + PII filter)

**Logging Pipeline**:

```
tracing::info!("AI request", provider = "OpenAI", latency_ms = 450)
    ↓ tracing spans/events
EnvFilter layer (check RUST_LOG)
    ↓ if enabled
PII Scrubbing layer (apply scrub_pii to message)
    ↓ scrubbed message
Format layer (timestamp + level + target + message)
    ↓ formatted string
File Appender (with rotation)
    ↓
~/.aether/logs/aether-2025-12-25.log
```

**PII Scrubbing Layer Implementation**:

```rust
struct PiiScrubbingLayer;

impl<S: Subscriber> Layer<S> for PiiScrubbingLayer {
    fn on_event(&self, event: &Event, _ctx: Context<S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        if let Some(message) = visitor.message {
            let scrubbed = scrub_pii(&message);
            // Re-emit event with scrubbed message
        }
    }
}
```

**Alternative Considered**: Manual logging with `log` crate
- **Rejected**: Less structured, no async support, more boilerplate

---

### Error Feedback Architecture

**Design Choice**: Extend `AetherError` with `suggestion` field, not separate suggestion database.

**Rationale**:
- Keeps error and suggestion together (single source of truth)
- No need for i18n in Phase 7 (all suggestions in English)
- Easy to add suggestions to existing error creation sites

**Error Definition**:

```rust
#[derive(Debug, thiserror::Error)]
pub enum AetherError {
    #[error("API authentication failed")]
    ApiKeyInvalid {
        provider: String,
        #[source]
        source: Option<Box<dyn Error>>,
        suggestion: Option<String>,  // Added field
    },

    // ... other variants
}

impl AetherError {
    pub fn api_key_invalid(provider: &str) -> Self {
        Self::ApiKeyInvalid {
            provider: provider.to_string(),
            source: None,
            suggestion: Some(format!(
                "Please verify your {} API key in Settings → Providers → {}",
                provider, provider
            )),
        }
    }
}
```

**Alternative Considered**: Separate `ErrorSuggestion` lookup table
- **Rejected**: Adds indirection, harder to maintain consistency

---

### Performance Profiling Architecture

**Design Choice**: Opt-in instrumentation with compile-time feature flag.

**Rationale**:
- Production binaries should have zero profiling overhead
- Developers and advanced users can enable profiling for diagnostics
- Conditional compilation removes profiling code entirely when disabled

**Feature Flag Usage**:

```toml
# Cargo.toml
[features]
profiling = ["tracing/max_level_debug"]

# Build with profiling
cargo build --features profiling

# Production build (no profiling)
cargo build --release
```

**Instrumentation Pattern**:

```rust
#[cfg(feature = "profiling")]
macro_rules! profile_scope {
    ($name:expr) => {
        let _guard = tracing::debug_span!($name).entered();
    };
}

#[cfg(not(feature = "profiling"))]
macro_rules! profile_scope {
    ($name:expr) => {};
}

// Usage
fn read_clipboard() -> Result<String> {
    profile_scope!("clipboard_read");
    // ... implementation
}
```

**Benefits**:
- Production binaries: 0% overhead
- Development builds: <5% overhead (tracing is fast)
- Users can enable via config without recompiling (if feature enabled)

---

## Data Flow Diagrams

### Complete Pipeline with Phase 7 Enhancements

```
User Action: Cmd+~ (Global Hotkey)
    ↓ [50ms target]
[Performance: hotkey_to_clipboard timer starts]
    ↓
Clipboard Read (text OR image)
    ↓ if image
[Image: Check size < max_image_size_mb]
    ↓ if oversized
[Error: "Image too large" + suggestion to resize]
    ↓ if valid
[Image: Base64 encode]
    ↓
[Performance: clipboard_to_memory timer starts]
    ↓
Memory Retrieval (with app context)
    ↓ [100ms target]
[Memory: Augment prompt with past interactions]
    ↓
[Performance: memory_to_ai timer starts]
    ↓
Router: Select provider based on rules
    ↓ if image
[Router: Filter to vision-capable providers only]
    ↓
AI Provider: Send request (text + image)
    ↓ [500ms target]
[Logging: "AI request sent to OpenAI GPT-4 Vision"]
    ↓
AI Response Received
    ↓
[Logging: "AI response received in 450ms"]
    ↓
[Performance: ai_to_paste timer starts]
    ↓
Output Mode Check
    ↓ if instant
[Clipboard: Write response, Simulate Cmd+V]
    ↓ if typewriter
[Typewriter: Character-by-character typing]
    ↓ with progress callbacks
[Halo: Update progress bar (0% → 100%)]
    ↓ if Escape pressed
[Typewriter: Cancel, paste remaining text]
    ↓
[Performance: paste_complete timer stops]
    ↓
[Logging: "Total pipeline: 850ms"]
    ↓
[Memory: Store interaction asynchronously]
    ↓
Halo: Success animation → Fade out
```

---

## Risk Mitigation Strategies

### Risk 1: Image Support Increases Attack Surface

**Threat**: Malicious images could exploit image parsing vulnerabilities.

**Mitigations**:
1. **Input Validation**: Check magic bytes before passing to arboard
2. **Size Limits**: Enforce max_image_size_mb (default 10MB)
3. **Format Whitelist**: Only PNG, JPEG, GIF (reject BMP, TIFF, etc.)
4. **Dependency Auditing**: Use `cargo audit` to check arboard for CVEs
5. **Sandboxing** (future): Run image processing in separate process

**Accepted Risk**: arboard is widely used and audited; benefits outweigh risks

---

### Risk 2: Typewriter Animation Feels Slow for Large Responses

**Threat**: 1000-char response at 50 cps = 20 seconds (too long for user patience).

**Mitigations**:
1. **Skip Mechanism**: Escape key immediately pastes remaining text
2. **Configurable Speed**: Allow up to 200 cps (1000 chars in 5 seconds)
3. **Smart Truncation**: For >500 char responses, paste first 100 chars, then typewriter
4. **Mode Toggle**: Instant mode available for power users

**Fallback**: If user feedback indicates typewriter is annoying, default to instant mode in Phase 8

---

### Risk 3: Logging Exposes PII Despite Scrubbing

**Threat**: PII scrubbing regex misses novel patterns (e.g., non-US phone formats).

**Mitigations**:
1. **Conservative Scrubbing**: Scrub anything resembling PII (false positives OK)
2. **Test Coverage**: Unit tests for international phone/email/ID formats
3. **User Review**: "Export Logs" shows what will be shared before sending
4. **Documentation**: Warn users to review logs before sharing in bug reports

**Long-term**: Integrate NER model for context-aware PII detection (Phase 8+)

---

### Risk 4: Performance Profiling Overhead Degrades UX

**Threat**: Profiling slows down Aether, defeating its "sub-100ms" promise.

**Mitigations**:
1. **Opt-in by Default**: Profiling disabled unless explicitly enabled
2. **Minimal Instrumentation**: Only 5 key stages (not every function call)
3. **Compile-time Removal**: Feature flag removes profiling code entirely
4. **Benchmarking**: Cargo bench validates overhead <5% when enabled

**Target**: <1% overhead when disabled, <5% when enabled

---

## Testing Strategy

### Unit Tests

**Image Clipboard**:
- Read PNG, JPEG, GIF from clipboard
- Handle corrupted images gracefully
- Enforce size limits
- Base64 encoding round-trip (encode → decode → identical)

**Typewriter**:
- Character timing accuracy (50 cps = 20ms per char)
- Unicode character handling
- Cancellation via Escape key
- Fallback to instant paste on error

**Logging**:
- PII scrubbing removes all sensitive patterns
- Log rotation triggers at midnight and 50MB limit
- Log files respect retention policy
- Privacy filter applied before file write

**Error Feedback**:
- All error types include suggestions
- Suggestions are actionable and specific
- UniFFI callback receives both message and suggestion

**Performance**:
- Profiling overhead <5% when enabled
- Stage timers accurately measure latency
- Slow operation warnings trigger at 2x threshold

### Integration Tests

**End-to-End Pipeline**:
1. Copy image → Trigger hotkey → Verify GPT-4 Vision receives Base64
2. AI response → Typewriter animation → Verify character-by-character output
3. Error condition → Verify suggestion displayed in Halo UI
4. Enable profiling → Process request → Verify latency logged

### Manual Testing Checklist

- [ ] Copy image in Preview → Trigger Aether → Describe image with GPT-4V
- [ ] Typewriter animation smooth at 50 cps, skippable with Escape
- [ ] Error messages show suggestions in Halo overlay (readable font)
- [ ] View Logs in Settings → Recent events visible and searchable
- [ ] Export Logs → ZIP file contains 3 days of scrubbed logs

---

## Open Questions for Approval

### 1. Image Format Priority

**Question**: If clipboard contains both PNG and JPEG (rare but possible on macOS), which format should we use?

**Options**:
- A) PNG (lossless, larger file)
- B) JPEG (lossy, smaller file)
- C) User configurable

**Recommendation**: A) PNG (preserve quality), document in spec

---

### 2. Typewriter Cancellation Behavior

**Question**: If user presses Cmd+~ again during typewriter animation, should we:

**Options**:
- A) Cancel current typing, paste remaining text, start new request
- B) Ignore new hotkey until current animation completes
- C) Cancel and discard remaining text, start new request immediately

**Recommendation**: A) Paste remaining (don't lose AI response), then start new request

---

### 3. Log Retention Default

**Question**: Should default log retention be 7 days or 30 days?

**Options**:
- A) 7 days (minimal disk usage ~350MB)
- B) 30 days (better debugging ~1.5GB)
- C) 14 days (compromise ~700MB)

**Recommendation**: A) 7 days (users can increase if needed)

---

### 4. Performance Profiling Visibility

**Question**: Should profiling data be visible in Settings UI, or only in log files?

**Options**:
- A) Log files only (simpler, developer-focused)
- B) Settings UI dashboard (user-friendly, motivates optimization)
- C) Both

**Recommendation**: A) Log files for Phase 7, defer UI dashboard to Phase 8

---

## Success Metrics

### Functional Success
- [ ] Image clipboard works with GPT-4 Vision and Claude 3 Opus
- [ ] Typewriter animation feels natural (>80% positive user feedback)
- [ ] Logs capture all critical events without PII leakage
- [ ] Error suggestions reduce support burden (measurable via GitHub issues)

### Performance Success
- [ ] Image clipboard read: <500ms for 10MB PNG
- [ ] Typewriter: <5% timing variance from configured speed
- [ ] Profiling overhead: <1% when disabled, <5% when enabled
- [ ] Log rotation: No dropped messages, <10ms latency

### Quality Success
- [ ] Test coverage: >80% for new code
- [ ] Zero regressions in existing features
- [ ] Documentation updated (README, CLAUDE.md, config.example.toml)

---

**Document Status**: DRAFT - Pending approval with proposal
**Last Updated**: 2025-12-25
**Author**: Claude Sonnet 4.5
