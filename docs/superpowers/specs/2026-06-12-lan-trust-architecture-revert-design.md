# LAN-Trust 架构回退：去认证 + 纯壳 Panel + 三产物发行

**日期**: 2026-06-12
**状态**: 已批准（brainstorming 全流程，用户逐节确认）
**范围**: 删除设备认证/配对/token 全套（~13k 行）；desktop/shell 重写为单 crate 双变体；发行矩阵扩为三产物

---

## 1. 背景与动机

当前"壳核分离"策略（panel + alephcore 同装、panel 可远程连接其他 core、设备配对 + token 信任传递）在个人部署场景下没有匹配的使用价值，反而带来：

- 运行线程/进程增加（bootstrap 握手、配对审批、设备管理）
- 权限管理复杂（~12.8k 行 auth 代码：token/pairing/devices/brute-force/guest/invitation/policy engine）
- 资源占用与心智负担增加

**回退目标**：Aleph 回归"自托管服务"模型（Home Assistant 式）——`aleph-server` 是产品本体，自己服务 Web UI；所有客户端（浏览器 / 桌面壳 / CLI）都只是指向 `http://<host>:18790` 的瘦客户端。

**信任模型**：从"设备配对 + token"简化为**网络边界即信任边界**：

- 绑 `127.0.0.1`（默认）= 只信本机
- 绑 `0.0.0.0`（一行配置 `[gateway] host = "0.0.0.0"`）= 信整个局域网，知情选择

已明确接受的风险：LAN 绑定下，局域网内任何设备对 agent 拥有完全控制权（含 PTY/shell 执行）。这是个人家庭部署的知情决策；默认 Loopback 保证不会"出厂即裸奔"。

## 2. 决策记录（用户已确认）

| 决策点 | 选择 |
|--------|------|
| App 形态 | 纯壳 + 完整版双变体；server 同时独立发行 |
| Auth 范围 | 全删，默认 Loopback，LAN 显式开启 |
| 工具权限层 | **保留**（ScopedToolService 三层 merge、Deny/Ask/confirm 门，管"agent 能干什么"，与"谁能连入"正交） |
| 壳的能力 | webview + 桌面集成（通知/托盘/全局快捷键/自更新/外链） |
| 项目位置 | monorepo 内重写 desktop/shell（复用 Tauri 脚手架/签名/发版 workflow） |
| 执行方案 | 单壳代码库双变体，一次发版到位，提交序列分层可二分 |
| Origin 校验 | **保留简化版**（唯一例外，见 §4.2） |

## 3. 架构总览

三个发行产物，同一 release、同一 CalVer 版本号：

| 产物 | 形态 | 目标用户 |
|------|------|---------|
| Aleph 完整版 | 桌面 App（内嵌 server，dmg/msi/deb） | 单机用户，零配置开箱即用 |
| Aleph Panel 纯壳版 | 桌面 App（无 server，dmg/msi/deb） | 已有局域网 server 的用户 |
| aleph-server | 裸二进制 ×3 平台 + `install.sh`（curl 安装） | 服务器/NAS 部署 |

**唯一访问路径**：`webview/浏览器 → http://<host>:18790 → panel（rust_embed，永远由 server 服务）→ 同源 WS → JSON-RPC`。

关键不变量：**纯壳不打包 WASM**——UI 永远从 server 加载，server 升级 UI 自动跟上，无版本错配。

## 4. Server 侧变更

### 4.1 删除清单（约 13,000 行，含 ~1k 行 auth 测试）

- `src/gateway/handlers/auth/` 整目录：connect 的 challenge/token/pairing 分支、bootstrap、devices、tier。`connect` 作为协议方法保留但简化为"无凭据，直接返回 hello/会话信息"（精简后的 handler 移出 auth/ 目录，落点由实现计划定，倾向并入 hello_snapshot 一侧）
- `src/gateway/security/` 删**纯 auth 模块**：token、pairing、device、brute_force、guest_session_manager、invitation_manager、policy_engine、identity_map、activity_log、activity_logger
  - ⚠️ `crypto.rs` 被 `src/secrets/vault.rs`、飞书 webhook、WhatsApp vault_store 使用，必须保留
  - **T5 审查修正（2026-06-12，方案 B）**：~~shared_token、token_readonly~~ **不删**。`shared_token.rs` 是生产密钥保险库本体（SecretVault 宿主，54 个消费者：providers/OAuth/channel 密钥/vault_store 工具/语音），`store/` 是保险库主密钥持久层 + cluster 设备记录（32 个消费者），`token_readonly.rs` 是 admin IPC bearer 查找（§4.2 "admin_api 不动"）——三者属 vault 链非设备认证，原列入系把"token=认证凭据"与"token=vault 主密钥"混淆。**Vault 抽离到 `src/secrets/` 是刻意推迟的未来工作，不属本次回退**。级联：`gateway/session.rs`（HTTP cookie 会话，T4 后零消费者）、`handlers/guests.rs`、`wizard/flows/pairing.rs` 一并删除
- `src/gateway/` 根：`auth_middleware.rs`（486）、`bootstrap.rs`（160）、`challenge.rs`（334）、`device_store.rs`（334）、`method_authz.rs`（361）、`trusted_proxy.rs`（178）、`auth_probe_tests.rs`（1043）
  - ~~`pairing_store.rs`（472）~~ **T3 审查修正（2026-06-12）：不删**。该文件是 channel 发送者配对 store（`channel.pairing.*`，inbound router 消费，属 §4.2 保留范围），与设备认证配对（`security/` 内，删）是两套东西；原列入系混淆。`src/gateway/handlers/pairing.rs` 同理保留
  - ~~`pair_loop_guard.rs`~~ **T4 审查修正（2026-06-12）：不删**。channel 适配器 bot↔bot 回复风暴防护（出生提交 `0a8e40389`），被 inbound_router + channel_policy（§4.2 保留）消费，与 HTTP/设备 auth 零关系；同属 channel vs device 混淆
- `caller_identity` 简化：所有请求隐式为 owner（仅 3 个文件涉及）
- CLI：`aleph auth *`、`aleph-server bootstrap-url`、配对/设备子命令全删；`aleph open` 保留但去掉 nonce（纯粹开浏览器到 server URL）
- Panel 路由：`/pair` 页面、`/auth/bootstrap`、`?token=` 处理全部消失

明确后果：`method_authz` 删除后，**PTY（终端）等全部 JSON-RPC 方法对连入者开放**。这是 LAN 信任模型的直接推论，不另设方法级门槛。

### 4.2 保留与简化

- `bind_mode.rs` + `[gateway] host`/`port` 配置不动；默认 `127.0.0.1`
- **`origin_policy.rs` 简化保留**（唯一的"验证"例外）。理由：浏览器对 WebSocket 不实施同源策略，零校验意味着用户浏览器里打开的任何公网恶意网页都可以 JS 直连 `ws://<lan-ip>:18790` 驱动 agent（DNS rebinding 同理）。这道护栏挡的是互联网，不是局域网邻居。默认策略：
  1. 放行无 Origin 头的请求（壳 webview、curl、原生客户端、CLI）
  2. 放行 Origin host 为 localhost（含 `127.0.0.0/8`、`::1`、`localhost` 字面量）或私网 IP 字面量（`10.x` / `192.168.x` / `172.16-31.x`）的请求
  3. 其余拒绝（公网域名 Origin 全部命中此条——攻击页 Origin 是域名不是私网 IP 字面量）
  4. `[gateway] allow_any_origin = true` 逃生口（默认 false）

  > **实现注（计划阶段核实）**：现有 `origin_policy.rs` 的"无 Origin / loopback / `tauri:` / 同源 / 显式 allowlist"规则已等价覆盖上述 1-3 条的安全目标（合法 LAN 浏览器访问天然同源，公网域名 Origin 与 Host 不符被拒，DNS rebinding 同理）——无需新增私网 IP 字面量判断，唯一改动是新增第 4 条逃生口。
- 工具权限层（ScopedToolService、channel_policy）、admin_api（CLI IPC）、rate_limiter、mdns_broadcaster（纯壳自动发现要用）、tailscale.rs、session/channel/execution 全部不动

### 4.3 Panel 侧（interfaces/webchat）

- 删：token 注入/登录残留、NotificationCenter 中的配对审批 UI、设备管理页、guest/邀请 UI、WS connect 的 challenge/token 状态机
- WS 连接简化为：connect（无凭据）→ hello → 正常会话
- 其余（对话/设置/外观，~62k 行）不动

## 5. 桌面壳设计（desktop/shell 原地重写）

单 crate，cargo feature **`embedded-core`** 区分双变体，共享 Tauri 脚手架/签名/图标。

### 5.1 共同部分（~1k 行量级）

- webview 加载目标 origin；`ConnectionTarget`（connection.rs 的 Local/Remote 解析）沿用
- 系统通知：notify.rs 改为订阅所配 origin 的 WS 事件流（R5"AI 主动到达"的桌面通道）
- 托盘驻留、全局快捷键呼出、自更新（update.rs）、外链跳系统浏览器、webview 麦克风权限（语音输入）

### 5.2 完整版（feature `embedded-core` 开）

- 打包 `externalBin` aleph-server
- 精简版 daemon 监督（daemon.rs 瘦身）：拉起 `aleph-server start`、版本接管、退出不杀守护
- 默认 target = Local（127.0.0.1:18790），可切远程
- bundle id 沿用现有 → 老用户自更新无缝接上

### 5.3 纯壳版（feature 关）

- 无 externalBin、无 daemon 代码
- 首次启动显示连接设置页（原生小窗）：手填 `IP[:端口]` + mDNS 自动发现的 server 列表点选（server 侧广播已有 mdns_broadcaster；壳侧 mDNS 浏览为少量新增代码）
- bundle id 用新的（如 `com.aleph.panel`），可与完整版并存安装

### 5.4 删除

- bootstrap-url/nonce 握手、配对深链（deeplink 中 pairing 部分）——两个变体都删
- perm_monitor（本机 daemon 桌面权限监控）：纯壳版删除（无本机 daemon 可监控）；完整版保留现状

原生 bridge（Swift 等）不受影响——它们是 aleph-server 的子进程，跟着 server 所在机器走，与壳无关。

## 6. 发行与 Workflow

`aleph-app-release.yml` 矩阵扩展，单 tag 出全部产物：

- 完整版 App ×3（dmg/msi/deb，同现状）
- 纯壳 App ×3（同平台，无 externalBin，体积极小）
- `aleph-server` 裸二进制：macOS arm64、Linux x64、Windows x64（CI 本来就为 externalBin 构建，只是多一步上传）
- `install.sh`：探测 OS/arch → 下载对应二进制 → 装入 `/usr/local/bin`（无权限则 `~/.local/bin`）→ 提示 `aleph-server start`。server 本就自守护（flock 单例），脚本只管下载。Windows 服务器用户手动下 exe，不提供 PowerShell 脚本
- `just verify-build` 同步扩矩阵；CHANGELOG/CalVer 流程不变

## 7. 配置与迁移

- 新知识点只有一个：`[gateway] host = "0.0.0.0"` 开局域网；文档写清安全含义
- 老用户：完整版 App 自更新到新版，行为兼容（本机 server、零配置），无感迁移
- `~/.aleph` 遗留 token/device 数据成为孤儿文件：server 启动时忽略，不主动删用户数据；release note 说明可手动清理
- 文档：CLAUDE.md「Auth UX」章节、SECURITY.md auth-ux 部分删除/改写；「分发形态」备注更新为三产物

## 8. 错误处理

- 纯壳连不上 server：原生错误页（重试 / 改地址 / 重扫 mDNS），不白屏
- 完整版 daemon 拉起失败：沿用现有 supervisor 重试逻辑（精简保留）
- 端口占用/二次启动：现有 flock exit 64 机制不动
- WS 断线重连：panel 现有逻辑不动

## 9. 测试策略

- **Server**：删 auth 后全量 `cargo test -p alephcore --lib`；新增 origin_policy 矩阵单测（localhost / 私网 IP / 公网域名 / 无 Origin / allow_any_origin）+ "无凭据 connect 直接成功"集成测试
- **壳**：双 feature 矩阵 `cargo check`（开/关 `embedded-core`）；三平台 CI verify-build（教训：`#[cfg(target_os)]` 门控代码本地 macOS 看不见 Linux/Windows 编译错误）
- **Panel**：必须跑 wasm target 构建验证（native check 通过不代表 wasm 通过）
- **E2E 手动验收**：
  1. 本机浏览器无凭据直访 panel，走通对话
  2. `host = "0.0.0.0"` 后局域网另一设备直访
  3. 纯壳填 IP 连远程 server
  4. 完整版零配置开箱

## 10. 红线对照

- **R6 一核多端**：强化——UI 彻底变薄客户端，server 是唯一大脑
- **R2 UI 逻辑唯一源**：壳更薄，业务 UI 全在 panel ✓
- **R1 大脑四肢分离**：壳的原生能力（通知/托盘）是纯 I/O ✓
- **R5 AI 主动到达**：桌面通知通道保留 ✓
- **R3/R10 核心轻量化**：净删 ~13k 行 ✓

## 11. 执行顺序（提交分层，单次发版）

1. Server 删 auth（middleware 摘除 → handlers/auth → security/* → 根文件 → CLI）→ 每步可编译可测
2. origin_policy 简化重写 + 新测试
3. Panel 删认证 UI + connect 简化 → wasm 构建验证
4. 壳重写（共同部分 → 双变体 feature）→ 双 feature check
5. Workflow 三产物矩阵 + install.sh
6. 文档清扫（CLAUDE.md / SECURITY.md / README）
