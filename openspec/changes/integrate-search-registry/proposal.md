# Integrate SearchRegistry into AetherCore

## Summary

将 SearchRegistry 集成到 AetherCore 作为持久化字段，并完成相关配置基础设施，为 search-settings-ui 提案的 UI 实现奠定基础。

此变更解决 `add-search-settings-ui` 提案中标识的三个核心架构缺陷。

## What

### Primary: SearchRegistry Integration

**核心变更**：
1. **AetherCore 字段**：添加 `search_registry: Arc<RwLock<Option<Arc<SearchRegistry>>>>`
2. **初始化逻辑**：从 `Config::search` 创建 SearchRegistry
3. **热重载支持**：配置更新时重建 SearchRegistry
4. **UniFFI 导出**：实现 `test_search_provider()` 异步方法

### Secondary: Configuration Improvements

**配套变更**：
1. **SearchOptions 传递**：从 config 读取并传递给 CapabilityExecutor
2. **PII 配置迁移**：添加 `PIIConfig` 到 `SearchConfig`，实现自动迁移逻辑
3. **Config 验证增强**：验证 search 配置的完整性

## Why

### Problem

**当前架构问题**：
1. **临时创建 SearchRegistry**：
   - CapabilityExecutor 每次接收 `None` 作为 search_registry 参数
   - 无法测试 search provider（需要持久化的 registry 实例）
   - 性能开销：每次请求都可能需要重新创建 providers

2. **配置不完整**：
   - SearchOptions 未从 config 读取（硬编码默认值）
   - PII 配置位于 `behavior.pii_scrubbing_enabled`，应该在 `search.pii`
   - 缺少配置迁移逻辑

3. **阻塞 UI 开发**：
   - search-settings-ui 需要 `test_search_provider()` API
   - 无法实现 provider 状态检测和配置验证
   - Swift UI 组件无法获取 provider 测试结果

### Solution

**架构改进**：
1. **持久化 SearchRegistry**：
   - 在 AetherCore 初始化时创建
   - 支持配置热重载（通过 RwLock）
   - 提供 test API 访问

2. **完整配置流程**：
   - Config → SearchRegistry（初始化）
   - Config → SearchOptions（传递给 executor）
   - Config → PIIConfig（迁移 + 验证）

3. **解锁 UI 开发**：
   - 实现 `test_search_provider()` UniFFI 方法
   - Provider 测试结果可通过 Swift 访问
   - 支持实时状态更新

### Impact

**用户体验**：
- 为 search-settings-ui 提供必要的后端支持
- Provider 配置验证（测试连接功能）
- 配置错误的即时反馈

**性能改进**：
- 避免重复创建 SearchRegistry 和 providers
- 缓存 provider 实例（Arc 共享）
- 5 分钟测试结果缓存（避免 API 滥用）

**技术债务清理**：
- 移除 `TODO: Add SearchRegistry from config`
- 移除 `TODO: Add SearchOptions from config`
- 统一 PII 配置位置（从 behavior 迁移到 search）
