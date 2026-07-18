# Spec B — chat/config 权限分层（核心侧）

> 2026-06-07 · 状态：设计已批准，待实现
> 关联：Spec A（桌面 shell 远程连接）依赖本 spec。**先 B 后 A。**

## 1. 背景与动机

Aleph 是「一核多端」：同一 `aleph-server` 核心可被多种客户端（本机桌面 shell、浏览器、手机、未来的远程桌面 shell）以 Channel/Panel 形式接入。

今天**所有**已配对设备都以 `DeviceRole::Operator` + `permissions=["*"]` 全权接入（`device_store.rs` 默认 `["*"]`）。一旦允许远程接入（Spec A），这意味着任何配对成功的远程设备都能改 provider 配置、装卸 skill/MCP、删设备、读 secret——**远程全权窗口**。

需求：**把"配置权"与"对话权"分离**。远程配对设备**默认只有 chat + 只读 dashboard**，没有配置权；任何改 Aleph 自身配置的操作（无论走 RPC 直调还是对话让 LLM 调工具）都被拦截并触发 **sudo 式现场审批**；除非 server 端操作者授权（配对时或事后）。

## 2. 现状盘点（复用，不重造）

核心侧已有近乎完整的基础设施：

- **`src/gateway/method_authz.rs`**：`MethodPrivilege::{Authenticated, Operator}` 分类器。已把 config 类 RPC 标为 **Operator-only**：
  - 命名空间整段：`secret.` / `secrets.` / `pty.`
  - 精确方法：`daemon.shutdown`、`config.apply` / `config.patch` / `config.reload`、`devices.remove` / `devices.revoke`、`pairing.approve` / `pairing.reject`、`plugins.{install,uninstall,enable,disable}`、`skills.{install,remove,update,install_dep}`、`mcp.{start,stop}`、`cron.{create,delete,update,toggle,run}`、heartbeat 写类…
  - 读类（`config.get` / `config.schema` / `*.list` / `cron.get` …）刻意留开。
  - **这张表就是"仅 Aleph 自身配置算 config"的精确边界 SSOT 起点。**
- **`src/gateway/device_store.rs`**：每设备 `permissions: Vec<String>`（默认 `["*"]`），SQLite 持久化。
- **`DeviceRole::Operator` vs guest/node** 角色已存在；guest 已是"受限工具访问"概念。
- **`src/gateway/event_scope.rs`**：`(topic前缀, 所需权限)` 事件门控；`pairing.*` 已要求 `admin`/`pairing` 权限——审批请求只投 operator 的能力现成。
- **`src/tools/scoped/dispatch.rs`**：`request_approval(name, reason)` 已能挂起工具调用求批，并触发 `PermissionRequest` + `Notification` observer（桌面通知/邮件/toast）。`ApprovalOutcome::{Approved, Rejected, Timeout}` 已定义；`denial_ledger` 已记拒绝。
- **`pairing.approve` / `devices.list` / `devices.revoke`** handler 已存在。

**结论**：本 spec 不新增角色枚举、不新造审批通道，只填三处缺口（§3）。

## 3. 三处缺口

### 缺口 1 — 配对授级（不再一律 Operator）

`pairing.approve` handler 扩参 `level: "chat" | "config"`，**默认 `chat`（安全默认）**：

- `config` 档 → `DeviceRole::Operator` + `permissions=["*"]`（=今天的行为）。
- `chat` 档 → 非 operator 角色 + `permissions=["chat","read"]`。
- 写入 `device_store` 的 `permissions` 与角色字段。
- **本机 loopback / 桌面 shell（Local 档）仍自动 operator**——零回归（本机自动 bootstrap 授权路径不经 `pairing.approve` 的 level 参数，保持 `*`）。

### 缺口 2 — 硬拒 → sudo 现场审批，且覆盖 RPC 与工具两条路径

建立**单一事实源分类器**：

```
// src/gateway/capability_authz.rs（或扩 method_authz）
fn config_capability_class(name: &str) -> MethodPrivilege
```

`name` 同时覆盖 **RPC 方法名**与**自管理工具名**（provider/channel/agent/skill/MCP/cron/secret/daemon 订阅…），与 `method_authz` 的 OPERATOR 表**同源**，避免两套漂移。在**两个分发点**都查它：

1. **RPC 分发路径**（`method_authz` 现命中点）——Panel 直接点按钮调 `config.apply` 等。
2. **工具分发路径**（`tools/scoped`）——**对话路径**：用户说"帮我改搜索配置" → LLM 调 config 工具 → 命中。这是 R8「工具即一切」下的**主要**拦截面。

命中后行为由"硬拒"改为：

- 若调用连接是 **operator 档** → 直通（今天的行为）。
- 若是 **chat 档** → **挂起**，经现有 `approval.requested` / `PermissionRequest` / `Notification` observer 向 **operator 端**推送授权请求（桌面通知 + Panel 内提醒，R5 主动到达）。
  - operator **批准** → 本次放行执行；可勾"记住" → 把该设备 `permissions` 永久提升到 config（写 `device_store`）。
  - **拒绝 / 超时** → 回结构化错误 `permission_required`，远程 Panel 渲染成"需 server 端授权"提醒；对话路径下 LLM 如实告知用户。
- **安全**：审批请求只投递给 operator 档连接（复用 `event_scope` 的 admin/pairing 事件门控），chat 档自己看不到、批不了自己的请求。

### 缺口 3 — 角色传播进工具上下文（关键接缝）

发起 agent run 的连接的 `DeviceRole` / `permissions` 必须随 run 注入 `ScopedToolService`，复用现有 SESSION/TURN 作用域那条线（参见 `tools/scoped/mod.rs` 的 `execute_with_cancel` 已 scope `TURN_CONTEXT`，近期已补 `SESSION_ID` scope）。这样工具分发点（缺口 2 第 2 路）才知道"当前 run 是谁发起的、是不是 chat 档"。

## 4. Panel UI（R2，全部在 Leptos）

- **配对审批卡**：原"批准 / 拒绝" → 增"**批准为 chat** / **批准为 config**"两钮（= 配对处授权按钮 + 级别选择）。
- **Devices 管理页**：每设备显示当前级别 + "**授权配置 / 降级为 chat**"钮（事后提升 / 收回）。
- **sudo 审批态**：chat 档触发的 config 操作，发起端 Panel 显示"等待 server 授权…"，operator 远端批准后自动继续，或提示被拒/超时。

R2 对账：权限判定 100% 在 core（R4）；Panel 只渲染网关下发的审批态与按钮；客户端（shell/浏览器/手机）不参与判定。

## 5. 安全边界与已知权衡

- **安全默认**：远程配对默认 `chat`，无配置权。要 config 必须 operator 显式授权。
- **bash 逃逸（已接受权衡）**：chat 档可跑 `bash` / `code_exec`，理论上能用 shell 直接改 `~/.aleph` 配置文件绕过 config 工具门控。idiomatic 路径（LLM 优先用 config 工具、R8/R9）已被覆盖；bash 逃逸作为**已知残留风险**记录在此，不在本 spec 范围内封堵（封堵需 chat 档禁 mutating 工具，会严重阉割远程助手能力，已在 brainstorming 中否决）。
- **凭证不外泄**：审批提升只改 server 端 `device_store`，不向 chat 档下发任何 operator 凭证。

## 6. 测试

- 分类器 SSOT：RPC 名与工具名命中/不命中表（参数化）。
- chat 档命中 config → 审批挂起（mock requester 返回 Approved/Rejected/Timeout 三态）。
- operator 档直通，无审批。
- "记住"提升后 `device_store` permissions 落库，二次调用直通。
- 拒绝/超时回 `permission_required` 结构化错误。
- 配对 `level=chat` 默认、`level=config` 授 `*`；本机路径仍 operator（回归）。
- 角色随 run 注入 ScopedToolService 的传播测试（chat session → config 工具被 gate）。

## 7. 红线对账

| 红线 | 落地 |
|---|---|
| R4 — Interface 无业务逻辑 | 权限判定全在 gateway/core |
| R2 — UI 唯一源 | 审批/授级 UI 全在 Leptos Panel |
| R7/R9 — LLM 主权 | 门控是确定性 infra（安全硬过滤，赋能层允许）；不替 LLM 做推理 |
| R8 — 工具即一切 | 正因 config 是工具，才必须在工具分发点拦截 |
| R10 — 薄 harness | 改动在 gateway/tools 既有分发点，不进 `src/harness/` |

## 8. 范围外（YAGNI）

- 比 chat/config 更细的多级 ACL（per-scope 细粒度）——延后到真有需求。
- 封堵 bash 逃逸。
- 远程 shell 本体（属 Spec A）。
