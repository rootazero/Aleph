# Liquid Glass Multi-Turn Window Design

> Design document for redesigning multi-turn conversation windows using macOS 26 Liquid Glass design language.

**Date**: 2026-01-12
**Status**: Approved
**Target**: macOS 26+ with backward compatibility

---

## Overview

Redesign the multi-turn conversation windows (ConversationDisplayWindow and MultiTurnInputWindow) using Apple's Liquid Glass design language introduced in macOS 26 (Tahoe).

### Design Decisions Summary

| Feature | Decision |
|---------|----------|
| Window Layout | Keep dual-window separation |
| Material Variant | `.regular` |
| Theme Tint | No tint, pure glass |
| Message Bubbles | Glass bubbles with GlassEffectContainer fusion |
| Button Style | Primary: `.glassProminent`, Secondary: SF Symbols |
| Corner Radius | `.containerConcentric` coordinated corners |
| List Animation | GlassEffectContainer Morphing |
| Compatibility | Conditional compilation + runtime check |

---

## Architecture

### Window Structure (Unchanged)

```
┌─────────────────────────────────────────────────────┐
│                                    ┌──────────────┐ │
│                                    │ Display      │ │
│                                    │ Window       │ │
│                                    │ (Top-right)  │ │
│                                    └──────────────┘ │
│                                                     │
│              ┌────────────────────┐                 │
│              │   Input Window     │                 │
│              │   (Center)         │                 │
│              └────────────────────┘                 │
│                                                     │
└─────────────────────────────────────────────────────┘
```

### Core Changes

| Component | Current | Liquid Glass |
|-----------|---------|--------------|
| Window Background | `VisualEffectBackground(.hudWindow)` | `.glassEffect(.regular)` |
| Message Bubbles | `Color.purple.opacity(0.2)` | `.glassEffect()` + `GlassEffectContainer` |
| Send Button | `.buttonStyle(.plain)` | `.buttonStyle(.glassProminent)` |
| Corner Radius | `cornerRadius: 12` | `RoundedRectangle(cornerRadius: .containerConcentric)` |
| List Animation | `.spring()` | `GlassEffectContainer` + `.glassEffectID()` morphing |

---

## Implementation Details

### 1. Compatibility Layer - AdaptiveGlassModifier

Create a unified view modifier that encapsulates version detection:

```swift
// AdaptiveGlassModifier.swift

import SwiftUI

struct AdaptiveGlassModifier: ViewModifier {
    func body(content: Content) -> some View {
        if #available(macOS 26, *) {
            content
                .glassEffect(.regular, in: RoundedRectangle(
                    cornerRadius: .containerConcentric
                ))
        } else {
            // Fallback: Current VisualEffect implementation
            content
                .background(
                    VisualEffectBackground(
                        material: .hudWindow,
                        blendingMode: .behindWindow
                    )
                )
                .clipShape(RoundedRectangle(cornerRadius: 12))
        }
    }
}

extension View {
    func adaptiveGlass() -> some View {
        modifier(AdaptiveGlassModifier())
    }
}
```

### 2. Message Bubbles - Glass Effect with Fusion

```swift
// GlassMessageBubbleView (macOS 26+)

@available(macOS 26, *)
struct GlassMessageBubbleView: View {
    let message: ConversationMessage
    let onCopy: () -> Void
    @Namespace private var bubbleNamespace

    private var isUser: Bool { message.role == .user }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            if isUser { Spacer(minLength: 40) }

            Text(message.content)
                .font(.system(size: 13))
                .textSelection(.enabled)
                .padding(12)
                .glassEffect(.regular, in: RoundedRectangle(
                    cornerRadius: .containerConcentric
                ))
                .glassEffectID(message.id, in: bubbleNamespace)

            if !isUser { Spacer(minLength: 40) }
        }
    }
}
```

Message list container with GlassEffectContainer:

```swift
// macOS 26+: Use GlassEffectContainer for bubble fusion
GlassEffectContainer(spacing: 8) {
    ForEach(viewModel.messages) { message in
        GlassMessageBubbleView(message: message, onCopy: {...})
    }
}

// Earlier versions: Keep current implementation
VStack(spacing: 12) {
    ForEach(viewModel.messages) { message in
        MessageBubbleView(message: message, onCopy: {...})
    }
}
```

### User/AI Differentiation (Without Color)

After removing color distinction:
- **Position alignment**: User right-aligned, AI left-aligned
- **Avatar icon (optional)**: Small icon before AI messages
- **Font weight (optional)**: AI uses `.regular`, User uses `.medium`

### 3. Input Window

```swift
struct MultiTurnInputView: View {
    @ObservedObject var viewModel: MultiTurnInputViewModel
    @Namespace private var inputNamespace

    var body: some View {
        VStack(spacing: 0) {
            contentArea
            Spacer(minLength: 0)
        }
    }

    private var contentArea: some View {
        VStack(spacing: 0) {
            inputField

            if viewModel.showCommandList {
                commandList
            }

            if viewModel.showTopicList {
                topicList
            }
        }
        .adaptiveGlass()  // Unified Glass effect
    }
}
```

### 4. Send Button

```swift
private var sendButton: some View {
    Group {
        if #available(macOS 26, *) {
            Button(action: viewModel.submit) {
                Image(systemName: "arrow.up")
                    .font(.system(size: 16, weight: .semibold))
            }
            .buttonStyle(.glassProminent)
            .buttonBorderShape(.circle)
            .controlSize(.regular)
            .disabled(viewModel.inputText.isEmpty)
        } else {
            // Fallback: Current implementation
            Button(action: viewModel.submit) {
                Image(systemName: "arrow.up.circle.fill")
                    .font(.system(size: 24))
                    .foregroundColor(.purple)
            }
            .buttonStyle(.plain)
            .disabled(viewModel.inputText.isEmpty)
        }
    }
}
```

### 5. List Morphing Animation

```swift
@available(macOS 26, *)
struct GlassInputContentView: View {
    @ObservedObject var viewModel: MultiTurnInputViewModel
    @Namespace private var morphNamespace

    var body: some View {
        GlassEffectContainer(spacing: 0) {
            // Input field area
            inputField
                .glassEffectID("input", in: morphNamespace)

            // Command list - morphs with input field
            if viewModel.showCommandList {
                commandList
                    .glassEffectID("commandList", in: morphNamespace)
                    .transition(.opacity.combined(with: .move(edge: .top)))
            }

            // Topic list
            if viewModel.showTopicList {
                topicList
                    .glassEffectID("topicList", in: morphNamespace)
                    .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
        .animation(.smooth(duration: 0.3), value: viewModel.showCommandList)
        .animation(.smooth(duration: 0.3), value: viewModel.showTopicList)
    }
}
```

### 6. List Row Items

```swift
@available(macOS 26, *)
struct GlassCommandRowView: View {
    let command: CommandNode
    let isSelected: Bool
    let onSelect: () -> Void

    @State private var isHovering = false

    var body: some View {
        Button(action: onSelect) {
            HStack(spacing: 12) {
                Image(systemName: command.icon.isEmpty ? "terminal" : command.icon)
                    .frame(width: 20)

                VStack(alignment: .leading, spacing: 2) {
                    Text("/\(command.key)")
                        .font(.system(size: 14, weight: .medium))
                    Text(command.description)
                        .font(.caption)
                        .foregroundColor(.secondary)
                }

                Spacer()
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(
                (isHovering || isSelected)
                    ? .white.opacity(0.1)
                    : .clear
            )
        }
        .buttonStyle(.plain)
        .onHover { isHovering = $0 }
    }
}
```

### 7. Conversation Display Window

```swift
struct ConversationDisplayView: View {
    @ObservedObject var viewModel: ConversationDisplayViewModel
    @State private var messagesContentHeight: CGFloat = 100

    var body: some View {
        VStack(spacing: 0) {
            titleBar
            Divider().opacity(0.3)

            if viewModel.hasMessages {
                messagesList
            } else {
                emptyState
            }

            if viewModel.isLoading {
                loadingIndicator
            }
        }
        .frame(width: 360)
        .adaptiveGlass()  // Unified Glass effect
    }

    private var titleBar: some View {
        HStack {
            // Remove purple dot, use cleaner design
            Text(viewModel.displayTitle)
                .font(.headline)
                .lineLimit(1)

            Spacer()

            // Copy button - shown on hover
            copyAllButton
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
    }

    private var loadingIndicator: some View {
        HStack(spacing: 6) {
            ForEach(0..<3, id: \.self) { index in
                Circle()
                    .fill(.primary.opacity(0.4))
                    .frame(width: 6, height: 6)
            }
        }
        .padding(.vertical, 10)
        // Use .primary instead of purple for pure glass style
    }
}
```

---

## File Changes

| File | Action | Description |
|------|--------|-------------|
| `AdaptiveGlassModifier.swift` | Create | Compatibility wrapper |
| `ConversationDisplayView.swift` | Modify | Glass background + message bubbles |
| `ConversationDisplayWindow.swift` | Minor | Remove clipShape (handled by Glass) |
| `MultiTurnInputView.swift` | Modify | Glass input + Morphing list |
| `MultiTurnInputWindow.swift` | Minor | Window config adjustments |

---

## Implementation Order

```
1. Create AdaptiveGlassModifier.swift
   └── Encapsulate version detection and fallback logic

2. Modify ConversationDisplayView
   ├── Replace background with .adaptiveGlass()
   ├── Add GlassMessageBubbleView (macOS 26+)
   └── Simplify title bar and loading indicator

3. Modify MultiTurnInputView
   ├── Replace background with .adaptiveGlass()
   ├── Add GlassInputContentView (macOS 26+ Morphing)
   ├── Update send button to .glassProminent
   └── Update list row styles

4. Testing
   ├── macOS 26 simulator/device testing
   └── Earlier version fallback testing
```

---

## References

- [Apple Liquid Glass Overview](https://www.apple.com/newsroom/2025/06/apple-introduces-a-delightful-and-elegant-new-software-design/)
- [iOS 26 Liquid Glass Reference (GitHub)](https://github.com/conorluddy/LiquidGlassReference)
- [WWDC 2025 Liquid Glass Design System](https://dev.to/arshtechpro/wwdc-2025-apples-liquid-glass-design-system-52an)
