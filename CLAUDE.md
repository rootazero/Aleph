# CLAUDE.md

## 🛑 架构红线 (Architectural Redlines)

以下为最高优先级约束，所有开发决策必须遵守。违反红线的代码不得合入。

### R1. 大脑与四肢绝对分离 (Brain-Limb Separation)

- **禁令**: 严禁在 `src` 中直接调用特定平台系统 API (AppKit, Vision, CoreGraphics, windows-rs)
- **原则**: 核心层只定义"能力契约 (Trait)"，物理实现由原生 Bridge (Swift / 其他) 通过 IPC 提供

### R2. UI 逻辑唯一源 (Single Source of UI Truth)

- **禁令**: 严禁在原生 Bridge 中实现具有业务逻辑的设置页面、表单或列表
- **原则**: 所有复杂业务 UI 在 Leptos (WASM) Panel 中实现，原生 Bridge 仅负责系统 API 调用与桥接

### R3. 核心轻量化 (Core Minimalism)

- **禁令**: 严禁为单一非核心功能在 core 中引入沉重的第三方库
- **原则**: 优先实现为 Skill (Python/Bash) 或 MCP Server。内核只调度，不搬砖
- **备注**: 代码层面的奥卡姆剃刀原则和 Rust 大文件拆分规范与此不冲突

### R4. Interface 层禁止业务逻辑 (I/O-Only Interfaces)

- **禁令**: 禁止在 Channel/Bot/CLI/Panel 中处理数据持久化、记忆检索或任务规划逻辑
- **原则**: Interface 层是"纯 I/O"— 输入转为 JSON-RPC 发给 Server，响应渲染给用户

### R5. AI 主动到达 (AI Comes to You)

- **原则**: 减少用户切换上下文的成本，AI 通过用户已有的工作通道主动提供帮助
- **实现**: 多端推送（Telegram / Slack / Email / 桌面通知 / Panel 浮窗等）、内联建议、订阅式 Daemon 触发
- **边界**: 不打扰用户 (不抢焦点、不弹模态对话框)，但不要因此拒绝必要的交互入口

### R6. 一核多端 (One Core, Many Channels)

- **形态**: Aleph 是常驻后台服务（aleph-server），UI 不是必需品；所有终端用户体验都由 Channel/Panel 通过 JSON-RPC 与 Core 对话产生
- **原则**: Rust Core 是唯一大脑，多端通道（CLI / Bot / WebChat Panel / 原生 Bridge）只负责 I/O 与渲染，不参与业务推理
- **备注**: 这已在 R1、R2、R4 中体现，此条作为产品层面的重申

### R7. LLM 主权原则 (LLM Sovereignty)

- **禁令**: 严禁用确定性代码替代 LLM 擅长的推理判断（意图识别、任务评估、路由决策、内容分类等）
- **原则**: 极简系统 + 强大 prompt = 释放 LLM 全部推理能力。把复杂留给模型，把简单留给系统
- **判断标准**: 对每个模块问 — "这是在赋能 LLM（给它做不到的能力），还是在越俎代庖（替它做推理）？"
- **赋能层（保留）**: Gateway 多端触达、Memory 持久记忆、Daemon 事件感知、Soul 人格、Provider 多厂商、Tool 执行能力、MCP 外部服务、Extension 插件生态、上下文压缩、安全硬过滤
- **越俎代庖（禁止）**: Intent Detection 规则引擎、POE 目标验证管线、多层 Tool Filter、Context Aggregation 多层合并、Dispatcher 意图分析

### R8. 工具即一切 (Everything is a Tool)

- **原则**: Aleph 自身的所有可配置操作都应暴露为工具，让 LLM 通过自然语言对话完成配置
- **实现**: Agent 管理（创建/切换/删除）、Provider 配置、Channel 配置、Skill/MCP 安装卸载、Daemon 订阅规则 — 全部是 Tool
- **核心循环**: `用户自然语言 → LLM 理解意图 → LLM 选择工具 → 工具执行 → 结果返回 LLM → LLM 回复用户`
- **效果**: 对话即管理面板。用户无需学习配置文件或 API，自然语言驱动一切

### R9. 智慧在 Prompt 中 (Intelligence Lives in the Prompt)

- **原则**: 被移除的中间件的"智慧"不是丢弃，而是迁移到 system prompt 模板中
- **实现**: 主循环的 LLM 一次调用自然覆盖所有判断（意图理解 + 工具选择 + 安全评估 + 完成度判断）
- **效果**: 零额外 LLM 调用，零中间件税，模型推理能力完整释放

### R10. 薄 Harness 哲学，笨循环编排核心 (Thin Harness, Dumb Loop)

> *"If you're not the model, you're the harness."* — Vivek Trivedy
> *"Models get stronger → harness gets thinner."* — Anthropic

- **薄 Harness 哲学 (Thin Harness)**: Aleph 采纳 Anthropic 流派，运行时极简、信任模型。Harness 是脚手架不是认知层。**模型越强，Harness 越薄** — 优秀的 Harness 必须通过"面向未来测试 (Future-Proof Test)"：换更强的模型，性能自然提升，无需修改 Harness 代码
- **笨循环 (Dumb Loop)**: `src/harness/` 仅承载 Think→Act 轮次调度，**不参与任何推理**。所有智能决策（意图理解、工具选择、安全评估、完成度判断）由 LLM 一次推理调用自然完成
- **核心边界**: `src/harness/` 控制在 **12 文件 / ~4900 行** 以内：
  - 顶层 (8)：`mod.rs` / `agent.rs` / `deps.rs` / `trait_def.rs` / `callback.rs` / `chain_context.rs` / `trace.rs` / `trace_sink.rs`
  - `agent/` 子目录 (4)：`think.rs` / `act.rs` / `guardrails.rs` / `prompt.rs`（Task 8/9/10 把 `agent.rs` 按 Think/Act/Guardrails/Prompt 拆分为四个子职责）
- **行数增长红线**：任何新增 LOC 必须先回答"加代码前必答 3 问"（脚手架 vs 认知 / 模型升级后是否还需要 / 是否有真实消费者）。新增文件需在 PR 描述里说明为何无法装进现有 12 个文件之一
- **循环里的 5 个"不"**:
  1. ❌ 不判断意图分类
  2. ❌ 不做工具过滤 / 相关性评分
  3. ❌ 不做完成度判断（除模型显式 stop）
  4. ❌ 不做内容审查 / 安全打分
  5. ❌ 不做错误恢复策略选择
- **12 模块各归其所**: 行业共识的 12 大 Harness 模块（Tools/Memory/Context/Prompt/State/Error/Guardrails/Verification/Subagents/Init...）每一个都有独立物理位置，**不在 `src/harness/` 内堆积**
- **YAGNI 撤回模式**: 任何"零现有消费者"的抽象立即删除/撤回，绝不"为未来留口"。dissolution 期间累计删除 ~5,200 行死代码
- **加代码前必答 3 问**:
  1. 这是脚手架还是认知？认知必须搬到 prompt
  2. 模型升级一档还需要它吗？不需要就删
  3. 现在有几个真实消费者？零个就撤回
- **关联**: 是 R3 (核心轻量化) + R7 (LLM 主权) + R9 (智慧在 Prompt) 在 Agent Harness 工程上的具体落地。详见 [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md)

---

## 🧬 设计原则 (Design Principles)

以下原则指导 Aleph 每一行代码的编写决策，是架构红线之下的工程纪律。

### P1. 低耦合 (Low Coupling)

- **模块间通过 Trait 通信，不依赖具体实现** — 模块只知道对方的契约，不知道对方的内部结构
- **禁止跨层直接调用** — Core 不直接调用 UI，UI 不直接操作数据库，Interface 不处理业务逻辑
- **依赖方向单向流动** — `Interface → Core → Domain`，禁止反向依赖
- **事件驱动解耦** — 模块间优先通过事件/消息传递状态变化，而非直接方法调用

### P2. 高内聚 (High Cohesion)

- **单一职责** — 每个模块/struct/函数只做一件事，做好一件事
- **相关逻辑物理聚合** — 紧密相关的类型、函数、trait 放在同一模块目录下，不要分散到不同子系统
- **命名即文档** — 模块名、函数名、类型名应准确反映其唯一职责，无需注释解释"它是干什么的"
- **大文件及时拆分** — 单文件超过 500 行应考虑按职责拆分为子模块 (参见 [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md))

### P3. 可扩展性 (Extensibility)

- **开放-封闭原则 (OCP)** — 对扩展开放，对修改封闭。新增功能通过实现 trait / 注册插件完成，不修改已有核心逻辑
- **策略模式优于条件分支** — 用 trait object / enum dispatch 替代 `if-else` 链或 `match` 的无限膨胀
- **插件化优先** — 非核心功能优先实现为 Skill / MCP Server / WASM 插件，而非硬编码进 Core
- **Schema 驱动** — 接口使用 JSON Schema (schemars) 自描述，新增字段不破坏旧客户端

### P4. 依赖倒置 (Dependency Inversion)

- **高层模块不依赖低层模块，两者都依赖抽象** — Core 定义 trait，具体实现在 crate 边界之外
- **实践**: `DesktopCapability` trait 在 core 中定义，native 实现在 `desktop/shared/`；`MemoryStore` trait 在 core 中定义，SQLite+sqlite-vec 实现在 `src/memory/` 但可替换
- **构造时注入** — 通过 `AppContext` / Builder 模式在启动时组装依赖，运行时不 `new` 具体类型

### P5. 最小知识原则 (Least Knowledge / Law of Demeter)

- **只与直接协作者通信** — `a.b().c().d()` 链式调用是设计缺陷的信号
- **封装内部结构** — 不暴露 struct 内部字段的引用链，提供有意义的方法代替
- **接口最小化** — pub API 只暴露调用者真正需要的，`pub(crate)` 优于 `pub`

### P6. 简洁性 (Simplicity — KISS & YAGNI)

- **奥卡姆剃刀** — 如无必要，勿增实体。不为假想的未来需求预留抽象
- **三次法则** — 代码重复不超过两处时不要提前抽象，第三次出现再提取
- **删除优于注释** — 废弃代码直接删除，不要注释掉保留。Git 是时光机
- **扁平优于嵌套** — 优先使用 early return / `?` 操作符，减少缩进层级

### P7. 防御性设计 (Defensive Design)

- **系统边界校验** — 在用户输入、外部 API 响应、IPC 消息的入口处严格校验，内部传递信任已校验的数据
- **优雅降级** — 外部依赖 (LLM/网络/文件系统) 失败时提供 fallback，不 panic
- **锁安全** — `.lock().unwrap_or_else(|e| e.into_inner())`，永远处理 poison
- **UTF-8 安全** — 字符串切片使用 `char_indices()` / `.get(..n)`，不用 `&s[..n]`

### P8. LLM 优先 (LLM-First)

- **语义理解交给 LLM** — 所有需要将自然语言转换为结构化意图的任务（意图识别、参数提取、命令路由），优先使用 LLM 语义理解，而非正则表达式或关键词匹配
- **禁止脆弱的模式匹配** — 不要用 regex 解析用户自然语言输入。正则只适用于格式固定的机器生成文本（如 JSON、URL、日志格式）
- **LLM 可做则 LLM 做** — 如果一项任务 LLM 能完成（分类、提取、推理、生成），就交给 LLM，而非硬编码规则
- **结构化输出** — LLM 返回 JSON 结构化结果，代码层只负责解析和执行，不做语义判断

---

## 🔧 开发指南

### 构建命令

| Command | Description |
|---------|-------------|
| `cargo run --bin aleph-server` | Start server (debug) |
| `cargo check -p alephcore` | Quick compile check |
| `cargo test -p alephcore --lib` | Run core tests |
| `just dev` | Dev server (rebuilds WASM first) |
| `just build` | Release build (WASM + server) |
| `just shell-dev` | Run the desktop App in dev mode |
| `just shell-build` | Build 完整桌面 App installers (.dmg/.msi/.deb，内置 aleph-server) |
| `just shell-build-lite` | Build Aleph Panel 纯壳 App installers (无 server，连局域网内 server) |
| `just test-all` | All tests (core + desktop + proptest) |
| `just clippy` | Lint |
| `just verify-build` | CI 验证三产物（完整 App / Panel 纯壳 App / 独立 server）三平台能否正常构建（build-only，不打 tag、不发布） |
| `just release YY.M.D` | **发版**: 更新 VERSION + 提交推送 + 触发 GitHub workflow（需先写 changelog） |

### Rust 工具链

- **MSRV = 1.95**（由 `sysinfo 0.39` 决定），在 `Cargo.toml` 的 `[workspace.package]` 与 `[package]` 两处 `rust-version` 声明。
- 仓库根的 `rust-toolchain.toml` 钉住具体 stable（当前 `1.96.0`），本地与 CI 自动使用同一工具链——无需 `rustup default` 或 `cargo +<ver>`。抬高 MSRV 时同步更新这两处。

> **分发形态**: Aleph 发布三类产物（同一 tag 一并出）：**完整桌面 App**（macOS `.dmg` / Windows `.msi` / Linux `.deb`，通过 Tauri `externalBin` 内置 `aleph-server`，单机零配置，首次启动自动拉起并接管旧 daemon）、**Aleph Panel 纯壳 App**（`embedded-core` feature 关闭，不带 server，连接局域网内任一 `aleph-server`）、**独立 `aleph-server` 二进制**（类 Unix 用 `scripts/install.sh` 经 `curl | bash`；Windows 用 `scripts/install.ps1` 经 `irm … | iex` 装到 `%LOCALAPPDATA%\Aleph\aleph-server.exe`；用于服务器 / NAS 部署）。
>
> **信任模型 = 网络边界**：server 默认只绑 `127.0.0.1`（只信本机）；在 `~/.aleph/config.toml` 写一行 `[gateway] host = "0.0.0.0"` 即显式开放整个局域网——**局域网内任何设备由此可连接 agent**（对话 + 读 + 执行工具/PTY/shell，仍随网络信任边界开放）。**唯一的方法级门槛是 device tier**：远程 Panel 默认 **Chat tier**，对 Aleph **自身配置**的变更（`self_config`/`skill_install`/provider 配置/`devices.*` 等 config 类 RPC 与工具）走 `src/gateway/method_authz.rs` 的 RPC+工具双门拦截，须 operator 经 `devices.set_level` 显式提权；本机(loopback) 始终 operator（单机零配置）。另一道协议护栏是 WS Origin 校验（`src/gateway/origin_policy.rs`，挡公网恶意网页**跨源**驱动 agent；经典 DNS-rebinding 已防御——same-origin 仅在 `Host` 为 IP 字面量/loopback 时放行，域名 `Host` 不再自动同源放行，域名部署须把 origin 加进 `[gateway] allowed_origins`，详见 SECURITY.md。`[gateway] allow_any_origin = true` 可关）。详见 [SECURITY.md#auth-ux](docs/reference/SECURITY.md#auth-ux)。

> **⚠️ Panel ↔ Daemon 资源嵌入链**: Panel UI（`interfaces/webchat/dist/*`）通过 `rust_embed`（`src/gateway/control_plane/assets.rs`）在 **`aleph-server` 编译时** 静态嵌入到二进制；运行中的 daemon **不会** 从磁盘读取 dist/*。改完 panel 源码后看不到效果，几乎都是漏了重编 binary 这一步。完整刷新链：
>
> 1. `just wasm` — 重建 `interfaces/webchat/dist/{aleph_panel.js, aleph_panel_bg.wasm, tailwind.css, index.html}`
> 2. `cargo build --release -p alephcore --bin aleph-server` — 让 `rust_embed` 把新 dist 烧进 binary
> 3. 替换正在跑的 binary，让 supervisor relaunch：
>    - **dev** (`cargo run` 启动的 daemon)：`./target/release/aleph-server stop` → `cargo run --release -p alephcore --bin aleph-server start`（Windows 同理，二进制为 `aleph-server.exe`）
>    - **.app daemon (macOS)**：`mv /Applications/Aleph.app/Contents/MacOS/aleph-server{,.bak}` → `cp target/release/aleph-server /Applications/Aleph.app/Contents/MacOS/` → `kill <pid>`（Tauri shell 的 supervisor 会自动 relaunch 新 binary 并 reload webview）
>    - **安装版 App daemon (Windows)**：先停（`aleph-server stop` 或 `taskkill /IM aleph-server.exe /F`）——**Windows 无法覆盖正在运行的 exe，必须先停**——再 `Copy-Item target\release\aleph-server.exe "$env:LOCALAPPDATA\Aleph\aleph-server.exe" -Force`（NSIS 默认按用户安装；旧 `aleph-server.exe` 与 `Aleph.exe` 同目录），最后重启 `Aleph.exe`，supervisor 会拉起新 binary 并 reload webview
>
> 单跑 `just wasm` / `just dev` **不够**——前者只更磁盘，后者只在 daemon 还没启动时有效。已经在跑的 daemon 必须替换 binary 才能感知 panel 改动。

### Windows 构建

> macOS 的 `just shell-build` / `just shell-dev` 在 Windows 上**同样适用**——justfile 已把 Swift bridge 等 macOS 专属步骤用 `$OSTYPE == darwin*` 守卫，并自动为产物追加 `.exe`。无需独立的 Windows 配方。产物是 NSIS `.exe` + `.msi` 安装包（`tauri.conf.json` 的 `bundle.targets = "all"`），通过 `externalBin` 内置 `aleph-server.exe`，单机零配置。

**一次性前置**（CI runner 自带；本机首次构建需手动装。justfile 配方经 Git Bash 执行——`set shell := ["bash", …]`——故需 Git for Windows）：

| 依赖 | 安装 | 说明 |
|------|------|------|
| MSVC C++ 生成工具 | Visual Studio「使用 C++ 的桌面开发」工作负载 | 提供 `x86_64-pc-windows-msvc` 链接器 |
| WebView2 Runtime | Windows 11 自带 | Tauri webview 宿主 |
| `protoc` | `scoop install protobuf`（或 `choco install protoc`） | 编译 `aleph-server` 的 protobuf 构建依赖（CI 同样安装） |
| wasm 目标 | `rustup target add wasm32-unknown-unknown` | 编译 Panel WASM |
| `wasm-bindgen-cli` | `cargo install wasm-bindgen-cli --version 0.2.122 --locked` | **版本必须与 `Cargo.lock` 里的 `wasm-bindgen` 完全一致**，否则 bindgen 拒绝生成 |
| `cargo-tauri` | `cargo install tauri-cli --version "^2.11" --locked` | 提供 `cargo tauri build/dev`；版本随 Cargo.lock 中 `tauri` 主版本 |
| `just` | `scoop install just`（或 `cargo install just`） | 跑 justfile 配方 |
| Git for Windows `usr\bin` 在 PATH | 把 `…\git\current\usr\bin` 加进 PATH | justfile 的 shebang 配方（`wasm` 等）需要 `cygpath`，否则 just 报 `could not find cygpath`；scoop 默认不把它加进 PATH |
| `wasm-opt`（可选） | `scoop install binaryen` | 缺失时 `just wasm` 跳过 WASM 压缩，不影响功能 |

**全量构建 + 启动**（PowerShell）：

```powershell
just shell-build                          # WASM → release server → cargo tauri build
# 安装包产物：target\release\bundle\nsis\Aleph_<ver>_x64-setup.exe
#             target\release\bundle\msi\Aleph_<ver>_x64_en-US.msi
.\target\release\aleph-desktop-shell.exe  # 直接运行已打包的壳（同目录已带 aleph-server.exe），免装安装包即可验证
```

dev 模式：`just shell-dev`（debug 编译 + staging daemon，热跑，不出安装包）。

### 版本管理

- **CalVer (日历版本)** — 格式 `YY.M.D`（两位年，月/日不补零，如 `26.5.21`），每天最多发布一个版本。该格式同时是合法 semver 并满足 Windows MSI 版本约束，可直接作为桌面 App 的 bundle 版本
- **VERSION 文件是唯一版本源** — `build.rs` 读取 VERSION → 注入 `ALEPH_VERSION` 环境变量 → 所有代码通过 `env!("ALEPH_VERSION")` 使用
- **禁止** 在代码中硬编码版本号，使用 `env!("ALEPH_VERSION")` 代替 `env!("CARGO_PKG_VERSION")`
- Panel System Info、Gateway 版本、MCP/ACP 协议版本、CLI --version 全部从 VERSION 文件读取
- GitHub workflow 也读取 VERSION 文件作为 release tag

### 发版流程 (Release Process)

`just release YY.M.D` 触发三产物×三平台构建发布（发版前先写 CHANGELOG.md）。完整两步流程、`just verify-build` 预检、CI fail-fast 轮询（`scripts/poll_release_run.py`）详见 [RELEASE.md](docs/reference/RELEASE.md)。

### Feature Flags

所有生产功能始终编译，无需 feature flags。仅保留测试用 features：`loom` (并发测试)、`test-helpers` (集成测试工具)。

### 提交规范

English commit messages. Format: `<scope>: <description>` — Example: `gateway: add WebSocket server foundation`

### 分支策略

**单分支开发模式**：所有开发工作直接在 main 分支进行。

### 语言规范

- Reply in Chinese
- Code comments in English
- Documentation in both

### Git Worktree 注意事项

`EnterWorktree` 会在每次 Bash 命令后强制重置 CWD 到 worktree 目录，即使 `cd` 切回主仓库也无效。因此在同一会话内执行 `git worktree remove` 会导致 Shell 永久损坏。**正确做法**：在 `EnterWorktree` 会话内只合并不删除，用新会话清理 worktree；或不用 `EnterWorktree`，手动用绝对路径管理。

### 进程管理 (Process Management)

Singleton 强制由 OS 级 `flock` 保证（Spec C, 2026-05-02 起改为结构化保护）：

- `aleph-server start` 在 `main()` 进入任何 DB/vault 操作之前先获取
  `~/.aleph/data/aleph.lock`。第二个 `start` 会立即以 exit 64 退出，
  并在 stderr 打印持锁进程的 PID。
- 所有 CLI 写子命令（`secret`、`hooks`、`plugins` 等）通过
  `with_policy` 分发：服务在跑时，写操作通过 `/v1/admin/*` IPC 转发；
  服务不在时，CLI 自己拿锁本地写入。两条路径都不会与服务竞争。
- OS 在进程退出（正常、panic、SIGKILL）时自动释放 `flock`。`kill -9 <pid>`
  之后**无需 sleep**，可立即 `aleph-server start`。
- 反向回归脚本 `scripts/spec_c_regression.sh` 锁住四条不变量：
  SQLite 走 `open_sqlite_safe`、vault/acp 走 `vault_io`/`atomic_io`、
  每个 CLI 子命令显式声明 policy、`acquire_instance_lock` 不再有遗留 caller。

如果看到 `Stale lock file detected (PID X not running)`，可以安全
`rm ~/.aleph/data/aleph.lock`（理论上不会出现，因为 flock 是 OS 管理的；
该诊断仅作防御性提示）。

### Trust model

LAN-trust：没有认证步骤，信任边界就是网络边界。默认只绑 `127.0.0.1`，
`[gateway] host = "0.0.0.0"` 显式开放局域网；WS Origin 校验是唯一保留的
协议护栏。完整模型见
[docs/reference/SECURITY.md#auth-ux](docs/reference/SECURITY.md#auth-ux)。

---

## 📚 文档索引

| 文档 | 链接 |
|------|------|
| ARCHITECTURE.md | [docs/reference/ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) |
| **PRODUCT_TOPOLOGY.md** | [docs/reference/PRODUCT_TOPOLOGY.md](docs/reference/PRODUCT_TOPOLOGY.md) — 产品形态：一套源码(panel/core/shell)→三产物(完整App/纯壳Panel/独立core)排列组合 + 参考部署拓扑(家庭服务器+瘦客户端) |
| **HARNESS_PHILOSOPHY.md** | [docs/reference/HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md) — 薄 Harness 哲学 + 笨循环编排核心（R11 详解） |
| AGENT_SYSTEM.md | [docs/reference/AGENT_SYSTEM.md](docs/reference/AGENT_SYSTEM.md) |
| AGENT_LOOP_CONTEXT_BUDGET.md | [docs/reference/AGENT_LOOP_CONTEXT_BUDGET.md](docs/reference/AGENT_LOOP_CONTEXT_BUDGET.md) |
| AGENT_LOOP_TOOL_EXECUTION.md | [docs/reference/AGENT_LOOP_TOOL_EXECUTION.md](docs/reference/AGENT_LOOP_TOOL_EXECUTION.md) |
| AGENT_LOOP_RECOVERY.md | [docs/reference/AGENT_LOOP_RECOVERY.md](docs/reference/AGENT_LOOP_RECOVERY.md) |
| MULTI_AGENT_SYSTEM.md | [docs/reference/MULTI_AGENT_SYSTEM.md](docs/reference/MULTI_AGENT_SYSTEM.md) |
| GATEWAY.md | [docs/reference/GATEWAY.md](docs/reference/GATEWAY.md) |
| CLUSTER.md | [docs/reference/CLUSTER.md](docs/reference/CLUSTER.md) — Aleph 集群（单中心非对称节点联邦）：reverse RPC + `node_invoke`/`node_file` + 命令 allowlist + 审批回中心 + 断线 fail-fast |
| TOOL_SYSTEM.md | [docs/reference/TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) |
| MEMORY_SYSTEM.md | [docs/reference/MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) |
| └─ RAW_MEMORY.md | [docs/reference/memory/RAW_MEMORY.md](docs/reference/memory/RAW_MEMORY.md) |
| └─ NOTES.md | [docs/reference/memory/NOTES.md](docs/reference/memory/NOTES.md) |
| └─ RETRIEVAL.md | [docs/reference/memory/RETRIEVAL.md](docs/reference/memory/RETRIEVAL.md) |
| └─ DREAM_DAEMON.md | [docs/reference/memory/DREAM_DAEMON.md](docs/reference/memory/DREAM_DAEMON.md) |
| EXTENSION_SYSTEM.md | [docs/reference/EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) |
| PLUGIN_SYSTEM.md | [docs/reference/PLUGIN_SYSTEM.md](docs/reference/PLUGIN_SYSTEM.md) |
| WORKFLOW_INTEROP.md | [docs/reference/WORKFLOW_INTEROP.md](docs/reference/WORKFLOW_INTEROP.md) |
| SECURITY.md | [docs/reference/SECURITY.md](docs/reference/SECURITY.md) |
| DESIGN_PATTERNS.md | [docs/reference/DESIGN_PATTERNS.md](docs/reference/DESIGN_PATTERNS.md) |
| CODE_ORGANIZATION.md | [docs/reference/CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md) |
| DOMAIN_MODELING.md | [docs/reference/DOMAIN_MODELING.md](docs/reference/DOMAIN_MODELING.md) |
| AGENT_DESIGN_PHILOSOPHY.md | [docs/reference/AGENT_DESIGN_PHILOSOPHY.md](docs/reference/AGENT_DESIGN_PHILOSOPHY.md) |
| SERVER_DEVELOPMENT.md | [docs/reference/SERVER_DEVELOPMENT.md](docs/reference/SERVER_DEVELOPMENT.md) |
| SANDBOX.md | [docs/reference/SANDBOX.md](docs/reference/SANDBOX.md) |
| SESSION_SERVICE.md | [docs/reference/SESSION_SERVICE.md](docs/reference/SESSION_SERVICE.md) |
| MODEL_PERCEIVABLE_ECOSYSTEM.md | [docs/reference/MODEL_PERCEIVABLE_ECOSYSTEM.md](docs/reference/MODEL_PERCEIVABLE_ECOSYSTEM.md) |
| SKILL_TRIGGER_ENHANCEMENT.md | [docs/reference/SKILL_TRIGGER_ENHANCEMENT.md](docs/reference/SKILL_TRIGGER_ENHANCEMENT.md) |
| WHATSAPP_ARCHITECTURE_DESIGN.md | [docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md](docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md) |
| DESKTOP_BRIDGE.md | [docs/reference/DESKTOP_BRIDGE.md](docs/reference/DESKTOP_BRIDGE.md) |
| DESKTOP_SHELL.md | [docs/reference/DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md) |
| WINDOWS_RUNTIME.md | [docs/reference/WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md) — Windows 运行时部署/运维：install.ps1 安装、前台运行、`--daemon` 限制与替代、单例锁、stop/status 注意事项、LAN 信任、刷新二进制链 |
| GOOGLE_MEET_BRIDGE.md | [docs/reference/GOOGLE_MEET_BRIDGE.md](docs/reference/GOOGLE_MEET_BRIDGE.md) — `google_meet` 薄工具契约 + 外部 transport bridge JSON-RPC 协议 |

---

## 🏢 官方仓库 (Official Repositories)

| 仓库 | 路径 | 说明 |
|------|------|------|
| Aleph (主项目) | `/Users/zouguojun/Workspace/Aleph` | Rust Core + 多端架构 |
| Aleph-docs | `/Users/zouguojun/Workspace/Aleph-docs` | 官方文档 |
| Aleph-homepage | `/Users/zouguojun/Workspace/Aleph-homepage` | 官方首页 |
| Aleph-mcp | `/Users/zouguojun/Workspace/Aleph-mcp` | 官方 MCP 项目 |
| Aleph-plugins | `/Users/zouguojun/Workspace/Aleph-plugins` | 官方插件 |
| Aleph-skills | `/Users/zouguojun/Workspace/Aleph-skills` | 官方技能 |

---

## 📝 Session Context

- **项目**: 自托管个人 AI 助手，Rust Core + 多端架构
- **核心循环**: Think → Act（极简两步循环，LLM 主权原则）
- **语言**: 使用中文对话

### Memory Prompt

When token is low to 10%, summarize this session to generate a "memory prompt" for next session inheritance.
