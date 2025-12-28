# OpenSpec Architecture Consolidation

**Change ID**: `consolidate-openspec-architecture`
**Status**: Proposed
**Priority**: High (blocks clean development)

## 问题

当前项目存在多个 OpenSpec 目录，导致混乱：
- `/openspec/` - 主目录
- `/Aether/openspec/` - 重复副本
- 深度嵌套的 openspec 结构（4-5 层）

## 解决方案

整合为**单一规范的 OpenSpec 架构**：
- ✅ 只保留 `/openspec/` 作为唯一根目录
- ✅ 删除所有重复和嵌套结构
- ✅ 扁平化 changes 层级
- ✅ 建立防止未来重复的机制

## 影响范围

- **删除**: `/Aether/openspec/` 整个目录
- **清理**: 嵌套的 openspec、.DS_Store 文件
- **更新**: .gitignore、文档引用
- **不影响**: 功能代码、CLI 工具

## 快速开始

### 验证提案

```bash
openspec validate consolidate-openspec-architecture --strict
```

### 查看文档

- [proposal.md](./proposal.md) - 完整提案和理由
- [tasks.md](./tasks.md) - 12 个实施任务
- [design.md](./design.md) - 架构设计和决策
- [specs/openspec-structure/spec.md](./specs/openspec-structure/spec.md) - 结构规范

## 实施时间

预计 **2-3 小时**：
- Phase 1: 审计和备份 (1h)
- Phase 2: 整合执行 (0.5h)
- Phase 3: 验证和文档 (0.75h)

## 风险评估

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|---------|
| 内容丢失 | Medium | High | 完整备份 + 逐文件比对 |
| 工具兼容性 | Low | Medium | 不改 CLI + 立即验证 |
| 文档引用失效 | Medium | Low | 全局搜索 + 更新 |

## 成功标准

- [ ] `openspec list` 显示所有 changes
- [ ] `openspec validate --all` 通过
- [ ] 只有一个 `/openspec/` 根目录
- [ ] 无嵌套 openspec 结构
- [ ] 所有测试通过

## 下一步

1. **审阅提案**: 阅读 proposal.md 和 design.md
2. **提问澄清**: 如有疑问，提出讨论
3. **批准**: 批准后开始实施
4. **执行**: 按 tasks.md 顺序执行
5. **验证**: 运行所有验证检查

## 联系

如有问题或建议，请通过以下方式反馈：
- GitHub Issues
- Pull Request Comments
- Team Chat

---

**注意**: 本变更是纯组织性质的重构，不改变任何功能代码。
