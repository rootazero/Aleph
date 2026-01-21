# Aether Directory Structure

This document provides a detailed view of the project's directory structure.

## Overview

Aether is a Monorepo with platform-specific directories. macOS uses [XcodeGen](https://github.com/yonaskolb/XcodeGen).

## Rust Core Module Count: ~37 Modules

| Category | Modules |
|----------|---------|
| **FFI** | 14 sub-modules (ffi/) |
| **Agent** | agent_loop/, agents/, components/ (8 components) |
| **Config** | config/ (types + policies + watcher) |
| **AI** | generation/ (10+ providers), providers/, rig_tools/ |
| **Memory** | Dual-layer (Raw + Facts), compression, SIMD |
| **Routing** | dispatcher/ (planner, scheduler, executor, model_router), intent/ (3 layers) |
| **Tools** | mcp/, skills/, search/ (6 providers), video/, vision/ |
| **Runtime** | runtimes/ (uv, fnm, yt-dlp, ffmpeg) |
| **Infra** | services/, event/, conversation/, payload/, three_layer/, thinker/ |

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
│       ├── event_handler.rs           # Event callback traits
│       ├── title_generator.rs         # Conversation title generation
│       ├── uniffi_core.rs             # UniFFI core re-exports
│       │
│       ├── agent_loop/                # Core observe-think-act-feedback cycle
│       │   ├── mod.rs, config.rs, decision.rs, state.rs
│       │   ├── callback.rs, guards.rs
│       │
│       ├── agents/                    # Unified agent system (sub-agent + rig-core)
│       │   ├── mod.rs, registry.rs, task_tool.rs, types.rs
│       │   └── integration_test.rs
│       │
│       ├── capability/                # Capability system (Strategy pattern)
│       │   ├── mod.rs, declaration.rs, request.rs, response_parser.rs
│       │   ├── strategy.rs, system.rs
│       │   └── strategies/            # Capability strategy implementations
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
│       ├── config/                    # Configuration management
│       │   ├── mod.rs, watcher.rs, tests.rs
│       │   └── types/                 # Config type definitions
│       │       ├── policies/          # Policy types
│       │
│       ├── conversation/              # Multi-turn conversation
│       │   ├── mod.rs, manager.rs, session.rs, turn.rs
│       │
│       ├── compressor/                # Context compression
│       │   ├── mod.rs, context.rs
│       │
│       ├── core/                      # Internal core types
│       │   ├── mod.rs, types.rs, memory_types.rs
│       │
│       ├── dispatcher/                # Multi-layer routing & task orchestration
│       │   ├── mod.rs, engine.rs, integration.rs, types.rs
│       │   ├── registry.rs, confirmation.rs, async_confirmation.rs
│       │   ├── cowork_types/          # DAG task definitions
│       │   ├── planner/               # LLM task decomposition
│       │   ├── scheduler/             # DAG scheduling
│       │   ├── executor/              # Task execution backends
│       │   ├── monitor/               # Progress monitoring
│       │   └── model_router/          # Intelligent model selection
│       │       ├── core/              # Core routing logic
│       │       ├── health/            # Health monitoring
│       │       ├── resilience/        # Fault tolerance
│       │       ├── intelligent/       # Smart routing P2
│       │       └── advanced/          # Advanced features P3
│       │
│       ├── event/                     # Event-driven architecture
│       │   ├── mod.rs, bus.rs, types.rs, handlers.rs
│       │
│       ├── ffi/                       # 14 FFI sub-modules
│       │   ├── mod.rs, processing.rs, tools.rs, memory.rs
│       │   ├── config.rs, skills.rs, mcp.rs, dispatcher.rs
│       │   ├── dispatcher_types.rs, generation.rs, init.rs
│       │   ├── session.rs, runtime.rs, agent_loop_adapter.rs
│       │
│       ├── generation/                # Media generation providers
│       │   ├── mod.rs, types.rs, registry.rs, mock.rs
│       │   └── providers/             # 10+ generation backends
│       │
│       ├── init_unified/              # Unified initialization
│       │   ├── mod.rs, coordinator.rs
│       │
│       ├── intent/                    # 3-layer intent detection
│       │   ├── mod.rs
│       │   ├── detection/             # L1-L3 classification
│       │   │   ├── classifier.rs, ai_detector.rs
│       │   ├── decision/              # Execution decision making
│       │   │   ├── router.rs, aggregator.rs, calibrator.rs
│       │   │   └── execution_decider.rs
│       │   ├── parameters/            # Parameter types
│       │   │   ├── types.rs, presets.rs, defaults.rs, context.rs
│       │   ├── support/               # Caching, rollback
│       │   │   ├── cache.rs, rollback.rs, agent_prompt.rs
│       │   └── types/                 # Core types
│       │       ├── task_category.rs, ffi.rs
│       │
│       ├── logging/                   # Structured logging
│       │   ├── mod.rs, file_logging.rs, pii_layer.rs
│       │
│       ├── mcp/                       # Model Context Protocol
│       │   ├── mod.rs, types.rs, service.rs, manager.rs
│       │
│       ├── memory/                    # Dual-layer memory system
│       │   ├── mod.rs, database.rs, context.rs, embedding.rs
│       │   ├── retrieval.rs, ai_retrieval.rs, fact_retrieval.rs
│       │   ├── ingestion.rs, augmentation.rs, cleanup.rs, simd.rs
│       │
│       ├── metrics/                   # Performance metrics
│       │   └── mod.rs
│       │
│       ├── payload/                   # Structured context protocol
│       │   ├── mod.rs, builder.rs, assembler.rs
│       │   ├── capability.rs, context_format.rs, intent.rs
│       │
│       ├── prompt/                    # Unified prompt management
│       │   ├── mod.rs, executor.rs, conversational.rs
│       │
│       ├── providers/                 # AI provider implementations
│       │   ├── mod.rs, rig_providers.rs
│       │
│       ├── rig_tools/                 # rig-core tool implementations
│       │   ├── mod.rs, error.rs, mcp_wrapper.rs
│       │   ├── search.rs, web_fetch.rs, youtube.rs, file_ops.rs
│       │   └── generation/            # Generation tool wrappers
│       │
│       ├── runtimes/                  # Runtime managers
│       │   ├── mod.rs, registry.rs, manager.rs, manifest.rs
│       │   ├── download.rs, uv.rs, fnm.rs, ytdlp.rs, ffmpeg.rs
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
│       ├── thinker/                   # LLM decision-making layer
│       │   ├── mod.rs, model_router.rs, prompt_builder.rs
│       │   ├── decision_parser.rs, tool_filter.rs
│       │
│       ├── three_layer/               # Control architecture
│       │   ├── mod.rs, tests.rs
│       │   ├── orchestrator/          # FSM state machine
│       │   ├── safety/                # Capability gating
│       │   └── skill/                 # Skill definition
│       │
│       ├── executor/                  # Task execution engine
│       │   ├── mod.rs, single_step.rs, types.rs
│       │   └── builtin_registry.rs
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
