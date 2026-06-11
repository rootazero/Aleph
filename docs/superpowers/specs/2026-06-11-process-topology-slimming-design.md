# 进程拓扑瘦身设计 (Process Topology Slimming)

- **日期**: 2026-06-11
- **范围**: 桌面 App 运行时进程数收敛，主攻重复 spawn 的 `AlephBridge`
- **触发**: 用户观测 macOS 上 Aleph 运行 9 个进程，尤其 3 个 `AlephBridge`，询问能否合并精简

## 1. 背景与现状

用户在运行中的 Aleph 桌面 App 上观测到 9 个进程：

| 进程 | ~MB | 性质 |
|---|---|---|
| `Aleph`（Tauri 主） | 40 | App 主进程 |
| `Aleph Graphics and Media`（WebKit GPU） | 21 | WKWebView 强制 helper |
| `Aleph Networking`（WebKit Networking） | 8 | WKWebView 强制 helper |
| `http://127.0.0.1:18790`（WebContent） | 159 | Panel 渲染进程（Leptos/WASM） |
| `tauri://localhost`（WebContent） | 14 | splash 跨源残留进程 |
| `aleph-server` | 53 | Rust 常驻 daemon（sidecar） |
| `AlephBridge` ×3 | 2.6×3 | Swift 原生桥，**意外重复** |

总占用 ~300MB。用户明确：内存不算高，主诉是**进程数偏多，尤其 3 个 bridge**。

### 1.1 窗口/WebView 拓扑（已查证）

`desktop/shell/src/main.rs` 建**单个** `main` 窗口，初始加载 `tauri://localhost/index.html`
（打包的 splash）；daemon 就绪后 `window.navigate()` 把同一窗口**跨源**切到
`http://127.0.0.1:18790`（live Panel）。跨源导航后，WebKit 为新源拉起新的 WebContent
进程（159MB），同时把旧的 `tauri://localhost` WebContent 留作 bfcache/挂起进程（14MB），
而 splash 永不被回退，于是该进程长期滞留。

### 1.2 三个 Bridge 的根因（已查证）

`SwiftBridge` 没有单例。每个需要桌面能力的子系统各自调用
`aleph_desktop_macos::MacOSPlatform::new()`，而该构造函数：

1. `Arc::new(SwiftBridge::new(helper_path))` —— 新建一个独立 Bridge 客户端；
2. 在 Tokio 上下文里 `handle.spawn(...)` 一个**构造期 warm-up handshake**，
   该 handshake 会强制 Bridge **立刻** spawn 出 Swift 子进程（而非等首个真实调用）。

aleph-server 进程内至少有 4 处 live 调用点（`grep` 已确认）：

- Presence reporter — `src/bin/aleph-server/commands/start/mod.rs:1856`
  （**且只用 `system()`，根本不碰 Bridge**）
- Mic-level reporter — `src/bin/aleph-server/commands/start/mod.rs:1898`（默认 OFF）
- Builtin tool registry — `src/executor/builtin_registry/builder/constructor.rs:168`
- Voice handler — `src/bin/aleph-server/commands/start/builder/handlers/settings.rs:818`

`settings.rs:818` 的注释甚至自述「A fresh platform handle — cheap, and mirrors how the
presence / mic-level reporters and the builtin tool registry each construct their own」——
在 Rust 层确实只是个 Arc，但每个实例都会**fork 一个真实 Swift 子进程**。3 个 live
`MacOSPlatform` 实例 = 3 个 `AlephBridge` 子进程，与观测完全吻合。

## 2. 判决：哪些可收敛，哪些锁死

**可收敛**
- **3 `AlephBridge` → 1（或空闲时 0）** —— 明确的重复 bug，本设计主目标。
- `tauri://localhost` splash 残留 WebContent —— 可尝试，靠 WebKit 配合，列为探索项。

**架构/系统红线，不动**
- `aleph-server` 不可内联进 App —— **违反 R6**（常驻核心，UI 可关、daemon 必须独立存活）。
- WebKit GPU / Networking / WebContent 多进程拆分 —— Apple 的 WKWebView 架构，Tauri 用系统
  WebView，无法合并。
- `http://127.0.0.1:18790` 的 159MB —— Panel 自身工作集，属 WASM/内存优化的**另一条线**，
  不在「进程合并」范畴，本设计不涉及。

## 3. 设计

### P1 — Bridge 进程级单例 + 去掉构造期 warm-up（必做）

**目标**：同一进程内所有 `MacOSPlatform` 共享唯一 `SwiftBridge`；Bridge 真正惰性，
只在首个真实 `desktop.*` 调用时 spawn。空闲时 0 个 Bridge 进程，用到时恰好 1 个。

**改动点（限定在 `desktop/macos/src/lib.rs`）**

1. 在 crate 内引入进程级单例：
   ```rust
   use std::sync::OnceLock;
   static SHARED_BRIDGE: OnceLock<Arc<SwiftBridge>> = OnceLock::new();

   fn shared_bridge() -> Arc<SwiftBridge> {
       SHARED_BRIDGE
           .get_or_init(|| Arc::new(SwiftBridge::new(resolve_helper_path())))
           .clone()
   }
   ```
   `resolve_helper_path()` 在进程内为常量结果（env / `~/.aleph/helpers` / exe 兄弟路径），
   单例化不会丢失路径差异。

2. `MacOSPlatform::new()` 改为 `let bridge = shared_bridge();`，
   **删除**其中 `handle.spawn(async move { ... handshake ... })` 的构造期 warm-up 块。

**惰性 spawn 的正确性（已查证）**
- `SwiftBridge` 本就 lazy：`client.rs` 的 `ensure_running` / `spawn_process` 在首个 `call`
  时按需 spawn，warm-up 只是提前触发。删掉 warm-up 后，首个真实 `desktop.*` 调用照常拉起。
- 并发安全：Bridge 用 `InflightTable` 按 `u64` id 多路复用并发请求，单例被多个子系统并发
  调用无需额外同步；长时捕获（`camera.clip` / `audio.record` / `speech.transcribe_file`）走
  `call_with_timeout`，逐调用多路复用，不会相互阻塞。
- Presence reporter 只调 `platform.system()`（`MacOSSystem`，不依赖 Bridge），删 warm-up 后
  它不再无谓拉起 Bridge 子进程。

**效果**：进程从 9 → 6~7（空闲 6 / 用到桌面能力时 7）。

**平台对等（顺手核对，非必须）**：Linux/Windows 的 `LinuxPlatform::new()` /
`WindowsPlatform::new()` 是否有同型重复，本设计仅在 macOS 侧落地；若发现同构问题，记为
后续 issue，不在本 spec 扩张范围。

### P2 — splash 残留 WebContent 回收（探索项，先验证再定夺）

**目标**：跨源 `navigate` 到 Panel 后，回收滞留的 `tauri://localhost` WebContent（14MB/1 进程）。

**性质**：WebKit 进程保留策略不完全受 Tauri 控制，投入产出比不确定（仅 14MB）。
因此本项**不直接实现**，而是先做一个时间盒 spike 验证可行性：

- 候选方向 A：跨源导航后，通过 Tauri 的 webview 句柄触发 WebKit 清理 bfcache / 挂起进程。
- 候选方向 B：splash 与 Panel 同源化（splash 也经 `tauri://` 自定义协议代理，避免跨源），
  使整个生命周期只占一个 WebContent 进程。

**验收门**：spike 若能稳定回收且不引入 splash/导航回归，则另起 spec 正式实现；否则记录结论
并放弃。本 spec 不承诺 P2 落地。

#### P2 Spike Result（2026-06-11）：**NOT-WORTH-IT**

调查（代码 + 运行态观测，未做全量重建）：

- **拓扑确认**：`desktop/shell/src/main.rs:205` 只建**单个** `main` 窗口（注释自称「the lone
  webview」），加载 `tauri://localhost/index.html`（splash），daemon 就绪后
  `navigate_to_panel`（:351）把同一 webview 跨源切到 `http://127.0.0.1:18790`。跨源后 WebKit
  为 Panel 起新 WebContent（159MB），把旧 `tauri://localhost`（14MB）留作挂起/bfcache 进程。
- **无干净 API**：栈是 Tauri **2** + wry 包装 WKWebView。shell 内无任何 bfcache / process-pool
  控制；Tauri/wry 公开面**不暴露**进程保留控制。唯一入口是 `with_webview` 取原始 `WKWebView`
  指针做 objc/私有 API 调用——脆弱、不受支持、易随系统升级失效（违反「面向未来测试」精神）。
- **候选方向 B 不可行**：splash 无法同源化到 `:18790`——splash 阶段 daemon 尚未起来（这正是
  splash 存在的理由），鸡生蛋。
- **并非真泄漏**：WebKit 挂起 WebContent 在内存压力下 jetsam 可回收；14MB 是软上限而非常驻泄漏。

**结论**：回收对象仅 ~14MB / 1 进程，且无受支持的 Tauri API，唯一手段（objc 私有调用）的脆弱性
成本远超收益。**放弃 P2**，不另起实现 spec。若未来 Tauri/wry 暴露官方 bfcache/进程控制 API，可
重新评估；在此之前进程数下限即为 P1 达成后的 6~7。

> 运行态旁证：spike 当时 GUI 窗口已关闭，仅剩 daemon（aleph-server）+ **3 个 AlephBridge**
> （pid 90126/90134/90136，各 ~6MB）持续存活——印证 P1 修复的必要性（bridge 是 daemon 子进程，
> UI 关闭也不释放）；新二进制部署后该数应降为 0（空闲）/ 1（用到桌面能力）。

## 4. 不做（Out of Scope）

- 内联 aleph-server（R6 红线）。
- 合并 WebKit GPU/Networking/WebContent（Apple 管控）。
- Panel 159MB 的 WASM/内存优化（另一条优化线）。
- Linux/Windows 平台的 Bridge 重构（仅核对，必要时另起 issue）。

## 5. 验收标准

- **功能不回归**：`desktop.*` 工具（screen/pim/ax/camera/audio/speech/permission）端到端可用。
- **进程数**：冷启动 + 不触发任何桌面能力时，`AlephBridge` 进程数 = **0**；
  触发任一桌面能力后稳定 = **1**（不再随子系统数量增长）。
- **并发**：多个子系统并发调用 Bridge 不串扰、不死锁（依赖既有 `InflightTable` 测试 +
  新增一个「两个 `MacOSPlatform` 实例共享同一子进程」的回归测试）。
- **编译**：`cargo check -p aleph-desktop-macos` 与现有 desktop 测试通过。

## 6. 风险与回退

- **风险**：删 warm-up 后首个桌面调用要承担一次性 spawn+handshake 延迟（原本被预热掩盖）。
  评估：handshake 在后台 60s 超时内完成，单次首调延迟可接受；如需可保留「首次 tool 调用前
  异步预热单例」的轻量钩子，但默认不加（YAGNI）。
- **回退**：单例与 warm-up 删除都是局部改动，`git revert` 即可恢复每实例独立 Bridge 的旧行为。
