# Phase 6: Windows Sandbox 高级安全增强

**Date**: 2026-04-23
**Status**: Planning
**Scope**: Windows 平台沙箱增强 — AppContainer 隔离 + WFP 网络过滤
**Parent**: [2026-04-23-sandbox-multiplatform-design.md](./2026-04-23-sandbox-multiplatform-design.md)

---

## 1. Goal

增强 Windows 平台沙箱安全性，解决 Phase 5 的已知限制：

1. **AppContainer 隔离**: 提供比 Restricted Token 更强的应用容器隔离
2. **WFP 网络过滤**: 实现 AllowHosts 策略的 OS 级强制执行
3. **文件系统命名空间隔离**: 超越 ACL 的基于路径的隔离
4. **向后兼容**: Phase 5 的 Restricted Token + Job Object 继续作为降级方案

## 2. 已知限制回顾

| 限制 | Phase 5 状态 | Phase 6 解决方案 |
|------|-------------|-----------------|
| AllowHosts 不可执行 | 警告 + 回退 | WFP 过滤驱动 |
| 文件系统基于 ACL | 功能可用 | AppContainer 命名空间 |
| 网络全有或全无 | 受限令牌控制 | WFP 条件过滤 |
| 无应用容器隔离 | 无 | AppContainer SID |

## 3. 架构设计

### 3.1 增强架构

```
WindowsSandboxDriver (Phase 6)
    ├── 策略解析 (SandboxCapabilities → WindowsPolicy)
    ├── 隔离级别选择
    │       ├── AppContainer (推荐, Win10+)
    │       └── Restricted Token (降级, 全平台)
    ├── 进程创建
    │       ├── AppContainer: CreateProcess + AppContainer SID
    │       └── Restricted: CreateProcessAsUser + Restricted Token
    ├── 网络过滤 (WFP)
    │       ├── 注册 WFP 呼出驱动 (callout driver)
    │       ├── 添加允许/拒绝过滤器
    │       └── 清理 (进程退出时)
    └── 资源限制 (Job Object)
            └── 与 Phase 5 相同
```

### 3.2 组件设计

#### AppContainer 隔离

```rust
pub struct AppContainer {
    sid: Vec<u8>,
    name: String,
    capabilities: Vec<AppContainerCapability>,
}

pub enum AppContainerCapability {
    InternetClient,
    InternetClientServer,
    PrivateNetworkClientServer,
    Custom(String),
}
```

**关键 API**:
- `CreateAppContainerProfile`: 创建 AppContainer 配置文件
- `DeriveAppContainerSidFromAppContainerName`: 派生 SID
- `CreateProcessAsUser` with AppContainer SID: 在容器中启动进程

#### WFP 网络过滤

```rust
pub struct WfpFilter {
    engine_handle: HANDLE,
    filter_ids: Vec<u64>,
}

impl WfpFilter {
    pub fn new() -> Result<Self, WfpError>;
    pub fn allow_host(&mut self, host: &str) -> Result<(), WfpError>;
    pub fn block_all(&mut self) -> Result<(), WfpError>;
    pub fn apply_to_process(&mut self, process_id: u32) -> Result<(), WfpError>;
}
```

**关键 API**:
- `FwpmEngineOpen0`: 打开 WFP 引擎
- `FwpmFilterAdd0`: 添加过滤器
- `FwpmCalloutAdd0`: 添加呼出驱动
- `FwpmSubLayerAdd0`: 添加子层

## 4. 实施计划

### Unit 1: AppContainer 基础结构
- `appcontainer.rs`: AppContainer 创建和管理
- `capabilities.rs`: AppContainer 能力映射
- 测试: AppContainer SID 创建

### Unit 2: WFP 网络过滤框架  
- `wfp.rs`: WFP 引擎封装
- `filter.rs`: 过滤器管理
- 测试: WFP 引擎打开/关闭

### Unit 3: AllowHosts 策略支持
- 将 AllowHosts 转换为 WFP 过滤器
- 进程级过滤应用
- 测试: 允许/拒绝特定主机

### Unit 4: 集成测试和验证
- AppContainer + WFP 集成测试
- 与现有 WorkspaceSandbox 集成
- 性能测试

## 5. 风险评估

| 风险 | 可能性 | 影响 | 缓解措施 |
|------|--------|------|---------|
| WFP 需要管理员权限 | 高 | 高 | 检测权限，无权限时回退到 Restricted Token |
| AppContainer 不兼容旧版 Windows | 中 | 中 | 运行时检测，Win8 以下回退 |
| WFP 过滤器冲突 | 中 | 中 | 使用专用子层，避免与其他安全软件冲突 |
| 性能影响 | 低 | 中 | 基准测试，必要时优化过滤器规则 |

## 6. 向后兼容

Phase 6 驱动将自动选择最佳可用隔离级别：

```rust
enum WindowsIsolationLevel {
    AppContainerWithWfp,    // 最佳: Win10+ with admin
    AppContainerOnly,       // 中等: Win10+ without admin  
    RestrictedToken,        // 基本: 所有 Windows 版本
}
```

选择逻辑：
1. 检测 Windows 版本 (Win10+ 支持 AppContainer)
2. 检测管理员权限 (WFP 需要)
3. 检测 WFP 引擎可用性
4. 选择最高可用级别

## 7. 文件变更

```
src/sandbox/platforms/windows/
    ├── mod.rs              # 更新: 添加 AppContainer + WFP 模块
    ├── driver.rs           # 更新: 集成 AppContainer + WFP
    ├── token.rs            # 不变: Phase 5 已实现
    ├── acl.rs              # 不变: Phase 5 已实现
    ├── job.rs              # 不变: Phase 5 已实现
    ├── appcontainer.rs     # 新增: AppContainer 隔离
    ├── wfp.rs              # 新增: WFP 网络过滤
    └── filter.rs           # 新增: 过滤器规则管理
```

## 8. Success Metrics

- [ ] AppContainer 在 Win10+ 上成功创建进程
- [ ] WFP 过滤器允许特定主机访问
- [ ] WFP 过滤器阻止未授权主机访问
- [ ] 无管理员权限时优雅回退到 Restricted Token
- [ ] 所有现有测试继续通过
- [ ] 新增 10+ 个 Windows 特定测试
