# Aether Directory Structure

This document provides a detailed view of the project's directory structure.

## Overview

Aether is a Monorepo with platform-specific directories. macOS uses [XcodeGen](https://github.com/yonaskolb/XcodeGen).

## Rust Core Module Count: 44 Modules

| Category | Modules |
|----------|---------|
| **FFI** | 9 sub-modules |
| **Agent** | agent/, agents/, components/ (8 modules) |
| **Config** | 10 type modules + policies |
| **AI** | generation/ (10+ providers), providers/, rig_tools/ |
| **Memory** | Dual-layer (Raw + Facts), compression |
| **Routing** | dispatcher/, intent/ (3 layers), router/ |
| **Tools** | mcp/, skills/, search/ (6 providers), video/, vision/ |
| **Runtime** | runtimes/ (uv, fnm, yt-dlp) |
| **Infra** | services/, event/, conversation/, cowork/, payload/ |

## Complete Directory Tree

```
aether/
├── .github/
│   └── workflows/
│       ├── rust-core.yml              # Rust CI (test, lint, build)
│       ├── macos-app.yml              # macOS app build
│       └── windows-app.yml            # Windows app build
│
├── core/                              # Rust Core Library
│   ├── Cargo.toml                     # [features] uniffi, cabi
│   ├── build.rs                       # UniFFI build script
│   ├── uniffi.toml                    # UniFFI configuration
│   ├── benches/                       # Performance benchmarks
│   ├── bindings/                      # Pre-generated bindings (reference)
│   ├── examples/                      # Usage examples
│   ├── tests/                         # Integration tests
│   └── src/
│       ├── lib.rs                     # Public API, UniFFI scaffolding
│       ├── aether.udl                 # UniFFI interface definition
│       ├── ffi_cabi.rs                # Windows C ABI exports
│       ├── error.rs                   # Error types
│       ├── initialization.rs          # First-time setup
│       ├── event_handler.rs           # Event callback traits
│       ├── cowork_ffi.rs              # Cowork FFI bindings
│       ├── title_generator.rs         # Conversation title generation
│       ├── uniffi_core.rs             # UniFFI core re-exports
│       │
│       ├── agent/                     # Agent execution engine
│       │   ├── mod.rs, config.rs, manager.rs, types.rs, conversation.rs
│       │
│       ├── agents/                    # Sub-agent system (Phase 4)
│       │   ├── mod.rs, registry.rs, task_tool.rs, types.rs
│       │   └── prompts/               # Agent system prompts
│       │
│       ├── capability/                # Capability definitions
│       │   ├── mod.rs, declaration.rs, executor.rs, vision.rs
│       │
│       ├── clarification/             # Phantom Flow interaction
│       │   ├── mod.rs, types.rs
│       │
│       ├── clipboard/                 # Image types (for AI providers)
│       │   └── mod.rs
│       │
│       ├── command/                   # Command completion system
│       │   ├── mod.rs, types.rs, registry.rs, suggestions.rs
│       │
│       ├── components/                # 8 Core agentic loop components
│       │   ├── mod.rs
│       │   ├── callback_bridge.rs     # Rust-Swift communication
│       │   ├── intent_analyzer.rs     # Intent detection
│       │   ├── loop_controller.rs     # Agentic loop state
│       │   ├── session_compactor.rs   # Memory compression
│       │   ├── session_recorder.rs    # Conversation history
│       │   ├── subagent_handler.rs    # Sub-agent orchestration
│       │   ├── task_planner.rs        # Multi-step planning
│       │   └── tool_executor.rs       # Unified tool dispatch
│       │
│       ├── config/                    # Configuration types
│       │   ├── mod.rs, types.rs, policies.rs, provider_entry.rs
│       │
│       ├── conversation/              # Multi-turn conversation
│       │   ├── mod.rs, manager.rs, session.rs, turn.rs
│       │
│       ├── core/                      # Internal core types
│       │   ├── mod.rs, types.rs, memory_types.rs
│       │
│       ├── cowork/                    # DAG task orchestration
│       │   ├── mod.rs, executor.rs, graph.rs, model_router.rs
│       │   ├── file_operations.rs, code_executor.rs
│       │
│       ├── dispatcher/                # Multi-layer routing
│       │   ├── mod.rs, config.rs, integration.rs
│       │   ├── tool_registry.rs, tool_types.rs
│       │
│       ├── event/                     # Event-driven architecture
│       │   ├── mod.rs, bus.rs, types.rs, handlers.rs
│       │
│       ├── ffi/                       # 9 FFI sub-modules
│       │   ├── mod.rs, core.rs, config_ffi.rs
│       │   ├── memory_ffi.rs, conversation_ffi.rs
│       │   ├── dispatcher_ffi.rs, mcp_ffi.rs
│       │   ├── search_ffi.rs, skills_ffi.rs
│       │
│       ├── generation/                # Media generation providers
│       │   ├── mod.rs, types.rs, registry.rs, mock.rs
│       │   └── providers/             # 10+ generation backends
│       │
│       ├── intent/                    # 3-layer intent detection
│       │   ├── mod.rs, classifier.rs, aggregator.rs
│       │   ├── calibrator.rs, cache.rs, types.rs
│       │   ├── context.rs, defaults.rs, presets.rs
│       │   └── rollback.rs
│       │
│       ├── logging/                   # Structured logging
│       │   ├── mod.rs, file_logging.rs, pii_layer.rs
│       │
│       ├── mcp/                       # Model Context Protocol
│       │   ├── mod.rs, types.rs, service.rs, manager.rs
│       │
│       ├── memory/                    # Dual-layer memory system
│       │   ├── mod.rs, database.rs, context.rs
│       │   ├── embedding.rs, facts.rs, retrieval.rs
│       │
│       ├── metrics/                   # Performance metrics
│       │   └── mod.rs
│       │
│       ├── payload/                   # Structured context protocol
│       │   ├── mod.rs, builder.rs, types.rs
│       │
│       ├── providers/                 # AI provider implementations
│       │   ├── mod.rs, rig_providers.rs
│       │
│       ├── rig_tools/                 # rig-core tool definitions
│       │   ├── mod.rs, registry.rs
│       │   ├── builtin_tools.rs, mcp_tools.rs, skill_tools.rs
│       │   └── generation/            # Generation tool wrappers
│       │
│       ├── runtimes/                  # Runtime managers
│       │   ├── mod.rs, registry.rs, traits.rs
│       │   ├── uv.rs, fnm.rs, ytdlp.rs
│       │
│       ├── search/                    # 6 search providers
│       │   ├── mod.rs, registry.rs, types.rs
│       │
│       ├── services/                  # Background services
│       │   ├── mod.rs, file_ops.rs, git_ops.rs, system_info.rs
│       │
│       ├── skills/                    # Skill system
│       │   ├── mod.rs, types.rs, registry.rs, installer.rs
│       │
│       ├── suggestion/                # AI response parsing
│       │   └── mod.rs
│       │
│       ├── utils/                     # Utilities
│       │   ├── mod.rs, pii.rs, text.rs
│       │
│       ├── video/                     # Video processing
│       │   ├── mod.rs, transcript.rs, youtube.rs
│       │
│       └── vision/                    # Vision capability
│           ├── mod.rs, service.rs, types.rs
│
├── platforms/
│   ├── macos/                         # macOS Application
│   │   ├── project.yml                # XcodeGen configuration
│   │   ├── Aether/
│   │   │   ├── Info.plist
│   │   │   ├── Aether.entitlements
│   │   │   ├── config.example.toml
│   │   │   ├── Assets.xcassets/       # App icons, colors
│   │   │   ├── Frameworks/            # libaethecore.dylib
│   │   │   ├── Generated/             # Reference bindings
│   │   │   ├── Resources/
│   │   │   │   ├── en.lproj/          # English localization
│   │   │   │   ├── zh-Hans.lproj/     # Chinese localization
│   │   │   │   ├── skills/            # Built-in skills
│   │   │   │   └── ProviderIcons/     # AI provider icons
│   │   │   └── Sources/
│   │   │       ├── main.swift         # NSApplicationMain entry
│   │   │       ├── AppDelegate.swift  # Menu bar lifecycle
│   │   │       ├── AetherBridgingHeader.h
│   │   │       ├── Generated/         # UniFFI Swift bindings
│   │   │       │   ├── aether.swift
│   │   │       │   ├── aetherFFI.h
│   │   │       │   └── aetherFFI.modulemap
│   │   │       ├── Components/
│   │   │       │   ├── Atoms/         # Basic UI elements
│   │   │       │   ├── Molecules/     # Composed components
│   │   │       │   ├── Organisms/     # Complex UI sections
│   │   │       │   └── Window/        # Window controllers
│   │   │       ├── Controllers/       # View controllers
│   │   │       ├── Coordinator/       # Input/Output/MultiTurn
│   │   │       ├── DesignSystem/      # Theme, colors, fonts
│   │   │       ├── DI/                # Dependency injection
│   │   │       ├── Extensions/        # Swift extensions
│   │   │       ├── Handlers/          # Event handlers
│   │   │       ├── Managers/          # State managers
│   │   │       ├── Models/            # Data models
│   │   │       ├── MultiTurn/         # Multi-turn conversation UI
│   │   │       ├── Protocols/         # Swift protocols
│   │   │       ├── Services/          # Swift services
│   │   │       ├── Store/             # State store
│   │   │       ├── Utils/             # Utilities
│   │   │       └── Vision/            # Screen capture UI
│   │   ├── AetherTests/               # Unit tests
│   │   └── AetherUITests/             # UI tests
│   │
│   └── windows/                       # Windows Application
│       ├── Aether.sln                 # Visual Studio solution
│       ├── Aether/
│       │   ├── Aether.csproj          # .NET 8.0 WinUI 3
│       │   ├── App.xaml               # Application entry
│       │   ├── App.xaml.cs
│       │   ├── MainWindow.xaml        # Main window (placeholder)
│       │   ├── MainWindow.xaml.cs
│       │   ├── app.manifest           # Windows manifest
│       │   ├── Interop/
│       │   │   └── NativeMethods.g.cs # csbindgen P/Invoke
│       │   └── libs/                  # aethecore.dll
│       └── Aether.Tests/              # Unit tests
│
├── shared/                            # Cross-Platform Resources
│   ├── config/
│   │   └── default-config.toml        # Default configuration
│   ├── locales/                       # Master locale files (future)
│   └── docs/                          # Shared documentation (future)
│
├── scripts/                           # Build Scripts
│   ├── build-core.sh                  # Build Rust (macos/windows/all)
│   ├── build-macos.sh                 # macOS full build
│   ├── build-windows.ps1              # Windows full build
│   └── generate-bindings.sh           # FFI binding generation
│
├── Scripts/                           # Legacy scripts (macOS)
│   ├── gen_bindings.sh
│   └── monitor_startup.sh
│
├── docs/                              # Documentation
│   ├── ARCHITECTURE.md
│   ├── CONFIGURATION.md
│   ├── DISPATCHER.md
│   ├── COWORK.md
│   └── ...
│
├── openspec/                          # OpenSpec change management
│   ├── AGENTS.md
│   ├── project.md
│   ├── specs/                         # Specifications
│   └── changes/                       # Change proposals
│
├── CLAUDE.md                          # AI assistant instructions
├── README.md
├── Cargo.toml                         # Workspace root
├── Cargo.lock
└── VERSION                            # 0.1.0
```
