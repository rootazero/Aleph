# PRODUCT_TOPOLOGY.md — 产品形态与分发拓扑

> 一套源码，三种打包。本文澄清「panel / core / shell」三个维护单元如何排列组合成
> 三个发布产物，以及每个产物对应的真实部署场景。与 [ARCHITECTURE.md](./ARCHITECTURE.md)、
> [GATEWAY.md](./GATEWAY.md)、[SECURITY.md](./SECURITY.md) 互补。

---

## 1. 核心原则：panel 永远是 core 的乘客

panel（Leptos/WASM UI）编译出 `interfaces/webchat/dist/*` 后，**通过 `rust_embed`
在编译 `aleph-server` 时静态嵌入到 core 二进制里**（压缩存储，见
`src/gateway/control_plane/assets.rs`）。运行中的 daemon 不读磁盘 dist，而是吐出二进制内嵌的那份，
经 HTTP 服务在 `:18790` 提供。

> **没有任何产物把 panel 作为独立资产打包。要看到 panel，背后必然有某个 core 在 serve 它。**
> 所谓「纯 panel」产物自身不含 panel 字节——它是一个连到远程 core 去拉 panel 的瘦原生外壳。

**推论**：改 panel 源码后，必须**重编 core**（让 rust_embed 重新嵌入）。只跑 `just wasm`
只更新磁盘 dist，跑着的 daemon 感知不到。验证嵌入是否生效只能
`curl http://<host>:18790/aleph_panel_bg.wasm | shasum` 对比磁盘 dist 的 SHA（`strings` 看不到压缩资产）。

---

## 2. 维护单元（源码层，3 个）

| 单元 | 路径 | 角色 |
|---|---|---|
| **Panel** | `interfaces/webchat/`（Leptos/WASM） | 全部业务 UI。编译进 core，不单独分发 |
| **Core** | `alephcore` → `aleph-server` | 大脑（Think→Act 循环）+ HTTP/WS Gateway + `rust_embed` 内置 panel + Memory/Tool/Provider… |
| **Shell** | `desktop/shell/`（Tauri） | 薄原生外壳：开窗口、`navigate` 到某个 core 的 `:18790`、托盘/菜单/通知/deep-link。R1/R2：纯 I/O，**不含业务逻辑** |

> Shell 靠一个 feature flag（`embedded-core`）切两种形态，本身不重复维护业务逻辑。
> 「一套 panel + 一套 core」是核心；shell 是薄薄的第三件。

---

## 3. 三产物 = 三件源码的排列组合

| 产物 | GUI 外壳 | 本地 core | panel 来自哪 | feature / 构建 | bundle id | 场景 |
|---|:---:|:---:|---|---|---|---|
| **完整桌面 App**（panel+core） | ✓ | ✓ 内置（Tauri externalBin） | 内置的那个 core | `embedded-core` **ON**，`just shell-build` | `ai.aleph.desktop` | 单机零配置桌面 |
| **Panel 纯壳 App**（纯 panel） | ✓ | ✗（`externalBin: []`） | **远程** core（局域网内任一 `aleph-server`） | `embedded-core` **OFF**，`just shell-build-lite`（`--no-default-features`） | `ai.aleph.panel` | 瘦客户端连远程脑 |
| **独立 core 二进制**（纯 core） | ✗ | ✓ 自身 | 自身（浏览器或纯壳来连） | `just build` + `scripts/install.sh`（`curl\|bash`） | — | 服务器 / NAS headless |

三者同一个 git tag 一并发布（workflow `aleph-app-release.yml`，三产物 × 三平台）。

---

## 4. 两个正交开关 → 决策树

三产物拆成两个独立问题即一目了然：

1. **要不要 GUI 外壳？** 要 → App；不要 → 独立 core 二进制。
2. **core 在不在本地？**（仅当有外壳时）在 → 完整 App（shell 把 core 当 externalBin 内置并 supervise）；不在 → 纯壳 App（navigate 到远程 core）。

```
                     有 GUI 外壳?
                    ┌─────┴─────┐
                   是           否
               core 本地?     独立 core 二进制（纯 core）
             ┌────┴────┐      └ aleph-server + 内置 panel，headless
            是         否
        完整 App     纯壳 App（纯 panel）
    shell+内置core   shell only → 连远程 core
```

---

## 5. 参考部署拓扑

### A. 单机桌面（完整 App）

一台 Mac 既是脑也是界面。装完整桌面 App，首次启动自动拉起内置 `aleph-server`（loopback），
GUI webview 连 `127.0.0.1:18790`。零配置。

### B. ★ 家庭服务器 + 瘦客户端（纯壳 Panel 的设计初衷）

> 这是「纯 panel」产物存在的根本理由，作为参考拓扑固化于此。

```
   ┌─────────────────────────┐         局域网          ┌──────────────────────────┐
   │  Mac mini（家庭服务器）   │  ws/http :18790        │  主力笔记本                 │
   │  aleph-server (纯 core)  │◄───────────────────────│  Panel 纯壳 App            │
   │  + 内置 panel            │   Remote target         │  (ai.aleph.panel, 无 core) │
   │  host = "0.0.0.0"        │                         │  navigate→ mac-mini:18790  │
   │  脑/LLM/Memory/DB 全在此  │                         │  只跑薄 webview 外壳        │
   └─────────────────────────┘                         └──────────────────────────┘
```

- **Mac mini**：常驻 Aleph 服务。脑、LLM 调用、Memory(SQLite+向量)、Tool 执行、Daemon 全在此。
  在 `~/.aleph/config.toml` 写 `[gateway] host = "0.0.0.0"` 开放局域网。
  - **推荐用纯 core（独立 `aleph-server`，`scripts/install.sh`）**：headless，无 GUI 浪费，最省。
  - 也可装**完整 App**（若你偶尔坐到 Mac mini 前也想要本机 GUI），但 GUI 外壳在服务器上通常用不到。
- **笔记本**：只装 **Panel 纯壳 App**。它不含 core、不跑 LLM/DB——只是个原生 webview 外壳，
  把界面指向 Mac mini 的 `:18790`。**这正是「无需装完整版、避免资源浪费」的诉求落地**：
  重活全在 Mac mini，笔记本只渲染。
  - 连接方式：纯壳 App 的连接目标 `ConnectionTarget`（`desktop/shell/src/connection.rs`）默认 Local，
    首启探测到本机无 core → 落到 connect 页，填入 Mac mini 的 Remote URL（默认 scheme `http`、端口 18790），
    持久化到 `~/.aleph/.desktop-shell-target`；之后启动直连。导航是 probe-gated（目标 Gateway 应答才 navigate，否则停在 connect 页，不白屏）。
  - **更轻的替代**：笔记本上直接用浏览器开 `http://<mac-mini-ip>:18790`，连 App 都不用装。
    纯壳 App 相对浏览器的增量价值 = 原生托盘/菜单/系统通知/deep-link/窗口管理。
  - **完整 App 也能指向远程**（2026-08-24 起）：菜单/托盘的 "Connect to Remote…" 在两种产物下都在，
    而完整 App 此前**每次启动都把持久化的 Remote 重置回 Local** —— 连得上、当次能用、下次启动静默忘记。
    现在 target 跨重启保留：指向 Remote 时它按 `reroute_for_target` 同一条规则先探针再导航，
    并**不启动内置 daemon**（"the remote daemon is not ours to manage"，与监督器 Remote 臂一致）；
    点 "Back to Local" 才把本机 core 拉起来。⚠️ 推论：指向远程期间本机没有 core，
    依赖它的 `aleph` CLI / 本地 channel 也就不在。守卫见
    `main.rs::{boot_honours_a_persisted_remote_target, boot_treats_a_remote_target_exactly_as_a_runtime_switch_does}`。

### C. 服务器 / NAS headless（纯 core）

Linux NAS 或云主机 `curl … install.sh | bash` 装独立 `aleph-server`，常驻服务。
任意设备用浏览器或纯壳 App 接入。与 B 同形，只是脑不在 Mac 上。

---

## 6. 信任模型 = 网络边界（务必理解）

- core 默认只绑 `127.0.0.1`（只信本机）。拓扑 B/C 必须显式 `[gateway] host = "0.0.0.0"` 才能被局域网连接。
- **一旦开放，局域网内任何设备都获得对 agent 的完全控制权**（含 PTY/shell 执行，无方法级门槛）——
  信任边界就是网络边界，没有认证步骤。唯一保留的协议护栏是 WS Origin 校验
  （`src/gateway/origin_policy.rs`，挡公网恶意网页跨源驱动；域名部署须把 origin 加进 `[gateway] allowed_origins`）。
- 详见 [SECURITY.md#auth-ux](./SECURITY.md)。把 Aleph 服务放在可信局域网内，勿直接裸奔公网。

---

## 7. 构建 / 部署影响

| 改了什么 | 需要重出的产物 |
|---|---|
| **panel 源码**（`interfaces/webchat/`） | 完整 App、独立 core（凡内嵌 panel 者）。**纯壳 App 不用重出**——它不含 panel，连上新 core 自然就新 |
| **core 源码**（`alephcore`） | 完整 App、独立 core。纯壳 App 不用 |
| **shell 源码**（`desktop/shell/`） | 完整 App、纯壳 App。独立 core 不用 |

本地部署完整 App 的标准链：`just wasm` → `cargo build --release -p alephcore --bin aleph-server`
（让 rust_embed 重嵌）→ 替换 `/Applications/Aleph.app/Contents/MacOS/aleph-server` → 重启 daemon。
或直接 `just shell-build` 出完整 .app 再替换。详见 CLAUDE.md「⚠️ Panel ↔ Daemon 资源嵌入链」。

---

## 8. WKWebView 渲染注记

完整 App 与纯壳 App 都用 webview 显示 panel，二者拉的是**同一 core 的同一份字节**（无代码分离）。
但渲染引擎是 **WKWebView(WebKit)**，与 Chrome(Blink) 存在差异：例如父级 `overflow:hidden + border-radius`
裁剪不住被提升为合成层的子元素（子带 `mix-blend-mode`、或元素自身带 `filter` 动画），需用 `mask` 兜底裁剪。
验证 WebKit 行为请用 **Safari**（同引擎），Chrome 不复现。浏览器直连场景则取决于该浏览器引擎。

---

## 构建命令速查

| 命令 | 产物 |
|---|---|
| `just build` | 独立 `aleph-server`（纯 core，内置 panel） |
| `just shell-build` | 完整桌面 App（.dmg/.msi/.deb，内置 core） |
| `just shell-build-lite` | Panel 纯壳 App（无 core，连远程） |
| `just shell-dev` / `just shell-dev-lite` | 上述两者的 dev 模式 |
| `just verify-build` | CI build-only 验证三产物 × 三平台 |
