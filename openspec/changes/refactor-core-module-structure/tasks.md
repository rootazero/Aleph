# Tasks: Refactor core.rs Module Structure

## 1. Preparation

- [ ] 1.1 Create `src/core/` directory
- [ ] 1.2 Run `cargo test` to establish baseline (all tests must pass)
- [ ] 1.3 Run `cargo clippy` to ensure no warnings before refactor

## 2. Extract Types Module

- [ ] 2.1 Create `src/core/types.rs` with shared types:
  - `RequestContext`
  - `MediaAttachment`
  - `CapturedContext`
  - `CompressionStats`
  - `StorageHelper`
  - `AppMemoryInfo` (if defined in core.rs)
- [ ] 2.2 Update imports in `mod.rs` to use `types::`
- [ ] 2.3 Verify compilation: `cargo check`

## 3. Extract Memory Operations Module

- [ ] 3.1 Create `src/core/memory.rs` with memory-related impl methods:
  - `get_memory_stats()`
  - `search_memories()`
  - `get_memory_app_list()`
  - `delete_memory()`
  - `clear_memories()`
  - `get_memory_config()`
  - `update_memory_config()`
  - `cleanup_old_memories()`
  - `trigger_compression()`
  - `get_compression_stats()`
  - `store_interaction_memory()`
  - `retrieve_and_augment_prompt()`
  - `retrieve_and_augment_user_input()`
  - `retrieve_memories_with_ai()`
  - `build_memory_exclusion_set()`
- [ ] 3.2 Add `pub(crate)` visibility where needed for inter-module access
- [ ] 3.3 Verify compilation: `cargo check`

## 4. Extract Config Operations Module

- [ ] 4.1 Create `src/core/config_ops.rs` with config management methods:
  - `lock_config()` (helper)
  - `load_config()`
  - `update_provider()`
  - `delete_provider()`
  - `update_routing_rules()`
  - `reload_router()`
  - `update_shortcuts()`
  - `update_behavior()`
  - `update_trigger_config()`
  - `validate_regex()`
  - `test_provider_connection()`
  - `test_provider_connection_with_config()`
  - `test_provider_internal()` (helper)
  - `get_default_provider()`
  - `set_default_provider()`
  - `get_enabled_providers()`
- [ ] 4.2 Verify compilation: `cargo check`

## 5. Extract MCP Operations Module

- [ ] 5.1 Create `src/core/mcp_ops.rs` with MCP capability methods:
  - `get_mcp_config()`
  - `update_mcp_config()`
  - `list_mcp_services()`
  - `get_service_description()` (helper)
  - `list_mcp_tools()`
  - `list_mcp_servers()`
  - `get_mcp_server()`
  - `get_mcp_server_status()`
  - `add_mcp_server()`
  - `update_mcp_server()`
  - `delete_mcp_server()`
  - `get_mcp_server_logs()`
  - `export_mcp_config_json()`
  - `import_mcp_config_json()`
- [ ] 5.2 Verify compilation: `cargo check`

## 6. Extract Search Operations Module

- [ ] 6.1 Create `src/core/search_ops.rs` with search capability methods:
  - `get_search_options_from_config()` (helper)
  - `create_search_registry_from_config()` (helper)
  - `update_search_config()`
  - `test_search_provider()`
  - `test_search_provider_with_config()`
- [ ] 6.2 Verify compilation: `cargo check`

## 7. Extract Tool Registry Module

- [ ] 7.1 Create `src/core/tools.rs` with dispatcher/tool registry methods:
  - `refresh_tool_registry()`
  - `list_unified_tools()`
  - `search_unified_tools()`
  - `get_tool_prompt_block()`
  - `list_tools()`
  - `list_tools_by_source()`
  - `search_tools()`
  - `refresh_tools()`
  - `confirm_action()`
  - `cancel_confirmation()`
  - `get_pending_confirmation()`
  - `get_pending_confirmation_count()`
  - `cleanup_expired_confirmations()`
- [ ] 7.2 Verify compilation: `cargo check`

## 8. Extract Conversation Module

- [ ] 8.1 Create `src/core/conversation.rs` with conversation management methods:
  - `start_conversation()`
  - `continue_conversation()`
  - `end_conversation()`
  - `has_active_conversation()`
  - `conversation_turn_count()`
  - `record_conversation_turn()` (helper)
- [ ] 8.2 Verify compilation: `cargo check`

## 9. Extract Processing Pipeline Module

- [ ] 9.1 Create `src/core/processing.rs` with AI processing methods:
  - `process_input()`
  - `process_with_ai_first()`
  - `build_enriched_payload()`
  - `execute_capability_and_continue()`
  - `build_routing_context()` (helper)
  - `build_matching_context()` (helper)
  - `is_semantic_matching_enabled()` (helper)
  - `get_memory_context_for_ai_first()` (helper)
  - `get_default_provider_instance()` (helper)
  - `get_provider_by_name()` (helper)
  - `handle_processing_error()` (helper)
- [ ] 9.2 Verify compilation: `cargo check`

## 10. Extract Tests Module

- [ ] 10.1 Create `src/core/tests.rs` with all `#[cfg(test)]` code:
  - `test_core_creation()`
  - `test_request_context_storage()`
  - `test_retry_without_context()`
  - `test_retry_max_limit()`
  - `test_clear_request_context()`
  - `test_context_capture_and_storage()`
  - `test_missing_context_error()`
  - `test_retrieve_and_augment_with_memory_disabled()`
  - `test_retrieve_and_augment_without_context()`
  - `test_full_aether_core_memory_pipeline()`
- [ ] 10.2 Update test imports to reference new module paths
- [ ] 10.3 Verify tests pass: `cargo test`

## 11. Create mod.rs (Main Module)

- [ ] 11.1 Create `src/core/mod.rs` with:
  - Module declarations (`mod types; mod memory; ...`)
  - Re-exports (`pub use types::*;`)
  - `AetherCore` struct definition
  - `new()` constructor
  - `get_router()`, `get_search_registry()` helper methods
  - Remaining core methods not extracted
  - Command completion methods (`get_root_commands`, `get_command_children`, `filter_commands`)
  - Core lifecycle methods (`start_listening`, `stop_listening`, `is_listening`)
  - Logging methods (`get_log_level`, `set_log_level`, `get_log_directory`)
  - Retry methods (`retry_last_request`, `store_request_context`, `clear_request_context`)
  - Context methods (`set_current_context`, `clone_for_storage`)
  - Test streaming methods (`test_streaming_response`, `test_typed_error`)
- [ ] 11.2 Update `src/core.rs` to become a thin re-export file

## 12. Update core.rs as Re-export File

- [ ] 12.1 Replace `src/core.rs` content with:
  ```rust
  //! AetherCore module - Main entry point for the Aether library
  //!
  //! This module is split into submodules for better organization:
  //! - `types`: Shared type definitions
  //! - `memory`: Memory storage and retrieval operations
  //! - `config_ops`: Configuration management
  //! - `mcp_ops`: MCP capability methods
  //! - `search_ops`: Search capability methods
  //! - `tools`: Dispatcher and tool registry
  //! - `conversation`: Multi-turn conversation management
  //! - `processing`: AI processing pipeline

  mod types;
  mod memory;
  mod config_ops;
  mod mcp_ops;
  mod search_ops;
  mod tools;
  mod conversation;
  mod processing;

  #[cfg(test)]
  mod tests;

  // Re-export public types and AetherCore
  pub use types::*;

  // Include the main module implementation
  mod core_impl;
  pub use core_impl::AetherCore;
  ```
- [ ] 12.2 Or use Rust 2018 `core/mod.rs` pattern (preferred)

## 13. Final Verification

- [ ] 13.1 Run full test suite: `cargo test`
- [ ] 13.2 Run clippy: `cargo clippy -- -D warnings`
- [ ] 13.3 Verify UniFFI binding generation: `cargo run --bin uniffi-bindgen generate ...`
- [ ] 13.4 Build release: `cargo build --release`
- [ ] 13.5 Verify Swift client compiles and runs

## 14. Documentation Update

- [ ] 14.1 Update `CLAUDE.md` project structure section to reflect new layout
- [ ] 14.2 Add inline documentation to each new module file

## Dependencies

- Tasks 2-10 can be done in **any order** after Task 1
- Task 11-12 must be done **after** all extraction tasks (2-10)
- Task 13-14 must be done **last**

## Parallelizable Work

The following task groups can be done in parallel by different developers:
- Group A: Tasks 2, 3 (types + memory)
- Group B: Tasks 4, 5, 6 (config + mcp + search)
- Group C: Tasks 7, 8, 9 (tools + conversation + processing)
- Sequential: Task 10-14 (integration, must be last)
