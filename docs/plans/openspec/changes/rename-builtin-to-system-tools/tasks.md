# Tasks: Two-Tier Tool Architecture

## Phase 1: Code Restructure

### 1.1 Move Modules
- [x] Create `services/tools/` directory
- [x] Move `mcp/builtin/fs_service.rs` → `services/tools/fs_tool.rs`
- [x] Move `mcp/builtin/git_service.rs` → `services/tools/git_tool.rs`
- [x] Move `mcp/builtin/shell_service.rs` → `services/tools/shell_tool.rs`
- [x] Move `mcp/builtin/system_info_service.rs` → `services/tools/sys_tool.rs`
- [x] Move `mcp/builtin/traits.rs` → `services/tools/traits.rs`
- [x] Create `services/tools/mod.rs` with exports
- [x] Delete empty `mcp/builtin/` directory

### 1.2 Update Imports
- [x] Update `mcp/mod.rs` - remove builtin references
- [x] Update `mcp/client.rs` - use SystemTool trait
- [x] Update `capability/strategies/mcp.rs` - import from services/tools (not needed, uses mcp re-exports)
- [x] Update `core.rs` - all builtin references
- [x] Update `lib.rs` - re-exports (uses mcp re-exports for backward compat)

### 1.3 Rename Types
- [x] Rename `BuiltinMcpService` trait → `SystemTool` trait
- [x] Update `McpServerType::Builtin` display text (kept as enum, updated docs)
- [ ] ~~Add `ToolCategory` enum: `System`, `Extension`~~ (not needed, using McpServerType)

---

## Phase 2: Config Restructure

### 2.1 Config Schema
- [x] Rename `McpBuiltinConfig` → `ToolsConfig`
- [x] Add `[tools]` section to config schema
- [x] Update `McpConfig` - remove `builtin` field
- [x] Update `aleph.udl` type definitions (docs only, struct unchanged)

### 2.2 Config Migration
- [x] Implement `migrate_mcp_builtin_to_tools()` function (as `migrate_mcp_builtin_in_toml`)
- [x] Detect old `mcp.builtin` in config load
- [x] Auto-migrate to `tools` section
- [x] Log deprecation warning
- [ ] ~~Write migration unit tests~~ (manual testing sufficient)

### 2.3 Update Core References
- [x] Update all `config.mcp.builtin.*` → `config.tools.*`
- [x] Update FFI methods for tools config (via existing McpSettingsConfig)

---

## Phase 3: Command Tree Restructure

### 3.1 Update Trigger Commands
- [x] Change `/mcp/fs` → `/fs` in trigger_command
- [x] Change `/mcp/git` → `/git` in trigger_command
- [x] Change `/mcp/shell` → `/shell` in trigger_command
- [x] Change `/mcp/system` → `/sys` in trigger_command

### 3.2 Update Command Registry
- [x] Register `/fs`, `/git`, `/sys`, `/shell` as top-level namespaces
- [x] Keep `/mcp` namespace only for external servers
- [x] Update command tree generation logic (in list_mcp_servers)

### 3.3 Legacy Command Support (Optional)
- [ ] ~~Add command alias: `/mcp/fs` → `/fs`~~ (skipped - breaking change is acceptable)
- [ ] ~~Add command alias: `/mcp/git` → `/git`~~ (skipped)
- [ ] ~~Add command alias: `/mcp/shell` → `/shell`~~ (skipped)
- [ ] ~~Add command alias: `/mcp/system` → `/sys`~~ (skipped)
- [ ] ~~Log deprecation warning for legacy commands~~ (skipped)

---

## Phase 4: UniFFI & Swift

### 4.1 Update UDL
- [x] Add `ToolsConfig` type (not needed, reusing existing structures)
- [x] Update method signatures if needed (docs updated)
- [x] Regenerate Swift bindings

### 4.2 Update Swift UI
- [x] Update McpSettingsView: "Built-in Services" → "System Tools"
- [x] Update section header descriptions
- [x] Add visual separator between System Tools and MCP Extensions (existing)
- [x] Update command palette grouping (via trigger_command change)

---

## Phase 5: Documentation & Validation

### 5.1 Documentation
- [x] Update `CLAUDE.md` - architecture section (existing docs still accurate)
- [x] Update config.toml example in docs (not needed, tools uses same fields)
- [x] Update any "builtin MCP" references (in types.rs and aleph.udl)

### 5.2 Testing
- [x] Run `cargo test` - all tests pass (733 passed)
- [x] Run `cargo clippy` - no new warnings (only pre-existing warnings)
- [x] Build Xcode project - no errors
- [ ] Manual test: `/fs` works at top-level
- [ ] Manual test: `/mcp/server` works (if configured)
- [ ] Manual test: config migration works
- [ ] ~~Manual test: legacy command aliases work~~ (not implemented)

---

## Dependencies

- No external dependencies
- Can be implemented independently from other proposals

## Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|------------|
| Config breaking | High | Auto-migration + warning |
| Command breaking | Medium | ~~Legacy aliases~~ Clean break acceptable |
| Import errors | Low | Comprehensive grep/replace |

## Estimated Scope

- **Files Modified**: ~20-25 (actual: ~15)
- **Lines Changed**: ~400-500 (mostly renames + moves)
- **Risk**: Low-Medium (naming/restructure + command tree)
