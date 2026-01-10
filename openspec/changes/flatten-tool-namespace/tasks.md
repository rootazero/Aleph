# Tasks: Flatten Tool Namespace

## Phase 1: Conflict Resolution System

### 1.1 Add Priority System
- [ ] 1.1.1 Add `ToolPriority` enum to `dispatcher/types.rs`
- [ ] 1.1.2 Add `priority()` method to `ToolSource`
- [ ] 1.1.3 Add `suffix()` method to `ToolSource` for renamed tools
- [ ] 1.1.4 Add unit tests for priority ordering

### 1.2 Implement Conflict Detection
- [ ] 1.2.1 Add `ConflictInfo` struct to `dispatcher/types.rs`
- [ ] 1.2.2 Add `ConflictResolution` enum
- [ ] 1.2.3 Add `check_conflict()` method to `ToolRegistry`
- [ ] 1.2.4 Add `resolve_conflict()` method to `ToolRegistry`
- [ ] 1.2.5 Add unit tests for conflict detection
- [ ] 1.2.6 Add unit tests for conflict resolution

### 1.3 Add Conflict Logging
- [ ] 1.3.1 Log warning when tool is renamed due to conflict
- [ ] 1.3.2 Store original_name in `UnifiedTool` for shadowed tools
- [ ] 1.3.3 Add `original_name: Option<String>` field to `UnifiedTool`

## Phase 2: Flatten MCP Tool Registration

### 2.1 Update MCP Tool Registration
- [ ] 2.1.1 Modify `register_mcp_tools()` to register as root commands
- [ ] 2.1.2 Generate routing regex for each MCP tool: `^/{tool}\s*`
- [ ] 2.1.3 Apply conflict resolution before registration
- [ ] 2.1.4 Add `mcp_server` and `mcp_tool` fields to routing config
- [ ] 2.1.5 Update `register_system_tools()` similarly

### 2.2 Update MCP Routing
- [ ] 2.2.1 Add `RoutingRuleConfig::mcp_server` field
- [ ] 2.2.2 Add `RoutingRuleConfig::mcp_tool` field
- [ ] 2.2.3 Update Router to handle MCP tool routing
- [ ] 2.2.4 Implement MCP tool execution in dispatcher

### 2.3 Test MCP Flat Routing
- [ ] 2.3.1 Unit test: `/git status` routes to MCP git-server
- [ ] 2.3.2 Unit test: Conflict between MCP `search` and Builtin `search`
- [ ] 2.3.3 Integration test: MCP tool execution via flat command

## Phase 3: Flatten Skill Registration

### 3.1 Update Skill Registration
- [ ] 3.1.1 Modify `register_skills()` to register as root commands
- [ ] 3.1.2 Generate routing regex for each skill: `^/{skill_id}\s*`
- [ ] 3.1.3 Apply conflict resolution before registration
- [ ] 3.1.4 Set capabilities: ["skills", "memory"]

### 3.2 Update Skill Routing
- [ ] 3.2.1 Update Router to handle skill routing
- [ ] 3.2.2 Ensure skill context is loaded correctly
- [ ] 3.2.3 Verify skills work without `/skill` prefix

### 3.3 Test Skill Flat Routing
- [ ] 3.3.1 Unit test: `/refine-text` routes to skill
- [ ] 3.3.2 Unit test: Conflict between Skill and MCP with same name
- [ ] 3.3.3 Integration test: Skill execution via flat command

## Phase 4: Remove Namespace Builtins

### 4.1 Update BUILTIN_COMMANDS
- [ ] 4.1.1 Remove `/mcp` from `BUILTIN_COMMANDS` in `builtin_defs.rs`
- [ ] 4.1.2 Remove `/skill` from `BUILTIN_COMMANDS`
- [ ] 4.1.3 Update `get_builtin_routing_rules()` (only 3 commands now)
- [ ] 4.1.4 Update unit tests for 3 builtin commands

### 4.2 Update aether.udl
- [ ] 4.2.1 Remove `CommandType::Namespace` if no longer needed
- [ ] 4.2.2 Remove `has_subtools` from `UnifiedToolInfo` if not used
- [ ] 4.2.3 Regenerate Swift bindings

### 4.3 Cleanup Rust Code
- [ ] 4.3.1 Remove `get_subtools_from_registry()` if not needed
- [ ] 4.3.2 Remove `get_subcommands_from_registry()` if not needed
- [ ] 4.3.3 Remove namespace-related helper methods

## Phase 5: Simplify Swift UI

### 5.1 Update CommandCompletionManager
- [ ] 5.1.1 Remove `currentParentKey` state variable
- [ ] 5.1.2 Remove `navigateIntoNamespace()` method
- [ ] 5.1.3 Remove `navigateBack()` method
- [ ] 5.1.4 Remove `isInNamespace` computed property
- [ ] 5.1.5 Simplify `selectCurrentCommand()` - no more namespace entry
- [ ] 5.1.6 Add sorting by source priority, then alphabetically

### 5.2 Update SubPanelView
- [ ] 5.2.1 Remove namespace back button
- [ ] 5.2.2 Add `SourceBadge` component
- [ ] 5.2.3 Update `CommandRowView` to show source badge
- [ ] 5.2.4 Update icon colors based on source type

### 5.3 Update RoutingView (Settings)
- [ ] 5.3.1 Update `PresetRulesListView` to show flat list with sections
- [ ] 5.3.2 Add section headers: "System", "MCP", "Skill", "Custom"
- [ ] 5.3.3 Update `ToolRowView` to show source badge
- [ ] 5.3.4 Show MCP server name as subtitle

### 5.4 Add Localization
- [ ] 5.4.1 Add `source.system` localization key
- [ ] 5.4.2 Add `source.mcp` localization key
- [ ] 5.4.3 Add `source.skill` localization key
- [ ] 5.4.4 Add `source.custom` localization key
- [ ] 5.4.5 Add Chinese translations

## Phase 6: Dynamic Routing Rules

### 6.1 Generate Rules from Registry
- [ ] 6.1.1 Add `generate_routing_rules()` method to `ToolRegistry`
- [ ] 6.1.2 Generate rules for MCP tools with proper regex
- [ ] 6.1.3 Generate rules for Skills with proper regex
- [ ] 6.1.4 Merge with builtin and custom rules

### 6.2 Update Router Initialization
- [ ] 6.2.1 Add `init_with_registry()` method to Router
- [ ] 6.2.2 Compile routing rules from registry
- [ ] 6.2.3 Re-initialize router when tools change

### 6.3 Test Dynamic Routing
- [ ] 6.3.1 Test MCP connect triggers router re-init
- [ ] 6.3.2 Test skill install triggers router re-init
- [ ] 6.3.3 Test routing works after dynamic changes

## Phase 7: Backward Compatibility (Optional)

### 7.1 Add Feature Flag
- [ ] 7.1.1 Add `flat_namespace` config option to `[dispatcher]`
- [ ] 7.1.2 Default to `true`
- [ ] 7.1.3 When `false`, keep `/mcp` and `/skill` namespaces

### 7.2 Command Translation Layer
- [ ] 7.2.1 Detect `/mcp ` prefix in input
- [ ] 7.2.2 Translate to flat command and show tip
- [ ] 7.2.3 Detect `/skill ` prefix in input
- [ ] 7.2.4 Translate to flat command and show tip

### 7.3 Deprecation Warnings
- [ ] 7.3.1 Log deprecation warning for namespace prefixes
- [ ] 7.3.2 Show UI toast suggesting flat command

## Phase 8: Documentation and Cleanup

### 8.1 Update CLAUDE.md
- [ ] 8.1.1 Document flat namespace architecture
- [ ] 8.1.2 Remove `/mcp` and `/skill` from examples
- [ ] 8.1.3 Document conflict resolution rules

### 8.2 Update User Documentation
- [ ] 8.2.1 Update command reference
- [ ] 8.2.2 Add migration guide for existing users

### 8.3 Final Cleanup
- [ ] 8.3.1 Remove deprecated code paths
- [ ] 8.3.2 Remove unused namespace-related types
- [ ] 8.3.3 Archive this change proposal

## Dependencies

```
Phase 1 (Conflict Resolution)
    ↓
Phase 2 (MCP Flatten) ──┬──► Phase 4 (Remove Namespace Builtins)
Phase 3 (Skill Flatten) ┘           ↓
                              Phase 5 (UI Simplification)
                                    ↓
                              Phase 6 (Dynamic Routing)
                                    ↓
                              Phase 7 (Optional: Backward Compat)
                                    ↓
                              Phase 8 (Documentation)
```

## Verification Checklist

After implementation, verify:

- [ ] User types `/git status` → MCP git tool executes
- [ ] User types `/refine-text` → Skill executes
- [ ] Command completion shows all tools in flat list
- [ ] Each tool has correct source badge (System, MCP, Skill, Custom)
- [ ] Conflict between sources results in lower-priority tool renamed
- [ ] Settings shows all tools grouped by source
- [ ] No `/mcp` or `/skill` in BUILTIN_COMMANDS
- [ ] L3 router sees all tools in flat list
- [ ] Dynamic tool changes (MCP connect/disconnect) update completion
- [ ] Localization works for source badges

## Rollback Plan

If issues arise:

1. Set `flat_namespace = false` in config
2. Re-enable `/mcp` and `/skill` builtins
3. Restore namespace navigation in UI
4. User can continue using old syntax

## Success Metrics

1. **Reduced cognitive load**: User doesn't need to remember `/mcp` or `/skill`
2. **Faster command entry**: One step instead of two (no namespace navigation)
3. **Consistent UX**: All tools invoked the same way
4. **Clear source visibility**: Badges show origin without command prefix pollution
