# Panel 配置页权限分层反映 (Config Tier Reflection)

**日期**: 2026-06-09
**范围**: 纯前端 (`interfaces/webchat/`),后端零改动
**关联红线**: R2 (UI 逻辑唯一源)、R4 (Interface 纯 I/O)、P7 (防御性设计)
**前序工作**: channel 2 层权限 (`f97b2b94f`)、Panel 远程复用 channel 身份 (`project_channel_session_origin_binding`)、壳核分离连接状态 chip (`project_shell_core_separation_gap_fix`)

---

## 1. 背景与问题

Aleph 壳核分离:Panel (Leptos/WASM) 通过 JSON-RPC 连接远程 `aleph-server`。连接按 2 层 tier 授权:

- **Chat tier (第一层,远程默认)** = `guest` 角色 = 对话 + 只读。
- **Config tier (第二层)** = `operator` 角色 = 全部配置写入(等同本地 loopback)。

**后端已完整落地且强制**(本次不动):

| 能力 | 现状 | 证据 |
|---|---|---|
| 2 层 tier 闸口(方法级 + 工具级) | ✅ | `handler.rs:1019-1044`、`tools/scoped/dispatch.rs:119` |
| 远程默认 Chat / 本地 loopback 默认 Config | ✅ | `handlers/auth/tier.rs:79-90` |
| 配对时选择 tier | ✅ | `pairing.approve` 接 `level:"config"\|"chat"` (`pairing.rs:241`) |
| 事后手动授权 / 降级 | ✅ | `devices.set_level(device_id, level)` 持久化 + 热刷新活连接 (`devices.rs:77-157`) |
| Panel 设备管理 UI(每设备 tier 徽章 + 提权/降级切换) | ✅ | `views/settings/security/gateway.rs:200-250` |
| 前端捕获自身 role | ✅ | `context.rs:145` `role` signal、`is_operator()` (`context.rs:205`) |

**真缺口(纯前端 UX 不诚实)**:`is_operator()` 仅被集群页 (`views/settings/network/cluster.rs`) 用于门控。其余约 20 个 config-write 设置子页**无条件渲染**写控件。Chat tier 远程用户体验:

> 打开 Providers / Channels / Security 等 → 完整可填表单 → 点保存 → 后端返回 `PERMISSION_DENIED` → 前端仅作通用错误打印,不解释"需 Config 权限",也不告知如何提权。

三个子缺口:
- **A.** 配置写控件不按 tier 门控(约 20 页无条件渲染)。
- **B.** 无"当前连接身份"指示(`role` 已存但从不显示)。
- **C.** `PERMISSION_DENIED` 无专门处理,报成通用错误。

---

## 2. 目标

让前端**诚实反映**后端已有的 tier 判定。非 operator(Chat tier)连接时:配置写控件锁定并解释、显示当前只读身份、写失败给可操作提示。数据查看页与配置值的**只读查看**保持开放(符合"第一层 = 对话 + 查看")。

**非目标**:任何后端改动;收紧 Chat tier 的敏感数据**读**权限;实时热提权(见 §6 限制)。

---

## 3. 设计:集中闸门 + 身份指示

复用现有 `is_operator()`(operator = Config tier,guest = Chat tier),不新造权限概念。

### 组件 1 — `PermissionBanner`(全局身份横幅)

挂进 **Settings 模式布局外壳**(包裹全部 `/settings/*` 路由的组件,`app.rs` SettingsRouter 附近)。`!is_operator()` 时常驻显示:

> "你当前以 **Chat(只读)** 身份连接,配置修改已锁定。如需修改,请联系 operator 在「设置 → 安全」中授予 Config 权限,或重新配对时选择 Config。"

一处挂载自动覆盖全部配置子页——这是"集中"的关键,避免逐页改。数据查看页(Dashboard/Memory)属于其他 mode,不受影响。

### 组件 2 — `ConfigGate`(配置页闸门包装,路由层集中)

可复用 Leptos 组件,在 **`SettingsRouter`(`app.rs:375`)路由 match 层**包住每个 config-write 页的整页 view —— `"/settings/providers" => view! { <ConfigGate><ProvidersView /></ConfigGate> }`。这是真正的"集中":门控逻辑只在路由一处,共享一个 `ConfigGate`,不逐页改 20 个文件的内部。镜像 `cluster.rs` 现有 `<Show when=move || state.is_operator()>` 范式:

- **operator** → 原样渲染 children(整页配置)。
- **非 operator** → 渲染锁定卡(`LockedNotice`:"配置修改需 Config 权限,请联系 operator 在「设置 → 安全」授予,或重新配对选择 Config")替代整页。

**行为说明**:Chat-tier 用户打开 config 页看到锁定卡,**不保留配置值只读查看**。配置页属于第二层;第一层的"查看数据"指 Dashboard/Memory/Trace/Logs/Usage 数据仪表盘(独立 mode,完全开放,不门控)。配置值(API key / channel token)敏感,锁定卡比半禁用表单更诚实安全。

应用清单(在 `SettingsRouter` 包 `<ConfigGate>` 的 config-write 路由):
- **Basic**:general
- **AI**:search、providers、embedding-providers、reranking-providers、generation-providers、model-route、memory
- **Extensions**:routing、mcp、plugins、skills、clawhub、acp
- **Advanced**:browser、security、auth、policies、execution
- **Channels**:`/settings/channels/{platform}` 各平台页(`ChannelPlatformPage`)
- **不挡**(read-only / 本地 / 连接管理):`/settings`(索引)、appearance、behavior、channels(概览只读)、network(含连接目标切换 + cluster.rs 已自带门控,Chat-tier 需保留切回本地的能力)

### 组件 3 — 连接 tier 指示

复用现有连接状态 chip(`dashboard_sidebar` footer)。在其上加当前身份徽章(Config / Chat),复用 `DeviceCard` 已有的 `settings.security.tier_config` / `tier_chat` i18n 标签与样式。用户始终看得见自己的身份。

### 组件 4 — `PERMISSION_DENIED` 友好映射(纵深防御)

在 api 错误层(`context.rs:510` 附近)识别 permission-denied 错误码 / 消息,映射成可操作提示("此操作需 Config 权限,请联系 operator 提权")。给任何未被 ConfigGate 包到的写路径兜底。

---

## 4. 数据流

```
connect 响应 → context.rs capture_role() → state.role signal
                                              │
                    ┌─────────────────────────┼─────────────────────────┐
                    ▼                          ▼                         ▼
          PermissionBanner            ConfigGate <Show>          连接 chip 身份徽章
        (Settings 布局, 非op显示)   (每写区, op渲染/非op锁定)    (dashboard_sidebar footer)
                                              │
                                  写 RPC 失败 → context.rs 错误层
                                              ▼
                                  PERMISSION_DENIED 友好映射
```

唯一真值源:`state.role` → `is_operator()`。四组件全是它的纯投影,无额外状态。

---

## 5. i18n

新增键组 `settings.permission.*`(en + zh 双语,parity 校验):
- `settings.permission.banner_chat` — 横幅文案
- `settings.permission.locked_notice` — ConfigGate 锁定占位文案
- `settings.permission.denied` — PERMISSION_DENIED 友好提示

身份徽章复用现有 `settings.security.tier_config` / `tier_chat`。

leptos_i18n 0.6 编译期 codegen + 严格 parity:提交前用脚本校验 en/zh JSON parse + key 全对等。

---

## 6. 边界与已知限制

- **后端零改动**:tier 闸口、pairing-time 选择、`devices.set_level`、设备管理 UI 全已存在。
- **不收紧敏感读**:Chat tier 的数据查看与配置值只读保持现状。若需限制敏感读,另开 spec。
- **热提权延迟**:operator 中途把远程设备升到 Config,后端下次请求即生效,但前端 `state.role` 仅在 connect 时捕获 → **刷新 / 重连后才反映**。本次接受此延迟;实时 re-capture 需前端订阅一个目前不存在的 role-change 事件,YAGNI 暂不做。spec 标注以待将来。
- **资源嵌入链**:改完 panel 源码后,daemon 经 `rust_embed` 在编译期烧入 dist,须 `just wasm` + 重编 `aleph-server` binary 才生效(部署步骤,实现后单列)。

---

## 7. 验证标准

1. **operator(Config tier / 本地 loopback)**:所有配置页写控件正常渲染可用,无横幅,chip 显示 Config 徽章。
2. **guest(Chat tier / 远程默认)**:Settings 区顶部显示锁定横幅;打开任一 config-write 页显示锁定卡(整页门控);network/概览/本地偏好页仍可访问;chip 显示 Chat 徽章;任何漏网写操作触发 `PERMISSION_DENIED` 时显示友好提示。
3. **数据查看页**(Dashboard/Memory/Trace/Logs/Usage):两种 tier 均正常可看,无门控。
4. **i18n**:en/zh key parity 脚本通过;无孤儿 / 缺失键。
5. **构建**:`just wasm` 通过;`-p aleph-panel` 单测通过(注意是 panel crate 非 alephcore)。

---

## 8. 文件影响预估

| 文件 | 改动 |
|---|---|
| `interfaces/webchat/src/components/`(新)`config_gate.rs` | 新增 ConfigGate + PermissionBanner 组件 |
| `app.rs`(SettingsRouter 布局) | 挂载 PermissionBanner |
| `views/settings/**`(约 20 页) | 写区包 ConfigGate(每页一行) |
| `components/dashboard_sidebar.rs`(或连接 chip 所在) | chip 加 tier 身份徽章 |
| `context.rs`(错误层) | PERMISSION_DENIED 友好映射 |
| i18n `en.json` / `zh.json` | 新增 `settings.permission.*` 键 |
