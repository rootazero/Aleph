# Implementation Tasks: Consolidate OpenSpec Architecture

**Change ID**: `consolidate-openspec-architecture`

本文档列出了整合 OpenSpec 架构的具体实施任务。

## Task Breakdown

### Phase 1: 审计和备份 (Audit & Backup)

#### Task 1: 创建完整备份
**Estimated effort**: 10 minutes
**Dependencies**: None
**Validation**: 备份文件存在且可恢复

**Steps**:
1. 创建时间戳备份
   ```bash
   cd /Users/zouguojun/Workspace/Aether
   tar -czf ~/aether_openspec_backup_$(date +%Y%m%d_%H%M%S).tar.gz \
     openspec/ \
     Aether/openspec/ \
     .claude/commands/openspec/
   ```
2. 验证备份完整性
   ```bash
   tar -tzf ~/aether_openspec_backup_*.tar.gz | head -20
   ```
3. 记录备份位置和大小
   ```bash
   ls -lh ~/aether_openspec_backup_*.tar.gz
   ```

**Success criteria**:
- [x] 备份文件已创建
- [x] 备份包含所有 openspec 目录
- [x] 备份大小合理（预期 < 5MB）
- [x] 可以列出备份内容

---

#### Task 2: 审计所有 OpenSpec 目录
**Estimated effort**: 20 minutes
**Dependencies**: Task 1
**Validation**: 完整的目录清单和差异报告

**Steps**:
1. 查找所有 openspec 相关目录
   ```bash
   find . -type d \( -name "openspec" -o -name "changes" -o -name "specs" \) \
     | grep -E "(openspec|changes|specs)" \
     | sort
   ```

2. 生成目录结构报告
   ```bash
   tree -L 4 openspec/ > /tmp/main_openspec_structure.txt
   tree -L 4 Aether/openspec/ > /tmp/aether_openspec_structure.txt 2>/dev/null || echo "None"
   ```

3. 列出所有 Markdown 文件
   ```bash
   find openspec/ Aether/openspec/ -name "*.md" | sort > /tmp/all_md_files.txt
   ```

4. 识别重复文件
   ```bash
   find openspec/ Aether/openspec/ -name "*.md" -exec basename {} \; \
     | sort | uniq -d > /tmp/duplicate_basenames.txt
   ```

**Success criteria**:
- [x] 生成完整目录清单
- [x] 识别所有 .md 文件
- [x] 列出重复的文件名
- [x] 记录嵌套深度

**Deliverables**:
- `/tmp/main_openspec_structure.txt`
- `/tmp/aether_openspec_structure.txt`
- `/tmp/all_md_files.txt`
- `/tmp/duplicate_basenames.txt`

---

#### Task 3: 比较和分析重复内容
**Estimated effort**: 30 minutes
**Dependencies**: Task 2
**Validation**: 明确知道哪些内容是重复的，哪些是独特的

**Steps**:
1. 比较 complete-phase2-testing-and-polish 两个版本
   ```bash
   # 主目录归档版本
   ls -la openspec/changes/archive/2025-12-23-complete-phase2-testing-and-polish/

   # Aether 子目录版本
   ls -la Aether/openspec/changes/complete-phase2-testing-and-polish/
   ```

2. 比较文件内容
   ```bash
   # 如果两个版本都有 proposal.md，比较它们
   diff openspec/changes/archive/2025-12-23-complete-phase2-testing-and-polish/proposal.md \
        Aether/openspec/changes/complete-phase2-testing-and-polish/openspec/changes/complete-phase2-testing-and-polish/proposal.md \
        2>/dev/null || echo "Files differ or don't exist at both locations"
   ```

3. 检查嵌套 openspec 的内容
   ```bash
   find Aether/openspec/changes/complete-phase2-testing-and-polish/openspec/ \
     -name "*.md" -exec echo "=== {} ===" \; -exec head -10 {} \;
   ```

4. 创建差异报告
   ```markdown
   ## 重复内容分析

   ### complete-phase2-testing-and-polish
   - 主目录位置: openspec/changes/archive/2025-12-23-complete-phase2-testing-and-polish/
   - Aether 位置: Aether/openspec/changes/complete-phase2-testing-and-polish/
   - 嵌套位置: Aether/openspec/.../openspec/changes/complete-phase2-testing-and-polish/
   - 结论: [重复/独特/部分重叠]

   ### 其他发现
   - {specs 目录: 损坏的命名，应删除
   - Scripts/ 目录: 是否需要保留
   ```

**Success criteria**:
- [x] 完成内容比较
- [x] 识别所有重复
- [x] 确定哪些内容可以安全删除
- [x] 记录独特内容（如有）

**Deliverables**:
- 差异分析报告

---

### Phase 2: 执行整合 (Execute Consolidation)

#### Task 4: 提取独特内容（如有）
**Estimated effort**: 15 minutes
**Dependencies**: Task 3
**Validation**: 独特内容已保存到正确位置

**Steps**:
1. 根据 Task 3 的分析，提取任何独特内容
2. 如果 Aether/openspec 有新的或更完整的文件，复制到主 openspec
3. 如果有新的 specs，合并到 openspec/specs/
4. 如果有新的 scripts，评估是否需要保留

**Success criteria**:
- [x] 所有独特内容已提取
- [x] 无内容丢失
- [x] 新内容位于正确位置

**Notes**:
- 如果 Task 3 确认完全重复，此任务可跳过

---

#### Task 5: 删除 Aether/openspec 目录
**Estimated effort**: 5 minutes
**Dependencies**: Task 4
**Validation**: 目录已删除，主 openspec 包含所有内容

**Steps**:
1. 最后确认无独特内容
   ```bash
   # 再次检查
   find Aether/openspec/ -name "*.md" | wc -l
   ```

2. 删除整个 Aether/openspec 目录
   ```bash
   rm -rf Aether/openspec/
   ```

3. 验证删除
   ```bash
   ls Aether/openspec/  # 应返回错误
   ```

4. 提交删除到 Git
   ```bash
   git status
   git add -u
   git commit -m "chore: remove duplicate Aether/openspec directory

   Consolidate all OpenSpec content to the main /openspec directory.
   See openspec/changes/consolidate-openspec-architecture for details."
   ```

**Success criteria**:
- [x] Aether/openspec/ 不存在
- [x] Git 已记录删除
- [x] 主 openspec/ 包含所有必要内容

---

#### Task 6: 清理 .DS_Store 和临时文件
**Estimated effort**: 5 minutes
**Dependencies**: Task 5
**Validation**: 无系统隐藏文件残留

**Steps**:
1. 删除所有 .DS_Store 文件
   ```bash
   find openspec/ -name ".DS_Store" -delete
   ```

2. 删除任何备份文件（如 `*.bak`, `*~`）
   ```bash
   find openspec/ \( -name "*.bak" -o -name "*~" -o -name "*.tmp" \) -delete
   ```

3. 检查损坏的目录名
   ```bash
   find openspec/ -name "{*" -o -name "}*"
   ```

4. 修复或删除损坏的目录

**Success criteria**:
- [x] 无 .DS_Store 文件
- [x] 无临时文件
- [x] 无损坏的目录名

---

#### Task 7: 更新 .gitignore
**Estimated effort**: 5 minutes
**Dependencies**: Task 6
**Validation**: .gitignore 包含正确的忽略规则

**Steps**:
1. 在项目根目录的 .gitignore 中添加
   ```gitignore
   # OpenSpec system files
   openspec/**/.DS_Store
   .DS_Store

   # Backup files
   *.bak
   *~
   *.tmp
   ```

2. 如果 Aether/ 下可能重新生成 openspec，添加忽略规则
   ```gitignore
   # Prevent duplicate openspec directories
   Aether/openspec/
   ```

3. 提交更新
   ```bash
   git add .gitignore
   git commit -m "chore: update .gitignore for openspec consolidation"
   ```

**Success criteria**:
- [x] .gitignore 已更新
- [x] 防止未来重复目录

---

### Phase 3: 验证和文档 (Validation & Documentation)

#### Task 8: 运行 OpenSpec 工具验证
**Estimated effort**: 10 minutes
**Dependencies**: Task 7
**Validation**: 所有 openspec 命令正常工作

**Steps**:
1. 验证 changes 列表
   ```bash
   openspec list
   # 应显示所有活跃的 changes:
   # - add-contextual-memory-rag
   # - add-macos-client-and-halo-overlay
   # - enhance-halo-overlay
   # - remove-audio-and-accessibility
   # - consolidate-openspec-architecture
   ```

2. 验证 specs 列表
   ```bash
   openspec list --specs
   # 应显示所有 8 个 specs
   ```

3. 运行完整验证
   ```bash
   openspec validate --all
   ```

4. 测试其他命令
   ```bash
   openspec show add-contextual-memory-rag
   openspec show core-library --type spec
   ```

**Success criteria**:
- [x] `openspec list` 显示所有 changes
- [x] `openspec list --specs` 显示所有 specs
- [x] `openspec validate --all` 无错误
- [x] 所有子命令正常工作

---

#### Task 9: 验证目录结构
**Estimated effort**: 10 minutes
**Dependencies**: Task 8
**Validation**: 目录结构符合规范

**Steps**:
1. 生成整合后的目录树
   ```bash
   tree -L 3 openspec/ > /tmp/consolidated_structure.txt
   ```

2. 验证关键结构
   ```bash
   # 应该只有一个 openspec 根目录
   find . -type d -name "openspec" | grep -v ".claude" | wc -l
   # 应返回 1

   # 检查嵌套深度（不应超过 3 层）
   find openspec/ -type d | awk -F/ '{print NF}' | sort -rn | head -1
   # 应 <= 5 (openspec/changes/xxx/specs/yyy)
   ```

3. 确认无嵌套 openspec
   ```bash
   find openspec/ -type d -name "openspec"
   # 应返回空（除了根目录本身）
   ```

4. 验证 changes 位置
   ```bash
   ls openspec/changes/
   # 应显示所有 changes 和 archive/
   ```

**Success criteria**:
- [x] 只有一个 /openspec/ 根目录
- [x] 无嵌套 openspec 目录
- [x] 最大深度合理（<= 5 层）
- [x] 所有 changes 在正确位置

**Deliverables**:
- `/tmp/consolidated_structure.txt`

---

#### Task 10: 创建迁移文档
**Estimated effort**: 20 minutes
**Dependencies**: Task 9
**Validation**: 完整的迁移记录文档

**Steps**:
1. 创建 MIGRATION.md
   ```markdown
   # OpenSpec Architecture Migration

   **Date**: 2025-12-24
   **Change ID**: consolidate-openspec-architecture

   ## Summary
   整合了分散的 OpenSpec 目录为单一规范架构。

   ## Changes Made

   ### Deleted
   - `/Aether/openspec/` - 完整删除（重复内容）
   - 所有 `.DS_Store` 文件
   - 嵌套的 openspec 目录

   ### Moved
   - (如有) 列出从 Aether/openspec 移动到主 openspec 的文件

   ### Preserved
   - `/openspec/` - 唯一的规范目录
   - `/.claude/commands/openspec/` - CLI 工具

   ## Verification
   - [x] `openspec list` 显示所有 changes
   - [x] `openspec validate --all` 通过
   - [x] 目录结构清晰无嵌套

   ## Backup Location
   `~/aether_openspec_backup_20251224_*.tar.gz`

   ## Rollback Procedure
   如需回滚：
   ```bash
   cd /Users/zouguojun/Workspace/Aether
   tar -xzf ~/aether_openspec_backup_*.tar.gz
   ```
   ```

2. 将 MIGRATION.md 放到 openspec/ 根目录

3. 更新 openspec/README.md（如有）

**Success criteria**:
- [x] MIGRATION.md 已创建
- [x] 记录所有变更
- [x] 包含回滚说明
- [x] 记录备份位置

**Deliverables**:
- `openspec/MIGRATION.md`

---

#### Task 11: 更新相关文档引用
**Estimated effort**: 15 minutes
**Dependencies**: Task 10
**Validation**: 所有文档路径正确

**Steps**:
1. 搜索可能引用旧路径的文件
   ```bash
   grep -r "Aether/openspec" . --include="*.md" --include="*.swift" --include="*.rs"
   ```

2. 检查 CLAUDE.md
   ```bash
   grep -n "openspec" CLAUDE.md
   ```

3. 检查 README.md
   ```bash
   grep -n "openspec" README.md
   ```

4. 更新所有过时的路径引用

5. 提交文档更新
   ```bash
   git add .
   git commit -m "docs: update openspec path references"
   ```

**Success criteria**:
- [x] 无引用 Aether/openspec 的文档
- [x] 所有路径指向 /openspec
- [x] README 和 CLAUDE.md 保持最新

---

#### Task 12: 最终验证和提交
**Estimated effort**: 10 minutes
**Dependencies**: Task 11
**Validation**: 整个整合变更已完成并提交

**Steps**:
1. 运行完整验证套件
   ```bash
   # OpenSpec 验证
   openspec validate --all

   # 构建验证
   cd Aether/core
   cargo build --release

   # 测试验证
   cargo test
   ```

2. 查看所有变更
   ```bash
   git status
   git diff --stat
   ```

3. 创建最终提交
   ```bash
   git add .
   git commit -m "refactor: consolidate OpenSpec architecture

   - Remove duplicate Aether/openspec directory
   - Establish single /openspec root directory
   - Clean up nested and orphaned structures
   - Update documentation references

   Closes: #consolidate-openspec-architecture"
   ```

4. 创建标签（可选）
   ```bash
   git tag -a openspec-consolidation-v1 -m "OpenSpec architecture consolidation"
   ```

**Success criteria**:
- [x] 所有验证通过
- [x] Git 历史清晰
- [x] 变更已提交
- [x] 可以安全推送

---

## Task Dependencies Graph

```
Task 1 (备份)
  └─→ Task 2 (审计)
        └─→ Task 3 (比较分析)
              └─→ Task 4 (提取独特内容)
                    └─→ Task 5 (删除 Aether/openspec)
                          ├─→ Task 6 (清理临时文件)
                          │     └─→ Task 7 (更新 .gitignore)
                          │           └─→ Task 8 (OpenSpec 验证)
                          │                 └─→ Task 9 (结构验证)
                          └─→ Task 10 (迁移文档)
                                ├─→ Task 11 (更新引用)
                                └─→ Task 12 (最终验证)
```

## Parallelizable Work

以下任务可以并行执行：
- Task 6 (清理) 和 Task 10 (文档) - 独立操作
- Task 8 (工具验证) 和 Task 9 (结构验证) - 不同方面的验证

## Estimated Timeline

**Phase 1 (审计)**: 1 hour
- Tasks 1-3

**Phase 2 (整合)**: 30 minutes
- Tasks 4-7

**Phase 3 (验证)**: 45 minutes
- Tasks 8-12

**总计**: ~2.25 hours

## Success Metrics

整合完成后验证：
- [ ] 所有 12 个任务完成
- [ ] `openspec list` 显示所有活跃 changes
- [ ] `openspec validate --all` 通过
- [ ] 只有一个 /openspec 根目录
- [ ] 无嵌套 openspec 结构
- [ ] MIGRATION.md 已创建
- [ ] 备份已保留
- [ ] 所有测试通过
- [ ] Git 历史清晰

## Rollback Plan

如需回滚整个迁移：

```bash
# 1. 恢复备份
cd /Users/zouguojun/Workspace/Aether
tar -xzf ~/aether_openspec_backup_*.tar.gz

# 2. 丢弃 Git 变更（如未推送）
git reset --hard HEAD~1  # 或者具体的 commit hash

# 3. 验证恢复
openspec list
ls Aether/openspec/

# 4. 重新评估问题
```

## Notes

- 保留备份至少 7 天
- 如果发现 Aether/openspec 有重要独特内容，暂停 Task 5，先完整提取
- 整合不改变 .claude/commands/openspec/ CLI 工具
- 本变更是纯组织性质，不影响功能代码
