# Add Search Settings UI

## Summary

为 Aether 添加完整的 Search Settings UI，包括：
1. 图形化配置 6 个搜索供应商（Tavily, SearXNG, Google, Bing, Brave, Exa）
2. 提供商测试连接、备选顺序管理、PII 隐私保护
3. 增强斜杠命令系统，支持精准匹配和空格验证
4. 预设 `/search`, `/mcp`, `/skill` 路由规则

此变更基于已实现的结构化上下文协议和搜索能力集成，将后端功能暴露给用户，实现零配置开箱即用。

## What

### Primary: Search Settings UI

**核心能力**：
1. **Provider Configuration Cards** - 6 个搜索供应商配置卡片
   - 状态指示器：⚠️ 未配置 / ✅ 可用 / ❌ 离线 / 🔄 测试中
   - 配置字段：API Key（SecureField）、Base URL、Search Depth 等
   - 测试连接按钮，实时验证配置正确性
   - 文档链接（每个供应商的 API 文档）

2. **Provider Testing API** - Rust Core UniFFI 导出
   - `test_search_provider(name: String) -> ProviderTestResult`
   - 异步测试，返回延迟或错误信息
   - 结果缓存 5 分钟，避免 API 配额滥用

3. **Preset Templates** - 供应商默认配置模板
   - 每个供应商的必填/可选字段定义
   - 默认值（如 SearXNG 的 `https://searx.be`）
   - 字段类型（SecureField, TextField, Picker）

4. **Fallback Order Management** - 拖拽排序 UI
   - 可视化备选供应商顺序
   - 同步到 `config.toml` 的 `fallback_providers` 数组

5. **PII Settings Migration** - 从 Behavior 移至 Search
   - 移动 PII scrubbing 配置到 Search 标签页
   - 配置自动迁移：`[behavior.pii_*]` → `[search.pii.*]`
   - 更新本地化键：`settings.behavior.pii_*` → `settings.search.pii_*`

### Secondary: Command Validation

**核心能力**：
1. **Whitespace Enforcement** - 强制命令与参数之间有空格
   - 有效格式：`/search quantum computing` ✅
   - 无效格式：`/searchquery` ❌
   - Router 验证逻辑在 regex 匹配前执行

2. **Token-Based Matching** - 基于 token 的精准匹配
   - `/se custom prompt` 不会匹配 `/search` 规则
   - 允许短命令与长命令共存
   - 第一匹配原则保持不变

3. **Halo Validation Hints** - 视觉验证反馈
   - 检测到缺少空格时，Halo 显示琥珀色边框提示
   - 提示消息：`"Add space: /search <your query>"`
   - 2 秒后自动消失，或 Escape 键手动关闭

4. **Preset Routing Rules** - 预设系统命令
   - `/search` - 内置搜索（`intent_type = "builtin_search"`）
   - `/mcp` - MCP 集成（保留，未来实现）
   - `/skill` - Skills 工作流（保留，未来实现）
   - 包含在 `Config::default()` 中

## Why

### Problem

**当前痛点**：
1. **配置门槛高**：搜索功能需手动编辑 TOML，普通用户难以使用
2. **缺少验证**：无法测试 API key 是否有效，配置错误难以发现
3. **命令冲突**：前缀匹配导致 `/se` 会误匹配 `/search`
4. **UI 分散**：PII 设置在 Behavior，与搜索功能脱节

### Solution

**解决方案**：
1. **图形化配置**：点击式配置供应商，无需查阅文档
2. **实时测试**：测试连接按钮，立即反馈配置状态
3. **精准匹配**：空格验证 + token 匹配，消除命令冲突
4. **逻辑分组**：PII 设置移至 Search，功能聚合

### Impact

**用户体验提升**：
- 配置时间：从 10+ 分钟（查文档 + 编辑 TOML）→ 2 分钟（UI 点击）
- 错误率：从 ~30%（手动编辑）→ <5%（UI 验证）
- 命令冲突：从经常发生 → 完全消除

**技术债务清理**：
- 统一 Settings UI 模式（与 Memory/Provider 一致）
- 为 MCP/Skills 预留扩展点
- 改进 Router 架构，支持更复杂的路由逻辑
