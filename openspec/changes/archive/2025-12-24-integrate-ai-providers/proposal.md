# Proposal: Integrate AI Providers

## Change ID
`integrate-ai-providers`

## Why

当前 Aleph 已完成基础架构（hotkey detection、clipboard management、memory module），但缺少最核心的 AI 处理能力。没有这个变更，用户无法：
- 将剪贴板内容发送到任何 AI 服务进行处理
- 根据不同场景选择合适的 AI 模型（代码用 Claude，绘图用 OpenAI）
- 利用本地 Ollama 模型避免 API 费用和隐私问题
- 享受记忆增强的上下文感知 AI 交互

这是 Aleph 路线图的 Phase 5，是实现完整"Cut → AI → Paste"流程的关键里程碑。

## Overview
实现 Phase 5 的核心目标：集成多个 AI 提供商（OpenAI、Claude、Ollama）并实现智能路由系统。该变更将使 Aleph 能够根据用户配置的规则将剪贴板内容路由到合适的 AI 提供商，并将响应结果返回。

## Motivation
Phase 5 的目标是：
1. 连接真实的 AI 服务（OpenAI、Claude、Ollama）
2. 实现配置驱动的智能路由系统
3. 完成端到端的"Cut → Process → Paste"流程
4. 将记忆模块与 AI pipeline 集成，实现上下文增强

## Goals
1. **AI Provider Interface**: 定义统一的 `AiProvider` trait，支持多种 AI 后端
2. **OpenAI Integration**: 实现 OpenAI API 客户端（GPT-4o、GPT-4o-mini）
3. **Claude Integration**: 实现 Anthropic Claude API 客户端
4. **Ollama Integration**: 实现本地 Ollama 命令行执行
5. **Router System**: 实现基于 regex 的路由规则匹配
6. **Configuration**: 扩展 config.toml 支持 providers 和 routing rules
7. **Error Handling**: 统一的错误处理和回退策略
8. **Memory Integration**: 将记忆模块与 AI pipeline 集成，实现上下文增强

## Scope

### In Scope
- `AiProvider` trait 定义（统一接口）
- OpenAI API 客户端（使用 `reqwest` + `tokio`）
- Claude API 客户端（使用 `reqwest` + `tokio`）
- Ollama 客户端（使用 `tokio::process::Command`）
- Router 模块（regex 匹配 + provider 选择）
- Config 扩展（providers、rules 配置）
- 异步处理管道（tokio async pipeline）
- API 超时和错误处理
- 记忆检索与提示词增强
- UniFFI 接口扩展（AI 处理相关回调）

### Out of Scope (Future Phases)
- Google Gemini 集成（Phase 5.5）
- DALL-E 图像生成（Phase 6）
- 流式响应（Phase 6）
- 用户 Settings UI（Phase 6）
- PII 过滤（Phase 6）
- 智能上下文窗口管理（Phase 6）

## Dependencies
- **Requires**: Phase 1-4 完成（hotkey、clipboard、memory module）
- **Blocks**: Phase 6（Settings UI 需要 AI providers 已实现）
- **Related**: `memory-augmentation` spec（需集成记忆检索）

## Affected Capabilities
这个变更将创建以下新的 capabilities：
1. `ai-provider-interface` - AI 提供商统一接口
2. `openai-provider` - OpenAI API 客户端
3. `claude-provider` - Claude API 客户端
4. `ollama-provider` - Ollama 本地模型客户端
5. `ai-routing` - 智能路由系统

同时会修改：
- `core-library` - 集成 AI pipeline 到 AlephCore
- `event-handler` - 新增 AI 处理状态回调
- 间接影响 `memory-augmentation` - 需要与 AI pipeline 集成

## Risks and Mitigations

### Risk 1: API Rate Limiting
- **Impact**: OpenAI/Claude API 可能因频繁调用被限流
- **Mitigation**:
  - 实现指数退避重试
  - 添加请求间隔配置选项
  - 用户可选择本地 Ollama 避免限流

### Risk 2: Network Latency
- **Impact**: API 调用可能耗时 2-5 秒，影响用户体验
- **Mitigation**:
  - Halo 动画提供视觉反馈
  - 实现超时机制（默认 30 秒）
  - 考虑未来添加本地缓存

### Risk 3: API Key Security
- **Impact**: 配置文件中的 API key 可能泄露
- **Mitigation**:
  - Phase 5 使用明文存储（本地文件权限保护）
  - Phase 6 迁移到 macOS Keychain
  - 文档中明确安全最佳实践

### Risk 4: Provider API Changes
- **Impact**: OpenAI/Claude 可能更改 API 格式
- **Mitigation**:
  - 使用稳定的 API 版本（v1）
  - 实现详细的错误日志
  - Trait 设计便于适配新版本

## Open Questions
1. **配置格式**: 是否需要支持环境变量（`$OPENAI_API_KEY`）？
   - **建议**: Phase 5 仅支持直接配置，Phase 6 添加环境变量

2. **默认 Provider**: 如果没有配置任何 provider，系统行为？
   - **建议**: 返回友好错误信息，引导用户配置

3. **记忆增强策略**: 记忆上下文应该如何注入到提示词？
   - **建议**: 在 system prompt 前添加 "Past Context:" 部分

4. **Ollama 模型验证**: 是否需要在启动时验证 Ollama 模型可用性？
   - **建议**: 首次调用时验证，失败后缓存状态避免重复检查

## Success Criteria
- [ ] 所有 spec deltas 通过 `openspec validate --strict`
- [ ] OpenAI provider 可成功调用 GPT-4o API
- [ ] Claude provider 可成功调用 Claude API
- [ ] Ollama provider 可成功执行本地模型
- [ ] Router 可正确匹配 regex 规则并选择 provider
- [ ] Config 可正确加载和验证 providers/rules 配置
- [ ] 记忆系统能够检索相关上下文并注入提示词
- [ ] 端到端测试：用户按 Cmd+~ → AI 处理 → 结果粘贴回去
- [ ] 所有单元测试通过（使用 mock provider）
- [ ] 错误处理覆盖所有失败场景（网络错误、超时、API 错误）

## Timeline
- Proposal Review: 1 day
- Implementation: 见 `tasks.md`
- Testing & Integration: 见 `tasks.md`

## Notes
- 该变更是 Phase 5 的完整实现，是 Aleph 最核心的功能
- 实现需严格遵循 trait-based 架构，确保可测试性
- 所有 API 调用必须使用 tokio async，避免阻塞
- 记忆增强功能是 Phase 5 的关键创新点，需确保性能（<100ms）
