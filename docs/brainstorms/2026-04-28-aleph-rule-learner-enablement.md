---
date: 2026-04-28
topic: aleph-rule-learner-enablement
---

# P0-1: 启用 RuleLearner → Skill 转化

## Problem Frame

Aleph 的 RuleLearner（NaiveBayes L2 规则生成器）是死代码。L3 执行成功后应该自动从中提取经验，转化为可执行的 **Skill** 存入记忆 Note 层。当前 LearningAgent 和 RuleLearner 都未连接到 Skill 生成管道。

## Requirements

### 核心架构

- **R1**: RuleLearner 作为独立模块激活，移除 `#[allow(dead_code)]` 标记
- **R2**: RuleLearner 只从 L3 **成功**执行中学习（第一阶段）

### 学习与 Skill 生成流程

- **R3**: L3 执行成功后，LearningAgent 监听并触发 RuleLearner 训练
- **R4**: RuleLearner 接收 (input, action) 对，使用 NaiveBayes 积累 PatternRecord
- **R5**: 当 PatternRecord 满足置信度阈值（`MIN_EXECUTIONS=3`, `MIN_CONFIDENCE=0.8`）时，RuleLearner 生成完整 **Skill 对象**
- **R6**: 生成的 Skill 包含: name, description, trigger pattern, action
- **R7**: Skill 通过 SkillSystem 存入 Note Layer（现有 Skill 枚举机制）

### Skill 生成内容

- **R8**: Skill name = `learned_{action_type}_{hash(pattern)}`（如 `learned_bash_git_status`）
- **R9**: Skill trigger = 从 input 提取的关键词 pattern（regex）
- **R10**: Skill action = 对应的 AtomicAction
- **R11**: Skill metadata 记录: 使用次数、成功率、首次学习时间

### 与现有 Skill System 集成

- **R12**: 生成的 Skill 注册到 Skill System 的 skill 枚举（现有 `skill_manage.rs` 机制）
- **R13**: Skill 可被 Skill System 发现、列出、执行（复用现有能力）
- **R14**: Note Layer 中 Skill 以 markdown skill 文件形式存储

### 验证

- **R15**: 添加 RuleLearner 单元测试，覆盖 PatternRecord 积累和 Skill 生成逻辑
- **R16**: 集成测试验证 L3 成功 → RuleLearner → Skill → Note Layer 完整链路

## Success Criteria

- [ ] `cargo test -p alephcore rule_learner` 通过
- [ ] L3 成功后 RuleLearner 正确积累 PatternRecord
- [ ] 满足置信度阈值时生成完整 Skill 对象
- [ ] Skill 通过 SkillSystem 正确存入 Note Layer
- [ ] Note Layer 中的 Skill 可被 Skill System 发现和执行

## Scope Boundaries

- **非目标**: LearningAgent 完整激活（部分实现即可，P0-2 再完整化）
- **非目标**: 失败反馈学习（属于 P1-3）
- **非目标**: DreamDaemon 记忆整合（属于 P0-3）
- **非目标**: RuleLearner 作为 L2 KeywordRule 使用（它是 Skill 生成器，不是 L2 加速器）

## Key Decisions

- **RuleLearner → Skill 转化**: RuleLearner 满置信度后生成完整 Skill 对象，而非 L2 KeywordRule
- **仅成功**: 第一阶段只从 L3 成功案例学习
- **现有 Skill 枚举复用**: 生成 Skill 通过现有 `skill_manage.rs` 机制注册，不重复建设

## Dependencies / Assumptions

- SkillSystem 已有 skill 枚举注册能力（`skill_manage.rs` 已有 `install_skill` 等方法）
- Note Layer 的 markdown skill 文件格式已定义
- FeatureExtractor 在 rule_learner.rs 中已实现
- ActionClass → AtomicAction 转换已实现（`action_to_class` 和 `action_to_type`）

## Outstanding Questions

### Resolve Before Planning

- **D1** [确认] Skill System 的 `install_skill` 方法签名和参数？需要读取 `skill_manage.rs` 确认 Skill 对象结构

### Deferred to Planning

- **D2** [技术] Skill 存入 Note Layer 的具体路径和格式？
- **D3** [技术] RuleLearner 状态是否需要持久化（重启后保留学习结果）？
- **D4** [技术] LearningAgent 如何监听 L3 执行结果（事件机制还是直接调用）？
