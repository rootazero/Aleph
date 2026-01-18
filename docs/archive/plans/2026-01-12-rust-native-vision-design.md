# Rust Native Vision Design

Date: 2026-01-12

## Overview

Implement native screen understanding capabilities in Aether using xcap + image crates for screen capture and AI providers (Claude/GPT-4o) for OCR and image understanding.

## Goals

- **Screen Understanding**: Capture screen regions and extract text using AI vision capabilities
- **Dual Purpose**: Independent OCR extraction + AI context enhancement
- **Excellent Chinese OCR**: Leverage AI providers' strong multilingual capabilities
- **Multiple Capture Modes**: Region selection, window capture, full screen
- **Output to Clipboard**: Extracted text copied to clipboard for user control

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Swift Layer                               │
├─────────────────────────────────────────────────────────────────┤
│  ScreenCaptureCoordinator                                        │
│  ├── Listen for hotkeys (Cmd+Shift+4 style)                      │
│  ├── Display capture UI (ScreenCaptureOverlay)                   │
│  ├── Call CGWindowListCreateImage for capture                    │
│  └── Pass PNG Data to Rust                                       │
└────────────────────────┬────────────────────────────────────────┘
                         │ UniFFI (Data transfer)
                         ▼
┌─────────────────────────────────────────────────────────────────┐
│                        Rust Core                                 │
├─────────────────────────────────────────────────────────────────┤
│  VisionService                                                   │
│  ├── process_vision(request) -> VisionResult                     │
│  ├── Image compression/format conversion (image crate)           │
│  ├── Build multimodal request                                    │
│  └── Call AI Provider (user-configured default)                  │
├─────────────────────────────────────────────────────────────────┤
│  VisionResult                                                    │
│  ├── extracted_text: String      // OCR result                   │
│  ├── description: Option<String> // Image description            │
│  └── confidence: f32             // Confidence score             │
└────────────────────────┬────────────────────────────────────────┘
                         │ UniFFI Callback
                         ▼
┌─────────────────────────────────────────────────────────────────┐
│  Swift: Write result to clipboard (NSPasteboard)                 │
└─────────────────────────────────────────────────────────────────┘
```

**Capture Mode Triggers:**
- **Region**: Hotkey → Fullscreen overlay → User drags selection → Capture
- **Window**: Hotkey + Option → Highlight current window → Click to confirm
- **Full Screen**: Hotkey + Shift → Capture immediately

## UDL Interface Definition

```udl
// aether.udl additions

enum CaptureMode {
    "Region",
    "Window",
    "FullScreen",
};

enum VisionTask {
    "OcrOnly",           // Extract text only
    "OcrWithContext",    // Extract text + use as AI context
    "Describe",          // Image description
};

dictionary VisionRequest {
    sequence<u8> image_data;    // Raw PNG data
    CaptureMode capture_mode;
    VisionTask task;
    string? prompt;             // Optional user prompt
};

dictionary VisionResult {
    string extracted_text;      // OCR extracted text
    string? description;        // Image description (if requested)
    string? ai_response;        // AI response (OcrWithContext mode)
    f32 confidence;
    u64 processing_time_ms;
};

// AetherCore new methods
interface AetherCore {
    // ... existing methods ...

    [Async]
    VisionResult process_vision(VisionRequest request);

    // Convenience method: OCR only
    [Async]
    string extract_text(sequence<u8> image_data);
};
```

**Design Notes:**
- `image_data` uses `sequence<u8>` for raw PNG to avoid Base64 encoding overhead
- `VisionTask` differentiates three use cases for flexible composition
- `prompt` is optional, used for additional instructions in `OcrWithContext` mode
- `extract_text` provides a convenience method for simple OCR calls

## Rust VisionService Implementation

```rust
// src/vision/mod.rs

pub struct VisionService {
    provider_manager: Arc<ProviderManager>,
    config: VisionConfig,
}

pub struct VisionConfig {
    pub max_image_dimension: u32,    // Max edge length, default 2048
    pub jpeg_quality: u8,            // Compression quality, default 85
    pub ocr_prompt: String,          // OCR system prompt
}

impl VisionService {
    pub async fn process_vision(&self, request: VisionRequest) -> Result<VisionResult> {
        let start = Instant::now();

        // 1. Image preprocessing
        let processed = self.preprocess_image(&request.image_data)?;

        // 2. Build prompt
        let prompt = self.build_prompt(&request);

        // 3. Call AI Provider (user-configured default)
        let response = self.call_vision_provider(&processed, &prompt).await?;

        // 4. Parse result
        let result = self.parse_response(response, &request.task)?;

        Ok(VisionResult {
            extracted_text: result.text,
            description: result.description,
            ai_response: result.ai_response,
            confidence: result.confidence,
            processing_time_ms: start.elapsed().as_millis() as u64,
        })
    }

    fn preprocess_image(&self, data: &[u8]) -> Result<ProcessedImage> {
        let img = image::load_from_memory(data)?;

        // Scale if needed (maintain aspect ratio)
        let img = if img.width() > self.config.max_image_dimension
                 || img.height() > self.config.max_image_dimension {
            img.resize(
                self.config.max_image_dimension,
                self.config.max_image_dimension,
                image::imageops::FilterType::Lanczos3
            )
        } else {
            img
        };

        // Convert to JPEG (reduce transfer size)
        let mut buffer = Vec::new();
        img.write_to(&mut Cursor::new(&mut buffer),
                     image::ImageFormat::Jpeg)?;

        Ok(ProcessedImage {
            data: buffer,
            mime_type: "image/jpeg".to_string(),
        })
    }

    fn build_prompt(&self, request: &VisionRequest) -> String {
        match request.task {
            VisionTask::OcrOnly => {
                self.config.ocr_prompt.clone()
            }
            VisionTask::OcrWithContext => {
                format!(
                    "Please extract the text from the image first, then answer the user's question.\n\nUser question: {}",
                    request.prompt.as_deref().unwrap_or("Please describe the content of this image")
                )
            }
            VisionTask::Describe => {
                "Please describe the content of this image in detail.".to_string()
            }
        }
    }

    async fn call_vision_provider(&self, image: &ProcessedImage, prompt: &str) -> Result<String> {
        // Get user-configured default provider from ProviderManager
        let provider = self.provider_manager.get_default_provider().await?;

        // Check if provider supports vision capability
        if !provider.capabilities().supports_vision {
            return Err(AetherError::ProviderNotSupportsVision(provider.name()));
        }

        provider.send_multimodal(image, prompt).await
    }
}
```

**Key Design Points:**
- Images auto-scaled to reasonable size, reducing API cost and latency
- PNG → JPEG conversion, 60-70% size reduction
- OCR prompt optimized to emphasize "output text only" to avoid AI adding extra explanations
- **Uses ProviderManager to get user-configured default provider, not hardcoded**

## Swift Screen Capture UI

```swift
// Sources/Vision/ScreenCaptureCoordinator.swift

@MainActor
class ScreenCaptureCoordinator: ObservableObject {
    @Published var isCapturing = false
    @Published var captureMode: CaptureMode = .region

    private var overlayWindow: NSWindow?
    private var overlayView: ScreenCaptureOverlayView?

    // MARK: - Trigger Capture

    func startCapture(mode: CaptureMode) {
        self.captureMode = mode

        switch mode {
        case .region:
            showRegionSelector()
        case .window:
            captureActiveWindow()
        case .fullScreen:
            captureFullScreen()
        }
    }

    // MARK: - Region Selection

    private func showRegionSelector() {
        let screen = NSScreen.main!
        overlayWindow = NSWindow(
            contentRect: screen.frame,
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )

        overlayWindow?.level = .screenSaver
        overlayWindow?.backgroundColor = NSColor.black.withAlphaComponent(0.3)
        overlayWindow?.isOpaque = false
        overlayWindow?.ignoresMouseEvents = false

        overlayView = ScreenCaptureOverlayView { [weak self] rect in
            self?.captureRegion(rect)
        }
        overlayWindow?.contentView = overlayView
        overlayWindow?.makeKeyAndOrderFront(nil)

        isCapturing = true
    }

    private func captureRegion(_ rect: CGRect) {
        dismissOverlay()

        guard let cgImage = CGWindowListCreateImage(
            rect,
            .optionOnScreenBelowWindow,
            kCGNullWindowID,
            [.boundsIgnoreFraming]
        ) else { return }

        processCapture(cgImage)
    }

    // MARK: - Window Capture

    private func captureActiveWindow() {
        guard let frontApp = NSWorkspace.shared.frontmostApplication,
              let windowList = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] else {
            return
        }

        let targetWindow = windowList.first {
            ($0[kCGWindowOwnerPID as String] as? Int32) == frontApp.processIdentifier
        }

        guard let bounds = targetWindow?[kCGWindowBounds as String] as? [String: CGFloat],
              let windowID = targetWindow?[kCGWindowNumber as String] as? CGWindowID else {
            return
        }

        let rect = CGRect(x: bounds["X"]!, y: bounds["Y"]!,
                          width: bounds["Width"]!, height: bounds["Height"]!)

        guard let cgImage = CGWindowListCreateImage(rect, .optionIncludingWindow, windowID, []) else {
            return
        }

        processCapture(cgImage)
    }

    // MARK: - Process Capture

    private func processCapture(_ image: CGImage) {
        let bitmap = NSBitmapImageRep(cgImage: image)
        guard let pngData = bitmap.representation(using: .png, properties: [:]) else { return }

        Task {
            let result = try await AetherCore.shared.extractText(Array(pngData))

            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(result, forType: .string)

            NotificationHelper.show("Text Copied", body: "Extracted \(result.count) characters")
        }
    }
}
```

**Region Selection Overlay:**
```swift
// ScreenCaptureOverlayView.swift

class ScreenCaptureOverlayView: NSView {
    private var startPoint: CGPoint?
    private var currentRect: CGRect?
    private var onComplete: (CGRect) -> Void

    override func mouseDown(with event: NSEvent) {
        startPoint = event.locationInWindow
    }

    override func mouseDragged(with event: NSEvent) {
        guard let start = startPoint else { return }
        let current = event.locationInWindow
        currentRect = CGRect(/* calculate selection rect */)
        needsDisplay = true
    }

    override func mouseUp(with event: NSEvent) {
        guard let rect = currentRect else { return }
        onComplete(convertToScreenCoordinates(rect))
    }

    override func draw(_ dirtyRect: NSRect) {
        // Draw semi-transparent overlay + selection highlight
    }
}
```

## Hotkey Configuration

```swift
// Sources/Vision/VisionHotkeyManager.swift

class VisionHotkeyManager {
    private let coordinator: ScreenCaptureCoordinator

    struct Hotkeys {
        var regionCapture: KeyCombo = .init(key: .four, modifiers: [.command, .shift])
        var windowCapture: KeyCombo = .init(key: .four, modifiers: [.command, .shift, .option])
        var fullScreenCapture: KeyCombo = .init(key: .three, modifiers: [.command, .shift])
    }

    func registerHotkeys() {
        HotkeyService.shared.register(hotkeys.regionCapture) { [weak self] in
            self?.coordinator.startCapture(mode: .region)
        }

        HotkeyService.shared.register(hotkeys.windowCapture) { [weak self] in
            self?.coordinator.startCapture(mode: .window)
        }

        HotkeyService.shared.register(hotkeys.fullScreenCapture) { [weak self] in
            self?.coordinator.startCapture(mode: .fullScreen)
        }
    }
}
```

**config.toml Configuration:**
```toml
[vision]
enabled = true
# default_provider - optional, uses global [providers].default if not set

[vision.image]
max_dimension = 2048
jpeg_quality = 85

[vision.hotkeys]
region_capture = "cmd+shift+4"
window_capture = "cmd+shift+option+4"
fullscreen_capture = "cmd+shift+3"

[vision.ocr]
prompt = "Please extract all text from the image, preserving original format and line breaks. Output only the extracted text without any explanations."
```

## File Structure

```
Aether/
├── core/src/
│   ├── vision/
│   │   ├── mod.rs              # VisionService main module
│   │   ├── config.rs           # VisionConfig
│   │   └── prompt.rs           # OCR prompt templates
│   ├── lib.rs                  # Add process_vision, extract_text exports
│   └── aether.udl              # Add Vision type definitions
│
├── Sources/
│   ├── Vision/
│   │   ├── ScreenCaptureCoordinator.swift
│   │   ├── ScreenCaptureOverlayView.swift
│   │   ├── VisionHotkeyManager.swift
│   │   └── VisionNotification.swift
│   └── AppDelegate.swift       # Integration entry point
```

## Implementation Steps

| Step | Content | Dependencies |
|------|---------|--------------|
| 1 | Rust: Define UDL interface (VisionRequest, VisionResult) | None |
| 2 | Rust: Implement VisionService core logic | Step 1 |
| 3 | Rust: Integrate into AetherCore, generate UniFFI bindings | Step 2 |
| 4 | Swift: Implement ScreenCaptureOverlayView (selection UI) | None |
| 5 | Swift: Implement ScreenCaptureCoordinator | Step 3, 4 |
| 6 | Swift: Implement VisionHotkeyManager + integration | Step 5 |
| 7 | Testing & optimization | All |

## Key Dependencies (Already Available)

- `xcap = "0.8"` - Backup screen capture (Rust layer)
- `image = "0.25"` - Image processing

**No new Rust dependencies required** - fully reuses existing infrastructure.

## Design Decisions

1. **Swift for Capture UI**: Platform-specific interaction is better with native Swift; Rust handles business logic
2. **User-Configured Provider**: VisionService uses ProviderManager to get user's default AI provider, not hardcoded
3. **PNG → JPEG Conversion**: Reduces data transfer by 60-70% with minimal quality loss
4. **Capability Check**: Validates provider supports vision before sending request
5. **Clipboard Output**: Simple, predictable user experience - user controls when to paste
