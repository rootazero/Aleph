# Proposal: Redesign Providers UI Layout

## Change ID
`redesign-providers-ui`

## Summary
Redesign the Providers settings UI to match the reference design language from `uisample.png`, featuring a card-based provider list on the left and a spacious edit panel on the right, with enlarged window dimensions to accommodate richer content.

## Why
用户在配置多个 AI 提供商时,当前界面存在以下问题:
1. **视觉拥挤**: 1000x700 的窗口对于复杂的提供商配置(包括 API 端点、高级设置等)显得局促
2. **状态不清晰**: 无法快速识别哪些提供商处于激活状态,需要点击查看详情才能确认
3. **测试反馈冗余**: 连接测试结果以大卡片形式显示,占用过多垂直空间
4. **操作不便**: 取消/保存按钮位置不符合 macOS 设计惯例(应在右下角)
5. **设计不一致**: 与参考设计 `uisample.png` 的现代化风格存在差距

通过这次重新设计,用户将获得:
- **更清晰的状态指示**: 通过蓝点/灰圈和切换开关,一目了然地看到提供商激活状态
- **更高效的工作流**: 更大的窗口和优化的布局减少滚动操作,提高配置效率
- **更好的反馈体验**: 内联测试结果显示紧凑且不干扰主要操作流程
- **更符合直觉的交互**: 按钮位置符合 macOS 标准,降低认知负担

## Motivation
The current Providers UI (ProvidersView.swift + ProviderEditPanel.swift) has been modernized but doesn't fully match the design vision shown in the reference mockup. Specifically:

1. **Window Size**: Current settings window is constrained (1000x700 in preview), limiting content visibility
2. **Layout**: While we have a left/right split, the proportions and spacing don't match the reference design
3. **Provider Cards**: Need to show Active/Inactive status more prominently with toggle switches
4. **Edit Panel**: Should include:
   - Active/Inactive status toggle at the top
   - Connection test button with inline result display (small text below button)
   - Cancel/Save buttons anchored to bottom-right corner
5. **Visual Hierarchy**: Need clearer separation between view mode and edit mode states

## Goals
- Match the reference design language from `uisample.png` for visual consistency
- Enlarge the settings window to provide more breathing room for content
- Add Active/Inactive toggle to provider cards and edit panel
- Display connection test results as small inline text (not modal/toast)
- Position Cancel/Save buttons in bottom-right corner
- Maintain existing functionality (add, edit, delete, test providers)

## Non-Goals
- Changing the underlying data model or API (ProviderConfig, AetherCore methods)
- Adding new provider types beyond what's currently supported
- Implementing batch operations (multi-select, bulk delete)
- Auto-detecting provider API endpoints

## Context
### Reference Design Analysis (uisample.png)
The reference design shows:
- **Left Panel (~480px)**: Provider cards with:
  - Provider icon, name, active indicator (blue dot)
  - Cards are selectable with highlight state
  - Search bar at top
  - "Add Custom Provider" button in header
- **Right Panel (~520px)**: Edit area with:
  - Provider name with "Active" badge and toggle switch
  - Provider description
  - API endpoint configuration (dark code block)
  - "Use with Claude Code" section (expandable)
  - Bottom-right: "Close" and "Save" buttons
- **Window**: Appears to be ~1000px wide minimum

### Current Implementation
- File: `Aether/Sources/ProvidersView.swift` (431 lines)
- File: `Aether/Sources/Components/Organisms/ProviderEditPanel.swift` (755 lines)
- File: `Aether/Sources/Components/Molecules/ProviderCard.swift` (371 lines)

### Technical Constraints
- SwiftUI framework limitations for custom layouts
- Must preserve UniFFI integration with Rust core
- Keychain integration must remain unchanged
- Config persistence through `core.updateProvider()` / `core.deleteProvider()`

## Impact Analysis
### Components Modified
- `ProvidersView.swift`: Window frame size, layout proportions
- `ProviderEditPanel.swift`: Add active toggle, reposition buttons, test result display
- `ProviderCard.swift`: Add active/inactive indicator
- `SettingsView.swift`: Adjust window size constraints

### Backward Compatibility
- No breaking changes to config.toml format
- Existing provider configurations will work without migration
- Keychain storage format unchanged

### Testing Strategy
- Manual testing: Visual regression against reference design
- Unit tests: No new business logic, existing tests sufficient
- Integration tests: Verify save/load flow still works with new UI

## Questions for Stakeholders
None - requirements are well-defined from reference mockup.

## Related Changes
- Depends on: `modernize-settings-ui` (already completed)
- Blocks: None
- Related: Settings window sizing may affect other tabs (Routing, Shortcuts, etc.)

## Approval Checklist
- [ ] Design mockup reviewed (`uisample.png` serves as reference)
- [ ] Technical feasibility confirmed (SwiftUI capabilities)
- [ ] No security/privacy concerns (UI-only change)
- [ ] Stakeholder sign-off
