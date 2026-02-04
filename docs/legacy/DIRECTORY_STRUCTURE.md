# Aleph Directory Structure

This document provides a detailed view of the project's directory structure.

## Overview

Aleph is a Monorepo with platform-specific directories:
- **macOS**: Native Swift + SwiftUI with XcodeGen
- **Tauri**: Cross-platform (Windows, Linux) with React + TypeScript
- **Windows (Native)**: ARCHIVED - use Tauri instead

## Rust Core Module Count: ~40 Public Modules

| Category | Modules |
|----------|---------|
| **FFI** | 22 sub-modules (ffi/) |
| **Agent** | agent_loop/, agents/, components/ (8 components) |
| **Config** | config/ (types + policies + watcher) |
| **AI** | generation/ (10+ providers), providers/, rig_tools/ |
| **Memory** | Dual-layer (Raw + Facts), compression, SIMD |
| **Routing** | dispatcher/ (planner, scheduler, executor, model_router), intent/ (3 layers) |
| **Tools** | mcp/, skills/, search/ (6 providers), video/, vision/ |
| **Runtime** | runtimes/ (uv, fnm, yt-dlp, ffmpeg) |
| **Infra** | services/, event/ (GlobalBus), conversation/, payload/, three_layer/, thinker/ |
| **Permission** | permission/ (rules, config, manager), question/ (structured Q&A) |
| **Compressor** | compressor/ (SmartCompactor, TurnProtector, ToolTruncator) |

## Complete Directory Tree

```
aleph/
├── .github/
│   └── workflows/
│       ├── rust-core.yml              # Rust CI (test, lint, build)
│       ├── macos-app.yml              # macOS app build
│       └── tauri-app.yml              # Tauri cross-platform build
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
│       ├── aleph.udl                 # UniFFI interface definition
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
│       ├── agents/                    # Unified agent system (sub-agents + delegation)
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
│       │   ├── mod.rs, types.rs       # SessionPart types (enhanced with StepStart/Finish, Snapshot, Patch)
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
│       ├── compressor/                # Smart context compression
│       │   ├── mod.rs, context_stats.rs, strategy.rs
│       │   ├── smart_strategy.rs      # SmartCompactionStrategy (NEW)
│       │   ├── smart_compactor.rs     # Unified SmartCompactor (NEW)
│       │   ├── tool_truncator.rs      # ToolTruncator with summary (NEW)
│       │   ├── turn_protector.rs      # TurnProtector for recent turns (NEW)
│       │   └── tests_integration/     # Integration tests
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
│       ├── event/                     # Event-driven architecture (Enhanced)
│       │   ├── mod.rs, bus.rs, types.rs, handler.rs
│       │   ├── global_bus.rs          # GlobalBus singleton (NEW)
│       │   ├── filter.rs              # EventFilter with builder (NEW)
│       │   ├── permission.rs          # Permission event types (NEW)
│       │   ├── question.rs            # Question event types (NEW)
│       │   └── tests/integration.rs   # Integration tests (NEW)
│       │
│       ├── ffi/                       # 22 FFI sub-modules
│       │   ├── mod.rs, processing.rs, tools.rs, memory.rs
│       │   ├── config.rs, skills.rs, mcp.rs, dispatcher.rs
│       │   ├── dispatcher_types.rs, generation.rs, init.rs
│       │   ├── session.rs, runtime.rs, agent_loop_adapter.rs
│       │   ├── plugins.rs, plan_confirmation.rs
│       │   ├── tool_discovery.rs      # Smart tool filtering
│       │   ├── dag_executor.rs        # DAG task execution
│       │   ├── prompt_helpers.rs      # Prompt building utils
│       │   ├── provider_factory.rs    # AI provider creation
│       │   ├── subscription.rs        # Event subscription FFI (NEW)
│       │   ├── typo_correction.rs, user_input.rs
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
│       ├── mcp/                       # Model Context Protocol (Enhanced)
│       │   ├── mod.rs                 # Module exports
│       │   ├── client.rs              # McpClient, McpClientBuilder
│       │   ├── types.rs               # MCP protocol types
│       │   ├── notifications.rs       # McpNotificationRouter, McpEvent
│       │   ├── prompts.rs             # McpPromptManager
│       │   ├── resources.rs           # McpResourceManager
│       │   ├── jsonrpc/               # JSON-RPC 2.0 protocol
│       │   ├── external/              # Server connection management
│       │   │   ├── connection.rs      # McpServerConnection
│       │   │   └── runtime.rs         # Runtime detection
│       │   ├── transport/             # Transport abstraction layer
│       │   │   ├── traits.rs          # McpTransport trait
│       │   │   ├── stdio.rs           # StdioTransport (local)
│       │   │   ├── http.rs            # HttpTransport (remote)
│       │   │   └── sse.rs             # SseTransport (bidirectional)
│       │   └── auth/                  # OAuth 2.0 authentication
│       │       ├── storage.rs         # OAuthStorage, OAuthTokens
│       │       ├── provider.rs        # OAuthProvider (PKCE)
│       │       └── callback.rs        # CallbackServer
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
│       ├── permission/                # Permission system (NEW - OpenCode compatible)
│       │   ├── mod.rs, error.rs       # Module exports and error types
│       │   ├── rule.rs                # PermissionRule, PermissionEvaluator
│       │   ├── config.rs              # PermissionConfig, default_config()
│       │   └── manager.rs             # PermissionManager with async ask/reply
│       │
│       ├── question/                  # Structured Q&A system (NEW)
│       │   ├── mod.rs, error.rs       # Module exports and error types
│       │   └── manager.rs             # QuestionManager with ask/reply
│       │
│       ├── discovery/                 # Multi-level component discovery (Claude Code compatible)
│       │   ├── mod.rs, scanner.rs, paths.rs, types.rs
│       │
│       ├── extension/                 # Extension system v2 (async FFI via UniFFI 0.31+)
│       │   ├── mod.rs, loader.rs, registry.rs, types.rs
│       │   ├── config/                # aleph.jsonc configuration
│       │   ├── hooks/                 # HookExecutor
│       │   ├── runtime/               # Node.js plugin runtime
│       │   └── sync_api.rs            # SyncExtensionManager (legacy)
│       │
│       ├── prompt/                    # Unified prompt management
│       │   ├── mod.rs, executor.rs, conversational.rs
│       │
│       ├── providers/                 # AI provider implementations
│       │   ├── mod.rs, rig_providers.rs
│       │
│       ├── rig_tools/                 # AlephTool implementations (search, web_fetch, etc.)
│       │   ├── mod.rs, error.rs, mcp_wrapper.rs
│       │   ├── search.rs, web_fetch.rs, youtube.rs, file_ops.rs
│       │   ├── skill_reader.rs        # read_skill, list_skills tools (NEW)
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
│       │   ├── mod.rs, model_router.rs
│       │   ├── prompt_builder.rs      # System prompt with tool_index, skill_mode
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
│   │   ├── Aleph/
│   │   │   ├── Info.plist
│   │   │   ├── Aleph.entitlements
│   │   │   ├── config.example.toml
│   │   │   ├── Assets.xcassets/       # App icons, colors
│   │   │   ├── Frameworks/            # libalephcore.dylib
│   │   │   ├── Generated/             # Reference bindings
│   │   │   ├── Resources/
│   │   │   │   ├── en.lproj/          # English localization
│   │   │   │   ├── zh-Hans.lproj/     # Chinese localization
│   │   │   │   ├── skills/            # Built-in skills
│   │   │   │   └── ProviderIcons/     # AI provider icons
│   │   │   └── Sources/
│   │   │       ├── main.swift         # NSApplicationMain entry
│   │   │       ├── AppDelegate.swift  # Menu bar lifecycle
│   │   │       ├── AlephBridgingHeader.h
│   │   │       ├── Generated/         # UniFFI Swift bindings
│   │   │       │   ├── aleph.swift
│   │   │       │   ├── alephFFI.h
│   │   │       │   └── alephFFI.modulemap
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
│   │   ├── AlephTests/               # Unit tests
│   │   └── AlephUITests/             # UI tests
│   │
│   ├── tauri/                         # Tauri Cross-Platform (Windows, Linux)
│   │   ├── package.json               # pnpm workspace config
│   │   ├── src-tauri/                 # Rust backend
│   │   │   ├── Cargo.toml
│   │   │   ├── tauri.conf.json        # Tauri configuration
│   │   │   └── src/                   # Rust source
│   │   └── src/                       # React frontend
│   │       ├── App.tsx
│   │       ├── components/            # React components
│   │       └── i18n/                  # Internationalization
│   │
│   └── windows/                       # [ARCHIVED] Windows Native
│       ├── ARCHIVED.md                # Archive notice
│       ├── Aleph.sln                 # Visual Studio solution
│       └── ...                        # See ARCHIVED.md
│
├── shared/                            # Cross-Platform Resources
│   ├── config/
│   │   └── default-config.toml        # Default configuration
│   ├── locales/                       # Master locale files (future)
│   └── docs/                          # Shared documentation (future)
│
├── scripts/                           # Build Scripts
│   ├── build-core.sh                  # Build Rust core for macOS
│   ├── build-macos.sh                 # macOS full build
│   └── generate-bindings.sh           # FFI binding generation
│
├── Scripts/                           # Legacy scripts (macOS)
│   ├── gen_bindings.sh
│   └── monitor_startup.sh
│
├── docs/                              # Documentation
│   ├── ARCHITECTURE.md
│   ├── CONFIGURATION.md
│   ├── DISPATCHER.md              # Includes Cowork task orchestration
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
