# Design Document: Enhanced Halo Overlay

## Architectural Overview

This design document outlines the technical architecture for Phase 3 enhancements to the Halo overlay system. The enhancements maintain the existing NSWindow-based architecture while adding new capabilities through modular extensions.

## System Architecture

### Current Architecture (Phase 2)

```
┌─────────────────────────────────────────────────────────────┐
│                     Rust Core (AlephCore)                   │
│  ┌────────────────────────────────────────────────────────┐ │
│  │  Event Loop (rdev) → AlephEventHandler callbacks      │ │
│  │    - on_state_changed(ProcessingState)                 │ │
│  │    - on_halo_show(HaloPosition, provider_color)        │ │
│  │    - on_halo_hide()                                    │ │
│  │    - on_error(String)                                  │ │
│  └────────────────────────────────────────────────────────┘ │
└──────────────────────┬──────────────────────────────────────┘
                       │ UniFFI Bridge
                       ↓
┌─────────────────────────────────────────────────────────────┐
│              Swift macOS Client (EventHandler)               │
│  ┌────────────────────────────────────────────────────────┐ │
│  │  HaloWindow (NSWindow)                                 │ │
│  │    ├── HaloView (SwiftUI)                             │ │
│  │    └── HaloState enum (idle/listening/processing/     │ │
│  │                         success/error)                 │ │
│  └────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

### Proposed Architecture (Phase 3)

```
┌─────────────────────────────────────────────────────────────┐
│                     Rust Core (AlephCore)                   │
│  ┌────────────────────────────────────────────────────────┐ │
│  │  Event Loop + NEW CALLBACKS:                           │ │
│  │    - on_response_chunk(text: String)         [NEW]    │ │
│  │    - on_error_typed(ErrorType, String)       [NEW]    │ │
│  │    - on_progress(percent: f32)                [NEW]    │ │
│  └────────────────────────────────────────────────────────┘ │
└──────────────────────┬──────────────────────────────────────┘
                       │ UniFFI Bridge (Extended)
                       ↓
┌─────────────────────────────────────────────────────────────┐
│              Swift macOS Client (EventHandler)               │
│  ┌────────────────────────────────────────────────────────┐ │
│  │  HaloWindow (NSWindow) - ENHANCED                      │ │
│  │    ├── ThemeEngine                           [NEW]    │ │
│  │    │   ├── CyberpunkTheme                              │ │
│  │    │   ├── ZenTheme                                    │ │
│  │    │   └── JarvisTheme                                 │ │
│  │    ├── HaloViewController                     [NEW]    │ │
│  │    │   ├── StreamingTextView                           │ │
│  │    │   ├── ErrorActionView                             │ │
│  │    │   └── ProgressIndicator                           │ │
│  │    ├── AudioManager                           [NEW]    │ │
│  │    └── HaloState (extended with text/progress)         │ │
│  └────────────────────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────────────────────┐ │
│  │  Settings (UserDefaults)                               │ │
│  │    - theme: Theme                             [NEW]    │ │
│  │    - soundEnabled: Bool                       [NEW]    │ │
│  │    - haloPreferences: HaloPrefs               [NEW]    │ │
│  └────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

## Component Design

### 1. Theme System

#### Theme Protocol

```swift
protocol HaloTheme {
    // Colors
    var listeningColor: Color { get }
    var processingColor: Color { get }
    var successColor: Color { get }
    var errorColor: Color { get }
    var textColor: Color { get }
    var backgroundColor: Color { get }

    // Shapes
    func listeningView() -> some View
    func processingView() -> some View
    func successView() -> some View
    func errorView(type: ErrorType, message: String) -> some View

    // Animations
    var transitionDuration: Double { get }
    var pulseAnimation: Animation { get }
}
```

#### Theme Implementations

**CyberpunkTheme:**
```swift
struct CyberpunkTheme: HaloTheme {
    let listeningColor = Color(hex: "#00ffff") // Cyan
    let processingColor = Color(hex: "#ff00ff") // Magenta

    func processingView() -> some View {
        ZStack {
            // Hexagonal outer ring
            HexagonShape()
                .stroke(processingColor, lineWidth: 3)
                .shadow(color: processingColor, radius: 10)

            // Glitch effect overlay
            GlitchOverlay()
                .blendMode(.screen)
        }
    }
}
```

**ZenTheme:**
```swift
struct ZenTheme: HaloTheme {
    let listeningColor = Color.white.opacity(0.8)
    let processingColor = Color(hex: "#90ee90") // Sage green

    func processingView() -> some View {
        ZStack {
            // Soft circular gradient
            Circle()
                .fill(RadialGradient(
                    colors: [processingColor, .clear],
                    center: .center,
                    startRadius: 20,
                    endRadius: 60
                ))

            // Breathing animation
            Circle()
                .stroke(lineWidth: 2)
                .foregroundColor(.white.opacity(0.5))
                .scaleEffect(breathingScale)
        }
    }
}
```

**JarvisTheme:**
```swift
struct JarvisTheme: HaloTheme {
    let arcReactorBlue = Color(hex: "#00d4ff")

    func processingView() -> some View {
        ZStack {
            // Hexagonal segments
            ForEach(0..<6) { i in
                HexSegment(index: i)
                    .fill(arcReactorBlue.opacity(0.3))
                    .rotationEffect(.degrees(Double(i) * 60))
            }

            // Pulsing core
            Circle()
                .fill(arcReactorBlue)
                .frame(width: 20, height: 20)
                .shadow(color: arcReactorBlue, radius: 15)
        }
    }
}
```

#### Theme Selection

```swift
enum Theme: String, Codable {
    case cyberpunk
    case zen
    case jarvis

    func makeTheme() -> any HaloTheme {
        switch self {
        case .cyberpunk: return CyberpunkTheme()
        case .zen: return ZenTheme()
        case .jarvis: return JarvisTheme()
        }
    }
}

class ThemeEngine: ObservableObject {
    @Published var currentTheme: Theme {
        didSet { UserDefaults.standard.set(currentTheme.rawValue, forKey: "selectedTheme") }
    }

    init() {
        let saved = UserDefaults.standard.string(forKey: "selectedTheme")
        currentTheme = Theme(rawValue: saved ?? "zen") ?? .zen
    }
}
```

### 2. Streaming Text Display

#### Data Flow

```
Rust: AI response chunk arrives
  ↓
Rust calls on_response_chunk("Hello ")
  ↓
Swift EventHandler receives callback
  ↓
Swift updates HaloState.processing(text: "Hello ")
  ↓
SwiftUI StreamingTextView re-renders with new text
  ↓
Typewriter animation displays character-by-character
```

#### Implementation

```swift
// Extended HaloState
enum HaloState: Equatable {
    case idle
    case listening
    case processing(providerColor: Color, streamingText: String? = nil)
    case success(finalText: String? = nil)
    case error(type: ErrorType, message: String)
}

// StreamingTextView component
struct StreamingTextView: View {
    let text: String
    @State private var visibleCharacters: Int = 0

    var body: some View {
        Text(String(text.prefix(visibleCharacters)))
            .font(.system(.body, design: .monospaced))
            .lineLimit(3)
            .frame(maxWidth: 300)
            .onAppear {
                animateText()
            }
            .onChange(of: text) { _ in
                animateText()
            }
    }

    private func animateText() {
        // Typewriter effect: reveal one character every 50ms
        Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { timer in
            if visibleCharacters < text.count {
                visibleCharacters += 1
            } else {
                timer.invalidate()
            }
        }
    }
}
```

#### Layout Strategy

```swift
// Dynamic Halo sizing
struct HaloView: View {
    @State var state: HaloState

    var body: some View {
        ZStack {
            // Animated frame size based on content
            AnimatedContainer(
                height: heightForState(state),
                content: contentForState(state)
            )
        }
        .animation(.spring(response: 0.4), value: state)
    }

    private func heightForState(_ state: HaloState) -> CGFloat {
        switch state {
        case .processing(_, let text):
            return text == nil ? 120 : 200 // Expand for text
        default:
            return 120
        }
    }
}
```

### 3. Enhanced Error Handling

#### UniFFI Interface Extension

```rust
// aleph.udl additions
enum ErrorType {
    "Network",
    "Permission",
    "Quota",
    "Timeout",
    "Unknown"
}

callback interface AlephEventHandler {
    // ... existing callbacks

    void on_error_typed(ErrorType error_type, string message);
    void on_retry_available(ErrorType error_type);
}
```

#### Swift Error UI

```swift
struct ErrorActionView: View {
    let errorType: ErrorType
    let message: String
    let onRetry: () -> Void
    let onOpenSettings: () -> Void

    var body: some View {
        VStack(spacing: 12) {
            // Error icon with shake animation
            Image(systemName: iconForError(errorType))
                .font(.system(size: 40))
                .foregroundColor(.red)
                .modifier(ShakeEffect())

            // Error message
            Text(message)
                .font(.caption)
                .foregroundColor(.white)
                .multilineTextAlignment(.center)

            // Action buttons
            HStack(spacing: 8) {
                if errorType == .network || errorType == .timeout {
                    Button("Retry") { onRetry() }
                        .buttonStyle(HaloButtonStyle())
                }

                if errorType == .permission {
                    Button("Open Settings") { onOpenSettings() }
                        .buttonStyle(HaloButtonStyle())
                }
            }
        }
        .padding()
    }

    private func iconForError(_ type: ErrorType) -> String {
        switch type {
        case .network: return "wifi.slash"
        case .permission: return "lock.shield"
        case .quota: return "exclamationmark.triangle"
        case .timeout: return "clock.badge.xmark"
        case .unknown: return "xmark.circle"
        }
    }
}
```

### 4. Audio Feedback System

#### AudioManager Design

```swift
class AudioManager {
    static let shared = AudioManager()

    private var soundEffects: [HaloState: AVAudioPlayer] = [:]
    private var isEnabled: Bool {
        UserDefaults.standard.bool(forKey: "soundEnabled")
    }

    init() {
        preloadSounds()
    }

    private func preloadSounds() {
        // Load all sounds into memory
        let sounds: [(HaloState, String)] = [
            (.listening, "listening"),
            (.processing, "processing"),
            (.success, "success"),
            (.error, "error")
        ]

        for (state, filename) in sounds {
            guard let url = Bundle.main.url(forResource: filename, withExtension: "aiff") else {
                continue
            }

            do {
                let player = try AVAudioPlayer(contentsOf: url)
                player.prepareToPlay()
                player.volume = 0.3 // 30% of system volume
                soundEffects[state] = player
            } catch {
                print("[AudioManager] Failed to load sound: \(filename)")
            }
        }
    }

    func play(for state: HaloState) {
        guard isEnabled else { return }

        // Stop previous sound
        soundEffects.values.forEach { $0.stop() }

        // Play new sound
        soundEffects[state]?.play()
    }
}
```

#### Sound Asset Specifications

| Sound File | Duration | Format | Description |
|-----------|----------|--------|-------------|
| `listening.aiff` | 100ms | 16-bit PCM | Soft "whoosh" upward sweep |
| `processing.aiff` | 1000ms (loop) | 16-bit PCM | Gentle ambient hum |
| `success.aiff` | 200ms | 16-bit PCM | Satisfying "ding" chime |
| `error.aiff` | 150ms | 16-bit PCM | Subtle "thud" impact |

**Audio Design Guidelines:**
- Low frequency content (< 500 Hz) for non-intrusive feel
- No sharp transients (avoid "pop" sounds)
- Stereo width: mono (no panning)
- Peak amplitude: -6 dB (headroom for mixing)

### 5. Performance Optimization

#### Frame Rate Profiling

```swift
class PerformanceMonitor {
    private var displayLink: CADisplayLink?
    private var frameTimestamps: [CFTimeInterval] = []

    func startMonitoring() {
        displayLink = CADisplayLink(target: self, selector: #selector(trackFrame))
        displayLink?.add(to: .main, forMode: .common)
    }

    @objc private func trackFrame(displayLink: CADisplayLink) {
        frameTimestamps.append(displayLink.timestamp)

        // Calculate FPS over last 60 frames
        if frameTimestamps.count > 60 {
            let elapsed = frameTimestamps.last! - frameTimestamps.first!
            let fps = Double(frameTimestamps.count) / elapsed

            if fps < 55 {
                print("[Performance] WARNING: FPS dropped to \(fps)")
                // Trigger performance degradation
                NotificationCenter.default.post(
                    name: .performanceDegradation,
                    object: nil
                )
            }

            frameTimestamps.removeFirst()
        }
    }
}
```

#### Performance Degradation Strategy

```swift
class PerformanceManager {
    @Published var effectsQuality: QualityLevel = .high

    enum QualityLevel {
        case high    // Full effects (Metal shaders, complex animations)
        case medium  // Simplified effects (no shaders, basic animations)
        case low     // Minimal effects (solid colors, no animations)
    }

    func detectCapabilities() {
        // Check GPU model
        guard let device = MTLCreateSystemDefaultDevice() else {
            effectsQuality = .low
            return
        }

        let gpuFamily = device.name.lowercased()

        if gpuFamily.contains("intel hd 3000") || gpuFamily.contains("intel hd 4000") {
            effectsQuality = .medium
        } else {
            effectsQuality = .high
        }
    }
}

// Apply quality settings to theme
struct CyberpunkTheme: HaloTheme {
    @EnvironmentObject var perfManager: PerformanceManager

    func processingView() -> some View {
        ZStack {
            HexagonShape()
                .stroke(processingColor, lineWidth: 3)

            // Conditionally render expensive effects
            if perfManager.effectsQuality == .high {
                GlitchShader() // Metal shader
            }
        }
    }
}
```

### 6. Accessibility Support

#### VoiceOver Implementation

```swift
extension HaloWindow {
    func updateAccessibility(for state: HaloState) {
        // Set accessibility label
        switch state {
        case .idle:
            self.accessibilityLabel = "Aleph idle"

        case .listening:
            self.accessibilityLabel = "Aleph listening"
            announceToVoiceOver("Aleph listening")

        case .processing(let color, _):
            let provider = providerName(for: color)
            self.accessibilityLabel = "Processing with \(provider)"
            announceToVoiceOver("Processing with \(provider)")

        case .success:
            self.accessibilityLabel = "Complete"
            announceToVoiceOver("Complete")

        case .error(let type, let message):
            self.accessibilityLabel = "Error: \(message)"
            announceToVoiceOver("Error: \(message)")
        }
    }

    private func announceToVoiceOver(_ message: String) {
        NSAccessibility.post(
            element: self,
            notification: .announcementRequested,
            userInfo: [
                NSAccessibility.NotificationUserInfoKey.announcement: message,
                NSAccessibility.NotificationUserInfoKey.priority: NSAccessibilityPriorityLevel.low
            ]
        )
    }
}
```

### 7. User Customization

#### Settings Storage

```swift
struct HaloPreferences: Codable {
    var size: HaloSize
    var opacity: Double
    var animationSpeed: Double

    enum HaloSize: String, Codable {
        case small = "80"
        case medium = "120"
        case large = "160"

        var dimension: CGFloat { CGFloat(Int(rawValue) ?? 120) }
    }

    static let `default` = HaloPreferences(
        size: .medium,
        opacity: 1.0,
        animationSpeed: 1.0
    )
}

class PreferencesManager: ObservableObject {
    @Published var haloPrefs: HaloPreferences {
        didSet { save() }
    }

    init() {
        if let data = UserDefaults.standard.data(forKey: "haloPreferences"),
           let prefs = try? JSONDecoder().decode(HaloPreferences.self, from: data) {
            haloPrefs = prefs
        } else {
            haloPrefs = .default
        }
    }

    private func save() {
        if let data = try? JSONEncoder().encode(haloPrefs) {
            UserDefaults.standard.set(data, forKey: "haloPreferences")
        }
    }
}
```

#### Applying Preferences

```swift
struct HaloView: View {
    @EnvironmentObject var prefs: PreferencesManager

    var body: some View {
        ZStack {
            // Theme content
        }
        .frame(
            width: prefs.haloPrefs.size.dimension,
            height: prefs.haloPrefs.size.dimension
        )
        .opacity(prefs.haloPrefs.opacity)
        .animation(
            .easeInOut(duration: 0.3 / prefs.haloPrefs.animationSpeed),
            value: state
        )
    }
}
```

### 8. Multi-Halo Queue Management

#### EventHandler with Queue

```swift
class EventHandlerImpl: AlephEventHandler {
    private var isProcessing = false
    private var pendingOperations: [HotkeyEvent] = []
    private let maxQueueDepth = 3

    func onHotkeyDetected() {
        if isProcessing {
            // Queue the operation
            if pendingOperations.count < maxQueueDepth {
                pendingOperations.append(HotkeyEvent(timestamp: Date()))
                updateQueueBadge()
            } else {
                // Queue full, show error
                showQueueFullError()
            }
            return
        }

        // Process immediately
        isProcessing = true
        processHotkey()
    }

    func onStateChanged(state: ProcessingState) {
        if state == .success || state == .error {
            isProcessing = false

            // Process next in queue
            if !pendingOperations.isEmpty {
                pendingOperations.removeFirst()
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                    self.processHotkey()
                }
            }
        }
    }

    private func updateQueueBadge() {
        let count = pendingOperations.count
        if count > 0 {
            haloWindow?.showQueueBadge(count: count)
        } else {
            haloWindow?.hideQueueBadge()
        }
    }
}
```

## Data Flow Examples

### Example 1: Streaming AI Response

```
1. User presses Cmd+~
2. Rust detects hotkey → clipboard copy → sends to OpenAI
3. Rust calls on_halo_show(position, color="#10a37f")
4. Swift shows Halo at cursor with green spinner (OpenAI color)
5. OpenAI starts streaming response: "Hello"
6. Rust calls on_response_chunk("Hello")
7. Swift updates HaloState.processing(color, text: "Hello")
8. StreamingTextView animates "Hello" letter-by-letter
9. OpenAI streams more: " World"
10. Rust calls on_response_chunk("Hello World")
11. Swift appends new text, typewriter continues
12. OpenAI finishes
13. Rust calls on_state_changed(.success)
14. Swift shows success checkmark → fade out
```

### Example 2: Network Error with Retry

```
1. User presses Cmd+~
2. Rust detects hotkey → clipboard copy → sends to Claude
3. Network request times out after 30s
4. Rust calls on_error_typed(.timeout, "Request timed out")
5. Swift updates HaloState.error(type: .timeout, message: "Request timed out")
6. ErrorActionView renders with "Retry" button
7. User clicks "Retry" button
8. Swift calls core.retryLastRequest()
9. Rust re-attempts request
10. If successful: on_state_changed(.success)
11. If failed again: on_error_typed(.network, "Network unavailable")
```

## Trade-offs and Decisions

### Decision 1: Theme Implementation Strategy

**Options:**
1. **Single HaloView with conditional rendering** (current approach)
2. **Separate view files per theme** (CyberpunkHaloView, ZenHaloView)
3. **Protocol-based theme system** (recommended)

**Decision**: Option 3 (Protocol-based)

**Rationale:**
- Better separation of concerns
- Easier to add new themes in future
- Testable in isolation
- No massive switch statements

**Trade-off**: Slightly more complex initial setup, but much cleaner long-term

### Decision 2: Streaming Text Storage

**Options:**
1. Store full text in HaloState enum
2. Store text in separate @State variable
3. Use ObservableObject for text streaming

**Decision**: Option 1 (Store in HaloState)

**Rationale:**
- Single source of truth
- Simpler state management
- Natural fit with SwiftUI's value-based rendering

**Trade-off**: HaloState enum grows larger, but remains manageable

### Decision 3: Audio Playback Timing

**Options:**
1. Play sounds in Rust before callback
2. Play sounds in Swift after state change
3. Use system sounds (NSSound)

**Decision**: Option 2 (Swift after state change)

**Rationale:**
- Keeps audio logic in UI layer (separation of concerns)
- Easier to toggle on/off without Rust changes
- Access to native AVFoundation features

**Trade-off**: Slight delay between state change and sound (~5-10ms), but acceptable

### Decision 4: Performance Degradation Trigger

**Options:**
1. Manually detect GPU at app launch
2. Dynamically monitor frame rate and degrade
3. User-selectable quality setting

**Decision**: Option 2 (Dynamic monitoring)

**Rationale:**
- Adaptive to actual performance, not just hardware specs
- Handles edge cases (thermal throttling, background load)
- Better user experience (automatic)

**Trade-off**: Requires continuous monitoring overhead (~0.5% CPU)

## Security Considerations

1. **Theme Asset Loading**: Validate all theme assets are from app bundle, no external loading
2. **Error Message Display**: Sanitize error messages to avoid XSS-like attacks via Halo text display
3. **Audio File Validation**: Verify audio files are valid AIFF before playback
4. **UserDefaults**: Use codable structs, not raw strings, to prevent injection attacks

## Compatibility

- **Minimum macOS**: 13.0 (Ventura)
- **Recommended macOS**: 14.0+ (Sonoma) for best Metal shader support
- **Hardware**:
  - High quality: M1/M2 Macs, Intel Iris Plus/Pro
  - Medium quality: Intel HD 5000/6000
  - Low quality: Intel HD 3000/4000

## Rollback Plan

If Phase 3 enhancements cause regressions:

1. **Theme System**: Fallback to hardcoded Zen theme (simplest)
2. **Streaming Text**: Disable text display, show spinner only
3. **Audio**: Disable sound effects (silent mode)
4. **Error UX**: Revert to generic error overlay
5. **Performance Optimizations**: Disable Metal shaders, use SwiftUI shapes only

Each component is independently toggleable via feature flags in UserDefaults.

## Testing Strategy

### Unit Tests
- Theme selection logic
- Preferences encoding/decoding
- Performance monitor FPS calculation
- Audio manager sound loading

### Integration Tests
- Theme switching with crossfade animation
- Streaming text accumulation
- Error retry flow (mock network)
- Queue management under rapid hotkeys

### Performance Tests
- Profile HaloView render time (< 16ms per frame)
- Measure memory usage with all 3 themes loaded (< 50MB delta)
- Test animation smoothness on Intel HD 4000 (target 30fps minimum)

### Manual Tests
- VoiceOver announcements on all state changes
- Sound playback at different system volumes
- Multi-monitor Halo positioning with themes
- Error action buttons functionality

## Open Implementation Questions

1. **Metal Shader Complexity**: Should cyberpunk glitch effect use pre-computed textures or real-time shaders?
   - **Recommendation**: Pre-computed textures for Phase 3; defer real-time shaders to Phase 6

2. **Streaming Text Line Wrapping**: How to handle long responses that exceed 3 lines?
   - **Recommendation**: Implement horizontal scrolling marquee for overflow text

3. **Theme Transition Animation**: Crossfade duration when user switches themes?
   - **Recommendation**: 0.5s crossfade with easeInOut curve

4. **Error Retry Logic Location**: Implement in Rust or Swift?
   - **Recommendation**: Rust core owns retry logic; Swift only triggers via callback

## Dependencies

### Swift Package Dependencies (if needed)
- None (use built-in AVFoundation, Metal, SwiftUI)

### Rust Crate Dependencies (if needed)
- None (existing tokio, reqwest sufficient)

## Migration Path from Phase 2

1. **No breaking changes** to existing Phase 2 APIs
2. Add new callbacks to `AlephEventHandler` (backward compatible via default implementations)
3. Extend `HaloState` enum with new associated values (existing cases unchanged)
4. HaloWindow initialization accepts optional `ThemeEngine` (defaults to Zen theme)

## Future Extensibility (Beyond Phase 3)

- **Phase 4**: Provider-specific custom animations (OpenAI = ripple, Claude = wave)
- **Phase 5**: User-uploaded custom theme JSON files
- **Phase 6**: Advanced effects: particle systems, shader art, generative backgrounds
- **Cross-platform**: Windows/Linux theme system will use same protocol pattern
