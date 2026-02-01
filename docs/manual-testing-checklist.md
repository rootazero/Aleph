# Manual Testing Checklist
# 手动测试清单

## Skill Compiler (Phase 10)
## 技能编译器（Phase 10）

### Evolution Tracking
### 进化追踪

- [ ] EN: Execute a POE task 3+ times successfully and verify metrics are recorded in evolution.db.
  ZH: 成功执行 POE 任务 3 次以上，确认指标记录在 evolution.db 中。
- [ ] EN: Run detection pipeline and verify candidates are found for repeated patterns.
  ZH: 运行检测管道，确认重复模式被发现为候选。
- [ ] EN: Generate a suggestion with AI provider and verify name/description are reasonable.
  ZH: 使用 AI 提供商生成建议，确认名称/描述合理。

### Approval Workflow
### 审批工作流

- [ ] EN: Submit a suggestion for approval and verify it appears in pending list.
  ZH: 提交建议进行审批，确认出现在待处理列表中。
- [ ] EN: Preview a pending skill and verify SKILL.md content looks correct.
  ZH: 预览待处理技能，确认 SKILL.md 内容正确。
- [ ] EN: Approve a suggestion and verify SKILL.md is created in skills directory.
  ZH: 批准建议，确认 SKILL.md 在技能目录中创建。
- [ ] EN: Reject a suggestion and verify it's removed from pending list.
  ZH: 拒绝建议，确认从待处理列表中移除。
- [ ] EN: Verify duplicate patterns are ignored (same pattern_id).
  ZH: 确认重复模式被忽略（相同 pattern_id）。

### Skill Generation
### 技能生成

- [ ] EN: Generate a skill and verify it appears in SkillsRegistry after reload.
  ZH: 生成技能，确认重新加载后出现在 SkillsRegistry 中。
- [ ] EN: Verify generated skill can be read via read_skill tool.
  ZH: 确认生成的技能可以通过 read_skill 工具读取。
- [ ] EN: If git configured, verify auto-commit creates a commit with the skill.
  ZH: 如果配置了 git，确认自动提交创建了包含技能的提交。

### Tool-Backed Skills
### 工具支持的技能

- [ ] EN: Generate a tool package and verify tool_definition.json is valid.
  ZH: 生成工具包，确认 tool_definition.json 有效。
- [ ] EN: Run self-test on generated tool and verify it passes basic validation.
  ZH: 对生成的工具运行自测，确认通过基本验证。
- [ ] EN: Register tool in ToolServer and verify it appears in tool list.
  ZH: 在 ToolServer 中注册工具，确认出现在工具列表中。
- [ ] EN: Verify unconfirmed tools require confirmation before execution.
  ZH: 确认未确认的工具在执行前需要确认。

### Safety Gating
### 安全门控

- [ ] EN: Analyze a safe suggestion and verify SafetyLevel::Safe is returned.
  ZH: 分析安全建议，确认返回 SafetyLevel::Safe。
- [ ] EN: Analyze a dangerous suggestion (with rm, sudo) and verify it's flagged.
  ZH: 分析危险建议（包含 rm, sudo），确认被标记。
- [ ] EN: Verify blocked patterns cannot be approved for generation.
  ZH: 确认被阻止的模式不能被批准生成。
- [ ] EN: Verify first-run confirmation is required for non-safe tools.
  ZH: 确认非安全工具需要首次运行确认。

### Configuration
### 配置

- [ ] EN: Set evolution.enabled=false and verify no tracking occurs.
  ZH: 设置 evolution.enabled=false，确认不发生追踪。
- [ ] EN: Adjust thresholds (min_success_count, min_success_rate) and verify detection respects them.
  ZH: 调整阈值（min_success_count, min_success_rate），确认检测遵循这些阈值。
- [ ] EN: Enable tool_generation.enabled and verify tool packages are created.
  ZH: 启用 tool_generation.enabled，确认创建工具包。

---

## Perception Snapshot (Phase 8.5)
## 感知快照（Phase 8.5）

- EN: Call `snapshot_capture` with `include_ax=true` and verify AX tree is populated.
  ZH: 使用 `include_ax=true` 调用 `snapshot_capture`，确认 AX 树有内容。
- EN: Call with `include_vision=true` and Screen Recording granted; verify `vision_blocks` is returned.
  ZH: 在已授权屏幕录制时调用 `include_vision=true`，确认返回 `vision_blocks`。
- EN: Call with `include_image=true` and verify `image_ref.path` points to a readable file.
  ZH: 使用 `include_image=true`，确认 `image_ref.path` 指向可读文件。
- EN: Deny Screen Recording permission and verify error `SCREEN_RECORDING_REQUIRED` is returned.
  ZH: 拒绝屏幕录制权限，确认返回 `SCREEN_RECORDING_REQUIRED`。
- EN: When Screen Recording is denied, verify the non-blocking toast shows and "Open System Settings" opens the Screen Recording pane.
  ZH: 拒绝屏幕录制权限时，确认出现非阻塞 Toast，点击"打开系统设置"进入屏幕录制权限页。
- EN: Verify `coordinate_space` equals `screen_points_top_left` and boxes are consistent.
  ZH: 确认 `coordinate_space` 为 `screen_points_top_left` 且坐标一致。
