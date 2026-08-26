# Identity & Soul — Deepening Round 2

**Branch**: `feat/identity-soul-deepen-2`
**Date**: 2026-08-24
**Scope**: `docs/reference/FEATURE_LOCATOR.md` §1.3 — 人格与灵魂注入

## 0. 参考对比摘要 (Gap Analysis)

| 维度 | openclaw | hermes-agent | Aleph 现状 | 差距 / 机会 |
|---|---|---|---|---|
| 身份文件清单 | `IDENTITY.md`(含 avatar 2MiB) + workspace 相对 | `SOUL.md`(per-agent) + `AGENTS.md`(cwd) + `USER.md`(profile) + `MEMORY.md` | 5 文件白名单(SOUL/IDENTITY/AGENTS/TOOLS/HEARTBEAT);MEMORY 由 curated | Aleph 多 TOOLS/HEARTBEAT(优于) |
| 结构化字段 | `parseIdentityMarkdown` 6 字段(name/theme/creature/vibe/emoji/avatar) | 单字符串 | `AgentIdentityProfile` 5 字段已移植 | 已对齐 + 缺生产消费(机会) |
| IDENTITY.md 合并写 | `mergeIdentityMarkdownContent` 去重 + 保留无关 markdown + 智能插入 | 单字符串覆盖 | 整体覆盖;缺 merge 语义 | **借鉴机会**(Phase 2) |
| 稳定区缓存 | — | 整个 stable tier session 级缓存;仅压缩触发重建 | Stable 区(50/75/80)按 layer 边界打 cache breakpoint,每轮重读 | 每轮重读浪费 IO(机会,Phase 3) |
| 跨平台 hint | — | `platform_hints` replace/append/字符串 per-platform | 无 | 借鉴机会 |
| USER.md profile | — | volatile 区,profile/user-side 记忆 | 无独立文件 | curated memory 代偿(不补) |
| 写入面对齐 | — | — | self_config/identity.set/agents.files/teams materialize 四面不对齐 | **熵减机会(Phase 1-2)** |

## 1. 重构计划 (分阶段,连线优先)

### Phase 1 — 熵减 (最高 ROI / 最低风险)

| # | 项 | 删/改 | 风险 | 收益 |
|---|---|---|---|---|
| 1.1 | 删 `.aleph/` 影子目录 read-prefer 行为 | 改 `resolve_path` (identity_files.rs:93-103),回退根目录 | 极低(无生产代码写 `.aleph/`) | 消除静默失效源 |
| 1.2 | 删死链 `ResolvedAgent.soul_md/agents_md` + `AgentInstanceConfig.system_prompt` + `gateway/identity_loader.rs` | 删结构体字段 + 整个模块;更新测试 | 中 | 消除 boot 期白做工;揭示 `agent_create.system_prompt` 参数的真实生效路径 |
| 1.3 | `identity.set` IO 错误映射 `INTERNAL_ERROR` | 改 `handle_set` 错误分支 | 低 | 客户端可正确分类 |
| 1.4 | 加 `>1MB` 写入拒绝守卫测试 | 新增 2 个测试 | 极低 | 守住写入上限边界 |
| 1.5 | SoulLayer `supports_mode` 补齐 + 守卫 | 改 soul.rs + 测试 | 低 | 与 Profile/IdentityFiles 层对齐;消除 Minimal 模式注入孤行 |
| 1.6 | 端到端守卫:写 → 下一轮 prompt 含新内容 | 新增 harness_bridge 测试 | 低 | 钉住写入面→注入链路 |

### Phase 2 — 收敛写入面

| # | 项 | 风险 | 收益 |
|---|---|---|---|
| 2.1 | `write_identity_file` 提取 async 原语,被 self_config 复用 | 中 | 消灭双份 MAX_FILE_CONTENT_SIZE + 双份主流程 |
| 2.2 | `IDENTITY.md` merge 写入语义(借鉴 openclaw) | 中 | 结构化写入不污染其他 markdown |

### Phase 3 — 增强 (Phase 1/2 后视情况再做)

| # | 项 | 风险 |
|---|---|---|
| 3.1 | Session-stable identity 缓存(hermes 借鉴) | 中(要保证 stale 检测) |
| 3.2 | 结构化优先截断 (替换 head70/tail20) | 高(动 prompt 字节 ratchet) |
| 3.3 | AgentIdentityProfile role/vibe/emoji 注入 prompt | 中 |

**本轮实施 Phase 1 + Phase 2.1**,其他留作下一轮。

## 2. 显式不做

- 不删 `SoulManifest`(807 行):仍承担 identity.get 预览职责,且 enum/字段有 serde 用户。删它需要破坏性变更,收益不及风险。
- 不动 `teams materialize` SOUL.md 覆写:风险面广,需要单独的 teams 改造配合。
- 不动 `agents.files.*` 写入面:Panel 路径,改动需要 UI 协议配合。
- 不动 prompt 层 stable/dynamic 分区结构(那是另一个 task 的领域)。
- 不引入新依赖。

## 3. 验证清单

- `cargo check -p alephcore --lib`
- `cargo clippy -p alephcore -- -D warnings`
- `cargo test -p alephcore --lib` 全绿
- 新增守卫全部断言通过
- 至少一个原版的写→注入端到端守卫钉住接缝