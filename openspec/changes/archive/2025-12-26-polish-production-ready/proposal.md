# Proposal: polish-production-ready

## Summary

Polish Aether to production-ready quality through five critical enhancements: image clipboard support for multimodal AI interactions, typewriter output effect for natural AI response delivery, structured logging with privacy protection, comprehensive error handling improvements, and performance profiling with optimizations. This change transforms Aether from a functional prototype into a production-grade system-level AI middleware.

## Background

Aether has successfully completed Phases 1-6:
- ✅ Rust core with UniFFI bindings
- ✅ Global hotkey and clipboard integration
- ✅ Halo overlay UI
- ✅ Memory module (local RAG)
- ✅ AI provider integration (OpenAI, Claude, Ollama)
- ✅ Settings UI with full configuration management

However, several production-critical features remain unimplemented:
1. **Image Support**: Clipboard currently only handles text, blocking multimodal AI use cases (vision, image generation)
2. **Output Delivery**: AI responses are pasted instantly, lacking natural typing animation
3. **Logging**: No structured logging system for debugging, monitoring, or user support
4. **Error UX**: Error handling exists but lacks user-friendly feedback mechanisms
5. **Performance**: No profiling data or optimization for real-world usage patterns

## Why

### User Impact

**Vision AI Blocked**: Users cannot leverage GPT-4 Vision or Claude 3 Opus for image description, OCR, visual analysis, or chart interpretation because clipboard only supports text.

**Unnatural Output**: AI responses paste instantly (0ms), which feels robotic and jarring. Users expect gradual delivery like ChatGPT's streaming, not instantaneous text appearance.

**Debugging Impossible**: When production issues occur (timeouts, errors, slow responses), users cannot provide diagnostic information because logs don't exist. Support burden increases as every bug report requires extensive back-and-forth.

**Cryptic Errors**: Error messages like "API request failed" leave users guessing. No guidance on whether to check internet, verify API key, or switch providers. Frustration leads to abandonment.

### Technical Debt

**Placeholder Code**: Image clipboard stubs exist (`has_image() → false`) since Phase 2 but were never implemented. Vision API integration requires removing these placeholders.

**Partial Logging**: `tracing` crate is partially integrated (Phase 5) but logs only go to console (invisible to users). File appender and PII scrubbing are missing.

**Unused Components**: `StreamingTextView.swift` exists for typewriter preview in Settings UI but isn't connected to actual output pipeline.

**No Observability**: Performance bottlenecks are unknown. Is memory retrieval slow? Is AI provider latency high? Clipboard operations? No data to guide optimization.

## Motivation

### Why Now?

**Competitive Pressure**: Raycast AI and similar tools support vision models. Aether risks becoming "text-only AI assistant" if Phase 7 delays.

**Production Readiness**: Users report bugs but cannot share diagnostic logs. Phase 7 unblocks self-service debugging and reduces maintainer burden.

**UX Polish**: Typewriter effect transforms Aether from "functional prototype" to "polished product." Small UX details compound into perceived quality.

**Foundation for Phase 8**: Real-time streaming (Phase 8 goal) requires typewriter infrastructure. Error recovery mechanisms need robust logging. Multimodal memory (image embeddings) needs image clipboard.

## Goals

### Primary Objectives
1. **Image Clipboard Support**: Enable reading/writing images from/to clipboard with Base64 encoding for AI APIs
2. **Typewriter Output Effect**: Implement character-by-character typing animation for AI responses (configurable speed)
3. **Structured Logging**: Add privacy-aware logging system for debugging and monitoring
4. **Enhanced Error Feedback**: Improve error messages with actionable guidance and recovery suggestions
5. **Performance Profiling**: Profile hotkey→AI→paste pipeline and optimize bottlenecks

### Non-Goals
- Video or file clipboard support (out of scope for Phase 7)
- Advanced image processing (resizing, format conversion) - use images as-is
- Remote logging/telemetry - all logs remain local
- Automated crash reporting - manual log collection only

## Scope

### Affected Components

**Rust Core:**
- `clipboard/` - Image read/write operations
- `input/` - Typewriter keyboard simulation
- `config/` - Logging and typewriter configuration
- All modules - Structured logging integration
- `error.rs` - Enhanced error types with suggestions

**Swift UI:**
- `AppDelegate.swift` - Typewriter output integration
- `HaloView.swift` - Typewriter animation state
- `BehaviorSettingsView.swift` - Typewriter speed configuration UI
- `GeneralSettingsView.swift` - Logging controls

**UniFFI:**
- `aether.udl` - Image clipboard methods
- `aether.udl` - Typewriter progress callbacks

### Out of Scope
- Real-time streaming AI responses (future enhancement)
- Image editing or manipulation
- Persistent log storage (logs are ephemeral, rotated daily)
- Crash dump generation

## Approach

### 1. Image Clipboard Support (HIGH PRIORITY)

**Implementation Strategy:**
- Leverage `arboard` crate's existing image support
- Add `read_image() → Result<ImageData>` and `write_image(ImageData)` to `ClipboardManager` trait
- Encode images as Base64 for OpenAI/Claude vision APIs
- Add MIME type detection (PNG, JPEG, GIF)
- Update config schema: `max_image_size_mb` (default: 10MB)

**Integration Points:**
- OpenAI provider: Use `image_url` field in messages API
- Claude provider: Use `image` content type in Messages API
- Router: Detect image clipboard → route to vision-capable models only
- Memory module: Store image embeddings via CLIP model (future)

### 2. Typewriter Output Effect (MEDIUM PRIORITY)

**Implementation Strategy:**
- Add `enigo` simulation of individual character presses
- Implement `typewriter_output(text: &str, chars_per_second: u32)` method
- Add progress callback: `on_typewriter_progress(percent: f32)`
- Config options: `output_mode` (instant | typewriter), `typing_speed` (10-200 chars/sec)
- Swift UI: Display typing progress in Halo overlay

**Performance Considerations:**
- Each character simulates keypress → ~5ms per char at 200 chars/sec
- For 1000-char response: 5s typing time at 200 cps (acceptable)
- Allow Escape key to skip animation and paste instantly

### 3. Structured Logging (HIGH PRIORITY)

**Implementation Strategy:**
- Use `tracing` crate (already partially integrated)
- Add `log` module with privacy filters
- Log levels: ERROR, WARN, INFO, DEBUG (controlled by `RUST_LOG` env var)
- Privacy protection: Scrub PII before logging (reuse `scrub_pii()` from memory module)
- Log rotation: Daily rotation, keep 7 days, 50MB max per file
- Log location: `~/.config/aether/logs/aether-YYYY-MM-DD.log`

**Logged Events:**
- Hotkey trigger (timestamp only, no clipboard content)
- AI provider selection and latency
- Memory retrieval hits/misses
- Error conditions with context
- Configuration changes
- Performance metrics (clipboard→AI→paste timing)

**Swift Integration:**
- Add "View Logs" button in Settings → General tab
- Add "Export Logs" for bug reports (last 3 days)
- Add "Clear Logs" action

### 4. Enhanced Error Feedback (MEDIUM PRIORITY)

**Implementation Strategy:**
- Extend `AetherError` with `suggestion: Option<String>` field
- Add error-specific recovery suggestions
- Examples:
  - API key invalid → "Please check your API key in Settings → Providers"
  - Network timeout → "Check your internet connection or try again"
  - Memory database locked → "Close other Aether instances"
- Update UniFFI callbacks: `on_error(message: String, suggestion: Option<String>)`
- Swift UI: Display suggestion in Halo error state

### 5. Performance Profiling (LOW PRIORITY)

**Implementation Strategy:**
- Add `metrics` module with timing instrumentation
- Profile key stages:
  1. Hotkey detection → Clipboard read (target: <50ms)
  2. Clipboard read → Memory retrieval (target: <100ms)
  3. Memory retrieval → AI request (target: <500ms)
  4. AI response → Clipboard write (target: <50ms)
  5. Clipboard write → Paste simulation (target: <100ms)
- Log slow operations (>2x target threshold)
- Add config: `enable_performance_logging` (default: false)

**Optimization Targets:**
- Memory retrieval: Cache embeddings for frequent apps
- Config loading: Cache parsed config, only reload on change (already implemented)
- Router initialization: Lazy compile regex patterns

## Dependencies

### External Crates (Already Included)
- `arboard` - Image clipboard support
- `enigo` - Keyboard simulation
- `tracing` / `tracing-subscriber` - Structured logging
- `tracing-appender` - Log rotation (new dependency)

### Internal Dependencies
- PII scrubbing from `memory/ingestion.rs` (reuse for logging)
- Existing config hot-reload mechanism
- Typewriter preview component from Settings UI

### Platform Dependencies
- macOS Accessibility API (already required)
- Keychain for API key storage (already integrated)

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|-----------|
| Image clipboard conflicts with text-only apps | HIGH | Fallback to text if image read fails; add config option to disable images |
| Typewriter animation feels slow on large responses | MEDIUM | Add skip animation hotkey (Escape); make speed configurable (up to 200 cps) |
| Logging exposes PII in files | HIGH | Reuse `scrub_pii()` before all log writes; add unit tests for PII leakage |
| Log files consume excessive disk space | MEDIUM | Implement rotation (7 days max, 50MB limit); add "Clear Logs" UI |
| Performance profiling overhead in production | LOW | Make profiling opt-in (disabled by default); use low-overhead instrumentation |

## Alternatives Considered

### 1. Image Support via File Paths (Rejected)
- **Approach**: Save clipboard image to temp file, pass file path to AI
- **Rejected**: Adds file I/O overhead, requires cleanup logic, introduces privacy risk of temp file persistence

### 2. Streaming AI Responses (Deferred)
- **Approach**: Use SSE streaming APIs for token-by-token delivery
- **Deferred**: Requires async typewriter implementation, complicates error handling; defer to Phase 8

### 3. Remote Logging Service (Rejected)
- **Approach**: Send logs to cloud service for aggregation
- **Rejected**: Violates privacy-first principle; all data must remain local

### 4. Compiled Logging (Rejected)
- **Approach**: Use `log` crate with compile-time filtering
- **Rejected**: `tracing` provides better structured logging, async support, and performance

## Open Questions

1. **Image Encoding Format**: Should we support multiple image formats (PNG, JPEG, WebP) or standardize on PNG?
   - **Recommendation**: Support PNG and JPEG (most common), convert others to PNG using `image` crate

2. **Typewriter Cancellation**: What happens if user presses another hotkey during typing animation?
   - **Recommendation**: Cancel current animation, paste remaining text instantly, start new request

3. **Log Retention Policy**: Should users control log retention duration in Settings?
   - **Recommendation**: Yes, add `log_retention_days` config (default: 7, range: 1-30)

4. **Performance Metrics Storage**: Should we persist metrics across sessions?
   - **Recommendation**: No, metrics are ephemeral (logged only); add opt-in telemetry in Phase 8 if needed

5. **Image Memory Storage**: Should images be stored in memory module?
   - **Recommendation**: Defer to Phase 8 (requires CLIP embeddings, multimodal vector DB)

## Success Criteria

### Functional Requirements
- [ ] Users can copy/paste images and send to vision-capable AI models
- [ ] AI responses are typed character-by-character at configurable speed (10-200 cps)
- [ ] Structured logs capture all critical events with PII scrubbing
- [ ] Error messages include actionable recovery suggestions
- [ ] Performance profiling identifies bottlenecks (>2x target latency)

### Quality Requirements
- [ ] All new code has 80%+ test coverage
- [ ] Image clipboard handles edge cases (corrupted images, unsupported formats)
- [ ] Typewriter animation can be skipped via Escape key
- [ ] Log files auto-rotate and respect size limits
- [ ] Performance overhead <5% for typical requests

### UX Requirements
- [ ] Image support is seamless (no config required for basic usage)
- [ ] Typewriter speed feels natural (default: 50 chars/sec)
- [ ] Logs are accessible via Settings UI (no terminal required)
- [ ] Errors provide clear guidance without technical jargon

## Timeline Estimate

**Total Effort**: ~3-4 weeks (1 developer)

**Phase 7.1 - Image Clipboard** (1 week)
- Implement image read/write in `clipboard/`
- Add Base64 encoding for API integration
- Update OpenAI/Claude providers for vision
- Test with GPT-4 Vision and Claude 3 Opus

**Phase 7.2 - Typewriter Output** (1 week)
- Implement character-by-character typing in `input/`
- Add progress callbacks and UI integration
- Add skip animation logic
- Test typing smoothness and cancellation

**Phase 7.3 - Structured Logging** (1 week)
- Configure `tracing` with PII filters
- Implement log rotation and size limits
- Add Settings UI for log viewing/export
- Test privacy protection

**Phase 7.4 - Error Feedback + Performance** (1 week)
- Enhance error types with suggestions
- Add performance instrumentation
- Profile and optimize hot paths
- Final integration testing

## Related Work

- **Phase 6**: Settings UI provides foundation for logging controls
- **Phase 5**: AI provider integration enables vision model support
- **Phase 4**: Memory module's PII scrubbing reused for logging
- **Typewriter Component**: Already exists in `StreamingTextView.swift`, needs integration

## Approval

This proposal requires approval before implementation begins. After approval:
1. Spec deltas will be created for each capability
2. Implementation tasks will be broken down in `tasks.md`
3. Code changes will follow the task sequence
4. Testing will validate all success criteria

---

**Proposer**: Claude Sonnet 4.5
**Date**: 2025-12-25
**Status**: PENDING_APPROVAL
