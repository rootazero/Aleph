# Proposal: Integrate Control Plane into Server

**Change ID**: `integrate-control-plane-into-server`
**Status**: Draft
**Created**: 2026-02-09
**Author**: Architecture Team

## Problem Statement

当前 Aleph 的架构存在以下问题：

1. **架构分离不清晰**：Dashboard 作为独立的 Web 应用，与 Server 通过 WebSocket 通信，导致：
   - 大量的网络通信代码（JSON-RPC 封装、重连逻辑、心跳检测）
   - 复杂的状态同步机制（Server → Event → Client → Signal 更新）
   - 版本不一致风险（Dashboard 和 Server 可能版本不匹配）
   - 跨域安全问题（CORS 配置、Token 刷新）

2. **职责混淆**：Dashboard 既承担"配置管理"职责，又承担"对话交互"职责，导致：
   - macOS Client 中存在大量设置 UI 代码（9 个 Settings 文件）
   - Dashboard 和 Client 功能重叠
   - 用户体验不一致

3. **代码冗余**：
   - `shared_ui_logic` 中约 40% 的代码用于处理网络纠错
   - 大量的 DTO 转换逻辑（core → protocol → dashboard）
   - 复杂的构建流程（cargo build + trunk build）

## Why

This change is necessary because:

1. **Architectural Clarity**: Separating control plane (configuration management) from interaction plane (conversation UI) creates a clear separation of concerns, making the system easier to understand and maintain.

2. **Code Simplification**: Embedding ControlPlane into Server eliminates 30-40% of network communication code, removes redundant DTO conversions, and simplifies the build process.

3. **User Experience**: Users get a unified management interface (similar to macOS System Settings) accessible from any client, with real-time configuration sync and zero deployment complexity.

4. **Version Consistency**: Embedding ensures the UI and API are always in sync, eliminating version mismatch issues.

5. **Security**: Same-origin policy simplifies authentication and eliminates CORS complexity.

## Proposed Solution

将 Dashboard 重命名为 **ControlPlane**（控制平面），并将其整合到 Server 端，实现"控制与交互的绝对分离"：

- **ControlPlane**（控制平面）：资源监控、配置管理、安全审计、插件管理、知识库治理
- **Client**（交互平面）：纯粹的消息输入、结果呈现、系统集成（截屏、全局热键）

### 核心架构变更

```
┌─────────────────────────────────────────────────────────────────┐
│                         CLIENT LAYER                             │
│   macOS App │ Tauri App │ CLI │ Telegram │ Discord │ WebChat   │
│   (纯交互界面，无配置逻辑)                                         │
└───────────────────────────────┬─────────────────────────────────┘
                                │ WebSocket (JSON-RPC 2.0)
                                │ ws://127.0.0.1:18789
┌───────────────────────────────┴─────────────────────────────────┐
│                         GATEWAY LAYER                            │
│   Router │ Session Manager │ Event Bus │ Channels │ Hot Reload  │
│   + ControlPlane HTTP Server (Embedded)                          │
└───────────────────────────────┬─────────────────────────────────┘
                                │
┌───────────────────────────────┴─────────────────────────────────┐
│                      CONTROL PLANE (Embedded)                    │
│   Leptos 0.8.15 WASM UI (rust-embed)                            │
│   http://127.0.0.1:18789/cp                                      │
│   - AI Provider 配置                                              │
│   - Agent 行为设置                                                │
│   - MCP 插件管理                                                  │
│   - 知识库治理                                                    │
│   - 安全审计                                                      │
└─────────────────────────────────────────────────────────────────┘
```

### 技术实现策略

采用 **"编译前置任务（Pre-build Task）"** 模式：

1. **物理结构**：
   ```
   core/
   ├── src/
   │   ├── gateway/
   │   │   └── control_plane/    # ControlPlane HTTP 服务
   │   └── ...
   ├── ui/control_plane/          # Leptos 前端代码（原 dashboard）
   │   ├── Cargo.toml
   │   ├── src/
   │   └── dist/                  # 编译产物
   └── build.rs                   # 自动化构建脚本
   ```

2. **编译流程**：
   - `build.rs` 在 `cargo build` 时自动调用 `trunk build --release ui/control_plane`
   - 使用 `rust-embed` 将 `dist/` 嵌入到 Server 二进制文件中
   - Server 启动时通过 Axum 提供静态文件服务

3. **通信简化**：
   - 使用 Leptos Server Functions (`#[server]`)，UI 直接调用 Server 端 Rust 函数
   - 消除 `shared_ui_logic` 中的网络层代码
   - 同源策略，无需 CORS 配置

## Benefits

1. **架构简化**：
   - 消除 40% 的网络通信代码
   - 消除冗余的 DTO 转换逻辑
   - 单一构建流程，版本强一致性

2. **职责清晰**：
   - ControlPlane：唯一的配置管理入口
   - Client：纯粹的对话交互界面
   - macOS Client 可以移除所有设置 UI 代码

3. **用户体验提升**：
   - 单一二进制文件分发（aleph-server 自带 ControlPlane）
   - 配置修改实时生效，无需重启
   - 统一的管理界面，类似 macOS 系统设置

4. **安全性增强**：
   - 同源策略，简化身份认证
   - 本地访问，无需公开端口

## Trade-offs

### 优势
- 代码量减少 30-40%
- 编译速度提升（增量编译）
- 开发体验改善（ControlPlane 可独立开发）
- 部署简化（单一二进制文件）

### 劣势
- Server 内存占用增加 5-20MB（嵌入 WASM 资源）
- 跨节点管理需要额外设计（未来可通过"节点切换"解决）

## Scope

### In Scope
1. 将 `clients/dashboard` 迁移到 `core/ui/control_plane`
2. 实现 `core/build.rs` 自动化构建脚本
3. 使用 `rust-embed` 嵌入静态资源
4. 在 `core/src/gateway/control_plane/` 实现 HTTP 服务
5. 简化 `shared_ui_logic`，移除网络层代码
6. 从 macOS Client 移除设置 UI，添加 ControlPlane 跳转链接

### Out of Scope
- 多节点管理功能（未来增强）
- ControlPlane 的新功能开发（本次仅迁移现有功能）
- 其他 Client（Tauri、CLI）的改造（后续独立变更）

## Success Criteria

1. **功能完整性**：
   - ControlPlane 包含所有原 Dashboard 功能
   - macOS Client 可通过链接访问 ControlPlane
   - 配置修改实时生效

2. **性能指标**：
   - Server 启动时间 < 2s
   - ControlPlane 首屏加载 < 500ms
   - 内存占用增加 < 20MB

3. **代码质量**：
   - 移除 `shared_ui_logic` 中至少 30% 的网络代码
   - 移除 macOS Client 中所有设置 UI 文件（9 个文件）
   - 通过所有现有测试

## Implementation Plan

详见 `tasks.md`

## Related Changes

- 依赖：无
- 后续：`simplify-client-architecture`（简化 Client 架构）

## References

- [CLAUDE.md - Server-Client 架构](../../CLAUDE.md#server-client-模式)
- [Shared UI Logic Design](../../docs/plans/2026-02-08-shared-ui-logic-design.md)
- [Gateway Documentation](../../docs/GATEWAY.md)
