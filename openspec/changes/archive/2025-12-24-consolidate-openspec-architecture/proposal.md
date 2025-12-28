# Proposal: Consolidate OpenSpec Architecture

**Change ID**: `consolidate-openspec-architecture`
**Status**: Proposed
**Author**: System
**Date**: 2025-12-24

## Overview

整合和清理当前混乱的 OpenSpec 文件夹架构，将分散在多个位置的 OpenSpec 内容合并到单一规范的目录结构中，消除重复和嵌套，提升项目文档的可维护性。

## Why

Multiple OpenSpec directories across the project create unnecessary complexity and maintenance burden. By consolidating to a single canonical structure, we:

1. **Reduce Cognitive Load**: Developers no longer need to search multiple locations for specifications and change proposals
2. **Eliminate Synchronization Issues**: No more inconsistencies between duplicate copies of the same content
3. **Improve Tool Reliability**: OpenSpec CLI commands work consistently without confusion from nested structures
4. **Enable Scalability**: Clear structure prevents future directory proliferation as the project grows
5. **Lower Maintenance Cost**: Single source of truth means changes only need to be made in one place

This consolidation unblocks clean development workflows and establishes foundation for future OpenSpec-based processes.

## Problem Statement

当前项目存在多个 OpenSpec 目录，导致文档管理混乱：

1. **多个 OpenSpec 根目录**:
   - `/openspec/` - 主要的规范目录
   - `/Aether/openspec/` - Aether 子项目的副本
   - 两者内容部分重叠但不一致

2. **深度嵌套结构**:
   - `/Aether/openspec/changes/complete-phase2-testing-and-polish/openspec/changes/complete-phase2-testing-and-polish/`
   - 存在 4-5 层嵌套的 openspec 目录

3. **重复的 change 定义**:
   - 某些 change 在主 openspec 和 Aether openspec 中都存在
   - 不清楚哪个是权威版本

4. **工具混淆**:
   - `openspec list` 只能看到主目录的 changes
   - Aether 子目录的 changes 被忽略或导致错误

这导致：
- 维护成本高（需要同步多个位置）
- 容易出错（修改遗漏某个副本）
- 工具行为不一致
- 新贡献者困惑

## Proposed Solution

建立**单一规范的 OpenSpec 架构**，遵循以下原则：

1. **单一根目录**: 只保留 `/openspec/` 作为唯一的规范根目录
2. **扁平化 changes**: 所有 changes 位于 `/openspec/changes/` 下，无嵌套
3. **消除重复**: 合并重复的内容，保留最新最完整的版本
4. **清理历史**: 将过时或重复的内容移到 archive

### 目标架构

```
/Users/zouguojun/Workspace/Aether/
├── openspec/                          # 唯一的 OpenSpec 根目录
│   ├── AGENTS.md                      # OpenSpec 代理指南
│   ├── project.md                     # 项目上下文
│   ├── changes/                       # 所有变更提案
│   │   ├── add-contextual-memory-rag/
│   │   ├── add-macos-client-and-halo-overlay/
│   │   ├── enhance-halo-overlay/
│   │   ├── remove-audio-and-accessibility/
│   │   ├── consolidate-openspec-architecture/  # 本提案
│   │   └── archive/                   # 已归档的变更
│   │       ├── 2025-12-23-add-rust-core-foundation/
│   │       └── 2025-12-23-complete-phase2-testing-and-polish/
│   └── specs/                         # 规范定义
│       ├── build-integration/
│       ├── clipboard-management/
│       ├── core-library/
│       ├── event-handler/
│       ├── hotkey-detection/
│       ├── macos-client/
│       ├── testing-framework/
│       └── uniffi-bridge/
│
├── Aether/                            # macOS 应用源码
│   ├── (no openspec directory)        # 删除重复的 openspec
│   └── ...
│
└── .claude/
    └── commands/
        └── openspec/                  # CLI 工具脚本（保持不变）
```

## Technical Approach

### Phase 1: 审计和分析

1. **清点所有 OpenSpec 内容**:
   ```bash
   find . -type d -name "openspec" -o -name "changes" -o -name "specs"
   ```

2. **识别重复和冲突**:
   - 比较主目录和 Aether 子目录的内容
   - 列出重复的 changes
   - 检查内容差异

3. **评估嵌套深度**:
   - 识别所有深度嵌套的结构
   - 确定需要提取的内容

### Phase 2: 合并和清理

1. **合并 complete-phase2-testing-and-polish**:
   ```bash
   # 当前位置：/Aether/openspec/changes/complete-phase2-testing-and-polish/
   # 已归档位置：/openspec/changes/archive/2025-12-23-complete-phase2-testing-and-polish/

   # 操作：检查是否有新内容，如有则提取，否则删除副本
   ```

2. **扁平化嵌套 openspec**:
   ```bash
   # 提取：/Aether/openspec/changes/complete-phase2-testing-and-polish/openspec/changes/
   # 到：  /openspec/changes/ 或 /openspec/changes/archive/
   ```

3. **删除空目录和重复**:
   - 删除 `/Aether/openspec/` 整个目录
   - 清理所有 `.DS_Store` 文件
   - 删除空的 `{specs` 目录（损坏的命名）

### Phase 3: 验证和文档

1. **运行 OpenSpec 工具验证**:
   ```bash
   openspec list          # 应显示所有 changes
   openspec list --specs  # 应显示所有 specs
   openspec validate --all
   ```

2. **更新引用**:
   - 检查是否有脚本或文档引用旧路径
   - 更新 CLAUDE.md 中的路径（如有）

3. **创建迁移记录**:
   - 在本 proposal 中记录所有移动的文件
   - 创建 MIGRATION.md 说明变更

## File Operations Plan

### 删除的目录

```
/Aether/openspec/                                    # 整个子目录
/Aether/openspec/changes/complete-phase2-testing-and-polish/openspec/  # 嵌套重复
```

### 移动/合并的内容

| 源路径 | 目标路径 | 操作 |
|--------|---------|------|
| `/Aether/openspec/changes/complete-phase2-testing-and-polish/` | 检查后归档或删除 | 与 `/openspec/changes/archive/2025-12-23-complete-phase2-testing-and-polish/` 比较 |
| 嵌套的 spec 文件 | 提取到正确位置或删除 | 如果是重复内容则删除 |

### 保留的结构

```
/openspec/                          # 唯一保留
/.claude/commands/openspec/         # CLI 工具（不动）
```

## Migration Strategy

### Step 1: 备份

```bash
# 创建完整备份
tar -czf ~/aether_openspec_backup_$(date +%Y%m%d).tar.gz \
  /Users/zouguojun/Workspace/Aether/openspec \
  /Users/zouguojun/Workspace/Aether/Aether/openspec
```

### Step 2: 审计差异

```bash
# 比较两个 openspec 目录
diff -r openspec/ Aether/openspec/

# 检查嵌套内容
find Aether/openspec -type f -name "*.md" | while read f; do
  echo "=== $f ==="
  head -5 "$f"
done
```

### Step 3: 执行迁移

```bash
# 1. 检查 Aether/openspec 是否有独特内容
# 2. 如有，提取到主 openspec
# 3. 删除 Aether/openspec
rm -rf Aether/openspec/

# 4. 清理 .DS_Store
find openspec/ -name ".DS_Store" -delete

# 5. 修复损坏的目录名
# (如 {specs -> specs)
```

### Step 4: 验证

```bash
# 确保所有 changes 可见
openspec list

# 确保所有 specs 可见
openspec list --specs

# 运行验证
openspec validate --all
```

## Risks & Mitigations

### Risk 1: 意外丢失内容
**Impact**: 删除了未归档的重要提案
**Likelihood**: Medium
**Mitigation**:
- 执行前完整备份
- 逐个比较文件内容再删除
- 使用 `git` 跟踪所有删除操作
- 保留备份 7 天以上

### Risk 2: 破坏 CLI 工具
**Impact**: `openspec` 命令无法正常工作
**Likelihood**: Low
**Mitigation**:
- 不触碰 `.claude/commands/openspec/` 目录
- 迁移后测试所有 openspec 子命令
- 准备回滚脚本

### Risk 3: 破坏进行中的工作
**Impact**: 正在开发的 changes 受影响
**Likelihood**: Low
**Mitigation**:
- 确认所有进行中的 changes 位于主 openspec
- 迁移不改变主 openspec 的内容
- 只删除确认重复或废弃的内容

## Success Criteria

### 功能验证

- [ ] `openspec list` 显示所有活跃的 changes
- [ ] `openspec list --specs` 显示所有规范
- [ ] `openspec validate --all` 通过无错误
- [ ] 所有路径都是单一层级（无嵌套 openspec）

### 结构验证

- [ ] 只存在一个 `/openspec/` 根目录
- [ ] 不存在 `/Aether/openspec/` 目录
- [ ] 所有 changes 都在 `/openspec/changes/` 或 `archive/` 下
- [ ] 无 4 层以上的嵌套目录

### 内容验证

- [ ] 无内容丢失（与备份比较）
- [ ] 无重复的 changes
- [ ] 所有 `.md` 文件格式正确

### 文档验证

- [ ] 创建了 MIGRATION.md 记录变更
- [ ] 更新了相关文档中的路径引用
- [ ] 本 proposal 记录了所有操作

## Open Questions

1. **Aether/openspec 的内容是否包含独特信息？**
   - 需要逐个比较 changes
   - 如有独特内容，决定如何合并

2. **嵌套的 openspec 目录是如何产生的？**
   - 可能是工具错误
   - 需要防止未来再次发生

3. **是否需要保留 Aether 子项目的独立 openspec？**
   - 当前判断：不需要（单体仓库）
   - 如果未来拆分子项目，再考虑

## Related Changes

- **Depends On**: None
- **Blocks**: None
- **Related**: 所有未来的 OpenSpec 相关工作都会受益于清晰的架构

## Spec Deltas

本变更不引入新的功能规范，属于**重构/组织变更**。

主要影响：
- **openspec-structure**: 定义单一规范目录结构
- **change-management**: 清理变更提案管理
- **documentation-organization**: 改进文档组织

## Implementation Notes

### 工具要求

- Bash shell (for scripting)
- `diff` (for comparison)
- `find`, `grep`, `tree` (for auditing)
- `tar` (for backup)
- `openspec` CLI (for validation)

### 时间估计

- Phase 1 (审计): 1 hour
- Phase 2 (迁移): 1 hour
- Phase 3 (验证): 0.5 hour
- 文档更新: 0.5 hour

**总计**: 3 hours

### 回滚策略

如果迁移失败：

```bash
# 从备份恢复
cd /Users/zouguojun/Workspace/Aether
tar -xzf ~/aether_openspec_backup_*.tar.gz

# 验证恢复
openspec list
git status
```

## Appendix: 当前状态快照

### 主 OpenSpec 目录

```
/openspec/
├── AGENTS.md
├── changes/
│   ├── add-contextual-memory-rag/         (7/108 tasks)
│   ├── add-macos-client-and-halo-overlay/ (117/147 tasks)
│   ├── enhance-halo-overlay/              (51/193 tasks)
│   ├── remove-audio-and-accessibility/    (23/26 tasks)
│   └── archive/
│       ├── 2025-12-23-add-rust-core-foundation/
│       └── 2025-12-23-complete-phase2-testing-and-polish/
├── project.md
└── specs/ (8 specs, 38 requirements)
```

### Aether 子目录（待清理）

```
/Aether/openspec/
└── changes/
    └── complete-phase2-testing-and-polish/
        ├── openspec/            # 嵌套重复
        │   └── changes/         # 再次嵌套
        │       └── complete-phase2-testing-and-polish/
        ├── Scripts/
        ├── specs/
        └── {specs               # 损坏的目录名
```

### CLI 工具（不改动）

```
/.claude/commands/openspec/
└── (保持原样)
```
