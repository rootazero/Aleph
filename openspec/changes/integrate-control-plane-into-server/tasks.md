# Tasks: Integrate Control Plane into Server

**Change ID**: `integrate-control-plane-into-server`

## Overview

本变更将 Dashboard 重命名为 ControlPlane 并整合到 Server 端，实现"控制与交互的绝对分离"。

## Task Breakdown

### Phase 1: Infrastructure Setup (基础设施搭建)

#### 1.1 Create ControlPlane Directory Structure
- [ ] 创建 `core/ui/control_plane/` 目录
- [ ] 复制 `clients/dashboard/Cargo.toml` 到 `core/ui/control_plane/`
- [ ] 更新 Cargo.toml 中的依赖路径
- [ ] 创建 `core/ui/control_plane/src/` 目录结构
- **验证**: `cargo check -p aleph-control-plane` 通过

#### 1.2 Implement Build Script
- [ ] 创建 `core/build.rs`
- [ ] 实现 Trunk 自动调用逻辑
- [ ] 添加 `rerun-if-changed` 指令
- [ ] 处理构建失败情况
- **验证**: `cargo build -p alephcore` 自动编译 ControlPlane

#### 1.3 Add rust-embed Dependency
- [ ] 在 `core/Cargo.toml` 添加 `rust-embed` 依赖
- [ ] 在 `core/Cargo.toml` 添加 `mime_guess` 依赖
- [ ] 添加 `control-plane` feature flag
- **验证**: `cargo build -p alephcore --features control-plane` 通过

#### 1.4 Implement Asset Embedding
- [ ] 创建 `core/src/gateway/control_plane/` 模块
- [ ] 实现 `assets.rs`（RustEmbed 宏）
- [ ] 实现 `server.rs`（HTTP 路由）
- [ ] 添加 MIME 类型检测
- [ ] 添加缓存头
- **验证**: 访问 `http://127.0.0.1:18789/cp` 返回 index.html

---

### Phase 2: UI Migration (UI 迁移)

#### 2.1 Copy Dashboard Source Code
- [ ] 复制 `clients/dashboard/src/` 到 `core/ui/control_plane/src/`
- [ ] 复制 `clients/dashboard/index.html`
- [ ] 复制 `clients/dashboard/styles/`
- [ ] 复制 `clients/dashboard/tailwind.config.js`
- [ ] 复制 `clients/dashboard/Trunk.toml`
- **验证**: `trunk build` 在 `core/ui/control_plane/` 成功

#### 2.2 Update Import Paths
- [ ] 更新 `shared-ui-logic` 的导入路径
- [ ] 更新 `aleph-protocol` 的导入路径
- [ ] 移除不必要的网络层导入
- **验证**: `cargo check -p aleph-control-plane` 无错误

#### 2.3 Convert RPC Calls to Server Functions
- [ ] 创建 `core/ui/control_plane/src/api/` 模块
- [ ] 实现 `config.rs`（配置管理 Server Functions）
- [ ] 实现 `providers.rs`（AI Provider Server Functions）
- [ ] 实现 `plugins.rs`（MCP 插件 Server Functions）
- [ ] 实现 `memory.rs`（知识库 Server Functions）
- [ ] 实现 `security.rs`（安全设置 Server Functions）
- **验证**: 每个 Server Function 可以编译

#### 2.4 Update UI Components
- [ ] 更新 `HomePage` 使用 Server Functions
- [ ] 更新 `ProvidersPage` 使用 Server Functions
- [ ] 更新 `PluginsPage` 使用 Server Functions
- [ ] 更新 `MemoryPage` 使用 Server Functions
- [ ] 更新 `SecurityPage` 使用 Server Functions
- **验证**: UI 组件可以编译

#### 2.5 Remove Network Layer from shared_ui_logic
- [ ] 识别 `shared_ui_logic` 中的网络层代码
- [ ] 移除 `connection/reconnect.rs`
- [ ] 移除 `protocol/rpc.rs` 中的 WebSocket 逻辑
- [ ] 保留核心类型定义
- [ ] 更新 feature flags
- **验证**: `cargo check -p shared-ui-logic` 通过，代码量减少 30%+

---

### Phase 3: Gateway Integration (网关集成)

#### 3.1 Integrate ControlPlane Router
- [ ] 在 `core/src/gateway/mod.rs` 导入 `control_plane` 模块
- [ ] 在 `Gateway::start()` 中添加 ControlPlane 路由
- [ ] 配置路由优先级（WebSocket > ControlPlane > 404）
- **验证**: Server 启动后可以访问 `/cp`

#### 3.2 Implement Server Function Context
- [ ] 创建 `ControlPlaneContext` 结构体
- [ ] 提供 `ConfigManager` 访问
- [ ] 提供 `MemorySystem` 访问
- [ ] 提供 `PluginRegistry` 访问
- [ ] 在 Leptos 中注册 Context
- **验证**: Server Functions 可以访问 Core 组件

#### 3.3 Add Authentication Middleware
- [ ] 实现 `auth_middleware`
- [ ] 本地访问自动信任
- [ ] 远程访问检查 Token
- [ ] 添加到 ControlPlane 路由
- **验证**: 本地访问无需认证，远程访问需要 Token

#### 3.4 Configure CORS (Optional)
- [ ] 评估是否需要 CORS（同源策略）
- [ ] 如果需要，添加 `tower-http` CORS 中间件
- **验证**: 浏览器控制台无 CORS 错误

---

### Phase 4: Client Simplification (客户端简化)

#### 4.1 Remove Settings UI from macOS Client
- [ ] 删除 `clients/macos/Aleph/Sources/BehaviorSettingsView.swift`
- [ ] 删除 `clients/macos/Aleph/Sources/GuestsSettingsView.swift`
- [ ] 删除 `clients/macos/Aleph/Sources/McpSettingsView.swift`
- [ ] 删除 `clients/macos/Aleph/Sources/PluginsSettingsView.swift`
- [ ] 删除 `clients/macos/Aleph/Sources/PoliciesSettingsView.swift`
- [ ] 删除 `clients/macos/Aleph/Sources/SearchSettingsView.swift`
- [ ] 删除 `clients/macos/Aleph/Sources/SecuritySettingsView.swift`
- [ ] 删除 `clients/macos/Aleph/Sources/SkillsSettingsView.swift`
- [ ] 保留 `SettingsView.swift`（仅作为跳转入口）
- **验证**: macOS Client 可以编译

#### 4.2 Add ControlPlane Link Button
- [ ] 创建 `ControlPlaneLinkButton.swift`
- [ ] 实现 `openControlPlane()` 方法（NSWorkspace.shared.open）
- [ ] 在菜单栏添加"打开控制面板"选项
- [ ] 在 `SettingsView` 添加跳转按钮
- **验证**: 点击按钮打开浏览器并访问 ControlPlane

#### 4.3 Update Configuration Sync Logic
- [ ] 移除 macOS Client 的配置写入逻辑
- [ ] 保留配置读取逻辑（用于显示当前状态）
- [ ] 监听 `config_updated` 事件
- [ ] 实时更新 UI 状态
- **验证**: ControlPlane 修改配置后，macOS Client 实时感知

#### 4.4 Simplify Conversation Window
- [ ] 移除对话窗口中的设置按钮
- [ ] 简化 UI 布局
- [ ] 优化性能
- **验证**: 对话窗口更简洁，性能提升

---

### Phase 5: Testing & Validation (测试与验证)

#### 5.1 Unit Tests
- [ ] 测试 `ControlPlaneAssets::get()`
- [ ] 测试 `serve_static()` 路由
- [ ] 测试 Server Functions
- [ ] 测试 `auth_middleware`
- **验证**: `cargo test -p alephcore --features control-plane` 全部通过

#### 5.2 Integration Tests
- [ ] 测试 Server 启动流程
- [ ] 测试 ControlPlane 首屏加载
- [ ] 测试配置修改流程
- [ ] 测试实时同步
- **验证**: 端到端测试通过

#### 5.3 Performance Benchmarks
- [ ] 测量 Server 启动时间
- [ ] 测量 ControlPlane 首屏加载时间
- [ ] 测量内存占用
- [ ] 测量 Server Function 调用延迟
- **验证**:
  - Server 启动 < 2s
  - 首屏加载 < 500ms
  - 内存增加 < 20MB
  - Server Function 延迟 < 10ms

#### 5.4 Manual Testing
- [ ] 测试所有 ControlPlane 页面
- [ ] 测试配置修改并保存
- [ ] 测试 macOS Client 跳转
- [ ] 测试实时同步
- [ ] 测试错误处理
- **验证**: 所有功能正常工作

---

### Phase 6: Documentation & Cleanup (文档与清理)

#### 6.1 Update Documentation
- [ ] 更新 `CLAUDE.md`（架构图）
- [ ] 更新 `docs/ARCHITECTURE.md`
- [ ] 更新 `docs/GATEWAY.md`
- [ ] 创建 `docs/CONTROL_PLANE.md`
- [ ] 更新 `README.md`
- **验证**: 文档准确反映新架构

#### 6.2 Update Build Instructions
- [ ] 更新 `CLAUDE.md` 中的构建命令
- [ ] 添加 ControlPlane 开发指南
- [ ] 添加故障排查指南
- **验证**: 新开发者可以按照文档构建

#### 6.3 Cleanup Old Code
- [ ] 归档 `clients/dashboard/` 到 `archive/dashboard/`
- [ ] 移除 `shared_ui_logic` 中的废弃代码
- [ ] 移除未使用的依赖
- [ ] 更新 `.gitignore`
- **验证**: `cargo clean && cargo build` 成功

#### 6.4 Update CI/CD
- [ ] 更新 GitHub Actions 构建脚本
- [ ] 添加 ControlPlane 构建步骤
- [ ] 更新发布流程
- **验证**: CI/CD 流水线通过

---

## Task Dependencies

```
Phase 1 (Infrastructure)
  ↓
Phase 2 (UI Migration)
  ↓
Phase 3 (Gateway Integration)
  ↓
Phase 4 (Client Simplification)
  ↓
Phase 5 (Testing)
  ↓
Phase 6 (Documentation)
```

## Parallelizable Tasks

以下任务可以并行执行：

- **Phase 2.3** (Server Functions) 和 **Phase 2.4** (UI Components) 可以并行
- **Phase 4.1** (Remove Settings) 和 **Phase 4.2** (Add Link) 可以并行
- **Phase 5.1** (Unit Tests) 和 **Phase 5.2** (Integration Tests) 可以并行

## Rollback Plan

如果任何阶段失败，可以回滚：

1. **Phase 1-2 失败**：删除 `core/ui/control_plane/`，恢复 `clients/dashboard/`
2. **Phase 3 失败**：移除 Gateway 中的 ControlPlane 路由
3. **Phase 4 失败**：恢复 macOS Client 的设置 UI 文件
4. **Phase 5 失败**：使用 feature flag 禁用 ControlPlane

## Estimated Effort

- **Phase 1**: 4 hours
- **Phase 2**: 8 hours
- **Phase 3**: 4 hours
- **Phase 4**: 6 hours
- **Phase 5**: 6 hours
- **Phase 6**: 4 hours

**Total**: ~32 hours (4 工作日)

## Success Metrics

- [ ] 代码量减少 30%+
- [ ] Server 启动时间 < 2s
- [ ] ControlPlane 首屏加载 < 500ms
- [ ] 内存占用增加 < 20MB
- [ ] 所有测试通过
- [ ] 文档完整更新
