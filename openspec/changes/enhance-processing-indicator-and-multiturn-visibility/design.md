# Design: Enhance Processing Indicator and Multi-turn Window Visibility

## Window Visibility Setting

### Configuration Schema

```toml
[behavior]
keep_window_visible_during_processing = true  # default: true
```

### Config Flow

```
┌─────────────────┐       ┌─────────────────┐       ┌─────────────────┐
│  config.toml    │  →    │  Rust Core      │  →    │  Swift UI       │
│                 │       │  BehaviorConfig │       │  Settings View  │
└─────────────────┘       └─────────────────┘       └─────────────────┘
        ↑                                                    │
        └────────────────── save changes ────────────────────┘
```

### BehaviorConfig Changes (Rust)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfig {
    // ... existing fields ...

    #[serde(default = "default_keep_window_visible")]
    pub keep_window_visible_during_processing: bool,
}

fn default_keep_window_visible() -> bool {
    true  // Window stays visible by default
}
```

### Settings UI Changes (Swift)

```swift
// BehaviorSettingsView.swift
@State private var keepWindowVisibleDuringProcessing: Bool = true

var keepWindowVisibleCard: some View {
    SettingsCard(
        title: "多轮对话窗口处理时保持显示",
        description: "启用后，AI思考和输出时对话窗口保持可见；关闭后窗口会暂时隐藏",
        icon: "rectangle.and.text.magnifyingglass"
    ) {
        Toggle("", isOn: $keepWindowVisibleDuringProcessing)
            .toggleStyle(.switch)
    }
}
```

## Architecture Overview

### Component Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                        User Interaction                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────────┐     ┌───────────────────────────────────┐ │
│  │ Single-turn Flow │     │      Multi-turn Flow              │ │
│  │  (Double Shift)  │     │    (Cmd+Opt+/)                    │ │
│  └────────┬─────────┘     └───────────────┬───────────────────┘ │
│           │                               │                      │
│           ▼                               ▼                      │
│  ┌────────────────┐            ┌─────────────────────────────┐  │
│  │InputCoordinator│            │  UnifiedInputCoordinator    │  │
│  └────────┬───────┘            │  ┌───────────────────────┐  │  │
│           │                     │  │ UnifiedInputView      │  │  │
│           │                     │  │ ┌─────────────────┐   │  │  │
│           │                     │  │ │ SubPanel (CLI)  │   │  │  │
│           │                     │  │ └─────────────────┘   │  │  │
│           │                     │  └───────────────────────┘  │  │
│           │                     └─────────────┬───────────────┘  │
│           │                                   │                  │
│           ▼                                   ▼                  │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │              ProcessingIndicatorWindow                       ││
│  │  ┌─────────────────────────────────────────────────────────┐││
│  │  │ Position Strategy:                                      │││
│  │  │  1. Try CaretPositionHelper.getBestPosition()          │││
│  │  │  2. Fallback:                                          │││
│  │  │     - Single-turn: NSEvent.mouseLocation               │││
│  │  │     - Multi-turn: Window corner position               │││
│  │  └─────────────────────────────────────────────────────────┘││
│  └─────────────────────────────────────────────────────────────┘│
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

## Multi-turn Window Visibility

### Behavior Based on Setting

**Option A: `keepWindowVisibleDuringProcessing = true` (Default)**
```
User sends message → Window stays visible → SubPanel shows "Processing..." → Response in CLI
                     ^^^^^^^^^^^^^^^^^^
                     Good UX: continuous context
```

**Option B: `keepWindowVisibleDuringProcessing = false`**
```
User sends message → Window hides → Indicator at cursor/fallback → Window reappears
                     ^^^^^^^^^^^^
                     Clean UX: minimal visual clutter
```

### Implementation Strategy

**UnifiedInputCoordinator changes:**
```swift
func handleConversationInput(_ input: String) {
    let keepVisible = configManager.behavior.keepWindowVisibleDuringProcessing

    if keepVisible {
        // Show processing status in SubPanel CLI
        subPanelState?.showCLIOutput(initialLines: [
            CLIOutputLine(type: .command, content: input),
            CLIOutputLine(type: .thinking, content: "AI 思考中...")
        ])
    } else {
        // Hide window during processing
        haloWindowController?.hide()
    }

    // Show processing indicator at cursor/window corner
    showProcessingIndicator()

    // Start conversation (async)
    conversationCoordinator?.continueConversation(...)
}
```

**OutputCoordinator changes:**
```swift
func executeTypewriterOutput(text: String, speed: Int, context: OutputContext) {
    let keepVisible = configManager.behavior.keepWindowVisibleDuringProcessing

    // For multi-turn with keepVisible, keep window
    if context.sessionType == .multiTurn && keepVisible {
        // Keep window visible, output will appear in target app
    } else {
        haloWindowController?.hide()  // Hide for single-turn or keepVisible=false
    }
}
```

## Processing Indicator Window

### Window Properties

```swift
final class ProcessingIndicatorWindow: NSWindow {
    // Window configuration
    styleMask: .borderless
    level: .floating
    backgroundColor: .clear
    isOpaque: false
    ignoresMouseEvents: true  // Click-through

    // Size: 48x48 (compact indicator)
    static let indicatorSize: CGFloat = 48
}
```

### SwiftUI Indicator View

```swift
struct ProcessingIndicatorView: View {
    @State private var rotation: Double = 0

    var body: some View {
        ZStack {
            // Blur background circle
            Circle()
                .fill(.ultraThinMaterial)
                .frame(width: 44, height: 44)

            // Spinning arc (theme color)
            Circle()
                .trim(from: 0, to: 0.7)
                .stroke(Color.accentColor, style: StrokeStyle(lineWidth: 3, lineCap: .round))
                .frame(width: 28, height: 28)
                .rotationEffect(.degrees(rotation))
                .animation(.linear(duration: 0.8).repeatForever(autoreverses: false))
                .onAppear { rotation = 360 }
        }
    }
}
```

### Position Tracking Logic

```swift
enum IndicatorPositionMode {
    case singleTurn
    case multiTurn(windowFrame: NSRect)
}

func getIndicatorPosition(mode: IndicatorPositionMode) -> NSPoint {
    // Step 1: Try cursor position
    if let caretPos = CaretPositionHelper.getCaretPosition(),
       isValidPosition(caretPos) {
        return caretPos
    }

    // Step 2: Fallback based on mode
    switch mode {
    case .singleTurn:
        // Fall back to mouse position
        return NSEvent.mouseLocation

    case .multiTurn(let windowFrame):
        // Fall back to window's top-left corner (with padding)
        return NSPoint(
            x: windowFrame.minX + 20,
            y: windowFrame.maxY - 20
        )
    }
}
```

### Integration Points

**Single-turn flow (InputCoordinator):**
```
Hotkey → Capture input → Show indicator at cursor/mouse
       → AI processing → Hide indicator
       → Output response
```

**Multi-turn flow (UnifiedInputCoordinator):**
```
Submit message → Show indicator at cursor/window corner
              → CLI shows "Processing..."
              → AI processing → Hide indicator
              → CLI shows response
              → Window stays visible
```

## State Machine Considerations

### Window State During Processing

**When `keepWindowVisibleDuringProcessing = true`:**

| Scenario | Window State | Indicator Position |
|----------|--------------|-------------------|
| Single-turn start | `.processing` | Cursor → Mouse |
| Multi-turn start | `.unifiedInput` (stays) | Cursor → Window corner |
| Multi-turn continue | `.unifiedInput` (unchanged) | Cursor → Window corner |
| ESC pressed | `.idle` → hide | Hide indicator |

**When `keepWindowVisibleDuringProcessing = false`:**

| Scenario | Window State | Indicator Position |
|----------|--------------|-------------------|
| Single-turn start | `.processing` | Cursor → Mouse |
| Multi-turn start | `.idle` (hides) | Cursor → Mouse |
| Multi-turn continue | `.idle` (hides) | Cursor → Mouse |
| ESC pressed | `.idle` → hide | Hide indicator |

### Indicator Lifecycle

```
Show: When AI processing begins
Update: Track position if cursor moves (optional for v1)
Hide: When AI response starts arriving
```

## Testing Strategy

### Manual Test Cases

| Test Case | Steps | Expected Result |
|-----------|-------|-----------------|
| Setting: keepVisible=true | Start Cmd+Opt+/, send message | Window stays visible |
| Setting: keepVisible=false | Start Cmd+Opt+/, send message | Window hides, indicator shows |
| Toggle setting | Change setting, restart, verify | Behavior matches setting |
| Single-turn indicator | Double-shift, observe | Indicator at cursor/mouse |
| Multi-turn indicator (visible) | Cmd+Opt+/, send, observe | Indicator at cursor/window corner |
| Multi-turn indicator (hidden) | Cmd+Opt+/, send, observe | Indicator at cursor/mouse |
| ESC dismissal | Cmd+Opt+/, ESC | Window hides, indicator gone |
| Cursor unavailable | Test in app without AX support | Correct fallback used |

### Apps to Test
1. **Notes.app** - Has good Accessibility support (cursor position available)
2. **TextEdit** - Basic Accessibility support
3. **Terminal** - Limited Accessibility (tests fallback)
4. **Electron apps** - Variable support (tests fallback robustness)
