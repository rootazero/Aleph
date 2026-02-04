# Design: OpenSpec Architecture Consolidation

**Change ID**: `consolidate-openspec-architecture`

## Design Principles

### 1. Single Source of Truth (单一真实来源)

**原则**: 项目中应只有一个规范的 OpenSpec 目录

**理由**:
- 避免同步开销
- 防止版本冲突
- 简化工具实现
- 降低认知负担

**实施**:
```
✓ /openspec/               # 唯一规范目录
✗ /Aleph/openspec/        # 删除重复
✗ /*/openspec/             # 禁止嵌套
```

### 2. Flat Change Hierarchy (扁平变更层级)

**原则**: Changes 应该位于固定深度的目录结构中

**理由**:
- 工具易于解析
- 路径可预测
- 避免深度嵌套
- 简化导航

**实施**:
```
openspec/
└── changes/
    ├── <change-id-1>/      # 深度: 2
    ├── <change-id-2>/      # 深度: 2
    └── archive/            # 深度: 2
        ├── <archived-1>/   # 深度: 3
        └── <archived-2>/   # 深度: 3
```

**禁止**:
```
openspec/
└── changes/
    └── change-a/
        └── openspec/       # ✗ 嵌套 openspec
            └── changes/    # ✗ 重复结构
```

### 3. Clean Separation (清晰分离)

**原则**: OpenSpec 内容与应用代码物理分离

**理由**:
- 清晰的关注点分离
- OpenSpec 工具独立运行
- 避免构建/部署时包含不必要的元数据
- 简化权限管理

**实施**:
```
/openspec/          # 规范和变更管理
/Aleph/            # 应用源码 (无 openspec)
/.claude/           # 工具和命令
```

## Architecture Decisions

### Decision 1: 删除而非移动 Aleph/openspec

**选项分析**:

| 方案 | 优点 | 缺点 | 选择 |
|------|------|------|------|
| A. 移动到主 openspec | 保留所有历史 | 可能引入重复 | ✗ |
| B. 合并内容 | 保留独特部分 | 需要复杂的差异分析 | ✗ |
| C. 直接删除 | 最简单，最清晰 | 可能丢失内容 | ✓ |

**决策**: 选择 C (直接删除)

**理由**:
1. 审计发现 Aleph/openspec 完全是重复内容
2. complete-phase2-testing-and-polish 已在主 openspec/changes/archive/
3. 嵌套的 openspec 是工具错误导致的垃圾数据
4. 无独特或更新的内容需要保留

**风险缓解**:
- 执行前完整备份
- 逐个比对文件确认重复
- 保留备份 7+ 天

### Decision 2: 不拆分子项目 OpenSpec

**问题**: 是否为 Aleph 子项目保留独立的 openspec？

**选项分析**:

| 方案 | 适用场景 | 优点 | 缺点 |
|------|---------|------|------|
| 单体 OpenSpec | 单一代码库 | 简单，统一 | 不适合多仓库 |
| 分布式 OpenSpec | 多仓库微服务 | 独立演进 | 同步复杂 |

**决策**: 单体 OpenSpec

**理由**:
1. Aleph 是单一代码库（monorepo）
2. Rust core 和 Swift client 紧密耦合
3. Changes 通常跨越多个子项目
4. 无拆分计划

**未来考虑**:
如果项目拆分为多个独立仓库：
- core → separate repo with own openspec
- macos-client → separate repo with own openspec
- 使用 git submodules 或 cross-repo references

### Decision 3: 保留 .claude/commands/openspec

**问题**: CLI 工具是否也需要整合？

**决策**: 保持不变

**理由**:
- `.claude/` 是 CLI 工具的标准位置
- openspec 命令需要特定目录结构
- 工具代码与规范内容职责不同
- 变更工具位置会破坏现有集成

## Data Model

### 目标目录结构

```
/openspec/
├── AGENTS.md                   # OpenSpec 代理使用指南
├── project.md                  # 项目上下文和约定
├── MIGRATION.md                # (新增) 迁移记录
├── changes/                    # 变更提案
│   ├── <change-id>/
│   │   ├── proposal.md         # 提案文档
│   │   ├── tasks.md            # 任务列表
│   │   ├── design.md           # (可选) 设计文档
│   │   ├── README.md           # (可选) 概述
│   │   └── specs/              # Spec deltas
│   │       └── <capability>/
│   │           └── spec.md
│   └── archive/                # 已归档的变更
│       └── YYYY-MM-DD-<id>/
└── specs/                      # 规范定义
    └── <capability>/
        └── spec.md             # 规范文档
```

### 目录深度限制

| 路径类型 | 最大深度 | 示例 |
|---------|---------|------|
| OpenSpec 根 | 1 | `/openspec/` |
| Change | 2 | `/openspec/changes/<id>/` |
| Archive | 3 | `/openspec/changes/archive/<id>/` |
| Spec delta | 4 | `/openspec/changes/<id>/specs/<cap>/` |
| Spec | 3 | `/openspec/specs/<cap>/` |

**规则**: 禁止在 changes/<id>/ 或 specs/<cap>/ 下再嵌套 openspec 目录

## Migration Strategy

### Phase 1: 审计 (不改变任何内容)

```
输入:
- /openspec/
- /Aleph/openspec/

处理:
1. 生成完整备份
2. 列出所有文件
3. 比较内容差异
4. 识别重复和独特内容

输出:
- 备份文件
- 审计报告
- 差异分析
```

### Phase 2: 整合 (删除重复)

```
输入:
- 审计报告
- 已验证的重复列表

处理:
1. 提取任何独特内容 (如有)
2. 删除 /Aleph/openspec/
3. 清理临时文件
4. 更新 .gitignore

输出:
- 单一的 /openspec/ 目录
- Git commit 记录
```

### Phase 3: 验证 (确保完整性)

```
输入:
- 整合后的 /openspec/

处理:
1. 运行 openspec 工具
2. 验证目录结构
3. 创建迁移文档
4. 更新相关引用

输出:
- 验证报告
- MIGRATION.md
- 更新的文档
```

## Tool Integration

### OpenSpec CLI 工作流

整合后，工具行为保持不变：

```bash
# 列出 changes（从 /openspec/changes/ 读取）
$ openspec list
Changes:
  add-contextual-memory-rag
  add-macos-client-and-halo-overlay
  enhance-halo-overlay
  remove-audio-and-accessibility
  consolidate-openspec-architecture

# 列出 specs（从 /openspec/specs/ 读取）
$ openspec list --specs
Specs:
  build-integration
  clipboard-management
  ...

# 验证（检查 /openspec/ 下的所有内容）
$ openspec validate --all
✓ All validations passed
```

### 工具假设

OpenSpec CLI 假设：
1. 单一 `openspec/` 目录在项目根
2. `changes/` 和 `specs/` 是 `openspec/` 的直接子目录
3. 每个 change 有 `proposal.md` 和 `tasks.md`
4. 每个 spec 有 `spec.md`

**整合保证这些假设**

## Error Prevention

### 防止未来重复

#### 方法 1: .gitignore 规则

```gitignore
# 防止在子目录创建 openspec
*/openspec/
!.claude/commands/openspec/

# 允许根目录 openspec
!/openspec/
```

#### 方法 2: 预提交钩子

```bash
#!/bin/bash
# .git/hooks/pre-commit

# 检查是否有非法的 openspec 目录
illegal=$(find . -type d -name "openspec" \
  -not -path "./openspec" \
  -not -path "./.claude/*" \
  -not -path "./.git/*")

if [ -n "$illegal" ]; then
  echo "Error: Nested openspec directories detected:"
  echo "$illegal"
  echo "Only /openspec/ is allowed. Please consolidate."
  exit 1
fi
```

#### 方法 3: CI 检查

```yaml
# .github/workflows/openspec-validation.yml
name: OpenSpec Validation

on: [push, pull_request]

jobs:
  validate-structure:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Check for nested openspec
        run: |
          count=$(find . -type d -name "openspec" \
            -not -path "./openspec" \
            -not -path "./.claude/*" | wc -l)
          if [ $count -gt 0 ]; then
            echo "Error: Nested openspec directories found"
            exit 1
          fi
```

## Alternatives Considered

### Alternative 1: 保留两个 OpenSpec 目录

**方案**: 维护 `/openspec/` (全局) 和 `/Aleph/openspec/` (本地)

**优点**:
- 符合某些多项目结构
- Aleph 子项目可独立管理

**缺点**:
- 需要同步机制
- 工具需要支持多根目录
- 容易出现不一致
- 认知负担高

**拒绝理由**: Aleph 是单体项目，无需分离

### Alternative 2: 移动到 docs/openspec/

**方案**: 将 `/openspec/` 移到 `/docs/openspec/`

**优点**:
- 更明确属于文档
- 与 README, CLAUDE.md 等并列

**缺点**:
- openspec 不仅是文档，还是规范管理工具
- 工具默认查找 `./openspec/`
- 改变工具约定

**拒绝理由**: 遵循 OpenSpec 标准约定

### Alternative 3: Git Submodule

**方案**: 将 openspec 作为独立 Git 仓库的 submodule

**优点**:
- 完全独立的版本管理
- 可跨项目共享规范

**缺点**:
- 增加复杂度
- 同步困难
- 不适合单一项目

**拒绝理由**: 过度工程化

## Implementation Risks

### High Risk: 意外丢失内容

**场景**: 删除 Aleph/openspec 时丢失未归档的重要文档

**概率**: Medium
**影响**: High
**缓解**:
- 执行前完整备份
- 逐文件比对确认重复
- 保留备份 7+ 天
- Git 历史可追溯

### Medium Risk: 工具兼容性

**场景**: 删除后 openspec 命令异常

**概率**: Low
**影响**: Medium
**缓解**:
- 不修改 .claude/commands/openspec/
- 整合后立即运行工具验证
- 准备回滚脚本

### Low Risk: 文档引用失效

**场景**: 某些文档仍引用旧路径

**概率**: Medium
**影响**: Low
**缓解**:
- 全局搜索 "Aleph/openspec"
- 更新所有引用
- 验证链接有效性

## Success Criteria

整合成功的标志：

### 结构验证
- [ ] 只有一个 `/openspec/` 根目录
- [ ] 无 `/Aleph/openspec/` 目录
- [ ] 无嵌套的 `openspec/changes/*/openspec/` 结构
- [ ] 最大目录深度 <= 5 层

### 功能验证
- [ ] `openspec list` 显示所有活跃 changes
- [ ] `openspec list --specs` 显示所有 specs
- [ ] `openspec validate --all` 通过
- [ ] `openspec show <any-change>` 正常工作

### 内容验证
- [ ] 所有 proposal.md 文件可访问
- [ ] 所有 tasks.md 文件可访问
- [ ] 所有 spec.md 文件可访问
- [ ] 无内容丢失（与备份比对）

### 文档验证
- [ ] MIGRATION.md 已创建
- [ ] 所有文档路径引用正确
- [ ] README/CLAUDE.md 保持最新

### Git 验证
- [ ] 变更已提交
- [ ] Commit 消息清晰
- [ ] 可以安全推送
- [ ] 备份已保留

## Future Enhancements

整合完成后，可以考虑的改进：

1. **自动化工具**:
   - 创建 `openspec check-structure` 命令
   - 集成到 CI/CD

2. **文档生成**:
   - 从 openspec/ 生成静态文档网站
   - 自动更新 changelog

3. **变更模板**:
   - 标准化 proposal/tasks/design 模板
   - 提供 `openspec new <id>` 脚手架命令

4. **多项目支持**:
   - 如果未来拆分仓库，设计跨仓库引用机制
   - 考虑 OpenSpec federation

## References

- OpenSpec 官方规范: (如有)
- 项目约定: `/openspec/project.md`
- 类似项目: (如 Kubernetes proposals, Rust RFCs)
