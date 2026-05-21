# Design: Refactor Unified Halo Window

## Architecture Overview

### Current Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        AppDelegate                          │
│                            │                                │
│         ┌──────────────────┼──────────────────┐             │
│         ↓                  ↓                  ↓             │
│  CommandModeCoordinator  HaloWindowController  ConversationManager
│         │                  │                  │             │
│         ↓                  ↓                  ↓             │
│   CommandListView     HaloWindow        ConversationInputView
│   (独立状态)          (多状态)           (内嵌)             │
└─────────────────────────────────────────────────────────────┘
```

### Target Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        AppDelegate                          │
│                            │                                │
│                            ↓                                │
│               UnifiedInputCoordinator                       │
│                            │                                │
│         ┌──────────────────┼──────────────────┐             │
│         ↓                  ↓                  ↓             │
│   FocusDetector    UnifiedHaloWindow    ConversationManager │
│         │                  │                  │             │
│         │         ┌────────┴────────┐         │             │
│         │         ↓                 ↓         │             │
│         │   MainInputView     SubPanelView    │             │
│         │         │                 │         │             │
│         │         │    ┌────────────┴────────┐│             │
│         │         │    ↓       ↓       ↓     ││             │
│         │         │ CommandList Selector CLI ││             │
│         └─────────┴────────────────────┴─────┘│             │
└─────────────────────────────────────────────────────────────┘
```

## Component Design

### 1. UnifiedInputCoordinator

**职责**：统一管理输入流程，替代原有的CommandModeCoordinator

```swift
@MainActor
final class UnifiedInputCoordinator {
    // Dependencies
    private weak var core: AlephCore?
    private weak var haloWindowController: HaloWindowController?
    private let focusDetector = FocusDetector()

    // State
    private var targetAppInfo: TargetAppInfo?
    private var hotkeyMonitor: Any?

    // MARK: - Hotkey Handling

    func handleUnifiedHotkey() {
        // 1. Check focus state
        guard let focusInfo = focusDetector.checkInputFocus() else {
            showFocusWarningToast()
            return
        }

        // 2. Store target app info
        targetAppInfo = focusInfo

        // 3. Show unified Halo window
        showUnifiedHaloWindow(at: focusInfo.caretPosition)
    }

    func outputToTargetApp(_ text: String) {
        guard let target = targetAppInfo else { return }
        // Use keyboard simulation to type text to target app
        KeyboardSimulator.shared.typeText(text)
    }
}
```

### 2. FocusDetector

**职责**：检测当前光标是否聚焦于文本输入区域

```swift
struct TargetAppInfo {
    let bundleId: String
    let windowTitle: String
    let caretPosition: NSPoint
    let focusedElement: AXUIElement?
}

final class FocusDetector {
    /// Check if cursor is focused in a text input field
    /// - Returns: Target app info if focused, nil otherwise
    func checkInputFocus() -> TargetAppInfo? {
        let systemWide = AXUIElementCreateSystemWide()

        // Get focused element
        var focusedRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(
            systemWide,
            kAXFocusedUIElementAttribute as CFString,
            &focusedRef
        ) == .success else {
            return nil
        }

        let element = focusedRef as! AXUIElement

        // Check if element is text input
        var roleRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(
            element,
            kAXRoleAttribute as CFString,
            &roleRef
        ) == .success else {
            return nil
        }

        let role = roleRef as? String
        let isTextInput = role == kAXTextFieldRole as String ||
                          role == kAXTextAreaRole as String ||
                          role == kAXComboBoxRole as String

        guard isTextInput else { return nil }

        // Get caret position
        let caretPosition = getCaretPosition(from: element)

        // Get app info
        let bundleId = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? ""
        let windowTitle = getWindowTitle()

        return TargetAppInfo(
            bundleId: bundleId,
            windowTitle: windowTitle,
            caretPosition: caretPosition,
            focusedElement: element
        )
    }
}
```

### 3. SubPanel State Machine

**状态枚举**：

```swift
enum SubPanelMode: Equatable {
    /// Panel is hidden (no content)
    case hidden

    /// Command completion list (triggered by `/` prefix)
    case commandCompletion(commands: [CommandNode], selectedIndex: Int)

    /// AI selector for user choices
    case selector(options: [SelectorOption], prompt: String, multiSelect: Bool)

    /// CLI output stream (shows AI backend operations)
    case cliOutput(lines: [CLIOutputLine])

    /// Confirmation dialog
    case confirmation(
        title: String,
        message: String,
        confirmLabel: String,
        cancelLabel: String
    )
}

struct SelectorOption: Identifiable, Equatable {
    let id: String
    let label: String
    let description: String?
    let isSelected: Bool
}

struct CLIOutputLine: Identifiable, Equatable {
    let id: UUID
    let timestamp: Date
    let type: CLIOutputType
    let content: String
}

enum CLIOutputType {
    case info
    case success
    case warning
    case error
    case command
}
```

### 4. SubPanelView Design

**视觉规范**：

```
┌─────────────────────────────────────┐
│  ← 1px 分隔线                       │
├─────────────────────────────────────┤
│                                     │
│    Content Area                     │  ← 动态高度 (0-300px)
│    - CommandList / Selector / CLI   │
│                                     │
├─────────────────────────────────────┤
│  ↑↓ Navigate  ⏎ Select  ⎋ Cancel   │  ← 操作提示 (可选)
└─────────────────────────────────────┘
```

**SwiftUI实现**：

```swift
struct SubPanelView: View {
    @ObservedObject var state: SubPanelState
    let maxHeight: CGFloat = 300

    var body: some View {
        VStack(spacing: 0) {
            // Divider
            if state.mode != .hidden {
                Divider()
                    .background(Color.secondary.opacity(0.3))
            }

            // Content
            contentView
                .frame(maxHeight: calculatedHeight)
                .clipped()

            // Hints (optional)
            if shouldShowHints {
                hintsView
            }
        }
        .background(
            RoundedRectangle(cornerRadius: 8)
                .fill(.ultraThinMaterial)
                .shadow(
                    color: .black.opacity(0.15),
                    radius: 6,
                    x: 0,
                    y: 4
                )
        )
        .animation(.spring(response: 0.3), value: state.mode)
    }

    @ViewBuilder
    private var contentView: some View {
        switch state.mode {
        case .hidden:
            EmptyView()

        case .commandCompletion(let commands, let selectedIndex):
            CommandCompletionList(
                commands: commands,
                selectedIndex: selectedIndex
            )

        case .selector(let options, let prompt, let multiSelect):
            SelectorView(
                options: options,
                prompt: prompt,
                multiSelect: multiSelect
            )

        case .cliOutput(let lines):
            CLIOutputView(lines: lines)

        case .confirmation(let title, let message, let confirm, let cancel):
            ConfirmationView(
                title: title,
                message: message,
                confirmLabel: confirm,
                cancelLabel: cancel
            )
        }
    }

    private var calculatedHeight: CGFloat {
        switch state.mode {
        case .hidden:
            return 0
        case .commandCompletion(let commands, _):
            return min(CGFloat(commands.count * 36 + 40), maxHeight)
        case .selector(let options, _, _):
            return min(CGFloat(options.count * 44 + 60), maxHeight)
        case .cliOutput(let lines):
            return min(CGFloat(lines.count * 20 + 20), maxHeight)
        case .confirmation:
            return 120
        }
    }
}
```

### 5. Unified Input Flow

**输入处理流程**：

```swift
// In UnifiedInputCoordinator
func processInput(_ text: String) {
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return }

    // Check for command prefix
    if trimmed.hasPrefix("/") {
        processCommand(trimmed)
    } else {
        processConversation(trimmed)
    }
}

private func processCommand(_ input: String) {
    // Parse command: "/en hello world" → command="en", content="hello world"
    let parts = input.dropFirst().split(separator: " ", maxSplits: 1)
    let commandKey = String(parts.first ?? "")
    let content = parts.count > 1 ? String(parts[1]) : ""

    // Route to command handler
    // The command is executed with content, not typed to app first
    core?.processCommand(
        command: commandKey,
        content: content,
        callback: { [weak self] result in
            self?.outputToTargetApp(result)
        }
    )
}

private func processConversation(_ input: String) {
    // Continue conversation
    conversationManager.submitContinuationInput(input)
}
```

## Data Flow

### 1. Hotkey → Halo Display

```
User presses Cmd+Opt+/
        ↓
UnifiedInputCoordinator.handleUnifiedHotkey()
        ↓
FocusDetector.checkInputFocus()
        ↓
    ┌───┴───────────────────────┐
    ↓                           ↓
  Focused                   Not Focused
    ↓                           ↓
Store TargetAppInfo       Show Toast
    ↓                       "请先点击输入框"
Show UnifiedHaloWindow
at caret position
```

### 2. Input → SubPanel Update

```
User types in MainInputView
        ↓
Input text changes
        ↓
Check prefix
        ↓
    ┌───┴───────────────────────┐
    ↓                           ↓
Starts with "/"            Normal text
    ↓                           ↓
Extract command prefix      SubPanel.hidden
    ↓
Filter commands
    ↓
SubPanel.commandCompletion(filtered)
```

### 3. Command Execution → Output

```
User presses Enter
        ↓
Parse input: "/en hello world"
        ↓
command="en", content="hello world"
        ↓
core.processCommand()
        ↓
SubPanel.cliOutput (optional, for long operations)
        ↓
AI Response
        ↓
outputToTargetApp(result)
        ↓
Hide Halo
```

## Animation Specifications

### SubPanel Height Animation

```swift
// Spring animation for smooth height transitions
.animation(
    .spring(
        response: 0.3,      // Duration
        dampingFraction: 0.8, // Bounciness
        blendDuration: 0.1
    ),
    value: subPanelHeight
)
```

### Halo Appear/Disappear

```swift
// Fade + scale for appear
.transition(
    .opacity.combined(with: .scale(scale: 0.95))
)
.animation(.easeOut(duration: 0.2), value: isVisible)
```

## Error Handling

### Focus Detection Failures

```swift
enum FocusDetectionResult {
    case focused(TargetAppInfo)
    case notFocused
    case accessibilityDenied
    case unknownError(Error)
}

// Handle each case appropriately
switch focusDetector.detectFocus() {
case .focused(let info):
    showHalo(at: info.caretPosition)
case .notFocused:
    showToast("请先点击输入框", type: .warning)
case .accessibilityDenied:
    showPermissionPrompt()
case .unknownError(let error):
    NSLog("[FocusDetector] Error: \(error)")
    // Fallback: show at mouse position
    showHalo(at: NSEvent.mouseLocation)
}
```

## Testing Strategy

### Unit Tests

1. **FocusDetector**
   - Test focus detection in various app types
   - Test caret position extraction
   - Test edge cases (no focus, permission denied)

2. **SubPanelState**
   - Test state transitions
   - Test height calculations
   - Test command filtering

3. **UnifiedInputCoordinator**
   - Test command parsing
   - Test conversation flow
   - Test output routing

### Integration Tests

1. **End-to-End Flow**
   - Hotkey → Halo → Input → SubPanel → Output
   - Command completion selection
   - Conversation continuation

### Manual Tests

1. Focus detection in:
   - VS Code
   - Notes
   - WeChat
   - Safari text fields
   - Terminal (special case)

2. SubPanel animations
3. Multi-monitor behavior
