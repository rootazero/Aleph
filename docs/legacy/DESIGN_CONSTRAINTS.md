# Aleph Design Constraints

This document outlines the key design constraints, anti-patterns to avoid, and critical success factors for the Aleph project.

## Key Design Constraints

### Modularity Requirements

Use trait-based abstractions for all core components:
- `AiProvider`, `Router`, `MemoryStore`
- `EmbeddingModel`, `SearchProvider`
- `RuntimeManager`

### Critical UI Behavior

**macOS Application Entry**:
- Use `main.swift` + `NSApplicationMain()` (macOS 26 bug workaround)
- Do NOT use SwiftUI `@main App` lifecycle on macOS 26+

**macOS Settings Window**:
- Use `NSPanel` (not NSWindow) for keyboard support without Dock activation
- Configure: `styleMask: [.titled, .closable, .resizable, .nonactivatingPanel]`

**macOS Halo Window**:
- `NSWindow` with `styleMask: .borderless`, `level: .floating`
- `backgroundColor: .clear`, `isOpaque: false`
- `ignoresMouseEvents: true` (click-through)
- **NEVER** call `makeKeyAndOrderFront()` to avoid focus theft

### Privacy & Security

- **PII Scrubbing**: Regex-based removal before API calls
- **Local-First**: All config and memory stored locally
- **No Telemetry**: Zero tracking, no analytics
- **API Key Storage**: macOS Keychain via Security framework

---

## Anti-Patterns to Avoid

### Architecture
- DO NOT use webviews (violates native-first principle)
- DO NOT create permanent GUI windows (violates "Ghost" philosophy)
- DO NOT hardcode AI providers (must be config-driven)
- DO NOT bypass RigAgentManager for AI calls
- DO NOT manually manage runtime installations (use RuntimeRegistry)

### macOS Specific
- DO NOT use SwiftUI App lifecycle on macOS 26+ (use main.swift + NSApplicationMain)
- DO NOT create Settings as NSWindow (use NSPanel)
- DO NOT call `makeKeyAndOrderFront()` on Halo window

### Concurrency
- DO NOT block main thread during API calls (use tokio async)
- DO NOT put business logic in Swift (belongs in Rust core only)

### FFI
- DO NOT manually write FFI bindings (use UniFFI)
- DO NOT ignore FFI boundary safety (use proper error handling)

### Permissions
- DO NOT ignore permissions errors (especially Accessibility)
- DO NOT skip permission pre-check in Rust core

---

## Critical Success Factors

1. **Zero Focus Loss**: Halo must never interfere with active window
2. **Sub-100ms Latency**: From hotkey press to Halo appearance
3. **Reliable Clipboard**: Handle all content types (text, images, rich text)
4. **Robust Permissions**: Clear UX for granting Accessibility access
5. **Memory Safety**: No crashes at FFI boundary
6. **Smooth Animations**: 60fps Halo transitions
7. **Auto Runtime**: Runtimes install/update without user intervention

---

## HaloState Machine (21 States)

```swift
enum HaloState {
    case idle, hidden, appearing, listening, thinking, processing
    case streaming, success, error, disappearing
    case multiTurnActive, multiTurnThinking, multiTurnStreaming
    case toolExecuting, toolSuccess, toolError
    case clarificationNeeded, clarificationReceived
    case agentPlanning, agentExecuting, agentComplete
}
```

---

## Settings UI Tabs (10+)

| Tab | Purpose |
|-----|---------|
| **General** | Theme, version, updates |
| **Providers** | AI provider configuration |
| **Routing** | Rule editor with drag-to-reorder |
| **Shortcuts** | Hotkey recorder |
| **Behavior** | Input/output modes |
| **Memory** | View/delete history, retention policies |
| **MCP** | MCP server configuration |
| **Skills** | Skill management |
| **Cowork** | Task orchestration settings |
| **Policies** | System behavior fine-tuning |
| **Runtimes** | Runtime version management |
