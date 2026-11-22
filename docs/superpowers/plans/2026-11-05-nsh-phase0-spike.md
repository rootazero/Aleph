# NSH Desktop 迁移 — Phase 0 Compatibility Spike 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 验证 NSH v0.4.0 能在 Aleph 工具链下构建，并从 NSH Rust app 成功连接 `aleph-server` 显示其版本号——这是整个 NSH 迁移的 go/no-go 关口。

**Architecture:** 在 Aleph workspace 外创建独立 spike crate `desktop/nsh-spike`（不加入 `workspace.members`），以 path 依赖引用本地 NSH clone，以 path 依赖引用 `shared/client` 和 `shared/protocol`。Spike 只验证可行性，不生产代码。

**Tech Stack:** Rust 1.99 + edition 2021、NSH v0.4.0（`/Volumes/TBU4/Github/nsh`）、`nsh-platform-macos`、`shared/client`（tokio 1.53）、`shared/protocol`

**Spec:** `docs/superpowers/specs/2026-11-05-nsh-desktop-refactor-design.md` §四 Phase 0

## Global Constraints

- NSH 依赖必须使用 path 依赖指向 `/Volumes/TBU4/Github/nsh`，不使用 crates.io 版本
- Spike crate **不加入** Aleph workspace 的 `workspace.members`，避免污染主构建
- 不修改 `aleph-server`、`shared/client`、`shared/protocol` 任何文件
- 不使用 `nsh-engine`（QuickJS）——本 spike 只用 `AppSource::Rust(Root)`
- 每个 Task 结束时记录实际发现（API 签名、版本冲突、运行时行为），写入 `desktop/nsh-spike/FINDINGS.md`
- Spike 完成后将 `FINDINGS.md` 的结论更新到设计文档 §四 和 §七 风险表

## Review Focus

1. **tokio runtime 线程边界**：NSH AppKit 事件循环在主线程，tokio `multi_thread` 必须在独立线程启动；如果 `shared/client` 的 `GatewayClient` 要求在 tokio context 中构建，跨线程传递需要 `Arc` + channel——Task 3 的 `test_gateway_connects` 验证。
2. **objc2 全局状态冲突**：NSH 和 Aleph `desktop/macos` 都依赖 objc2，path 依赖场景下 Cargo 可能选择不同的 feature 集导致链接冲突——Task 1 的编译步骤会暴露。
3. **`AppSource::Rust(Root)` 的 Root 类型**：`Root` 是 `nsh_engine::Root`（实为 `Box<dyn Fn() -> AnyElement>`），需要确认 Rust 2021 edition 下能跨 crate 使用——Task 2 的最小窗口测试。
4. **`shared/client` 的 TLS 依赖**：`desktop/shell` 用 `native-tls`；spike 作为独立 crate 引用时需要确认 feature 标志能正确传递，不会因为 feature 未激活导致 TLS 握手失败——Task 3 验证。
5. **NSH edition 2024 与 Aleph edition 2021 混用**：Cargo path 依赖允许不同 edition，但 `proc-macro` crate（`nsh-macros`）的 edition 会影响宏展开——Task 2 的 `#[component]` 宏使用时确认。

---

### Task 1: Spike crate 骨架 + NSH 编译验证

**Files:**
- Create: `desktop/nsh-spike/Cargo.toml`
- Create: `desktop/nsh-spike/src/main.rs`
- Create: `desktop/nsh-spike/FINDINGS.md`

**Interfaces:**
- Produces: 可编译的 NSH macOS app 骨架（无业务逻辑），供 Task 2 扩展

- [ ] **Step 1: 创建 `desktop/nsh-spike/Cargo.toml`**

```toml
[package]
name = "aleph-nsh-spike"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "aleph-nsh-spike"
path = "src/main.rs"

[dependencies]
nsh-core = { path = "/Volumes/TBU4/Github/nsh/crates/nsh-core" }
nsh-base = { path = "/Volumes/TBU4/Github/nsh/crates/nsh-base", default-features = false }
nsh-engine = { path = "/Volumes/TBU4/Github/nsh/crates/nsh-engine", default-features = false }

[target.'cfg(target_os = "macos")'.dependencies]
nsh-platform-macos = { path = "/Volumes/TBU4/Github/nsh/crates/nsh-platform-macos", default-features = false }

log = "0.4"
```

- [ ] **Step 2: 创建最小 `desktop/nsh-spike/src/main.rs`**

```rust
fn main() -> std::process::ExitCode {
    #[cfg(target_os = "macos")]
    {
        use nsh_platform_macos::{AppOptions, AppSource, Root, run_app};
        use nsh_core::prelude::*;

        #[component]
        fn App() -> impl IntoElement {
            text("NSH Spike: compiles OK")
        }

        let root: Root = Box::new(|| App().into_element());
        run_app(AppSource::Rust(root), AppOptions::default())
    }
    #[cfg(not(target_os = "macos"))]
    std::process::ExitCode::SUCCESS
}
```

- [ ] **Step 3: 创建 `desktop/nsh-spike/FINDINGS.md`**

初始内容：
```markdown
# NSH Spike Findings

## Task 1: 编译验证
- [ ] 状态：
- 实际 NSH commit：（填入 `git -C /Volumes/TBU4/Github/nsh rev-parse HEAD`）
- 发现的问题：
```

- [ ] **Step 4: 尝试编译**

```bash
cd desktop/nsh-spike && cargo build 2>&1 | head -60
```

预期：编译成功或报出具体错误（记录到 FINDINGS.md）

- [ ] **Step 5: 记录编译结果到 FINDINGS.md**

如果编译失败，记录错误类型：
- `E0463`（找不到 crate）→ path 依赖路径错误
- feature 冲突 → 记录具体 feature 名
- objc2 链接错误 → 记录具体符号

- [ ] **Step 6: Commit（仅 spike 文件，不影响主 workspace）**

```bash
git add desktop/nsh-spike/
git commit -m "chore(spike): add nsh-spike crate skeleton for Phase 0 compatibility verification"
```

---

### Task 2: 最小 NSH 窗口运行验证

**前提**：Task 1 编译通过

**Files:**
- Modify: `desktop/nsh-spike/src/main.rs`
- Modify: `desktop/nsh-spike/FINDINGS.md`

**Interfaces:**
- Consumes: Task 1 的可编译骨架
- Produces: 能打开空白窗口的 NSH app，确认 `AppSource::Rust(Root)`、`#[component]` 宏、`AppOptions` 运行时行为正确

- [ ] **Step 1: 扩展 `main.rs`，加入 AppOptions 配置**

在 `AppOptions` 中填入：
```rust
AppOptions {
    title: "Aleph NSH Spike".to_string(),
    size: (800, 600),
    app_id: "com.aleph.nsh-spike".to_string(),
    ..AppOptions::default()
}
```

- [ ] **Step 2: 编译并运行**

```bash
cd desktop/nsh-spike && cargo run 2>&1
```

预期：弹出 800×600 窗口，显示 "NSH Spike: compiles OK"

- [ ] **Step 3: 记录运行结果到 FINDINGS.md**

确认以下事项（逐条填写）：
- `AppSource::Rust(Root)` 签名与设计文档 §4.3 描述一致？
- `#[component]` 宏在 edition 2021 的 crate 中能展开？
- `AppOptions::default()` 可用，字段与文档一致？
- 窗口正常出现，无 objc2 runtime crash？

- [ ] **Step 4: Commit**

```bash
git add desktop/nsh-spike/
git commit -m "chore(spike): verify NSH window opens with AppSource::Rust(Root)"
```

---

### Task 3: tokio 集成 + Gateway 连接验证

**前提**：Task 2 窗口运行正常

**Files:**
- Modify: `desktop/nsh-spike/Cargo.toml`（添加依赖）
- Modify: `desktop/nsh-spike/src/main.rs`
- Modify: `desktop/nsh-spike/FINDINGS.md`

**Interfaces:**
- Consumes: Task 2 的运行窗口；`shared/client::GatewayClient`（`connect` 方法）；`shared/protocol`（握手类型）
- Produces: 窗口内显示从本地 `aleph-server` 取到的版本字符串，证明 NSH 主线程 + tokio 后台线程 + Gateway IPC 三者可以协同工作

- [ ] **Step 1: 更新 `Cargo.toml`，添加 `shared/client`、`shared/protocol`、tokio**

```toml
[dependencies]
# ... 已有依赖 ...
aleph-client = { path = "../../shared/client" }
aleph-protocol = { path = "../../shared/protocol" }
tokio = { version = "1.53", features = ["rt-multi-thread", "sync", "time", "macros"] }
```

- [ ] **Step 2: 实现 tokio 后台线程 + channel 模式**

在 `main.rs` 中，在 `run_app` 之前启动 tokio runtime：

```rust
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

// 在独立线程上跑 tokio
let server_version: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
let version_clone = server_version.clone();

std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async move {
        // 连接本地 aleph-server（默认地址）
        // 取 server version 后写入 version_clone
        // 使用 aleph_client::GatewayClient 的实际 API（根据 shared/client 签名填写）
        let _ = version_clone; // placeholder — 根据实际 GatewayClient API 实现
    });
});
```

在 `App` 组件中读取 `server_version` 显示。

- [ ] **Step 3: 查阅 `shared/client` 实际连接 API**

```bash
head -60 /Volumes/TBU4/Workspace/Aleph/shared/client/src/gateway_client.rs
```

将实际的 `connect` / `hello` 签名记录到 FINDINGS.md，用来完成 Step 2 的 placeholder。

- [ ] **Step 4: 编译**

```bash
cd desktop/nsh-spike && cargo build 2>&1 | head -60
```

预期：编译成功。如出现 feature 冲突或 TLS 错误，记录到 FINDINGS.md。

- [ ] **Step 5: 运行，确认窗口显示版本**

```bash
cd desktop/nsh-spike && cargo run 2>&1
```

预期：窗口显示 `aleph-server v{version}` 或连接错误信息（连接失败也是有效信息，证明链路通了）。

- [ ] **Step 6: 完整填写 FINDINGS.md**

必须覆盖：
- tokio `1.53` 与 NSH `"1"` 在同一进程内是否冲突
- `shared/client` 的 TLS feature 在独立 crate 中能否正常激活
- NSH 主线程 + tokio 后台线程的 channel 通信模式是否工作
- `GatewayClient` 的实际连接 API（与设计文档假设的差异）
- 任何需要更新到设计文档的发现

- [ ] **Step 7: 更新设计文档风险表**

将 FINDINGS.md 的结论同步到：
`docs/superpowers/specs/2026-11-05-nsh-desktop-refactor-design.md` §四 §七

- [ ] **Step 8: Commit**

```bash
git add desktop/nsh-spike/ docs/superpowers/specs/2026-11-05-nsh-desktop-refactor-design.md
git commit -m "chore(spike): verify tokio + Gateway connection from NSH app — Phase 0 complete"
```

---

## Spike 完成标准（go/no-go）

**Go（继续 Phase 1）**：
- Task 1 编译通过
- Task 2 窗口正常打开
- Task 3 编译通过 + 连接到 `aleph-server`（或明确知道失败原因且可修复）

**No-go（开新会话讨论）**：
- `AppSource::Rust(Root)` 类型签名与文档描述根本不符
- tokio runtime 与 NSH 主线程存在无法解决的冲突
- objc2 链接冲突无法在不修改 NSH 的情况下解决

---

## 什么我没做

- 没有验证 Linux/Windows 平台（macOS 优先）
- 没有测试 `register_component()` NativeComponent 注册（Phase 2 任务）
- 没有测试事件订阅和 Session RPC（Phase 1 任务）
- 没有添加 `desktop/nsh-spike` 到 CI（spike 是一次性验证，不进入持续构建）
