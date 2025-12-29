# Aether

**A system-level AI middleware for macOS** - Invisible AI intelligence at your cursor, zero context switching.

Aether acts as an invisible "ether" connecting your intent with AI models through a frictionless, native interface. No webviews, no dock icon, no windows - just press a hotkey and let AI transform your selected text.

![Phase](https://img.shields.io/badge/Phase-6%20(Settings%20UI)-blue)
![Platform](https://img.shields.io/badge/Platform-macOS%2013%2B-lightgrey)
![Swift](https://img.shields.io/badge/Swift-5.0-orange)
![Rust](https://img.shields.io/badge/Rust-1.70%2B-red)

## ✨ Core Philosophy: "Ghost" Aesthetic

- **Invisible First**: No dock icon, no permanent window. Only background process + menu bar
- **De-GUI**: Ephemeral UI that appears at cursor, then dissolves
- **Frictionless**: Brings AI directly to your cursor without context switching
- **Native-First**: 100% native code - Rust core with SwiftUI, zero webviews

## 🎯 Features

### Transmutation Workflow

1. **Select** text/image in ANY app
2. **Press** global hotkey (Cmd+~ by default)
3. **Watch** beautiful "Halo" overlay appear at your cursor
4. **Receive** AI-transformed result pasted back instantly

### Multi-Model Orchestration

- **Smart Routing**: Automatically route requests to different AI providers based on input patterns
- **Supported Providers**:
  - OpenAI (GPT-4o, GPT-4o-mini)
  - Anthropic Claude (Claude 3.5 Sonnet, Opus)
  - Local Ollama (Llama 3.2, CodeLlama, Mistral, etc.)
  - Custom OpenAI-compatible APIs (DeepSeek, Moonshot, Azure OpenAI)
- **Provider Colors**: Visual feedback with provider-specific Halo colors

### Context-Aware Memory (Local RAG)

- **App/Window-Based Memory**: Aether remembers past interactions per application and window
- **Automatic Context Injection**: Retrieved memories are seamlessly injected into AI prompts
- **Privacy-First**: All memory data stored locally, never synced to cloud
- **Configurable Retention**: Auto-delete memories after N days

### Full Configuration UI

**Modern macOS 26 Design Language:**
- **Integrated Traffic Lights**: Custom red/yellow/green buttons embedded in rounded sidebar
- **Content-First Layout**: Hidden title bar with content extending to window edge
- **Continuous Curves**: 18pt rounded corners for refined, native appearance
- **Adaptive Materials**: Self-adjusting Dark/Light mode backgrounds

**Settings Tabs:**
- **Providers Tab**: Add/edit/delete AI providers, test connections, manage API keys
- **Routing Tab**: Create rules with drag-to-reorder, import/export as JSON
- **Shortcuts Tab**: Customize hotkeys with visual recorder and conflict detection
- **Behavior Tab**: Configure input/output modes, typing speed, PII scrubbing
- **Memory Tab**: View/delete memories, configure retention policies
- **General Tab**: Theme selection, version info, check for updates

### Security & Privacy

- **Keychain Integration**: API keys stored securely in macOS Keychain (not in config files)
- **PII Scrubbing**: Automatically redact sensitive information (emails, phone numbers, SSN, credit cards)
- **Local-First Memory**: Raw memory data never leaves your device
- **Zero Telemetry**: No tracking, no analytics

### Internationalization (i18n)

- **Supported Languages**:
  - 🇬🇧 English (en) - Base language
  - 🇨🇳 Simplified Chinese (zh-Hans) - 简体中文 (100% translated, 249 keys)
- **System Language Detection**: Automatically follows your macOS system language
- **Graceful Fallback**: Unsupported languages fallback to English
- **Complete Coverage**: All UI text, error messages, alerts, and system prompts localized
- **Contribute Translations**: See [docs/LOCALIZATION.md](docs/LOCALIZATION.md) for guidelines
- **Fully Localized**:
  - Settings UI (all tabs: General, Providers, Routing, Shortcuts, Behavior, Memory)
  - Menu bar items and tooltips
  - Permission prompts and system dialogs
  - Error messages and alerts

## 🚀 Quick Start

### Prerequisites

- macOS 13.0 or later
- Xcode 14+ (for building from source)
- Rust 1.70+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- XcodeGen (`brew install xcodegen`)
- Python 3.x with `uniffi-bindgen` (`pip3 install uniffi-bindgen`)

### Building from Source

```bash
# Clone the repository
git clone https://github.com/your-repo/aether.git
cd aether

# Build Rust core
cd Aether/core
cargo build --release

# Generate Xcode project
cd ../..
xcodegen generate

# Open in Xcode and build
open Aether.xcodeproj
```

### First Run

1. **Launch Aether** (no dock icon - check menu bar for ✨ icon)
2. **Grant Accessibility Permission**:
   - System Settings → Privacy & Security → Accessibility
   - Click "+" and add Aether
   - This allows hotkey detection and keyboard simulation
3. **Open Settings** (Cmd+, or click menu bar icon → Settings)
4. **Add AI Provider**:
   - Go to Providers tab
   - Click "Configure" for OpenAI
   - Enter your API key from https://platform.openai.com
   - Click "Save"
5. **Test It**:
   - Select text in any app: "What is the capital of France?"
   - Press `Cmd+~`
   - Watch the magic happen! ✨

## ⚙️ Configuration

### Config File Location

`~/.config/aether/config.toml`

### Quick Configuration

Open Settings (Cmd+,) and use the native UI to configure everything visually.

### Manual Configuration

For advanced users, you can edit `config.toml` directly. See [`config.example.toml`](Aether/config.example.toml) for detailed documentation.

**Hot-Reload**: Changes to `config.toml` take effect within 1 second (no restart needed)

### Example Configuration

```toml
# Shortcuts
[shortcuts]
summon = "Command+Grave"    # Cmd+~ to trigger
cancel = "Escape"           # Esc to cancel

# Behavior
[behavior]
input_mode = "cut"          # cut | copy
output_mode = "typewriter"  # typewriter | instant
typing_speed = 50           # 10-200 chars/sec
pii_scrubbing_enabled = false

# Providers
[providers.openai]
api_key = "sk-..."          # Stored in Keychain, not here
model = "gpt-4o"
color = "#10a37f"

[providers.claude]
api_key = "sk-ant-..."
model = "claude-3-5-sonnet-20241022"
color = "#d97757"

# Routing Rules
[[rules]]
regex = "^/code"            # Prefix with /code for coding
provider = "claude"
system_prompt = "You are a senior engineer. Output code only."

[[rules]]
regex = ".*"                # Catch-all
provider = "openai"
```

### API Key Security

⚠️ **API keys are stored in macOS Keychain, NOT in `config.toml`**

When you add a provider via Settings UI:
1. API key is saved to Keychain as "Aether:provider-name"
2. Config file only contains provider metadata (model, color, etc.)
3. Keys are encrypted and protected by macOS security

**Never commit `config.toml` with API keys to version control!**

## 📖 Documentation

- **[Settings UI Guide](docs/settings-ui-guide.md)**: Complete guide to all settings tabs with screenshots
- **[Configuration Reference](Aether/config.example.toml)**: Detailed config file documentation
- **[Manual Testing Checklist](docs/manual-testing-checklist.md)**: Comprehensive testing guide
- **[Development Guide](CLAUDE.md)**: Architecture, build instructions, development phases

## 🏗️ Architecture

### The New Architecture: Rust Core + UniFFI + Native UI

**NO WEBVIEWS. NO TAURI. NO ELECTRON.**

```
┌─────────────────────────────────────────────────────────┐
│                     macOS Native UI                     │
│              (Swift + SwiftUI + NSWindow)               │
│  Settings │ Menu Bar │ Halo Overlay (Transparent)      │
└──────────────────────┬──────────────────────────────────┘
                       │ UniFFI Bindings
                       │ (Auto-generated FFI)
┌──────────────────────┴──────────────────────────────────┐
│                     Rust Core Library                    │
│                    (cdylib + staticlib)                  │
│                                                           │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────────┐ │
│  │  Clipboard  │  │  Input Sim   │  │  Global Hotkey │ │
│  │  (arboard)  │  │   (enigo)    │  │    (rdev)      │ │
│  └─────────────┘  └──────────────┘  └────────────────┘ │
│                                                           │
│  ┌─────────────────────────────────────────────────────┐│
│  │          Smart Router (Regex-based Rules)           ││
│  └─────────────────────────────────────────────────────┘│
│                                                           │
│  ┌──────────────┐  ┌────────────┐  ┌──────────────────┐│
│  │   OpenAI     │  │   Claude   │  │   Ollama (Local) ││
│  │  (reqwest)   │  │ (reqwest)  │  │  (Command::spawn)││
│  └──────────────┘  └────────────┘  └──────────────────┘│
│                                                           │
│  ┌─────────────────────────────────────────────────────┐│
│  │        Memory Module (Local RAG with LanceDB)       ││
│  │  Embedding: all-MiniLM-L6-v2 (local, no cloud)     ││
│  └─────────────────────────────────────────────────────┘│
│                                                           │
│  ┌─────────────────────────────────────────────────────┐│
│  │     Config (TOML + Keychain + Hot-Reload Watcher)  ││
│  └─────────────────────────────────────────────────────┘│
└───────────────────────────────────────────────────────────┘
```

### Key Technologies

- **Rust Core**: `tokio`, `reqwest`, `arboard`, `enigo`, `rdev`, `lancedb`, `ort`
- **Swift UI**: SwiftUI, AppKit (NSWindow, NSStatusBar)
- **FFI Bridge**: UniFFI (auto-generates Swift bindings from `.udl` file)
- **Config**: TOML (`serde`), Keychain (Security.framework), FSEvents watcher
- **Memory**: Vector DB (LanceDB/SQLite+vec), Embedding model (ONNX Runtime)

## 🛠️ Development

### Project Structure

```
aether/
├── project.yml                # XcodeGen config (source of truth)
├── Aether.xcodeproj/          # Generated by XcodeGen (not in git)
├── Aether/
│   ├── Sources/               # Swift source files
│   │   ├── SettingsView.swift
│   │   ├── ProvidersView.swift
│   │   ├── RoutingView.swift
│   │   ├── HaloWindow.swift
│   │   └── Generated/         # UniFFI bindings (auto-gen)
│   ├── Frameworks/
│   │   └── libaethecore.dylib # Rust library
│   └── core/                  # Rust core library
│       ├── Cargo.toml
│       ├── src/
│       │   ├── lib.rs         # UniFFI exports
│       │   ├── aether.udl     # UniFFI interface definition
│       │   ├── core.rs
│       │   ├── router/
│       │   ├── providers/
│       │   ├── memory/
│       │   └── config/
│       └── uniffi.toml
├── AetherTests/               # Unit tests (Swift)
├── docs/                      # Documentation
└── config.example.toml        # Config template
```

### Running Tests

```bash
# Rust core tests (32 tests)
cd Aether/core
cargo test config:: --lib

# Swift integration tests
xcodegen generate
xcodebuild test -project Aether.xcodeproj -scheme AetherTests
```

### Code Style

- **Rust**: Use `cargo fmt` and `cargo clippy`
- **Swift**: Follow SwiftUI best practices
- **Comments**: English for code, Chinese for user-facing messages (if needed)

## 📋 Development Phases

- ✅ **Phase 1**: Core Infrastructure (Rust + UniFFI + Swift integration)
- ✅ **Phase 2**: Hotkey & Clipboard Integration
- ✅ **Phase 3**: Halo Overlay (Transparent native window)
- ✅ **Phase 4**: Memory Module (Local RAG with context-aware retrieval)
- ✅ **Phase 5**: AI Integration (OpenAI, Claude, Ollama)
- ✅ **Phase 6**: Settings UI (Full configuration management) ← **CURRENT**
- ⏳ **Phase 7**: Polish & Optimization (Image support, PII scrubbing, typewriter effect)

See [CLAUDE.md](CLAUDE.md) for detailed phase breakdown.

## 🔒 Security Considerations

### Accessibility Permissions

Aether requires Accessibility permission to:
- Detect global hotkeys
- Simulate keyboard input (Cmd+C, Cmd+V)
- Query active window title for memory context

**Why this is safe:**
- Open source code (you can audit everything)
- No network access except to configured AI providers
- No telemetry or analytics
- API keys stored in macOS Keychain (encrypted)

### API Key Protection

1. **Never hardcode API keys** in code or config files
2. **Always use Keychain** for storage (Settings UI handles this)
3. **Never commit `config.toml`** with API keys to version control
4. **Use `.gitignore`** to exclude config files with secrets

### PII Scrubbing

When enabled, Aether automatically redacts:
- Email addresses → `[EMAIL_REDACTED]`
- Phone numbers → `[PHONE_REDACTED]`
- SSN → `[SSN_REDACTED]`
- Credit cards → `[CARD_REDACTED]`

This prevents sensitive information from being sent to cloud AI providers.

## 🤝 Contributing

Contributions welcome! Here's how:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

**Before submitting:**
- Run tests: `cargo test` (Rust) and `xcodebuild test` (Swift)
- Format code: `cargo fmt` and `cargo clippy`
- Update documentation if needed

## 🐛 Known Issues

- **Hot-Reload**: File watcher may not detect changes in vim's in-place edit mode (use `:w` not `:wq`)
- **Ollama**: First request may be slow (model loading time)
- **Typewriter**: Very fast speeds (>150 cps) may be choppy on older Macs

See [GitHub Issues](https://github.com/your-repo/aether/issues) for full list.

## 📜 License

[MIT License](LICENSE)

## 🙏 Acknowledgments

- **UniFFI**: Seamless Rust ↔ Swift FFI
- **rdev**: Cross-platform global hotkey detection
- **arboard**: Clipboard management
- **enigo**: Keyboard simulation
- **LanceDB**: Fast vector database for local RAG
- **ONNX Runtime**: Efficient embedding model inference

## 📞 Support

- **Documentation**: See `docs/` directory
- **Issues**: [GitHub Issues](https://github.com/your-repo/aether/issues)
- **Discussions**: [GitHub Discussions](https://github.com/your-repo/aether/discussions)

---

**Made with ❤️ by the Aether team**

*Bringing AI to your fingertips, one hotkey at a time.*
