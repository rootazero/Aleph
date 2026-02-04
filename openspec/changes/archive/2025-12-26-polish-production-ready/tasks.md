# Tasks: polish-production-ready

## Overview

This document breaks down Phase 7 implementation into ordered, verifiable tasks. Tasks are grouped by feature but ordered to deliver incremental value while respecting dependencies.

**Estimated Total Effort**: 3-4 weeks (1 developer)

---

## Phase 7.1: Image Clipboard Support (Week 1)

### Task 1.1: Extend ClipboardManager Trait with Image Methods

**Goal**: Add image operations to clipboard abstraction layer.

**Implementation**:
1. Define `ImageData` struct in `clipboard/mod.rs`:
   ```rust
   pub struct ImageData {
       pub data: Vec<u8>,
       pub format: ImageFormat,
   }

   pub enum ImageFormat {
       Png,
       Jpeg,
       Gif,
   }
   ```
2. Extend `ClipboardManager` trait:
   ```rust
   fn read_image(&self) -> Result<Option<ImageData>>;
   fn write_image(&self, image: ImageData) -> Result<()>;
   fn has_image(&self) -> bool;
   ```
3. Update `MockClipboardManager` for testing

**Validation**:
- [ ] Trait compiles without errors
- [ ] Mock implementation passes unit tests
- [ ] No breaking changes to existing `read_text()`/`write_text()` methods

**Files Changed**:
- `Aleph/core/src/clipboard/mod.rs`
- `Aleph/core/src/clipboard/mock.rs`

---

### Task 1.2: Implement Image Support in ArboardManager

**Goal**: Use arboard crate's image capabilities for real clipboard access.

**Implementation**:
1. Implement `read_image()` using `arboard::ImageData`
2. Add format detection from MIME type or magic bytes
3. Implement `write_image()` with format conversion
4. Add size validation (check against `max_image_size_mb` config)

**Edge Cases**:
- Clipboard contains text only → return `Ok(None)`
- Image corrupted → return `Err(AlephError::ClipboardError)`
- Unsupported format (BMP, TIFF) → return error with conversion suggestion

**Validation**:
- [ ] Copy PNG image in macOS → `read_image()` returns valid ImageData
- [ ] Copy JPEG → Correct format detected
- [ ] Copy 15MB image → Error with size limit suggestion
- [ ] `write_image()` → Other apps can paste image

**Files Changed**:
- `Aleph/core/src/clipboard/arboard_manager.rs`

---

### Task 1.3: Add Base64 Encoding for API Integration

**Goal**: Convert images to Base64 data URIs for AI provider APIs.

**Implementation**:
1. Add `base64` crate dependency to `Cargo.toml`
2. Implement `ImageData::to_base64()` method:
   ```rust
   pub fn to_base64(&self) -> String {
       let mime = match self.format {
           ImageFormat::Png => "image/png",
           ImageFormat::Jpeg => "image/jpeg",
           ImageFormat::Gif => "image/gif",
       };
       format!("data:{};base64,{}", mime, base64::encode(&self.data))
   }
   ```
3. Implement `ImageData::from_base64()` for decoding

**Validation**:
- [ ] PNG → Base64 → starts with "data:image/png;base64,"
- [ ] Base64 decoding → Original image bytes match
- [ ] Large image (5MB) → Encoding completes in <500ms

**Files Changed**:
- `Aleph/core/Cargo.toml`
- `Aleph/core/src/clipboard/mod.rs`

---

### Task 1.4: Extend OpenAI Provider for Vision API

**Goal**: Send images to GPT-4 Vision model.

**Implementation**:
1. Update `OpenAiProvider::process()` to accept optional image:
   ```rust
   pub async fn process_with_image(
       &self,
       text: &str,
       image: Option<ImageData>,
       system_prompt: Option<&str>
   ) -> Result<String>
   ```
2. Modify request JSON to include `image_url` field when image present
3. Auto-select `gpt-4-vision-preview` model if image provided (override config)
4. Set `max_tokens: 4096` for vision requests (longer responses)

**Example Request**:
```json
{
  "model": "gpt-4-vision-preview",
  "messages": [
    {
      "role": "user",
      "content": [
        {"type": "text", "text": "Describe this image"},
        {"type": "image_url", "image_url": {"url": "data:image/png;base64,..."}}
      ]
    }
  ],
  "max_tokens": 4096
}
```

**Validation**:
- [ ] Text-only request → Existing behavior unchanged
- [ ] Text + image → Vision API called with Base64 data
- [ ] Image-only (no text) → Use default prompt "Describe this image"
- [ ] Response parsing works for vision model output

**Files Changed**:
- `Aleph/core/src/providers/openai.rs`

---

### Task 1.5: Extend Claude Provider for Vision API

**Goal**: Send images to Claude 3 Opus/Sonnet with image content blocks.

**Implementation**:
1. Update `ClaudeProvider::process()` signature (same as OpenAI)
2. Use Messages API with `image` content type:
   ```json
   {
     "model": "claude-3-opus-20240229",
     "max_tokens": 4096,
     "messages": [{
       "role": "user",
       "content": [
         {"type": "text", "text": "Describe this image"},
         {"type": "image", "source": {
           "type": "base64",
           "media_type": "image/png",
           "data": "iVBORw0KGgo..."
         }}
       ]
     }]
   }
   ```
3. Auto-select Opus/Sonnet model (Claude Haiku lacks vision)

**Validation**:
- [ ] Text + image → Claude API receives correct content blocks
- [ ] Response parsing handles vision output
- [ ] Model auto-selection works (Opus > Sonnet > error if Haiku configured)

**Files Changed**:
- `Aleph/core/src/providers/claude.rs`

---

### Task 1.6: Router Integration for Vision Capability Detection

**Goal**: Route image requests only to vision-capable providers.

**Implementation**:
1. Add `supports_vision()` method to `AiProvider` trait
2. Implement for each provider:
   - OpenAI: true (GPT-4V)
   - Claude: true (Opus/Sonnet)
   - Ollama: false (unless using LLaVA model - defer to Phase 8)
3. Update `Router::route()` to filter providers when image present:
   ```rust
   if clipboard_has_image {
       let vision_providers: Vec<_> = matched_providers
           .into_iter()
           .filter(|p| p.supports_vision())
           .collect();

       if vision_providers.is_empty() {
           return Err(AlephError::no_vision_provider());
       }
   }
   ```

**Validation**:
- [ ] Image + rule matches Ollama → Error with suggestion to use OpenAI/Claude
- [ ] Image + rule matches OpenAI → Routes successfully
- [ ] Text-only request → No vision filtering applied

**Files Changed**:
- `Aleph/core/src/providers/mod.rs`
- `Aleph/core/src/router/mod.rs`

---

### Task 1.7: UniFFI Bindings for Image Clipboard

**Goal**: Expose image clipboard to Swift UI.

**Implementation**:
1. Add to `aleph.udl`:
   ```idl
   dictionary ImageData {
     sequence<u8> data;
     ImageFormat format;
   };

   enum ImageFormat {
     "Png",
     "Jpeg",
     "Gif",
   };

   interface AlephCore {
     [Throws=AlephException]
     ImageData? read_clipboard_image();

     boolean has_clipboard_image();
   };
   ```
2. Regenerate Swift bindings: `cargo run --bin uniffi-bindgen generate ...`

**Validation**:
- [ ] Swift builds without errors
- [ ] `core.hasClipboardImage()` callable from Swift
- [ ] `ImageData` struct accessible in Swift

**Files Changed**:
- `Aleph/core/src/aleph.udl`
- `Aleph/Sources/Generated/aleph.swift` (regenerated)

---

### Task 1.8: Integration Testing for Image Pipeline

**Goal**: Validate end-to-end image flow from clipboard to AI.

**Implementation**:
1. Create `tests/integration_image.rs`
2. Test scenarios:
   - Read PNG from clipboard → Send to mock vision provider
   - Oversized image → Error with size limit
   - Unsupported format → Error with conversion suggestion
   - Vision provider unavailable → Fallback suggestion

**Validation**:
- [ ] All integration tests pass
- [ ] Test coverage >80% for image-related code

**Files Changed**:
- `Aleph/core/tests/integration_image.rs`

---

## Phase 7.2: Typewriter Output Effect (Week 2)

### Task 2.1: Extend InputSimulator Trait with Typewriter Method

**Goal**: Add character-by-character typing to input simulation abstraction.

**Implementation**:
1. Extend `InputSimulator` trait:
   ```rust
   async fn type_string_animated(
       &self,
       text: &str,
       chars_per_second: u32,
       cancellation_token: CancellationToken,
   ) -> Result<()>;
   ```
2. Add cancellation token support (tokio_util::sync::CancellationToken)

**Validation**:
- [ ] Trait compiles
- [ ] Mock implementation supports cancellation

**Files Changed**:
- `Aleph/core/src/input/mod.rs`

---

### Task 2.2: Implement Typewriter in EnigoSimulator

**Goal**: Use enigo to type characters with timing control.

**Implementation**:
1. Implement `type_string_animated()`:
   ```rust
   let delay = Duration::from_secs(1) / chars_per_second;
   for (i, ch) in text.chars().enumerate() {
       if cancellation_token.is_cancelled() {
           return Ok(()); // Exit early
       }

       self.enigo.key_sequence(&ch.to_string());
       tokio::time::sleep(delay).await;
   }
   ```
2. Handle special characters (newline → Enter, tab → Tab)
3. Unicode character support (use `chars()` iterator, not bytes)

**Edge Cases**:
- Escape key pressed → Cancellation token triggered externally
- Keyboard simulation fails → Return error, caller falls back to instant paste

**Validation**:
- [ ] "Hello world!" at 50 cps → Types in ~240ms (12 chars / 50)
- [ ] Unicode "你好世界" → Types correctly
- [ ] Cancellation mid-animation → Typing stops immediately
- [ ] Newlines → Enter key pressed, cursor moves to next line

**Files Changed**:
- `Aleph/core/src/input/enigo_simulator.rs`

---

### Task 2.3: Add Typewriter State and Callbacks

**Goal**: Notify Swift UI of typewriter progress.

**Implementation**:
1. Add to `ProcessingState` enum:
   ```rust
   enum ProcessingState {
       // Existing states...
       TypingOutput,  // New state
   }
   ```
2. Add callbacks to `AlephEventHandler`:
   ```rust
   fn on_typewriter_progress(percent: f32);
   fn on_typewriter_cancelled();
   ```
3. Update UniFFI definitions in `aleph.udl`

**Validation**:
- [ ] State transitions: ProcessingWithAI → TypingOutput → Success
- [ ] Progress callback invoked at ~10% intervals
- [ ] Cancellation callback triggers when Escape pressed

**Files Changed**:
- `Aleph/core/src/event_handler.rs`
- `Aleph/core/src/aleph.udl`

---

### Task 2.4: Integrate Typewriter into AlephCore Pipeline

**Goal**: Replace instant paste with typewriter animation.

**Implementation**:
1. Update `AlephCore::process_with_ai()`:
   ```rust
   // After AI response received
   let output_mode = self.config.lock().unwrap().behavior.output_mode;

   match output_mode {
       OutputMode::Instant => {
           self.clipboard_manager.write_text(&response)?;
           self.input_simulator.paste()?;
       }
       OutputMode::Typewriter => {
           let typing_speed = self.config.lock().unwrap().behavior.typing_speed;
           self.handler.on_state_changed(ProcessingState::TypingOutput);

           self.input_simulator.type_string_animated(
               &response,
               typing_speed,
               self.cancellation_token.clone()
           ).await?;
       }
   }
   ```
2. Add cancellation token to AlephCore struct
3. Trigger cancellation on new hotkey press or Escape key

**Validation**:
- [ ] `output_mode = "typewriter"` → Response typed character-by-character
- [ ] `output_mode = "instant"` → Existing behavior (paste)
- [ ] Escape during typing → Remaining text pasted instantly

**Files Changed**:
- `Aleph/core/src/core.rs`

---

### Task 2.5: Add Typewriter Configuration to BehaviorConfig

**Goal**: Allow users to configure typing speed and output mode.

**Implementation**:
1. Extend `BehaviorConfig`:
   ```rust
   pub struct BehaviorConfig {
       pub output_mode: OutputMode,  // "instant" | "typewriter"
       pub typing_speed: u32,         // 10-200 chars/sec
       // ... existing fields
   }

   pub enum OutputMode {
       Instant,
       Typewriter,
   }
   ```
2. Add validation: `typing_speed` must be 10-200 (clamp if outside range)
3. Update `config.example.toml` with new options

**Validation**:
- [ ] Config loads with new fields
- [ ] Invalid typing speed → Clamped to valid range
- [ ] Settings UI can read/write output_mode and typing_speed

**Files Changed**:
- `Aleph/core/src/config/mod.rs`
- `Aleph/config.example.toml`

---

### Task 2.6: Update Settings UI for Typewriter Controls

**Goal**: Add typewriter configuration to Behavior Settings tab.

**Implementation**:
1. Update `BehaviorSettingsView.swift`:
   ```swift
   Picker("Output Mode", selection: $outputMode) {
       Text("Instant Paste").tag("instant")
       Text("Typewriter Effect").tag("typewriter")
   }

   if outputMode == "typewriter" {
       Slider(value: $typingSpeed, in: 10...200, step: 10) {
           Text("Typing Speed: \(Int(typingSpeed)) chars/sec")
       }
   }
   ```
2. Add preview button to test typing animation (demo text)

**Validation**:
- [ ] UI displays current config values
- [ ] Changing output mode updates config.toml
- [ ] Typing speed slider functional (10-200 range)
- [ ] Preview button types demo text at selected speed

**Files Changed**:
- `Aleph/Sources/BehaviorSettingsView.swift`

---

### Task 2.7: Swift UI Integration for Typewriter Progress

**Goal**: Display typing progress in Halo overlay.

**Implementation**:
1. Update `HaloView.swift` to show progress bar when `state == .typingOutput`
2. Implement `EventHandler.onTypewriterProgress()`:
   ```swift
   func onTypewriterProgress(percent: Float) {
       DispatchQueue.main.async {
           self.haloWindow?.updateProgress(percent)
       }
   }
   ```
3. Add progress bar animation (smooth 0% → 100%)

**Validation**:
- [ ] Halo shows progress bar during typing
- [ ] Progress updates smoothly (no jitter)
- [ ] Cancellation hides Halo immediately

**Files Changed**:
- `Aleph/Sources/HaloView.swift`
- `Aleph/Sources/EventHandler.swift`

---

### Task 2.8: Typewriter Integration Tests

**Goal**: Validate typewriter timing and cancellation.

**Implementation**:
1. Create `tests/integration_typewriter.rs`
2. Test scenarios:
   - 10-char string at 50 cps → ~200ms duration
   - Cancellation after 50% → Remaining text pasted
   - Unicode characters → Correct timing (count as single unit)
   - Escape key simulation → Cancellation works

**Validation**:
- [ ] All tests pass
- [ ] Timing accuracy within ±10% of target

**Files Changed**:
- `Aleph/core/tests/integration_typewriter.rs`

---

## Phase 7.3: Structured Logging (Week 3)

### Task 3.1: Configure Tracing Subscriber with File Appender

**Goal**: Set up tracing infrastructure for logging to both console and file.

**Implementation**:
1. Add `tracing-appender` dependency to `Cargo.toml`
2. Update `init_logging()` in `lib.rs`:
   ```rust
   use tracing_appender::rolling::{RollingFileAppender, Rotation};

   let log_dir = dirs::config_dir()
       .unwrap()
       .join("aleph")
       .join("logs");
   std::fs::create_dir_all(&log_dir)?;

   let file_appender = RollingFileAppender::new(
       Rotation::DAILY,
       log_dir,
       "aleph.log"
   );

   let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

   tracing_subscriber::registry()
       .with(EnvFilter::from_default_env())
       .with(fmt::layer())  // Console output
       .with(fmt::layer().with_writer(non_blocking))  // File output
       .init();
   ```
3. Store `_guard` in static to prevent premature drop

**Validation**:
- [ ] Logs appear in `~/.aleph/logs/aleph-YYYY-MM-DD.log`
- [ ] Daily rotation works (new file at midnight)
- [ ] Console and file both receive log messages

**Files Changed**:
- `Aleph/core/Cargo.toml`
- `Aleph/core/src/lib.rs`

---

### Task 3.2: Implement PII Scrubbing Layer

**Goal**: Strip sensitive information from logs before writing.

**Implementation**:
1. Create `src/logging/pii_filter.rs`:
   ```rust
   pub struct PiiScrubbingLayer;

   impl<S: Subscriber> Layer<S> for PiiScrubbingLayer {
       fn on_event(&self, event: &Event, _ctx: Context<S>) {
           // Extract message field
           let mut visitor = MessageVisitor::default();
           event.record(&mut visitor);

           if let Some(msg) = visitor.message {
               let scrubbed = scrub_pii(&msg);
               // Re-emit with scrubbed message
           }
       }
   }
   ```
2. Reuse `scrub_pii()` from `memory/ingestion.rs` (move to shared utility)
3. Add API key pattern: `sk-[a-zA-Z0-9]+` → `[REDACTED]`

**Validation**:
- [x] Log "User email: john@example.com" → File contains "User email: [EMAIL]"
- [x] Log "API key: sk-abc123" → File contains "API key: [REDACTED]"
- [x] Log "App: com.apple.Notes" → File unchanged (no PII)

**Files Changed**:
- `Aleph/core/src/utils/mod.rs` (created)
- `Aleph/core/src/utils/pii.rs` (created with comprehensive PII scrubbing)
- `Aleph/core/src/logging/mod.rs` (created)
- `Aleph/core/src/logging/pii_filter.rs` (created with PiiScrubbingLayer)
- `Aleph/core/src/memory/ingestion.rs` (updated to use shared scrub_pii)
- `Aleph/core/src/lib.rs` (added utils and logging modules)

---

### Task 3.3: Implement Log Retention Policy

**Goal**: Automatically delete old log files based on config.

**Implementation**:
1. Add to `GeneralConfig`:
   ```rust
   pub struct GeneralConfig {
       pub log_retention_days: u32,  // Default: 7
       // ... existing fields
   }
   ```
2. Create `src/logging/retention.rs`:
   ```rust
   pub fn cleanup_old_logs(log_dir: &Path, retention_days: u32) -> Result<()> {
       let cutoff = SystemTime::now() - Duration::from_days(retention_days);

       for entry in fs::read_dir(log_dir)? {
           let entry = entry?;
           let metadata = entry.metadata()?;

           if metadata.modified()? < cutoff {
               fs::remove_file(entry.path())?;
           }
       }
       Ok(())
   }
   ```
3. Call `cleanup_old_logs()` on AlephCore startup

**Validation**:
- [ ] Logs older than 7 days → Deleted on startup
- [ ] Retention configurable (1-30 days)
- [ ] Cleanup errors logged but don't crash app

**Files Changed**:
- `Aleph/core/src/config/mod.rs`
- `Aleph/core/src/logging/retention.rs`
- `Aleph/core/src/core.rs` (call cleanup on init)

---

### Task 3.4: Add Logging Throughout Codebase

**Goal**: Instrument all critical events with appropriate log levels.

**Implementation**:
1. Hotkey detection:
   ```rust
   tracing::info!("Global hotkey triggered");
   ```
2. AI provider selection:
   ```rust
   tracing::info!(provider = %provider_name, model = %model, "AI provider selected");
   ```
3. AI response:
   ```rust
   tracing::info!(latency_ms = %duration.as_millis(), "AI response received");
   ```
4. Memory retrieval:
   ```rust
   tracing::debug!(count = %memories.len(), app = %app_bundle_id, "Retrieved memories");
   ```
5. Config changes:
   ```rust
   tracing::info!(field = "behavior.typing_speed", value = %new_speed, "Configuration updated");
   ```
6. Errors:
   ```rust
   tracing::error!(error = %e, "API request failed");
   ```

**Validation**:
- [ ] All critical events logged
- [ ] No clipboard content in logs (privacy check)
- [ ] No API keys in logs (even when errors occur)

**Files Changed**:
- `Aleph/core/src/core.rs`
- `Aleph/core/src/router/mod.rs`
- `Aleph/core/src/providers/*.rs`
- `Aleph/core/src/memory/*.rs`

---

### Task 3.5: UniFFI Logging Controls

**Goal**: Expose logging configuration to Swift UI.

**Implementation**:
1. Add to `aleph.udl`:
   ```idl
   interface AlephCore {
       string get_log_level();
       void set_log_level(string level);
       string get_log_directory();
   };
   ```
2. Implement methods in `AlephCore`:
   ```rust
   pub fn get_log_level(&self) -> String {
       std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string())
   }

   pub fn set_log_level(&self, level: String) -> Result<()> {
       std::env::set_var("RUST_LOG", &level);
       // Reload tracing filter
       tracing::subscriber::set_global_default(...)?;
       Ok(())
   }
   ```

**Validation**:
- [ ] `core.getLogLevel()` returns current level
- [ ] `core.setLogLevel("debug")` → Debug logs appear immediately
- [ ] `core.getLogDirectory()` → Returns `~/.aleph/logs`

**Files Changed**:
- `Aleph/core/src/aleph.udl`
- `Aleph/core/src/core.rs`

---

### Task 3.6: Settings UI for Log Viewing

**Goal**: Add log viewer to Settings → General tab.

**Implementation**:
1. Add "View Logs" button to `GeneralSettingsView.swift`
2. Create `LogViewerView.swift`:
   ```swift
   struct LogViewerView: View {
       @State private var logContent: String = ""

       var body: some View {
           ScrollView {
               Text(logContent)
                   .font(.system(.caption, design: .monospaced))
                   .frame(maxWidth: .infinity, alignment: .leading)
           }
           .onAppear {
               loadLogs()
           }
       }

       func loadLogs() {
           let logDir = core.getLogDirectory()
           // Read last 1000 lines from latest log file
       }
   }
   ```
3. Add syntax highlighting (error=red, warn=yellow, info=white)
4. Add "Export Logs" button → Save as ZIP
5. Add "Clear Logs" button → Delete all log files after confirmation

**Validation**:
- [ ] "View Logs" opens modal with recent logs
- [ ] Logs searchable (Cmd+F works)
- [ ] "Export Logs" creates ZIP with 3 days of logs
- [ ] "Clear Logs" deletes all files, requires confirmation

**Files Changed**:
- `Aleph/Sources/GeneralSettingsView.swift`
- `Aleph/Sources/LogViewerView.swift`

---

### Task 3.7: Logging Integration Tests

**Goal**: Validate logging behavior and privacy protection.

**Implementation**:
1. Create `tests/integration_logging.rs`
2. Test scenarios:
   - Log message with email → File contains [EMAIL]
   - Log API key → File contains [REDACTED]
   - Retention policy → Old logs deleted
   - Log level change → Takes effect immediately

**Validation**:
- [ ] All logging tests pass
- [ ] PII scrubbing covers all patterns

**Files Changed**:
- `Aleph/core/tests/integration_logging.rs`

---

## Phase 7.4: Error Feedback & Performance Profiling (Week 4)

### Task 4.1: Extend AlephError with Suggestions

**Goal**: Add suggestion field to all error types.

**Implementation**:
1. Update `error.rs`:
   ```rust
   #[derive(Debug, thiserror::Error)]
   pub enum AlephError {
       #[error("{message}")]
       ApiKeyInvalid {
           message: String,
           provider: String,
           suggestion: Option<String>,
       },
       // ... update all variants
   }

   impl AlephError {
       pub fn api_key_invalid(provider: &str) -> Self {
           Self::ApiKeyInvalid {
               message: "API authentication failed".to_string(),
               provider: provider.to_string(),
               suggestion: Some(format!(
                   "Please verify your {} API key in Settings → Providers → {}",
                   provider, provider
               )),
           }
       }

       pub fn suggestion(&self) -> Option<&str> {
           match self {
               Self::ApiKeyInvalid { suggestion, .. } => suggestion.as_deref(),
               // ... other variants
           }
       }
   }
   ```

**Validation**:
- [x] All error types include optional suggestion
- [x] `error.suggestion()` returns correct text
- [x] Backwards compatibility (existing error creation still works)

**Files Changed**:
- `Aleph/core/src/error.rs`

---

### Task 4.2: Add Error Suggestions for Common Failure Modes

**Goal**: Provide actionable guidance for all user-facing errors.

**Implementation**:
1. API errors:
   - 401 Unauthorized → "Verify API key in Settings"
   - 429 Rate limit → "Wait 60s or upgrade plan"
   - Network timeout → "Check internet connection or try different provider"
2. Config errors:
   - Invalid regex → "Check syntax at regex101.com"
   - Missing API key → "Add key in Settings → Providers"
3. Clipboard errors:
   - Empty clipboard → "Select text/image before pressing Cmd+~"
   - Image too large → "Resize to under 10MB"
4. Memory errors:
   - Database locked → "Close other Aleph instances"
   - Disk full → "Free space or adjust retention policy"

**Validation**:
- [ ] Each error type has specific, actionable suggestion
- [ ] Suggestions reference UI locations (Settings tabs, menu items)
- [ ] No generic "Something went wrong" messages

**Files Changed**:
- `Aleph/core/src/error.rs`
- `Aleph/core/src/providers/*.rs`
- `Aleph/core/src/memory/*.rs`

---

### Task 4.3: Update UniFFI Error Callback with Suggestion

**Goal**: Pass suggestions to Swift UI for display in Halo.

**Implementation**:
1. Update `aleph.udl`:
   ```idl
   callback interface AlephEventHandler {
       void on_error(string message, string? suggestion);
   };
   ```
2. Update Rust callback invocations:
   ```rust
   self.handler.on_error(
       error.to_string(),
       error.suggestion().map(|s| s.to_string())
   );
   ```

**Validation**:
- [x] Swift receives both message and suggestion
- [x] Null suggestion handled gracefully (no crash)

**Files Changed**:
- `Aleph/core/src/aleph.udl`
- `Aleph/core/src/core.rs` (update error handling)

---

### Task 4.4: Update Halo UI to Display Error Suggestions

**Goal**: Show suggestions below error messages in Halo overlay.

**Implementation**:
1. Update `HaloView.swift`:
   ```swift
   struct ErrorStateView: View {
       let message: String
       let suggestion: String?

       var body: some View {
           VStack(spacing: 8) {
               Text(message)
                   .font(.headline)
                   .foregroundColor(.red)

               if let suggestion = suggestion {
                   Text(suggestion)
                       .font(.caption)
                       .foregroundColor(.gray)
                       .multilineTextAlignment(.center)
                       .lineLimit(3)
               }
           }
       }
   }
   ```
2. Update `EventHandler.onError()`:
   ```swift
   func onError(message: String, suggestion: String?) {
       DispatchQueue.main.async {
           self.haloWindow?.showError(message, suggestion: suggestion)
       }
   }
   ```

**Validation**:
- [x] Error with suggestion → Both texts displayed
- [x] Error without suggestion → Only message shown
- [x] Long suggestions → Truncated to 2 lines with lightbulb icon
- [ ] Error disappears after 5 seconds (currently manual dismiss via button)

**Files Changed**:
- `Aleph/Sources/HaloView.swift`
- `Aleph/Sources/EventHandler.swift`

---

### Task 4.5: Implement Performance Metrics Module

**Goal**: Add timing instrumentation for profiling.

**Implementation**:
1. Create `src/metrics/mod.rs`:
   ```rust
   pub struct StageTimer {
       name: String,
       start: Instant,
       metadata: HashMap<String, String>,
   }

   impl StageTimer {
       pub fn start(name: &str) -> Self {
           Self {
               name: name.to_string(),
               start: Instant::now(),
               metadata: HashMap::new(),
           }
       }

       pub fn with_meta(mut self, key: &str, value: &str) -> Self {
           self.metadata.insert(key.to_string(), value.to_string());
           self
       }

       pub fn stop(self) {
           let elapsed = self.start.elapsed();
           tracing::debug!(
               stage = %self.name,
               duration_ms = %elapsed.as_millis(),
               metadata = ?self.metadata,
               "Stage completed"
           );
       }
   }
   ```
2. Add config option: `enable_performance_logging: bool` (default: false)

**Validation**:
- [x] Timer measures accurately (±1ms tolerance)
- [x] Metadata included in debug logs
- [x] Zero overhead when profiling disabled

**Files Changed**:
- `Aleph/core/src/metrics/mod.rs`
- `Aleph/core/src/config/mod.rs`

---

### Task 4.6: Instrument Critical Pipeline Stages

**Goal**: Add performance timing to all key operations.

**Implementation**:
1. Hotkey → Clipboard:
   ```rust
   let _timer = StageTimer::start("hotkey_to_clipboard");
   let content = self.clipboard_manager.read_text()?;
   ```
2. Clipboard → Memory:
   ```rust
   let _timer = StageTimer::start("memory_retrieval")
       .with_meta("app", &app_bundle_id);
   let memories = self.memory_store.retrieve(...)?;
   ```
3. Memory → AI Request:
   ```rust
   let _timer = StageTimer::start("ai_request")
       .with_meta("provider", provider_name)
       .with_meta("model", model_name);
   let response = provider.process(...).await?;
   ```
4. Total pipeline:
   ```rust
   let _timer = StageTimer::start("total_pipeline");
   // Entire process_with_ai() method
   ```

**Validation**:
- [x] All stages logged when profiling enabled
- [x] Logs include metadata (provider, model, app)
- [x] No performance degradation when profiling disabled

**Files Changed**:
- `Aleph/core/src/core.rs`
- `Aleph/core/src/memory/*.rs` (not modified - uses existing logging)
- `Aleph/core/src/router/mod.rs` (not modified - uses existing logging)

---

### Task 4.7: Add Slow Operation Warnings

**Goal**: Log warnings when operations exceed 2x target latency.

**Implementation**:
1. Define target latencies:
   ```rust
   const TARGET_CLIPBOARD_MS: u64 = 50;
   const TARGET_MEMORY_MS: u64 = 100;
   const TARGET_AI_MS: u64 = 500;
   ```
2. Check thresholds on timer stop:
   ```rust
   impl StageTimer {
       pub fn stop_with_target(self, target_ms: u64) {
           let elapsed_ms = self.start.elapsed().as_millis() as u64;

           if elapsed_ms > target_ms * 2 {
               tracing::warn!(
                   stage = %self.name,
                   actual_ms = %elapsed_ms,
                   target_ms = %target_ms,
                   "Slow operation detected"
               );
           } else {
               tracing::debug!(stage = %self.name, duration_ms = %elapsed_ms);
           }
       }
   }
   ```

**Validation**:
- [x] Slow memory retrieval (250ms) → Warn logged
- [x] Fast operations → Only debug log
- [x] Warnings logged even when profiling disabled

**Files Changed**:
- `Aleph/core/src/metrics/mod.rs`
- `Aleph/core/src/core.rs` (use `stop_with_target()`)

---

### Task 4.8: Final Integration Testing and Benchmarks

**Goal**: Validate all Phase 7 features work together.

**Implementation**:
1. End-to-end tests:
   - Image + typewriter + logging → All features work simultaneously
   - Error with suggestion → Displayed in Halo with correct formatting
   - Performance profiling → Logs include all stages
2. Benchmarks:
   - Image clipboard read: <500ms for 10MB
   - Typewriter: <10% timing variance from config
   - Profiling overhead: <5% when enabled
3. Manual testing checklist:
   - [ ] Copy image → Describe with GPT-4V → Typewriter types response
   - [ ] Trigger error (e.g., invalid API key) → Suggestion displayed
   - [ ] Enable profiling → View logs → Stage breakdown visible
   - [ ] Export logs → ZIP includes 3 days, PII scrubbed

**Validation**:
- [x] All integration tests pass (metrics module tests)
- [x] Performance benchmarks created and functional
- [ ] Manual testing checklist complete (deferred - requires full UI integration)

**Files Changed**:
- `Aleph/core/tests/integration_phase7.rs` (not created - metrics tests in mod.rs)
- `Aleph/core/benches/performance_benchmarks.rs` (created)
- `docs/manual-testing-checklist.md` (deferred)

---

## Post-Implementation Tasks

### Task 5.1: Update Documentation

**Goal**: Document all Phase 7 features for users and developers.

**Implementation**:
1. Update `README.md`:
   - Add "Image Support" section with example (GPT-4 Vision)
   - Add "Typewriter Output" section with speed configuration
   - Add "Logging" section with privacy note
2. Update `CLAUDE.md`:
   - Mark Phase 7 tasks as completed
   - Add code examples for new features
3. Update `config.example.toml`:
   - Add image, typewriter, logging settings with comments
4. Create `docs/logging-guide.md`:
   - How to enable debug logs
   - How to export logs for bug reports
   - Privacy guarantees (PII scrubbing)

**Validation**:
- [ ] Documentation accurate and complete
- [ ] Example config matches actual config schema
- [ ] User-facing docs written clearly (no jargon)

**Files Changed**:
- `README.md`
- `CLAUDE.md`
- `Aleph/config.example.toml`
- `docs/logging-guide.md`

---

### Task 5.2: Archive Phase 7 Proposal

**Goal**: Mark Phase 7 as deployed and archive OpenSpec proposal.

**Implementation**:
1. Run `openspec archive polish-production-ready`
2. Update `openspec/project.md` with Phase 7 learnings
3. Update specs:
   - `clipboard-management` - Add image operations
   - `event-handler` - Add typewriter callbacks
   - `core-library` - Add logging and metrics

**Validation**:
- [ ] Archive command succeeds
- [ ] All specs updated with Phase 7 changes

**Files Changed**:
- `openspec/changes/polish-production-ready/` (archived)
- `openspec/specs/*/spec.md` (updated)
- `openspec/project.md`

---

## Task Dependencies

```
Phase 7.1 (Image Clipboard)
├─ 1.1 → 1.2 (trait before impl)
├─ 1.2 → 1.3 (impl before encoding)
├─ 1.3 → 1.4, 1.5 (encoding before provider integration)
├─ 1.4, 1.5 → 1.6 (providers before router)
├─ 1.6 → 1.7 (router before UniFFI)
└─ 1.7 → 1.8 (UniFFI before testing)

Phase 7.2 (Typewriter)
├─ 2.1 → 2.2 (trait before impl)
├─ 2.2 → 2.3 (impl before state management)
├─ 2.3 → 2.4 (state before integration)
├─ 2.4 → 2.5 (integration before config)
├─ 2.5 → 2.6 (config before UI)
├─ 2.6 → 2.7 (UI before Swift integration)
└─ 2.7 → 2.8 (Swift before testing)

Phase 7.3 (Logging)
├─ 3.1 → 3.2 (appender before PII filter)
├─ 3.2 → 3.3 (PII filter before retention)
├─ 3.3 → 3.4 (retention before instrumentation)
├─ 3.4 → 3.5 (instrumentation before UniFFI)
├─ 3.5 → 3.6 (UniFFI before UI)
└─ 3.6 → 3.7 (UI before testing)

Phase 7.4 (Error & Performance)
├─ 4.1 → 4.2 (error type before suggestions)
├─ 4.2 → 4.3 (suggestions before UniFFI)
├─ 4.3 → 4.4 (UniFFI before UI)
├─ 4.5 → 4.6 (metrics module before instrumentation)
├─ 4.6 → 4.7 (instrumentation before warnings)
└─ All 4.x → 4.8 (all features before final testing)

Post-Implementation
├─ All phases → 5.1 (features before docs)
└─ 5.1 → 5.2 (docs before archive)
```

---

## Parallelizable Work

The following tasks can be worked on in parallel once their dependencies are met:

**Week 1 (Image Support)**:
- Tasks 1.4 and 1.5 (OpenAI and Claude providers) can be done in parallel

**Week 2 (Typewriter)**:
- Tasks 2.5 and 2.6 (Config and UI) can be done in parallel
- Tasks 2.7 and 2.8 (Swift integration and tests) can overlap

**Week 3 (Logging)**:
- Tasks 3.4 and 3.5 (Instrumentation and UniFFI) can be done in parallel

**Week 4 (Error & Performance)**:
- Tasks 4.1-4.4 (Error suggestions) and 4.5-4.7 (Performance) are independent streams

---

## Acceptance Criteria (Rollup)

### Image Clipboard
- [ ] Copy PNG/JPEG/GIF → Aleph detects image
- [ ] Send to GPT-4V or Claude Opus → Receives Base64 data
- [ ] Oversized image → Error with resize suggestion
- [ ] Ollama selected with image → Error with provider switch suggestion

### Typewriter Output
- [ ] AI response types at configured speed (±10% accuracy)
- [ ] Escape key skips animation, pastes remaining text
- [ ] Instant mode bypasses typewriter (paste immediately)
- [ ] Halo shows typing progress bar (0% → 100%)

### Structured Logging
- [ ] Logs written to `~/.aleph/logs/aleph-YYYY-MM-DD.log`
- [ ] PII scrubbed (email, phone, API keys → placeholders)
- [ ] Old logs auto-deleted after retention period (default 7 days)
- [ ] Settings UI allows viewing, exporting, clearing logs

### Error Feedback
- [ ] All errors include actionable suggestions
- [ ] Halo displays suggestion below error message
- [ ] Suggestions reference specific UI locations (Settings tabs)

### Performance Profiling
- [ ] Profiling opt-in via config (disabled by default)
- [ ] Stage timings logged (clipboard, memory, AI, paste)
- [ ] Slow operations (>2x target) trigger warnings
- [ ] Overhead <1% when disabled, <5% when enabled

---

**Tasks Total**: 35 tasks across 4 weeks
**Deliverable**: Production-ready Aleph with image support, typewriter output, comprehensive logging, helpful errors, and performance insights.
