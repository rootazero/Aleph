# Tasks: Claude Code Compatible Plugin System

## 1. Core Framework (Phase 1) ✅
- [x] 1.1 Create `core/src/plugins/mod.rs` with module structure
- [x] 1.2 Implement `error.rs` with plugin error types
- [x] 1.3 Implement `types.rs` with shared data structures
- [x] 1.4 Implement `manifest.rs` for plugin.json parsing
- [x] 1.5 Implement `scanner.rs` for plugin discovery
- [x] 1.6 Implement `loader.rs` for plugin loading orchestration
- [x] 1.7 Implement `registry.rs` for plugin storage and state
- [x] 1.8 Add `plugins` module to `core/src/lib.rs`

## 2. Skill Support (Phase 2) ✅
- [x] 2.1 Implement `components/skill.rs` for SKILL.md parsing
- [x] 2.2 Add YAML frontmatter parser (description, name, disable-model-invocation)
- [x] 2.3 Implement `$ARGUMENTS` placeholder support
- [x] 2.4 Add skill namespace handling (plugin-name:skill-name)
- [x] 2.5 Integrate with Thinker's PromptConfig for skill injection

## 3. Hook Support (Phase 3) ✅
- [x] 3.1 Implement `components/hook.rs` for hooks.json parsing
- [x] 3.2 Create HookEvent to EventType mapping
- [x] 3.3 Implement matcher regex support for tool filtering
- [x] 3.4 Implement command hook action (shell execution)
- [x] 3.5 Implement prompt hook action (LLM evaluation)
- [x] 3.6 Implement agent hook action (agent invocation)
- [x] 3.7 Add `${CLAUDE_PLUGIN_ROOT}` variable substitution
- [x] 3.8 Integrate with EventBus subscription system

## 4. Agent Support (Phase 4) ✅
- [x] 4.1 Implement `components/agent.rs` for agent.md parsing
- [x] 4.2 Parse YAML frontmatter (description, capabilities)
- [x] 4.3 Convert PluginAgent to Aether's AgentDef format
- [x] 4.4 Integrate with AgentRegistry for agent registration

## 5. MCP Integration (Phase 5) ✅
- [x] 5.1 Implement `components/mcp.rs` for .mcp.json parsing
- [x] 5.2 Implement runtime path resolution in `integrator.rs`
- [x] 5.3 Map npx/node commands to fnm-managed paths
- [x] 5.4 Map uvx/python commands to uv-managed paths
- [x] 5.5 Integrate with existing McpClient for server startup

## 6. Integration Layer (Phase 6) ✅
- [x] 6.1 Implement `integrator.rs` for core system integration
- [x] 6.2 Create PluginManager as main entry point
- [x] 6.3 Implement load_all() for startup loading
- [x] 6.4 Implement load_plugin() for development mode
- [x] 6.5 Implement unload_plugin() for runtime unloading
- [x] 6.6 Implement set_enabled() for enable/disable

## 7. FFI and CLI (Phase 7) ✅
- [x] 7.1 Add FFI exports in `core/src/ffi/plugins.rs`
- [x] 7.2 Export list_plugins() function
- [x] 7.3 Export enable_plugin/disable_plugin() functions
- [x] 7.4 Export execute_plugin_skill() function
- [x] 7.5 Update UniFFI bindings (aether.udl)

## 8. Configuration (Phase 8) ✅
- [x] 8.1 Plugin directory at ~/.config/aether/plugins/
- [x] 8.2 Implement plugins.json for plugin state persistence
- [x] 8.3 Add plugin directory configuration (default_plugins_dir)
- [x] 8.4 Add dev_paths for development plugin loading

## 9. Testing (Phase 9) ✅
- [x] 9.1 Create test plugin fixture (test_fixtures module)
- [x] 9.2 Write unit tests for manifest parsing (15 tests)
- [x] 9.3 Write unit tests for SKILL.md parsing (12 tests)
- [x] 9.4 Write unit tests for hooks.json parsing (8 tests)
- [x] 9.5 Write integration tests for plugin loading (10 tests)
- [x] 9.6 Write integration tests for skill execution (5 tests)

Total: 50 tests passing

## 10. Documentation (Phase 10) ✅
- [x] 10.1 Add plugin system documentation to docs/PLUGINS.md
- [x] 10.2 Document Claude Code compatibility notes
- [x] 10.3 Architecture diagrams and API reference

## Summary

**All phases completed.** The plugin system is fully implemented with:

- 12 Rust modules in `core/src/plugins/`
- FFI exports in `core/src/ffi/plugins.rs`
- UniFFI definitions in `core/src/aether.udl`
- 50 passing tests
- Comprehensive documentation
