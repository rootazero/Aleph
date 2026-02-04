# Project Context

## Purpose

**Aleph** is a system-level AI middleware for macOS (with future Windows/Linux support) that acts as an invisible "ether" connecting user intent with AI models through a frictionless, native interface. The project embodies a "Ghost" aesthetic - no permanent windows, no dock icon, only ephemeral UI that appears at the cursor when summoned.

**Core Value Proposition:**
- Brings AI intelligence directly to the cursor in ANY application
- Zero context switching - user never leaves their current app
- Multi-AI orchestration with smart routing (OpenAI, Claude, Gemini, Ollama)
- Native-first architecture - zero webviews, maximum performance

## Tech Stack

### Backend (Rust Core)
- **Language**: Rust (compiled as `cdylib` + `staticlib`)
- **FFI Bridge**: Mozilla UniFFI (automatic binding generation)
- **Async Runtime**: `tokio`
- **HTTP Client**: `reqwest`
- **Clipboard**: `arboard` (text & images)
- **Keyboard Simulation**: `enigo`
- **Global Hotkeys**: `rdev`
- **Configuration**: `serde` + TOML

### Frontend (Native UI)
- **macOS (Primary)**: Swift + SwiftUI
  - NSWindow for transparent overlay (Halo)
  - NSStatusBar for menu bar integration
  - No Dock icon (LSUIElement=YES)
- **Windows (Future)**: C# + WinUI 3
- **Linux (Future)**: Rust + GTK4

### AI Providers
- OpenAI (GPT-4o, DALL-E)
- Anthropic (Claude)
- Google (Gemini)
- Local models (Ollama)

## Project Conventions

### Code Style

**Rust:**
- Use `clippy` with strict lints
- Format with `rustfmt`
- Prefer trait-based abstractions for modularity
- Use `Arc<Mutex<T>>` for shared state across threads
- All public APIs must be async-safe (use `tokio` primitives)
- Error handling: Use `Result<T, E>` with custom error types, never panic in library code

**Swift:**
- SwiftUI for all UI components
- Use Swift Concurrency (async/await) for async operations
- Prefix callback methods with `on` (e.g., `onStateChanged`, `onHaloShow`)
- Use `DispatchQueue.main.async` for UI updates from Rust callbacks
- Follow Apple's Human Interface Guidelines for menu bar apps

**Naming Conventions:**
- Rust modules: `snake_case`
- Rust types: `PascalCase`
- Swift types: `PascalCase`
- Swift properties: `camelCase`
- UniFFI interfaces: `PascalCase` for types, `snake_case` for methods
- Change IDs: `kebab-case`, verb-led (e.g., `add-halo-overlay`, `update-clipboard-manager`)

### Architecture Patterns

**Separation of Concerns:**
- **Rust Core**: All business logic, AI routing, clipboard/input management, configuration
- **Native UI**: Only rendering, user interaction, and implementing event handler callbacks
- **NO business logic in Swift** - all decisions happen in Rust

**Communication Pattern:**
- Rust → UniFFI → Swift (callback-based)
- Rust defines `AlephEventHandler` trait
- Swift implements the trait to receive state updates
- Use `Arc<dyn AlephEventHandler>` in Rust for thread-safe callbacks

**Modularity:**
- All core components use traits: `ClipboardManager`, `InputSimulator`, `AiProvider`, `Router`
- Easy swapping of implementations
- Mock implementations for testing

**Key Architectural Principles:**
1. **Native-First**: No webviews, no Electron, no Tauri for UI
2. **Invisible-First**: No permanent windows, Halo overlay is ephemeral
3. **Focus Protection**: UI must NEVER steal focus from active application
4. **Privacy-First**: PII scrubbing, local-first config, no telemetry

### Testing Strategy

**Rust Core:**
- Unit tests for all modules (`cargo test`)
- Integration tests for clipboard/keyboard simulation
- Mock AI providers for deterministic E2E tests
- Test coverage: Aim for 80%+ on core routing/provider logic
- Use `#[cfg(test)]` modules in each source file

**Swift Client:**
- XCTest for SwiftUI components
- Manual testing preferred for overlay window behavior (focus, transparency)
- Test accessibility permissions flow
- Test config loading/saving

**Manual Testing Critical Paths:**
- Global hotkey detection across different apps
- Clipboard operations (text, images, rich text)
- Halo overlay appearance/animations
- Focus preservation during Cut/Paste cycle
- Permission prompts (Accessibility, Keychain)

### Git Workflow

**Branching Strategy:**
- `main` - Production-ready code
- `feature/[change-id]` - For OpenSpec change proposals (e.g., `feature/add-halo-overlay`)
- `bugfix/[description]` - For bug fixes
- Create PR for each change proposal

**Commit Conventions:**
- Follow [Conventional Commits](https://www.conventionalcommits.org/)
- Format: `type(scope): description`
- Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`
- Scope: `core`, `macos`, `uniffi`, `config`, `providers`
- Example: `feat(core): add smart clipboard fallback mechanism`

**OpenSpec Integration:**
- Create change proposal BEFORE implementation (Stage 1)
- Link PR to change proposal in PR description
- After merge and deployment, archive the change (Stage 3)
- Keep `openspec/specs/` in sync with production code

## Domain Context

### User Interaction Flow ("Transmutation")
1. User selects text/image in ANY app (WeChat, Notes, VSCode, etc.)
2. Presses global hotkey (Cmd+~ on macOS)
3. Aleph simulates Cut (Cmd+X) - content "disappears" for physical feedback
4. Halo spinner appears at cursor location (transparent, click-through overlay)
5. Rust routes clipboard content to appropriate AI provider based on rules
6. AI responds, Rust writes result to clipboard
7. Rust simulates Paste (Cmd+V)
8. Halo dissolves with success animation

### Context Acquisition Strategy: "Smart Fallback"
1. **The Probe**: Simulate `Cmd + C` silently
2. **The Check**: Compare new clipboard with previous state
   - **Scenario A (Explicit Selection)**: Clipboard changed → Process selected text only
   - **Scenario B (Implicit Context)**: No change → Assume no selection
3. **The Fallback**: For Scenario B, simulate `Cmd + A` (Select All) + `Cmd + X` (Cut)
4. **Processing**: Route to appropriate AI provider

### AI Routing Logic
- **Regex-based rules**: Match clipboard content against user-defined patterns
- **Provider selection**: Route to OpenAI, Claude, Gemini, or Ollama based on rules
- **Fallback strategy**: Retry with default provider on timeout/error
- **System prompt injection**: Each rule can override system prompt

### Halo States
- **Idle**: Hidden
- **Listening**: Micro-contraction animation (< 200ms)
- **Processing**: Smooth spinner (color = provider theme color)
- **Success**: Green checkmark → fade out
- **Error**: Red shake + brief error text overlay

## Important Constraints

### Performance Constraints
- **Sub-100ms latency**: From hotkey press to Halo appearance
- **60fps animations**: Halo transitions must be smooth
- **No blocking**: All AI calls must be async (tokio runtime)
- **Memory efficient**: Use streaming for large clipboard content

### Platform Constraints (macOS)
- **Accessibility permissions required**: For keyboard simulation
- **No Dock icon**: LSUIElement=YES in Info.plist
- **Menu bar only**: NSStatusBar for settings access
- **Focus protection**: NSWindow must never steal focus (`ignoresMouseEvents: true`)
- **Sandboxing**: App must request minimal entitlements

### Security Constraints
- **PII scrubbing**: Regex-based removal of phone/email before cloud API calls
- **Local-first**: Config stored in `~/.aleph/config.toml`
- **No telemetry**: Zero tracking, no analytics
- **API key storage**: Use macOS Keychain (via `Security` framework in Swift)

### UI Constraints
- **Halo Window Requirements**:
  - NSWindow with `styleMask: .borderless`
  - `level: .floating` (above all apps)
  - `backgroundColor: .clear`
  - `isOpaque: false`
  - `ignoresMouseEvents: true` (click-through)
  - `collectionBehavior: [.canJoinAllSpaces, .stationary, .ignoresCycle]`
- **Never call `makeKeyAndOrderFront()`** - use `orderFrontRegardless()`

## External Dependencies

### AI APIs
- **OpenAI API**: `https://api.openai.com/v1` (supports custom base URLs for proxies)
- **Anthropic Claude API**: `https://api.anthropic.com/v1`
- **Google Gemini**: CLI-based (`gemini-cli` command)
- **Ollama**: Local CLI execution (`ollama run [model]`)

### macOS System APIs
- **Accessibility API**: AXIsProcessTrusted() for keyboard simulation permissions
- **Keychain Services**: Secure API key storage
- **NSStatusBar**: Menu bar integration
- **NSWindow**: Overlay window management

### Rust Crates
- `uniffi` - FFI binding generation
- `tokio` - Async runtime
- `reqwest` - HTTP client
- `arboard` - Clipboard manager
- `enigo` - Keyboard/mouse simulation
- `rdev` - Global hotkey listener
- `serde` - Serialization (config.toml)
- `toml` - TOML parser

### Build Tools
- **Rust**: `cargo` (1.70+)
- **Swift**: Xcode 15+, Swift 5.9+
- **UniFFI**: `uniffi-bindgen` for binding generation
- **macOS**: macOS 13+ (Ventura) for development

## Configuration Storage

**Location**: `~/.aleph/config.toml`

**Schema**: TOML format with sections:
- `[general]` - Theme, sound, default provider
- `[shortcuts]` - Hotkey bindings
- `[behavior]` - Input/output modes, typing speed
- `[providers.*]` - API keys, models, base URLs per provider
- `[[rules]]` - Routing rules (regex → provider + system prompt)

**Access Pattern**:
- Swift UI reads/writes config via Rust core API
- Rust core watches config file for changes (hot-reload)
- Changes trigger re-initialization of providers/router

## Anti-Patterns to Avoid

- **DO NOT** use webviews (violates native-first principle)
- **DO NOT** create permanent GUI windows (violates "Ghost" philosophy)
- **DO NOT** require manual app switching (defeats frictionless UX)
- **DO NOT** hardcode AI providers (must be config-driven)
- **DO NOT** ignore permissions errors (especially Accessibility)
- **DO NOT** block main thread during API calls (use tokio async)
- **DO NOT** put business logic in Swift (belongs in Rust core only)
- **DO NOT** manually write FFI bindings (use UniFFI)
- **DO NOT** steal focus with Halo window (use `ignoresMouseEvents: true`)
