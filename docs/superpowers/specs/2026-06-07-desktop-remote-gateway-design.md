# Spec A — 桌面 shell 连接远程 Gateway

> 2026-06-07 · 状态：设计已批准，待实现
> 依赖：Spec B（chat/config 权限分层）。**先 B 后 A**——B 落地后远程设备默认 chat 档即天然安全，否则会有"远程全权"窗口期。

## 1. 背景与动机

桌面 shell（`desktop/shell/`，crate `aleph-desktop-shell`）今天硬连本机 daemon：webview 钉死 `http://127.0.0.1:18790`，并负责启动+健康监管本机 `aleph-server`。

「一核多端」(R6) 的字面落地要求：同一 shell 肢体能**脱离本机 Gateway**，通过 URL 或 IP 连接**远程 Gateway**。本期**不**为远程提供独立安装，只让现有桌面 shell 可切换连接目标。

## 2. 现状关键耦合点

- `main.rs`：`PANEL_URL = "http://127.0.0.1:18790"` 硬编码；`spawn_background` 固定 `reconcile_for_version` + `ensure_ready`（启动+监管本机 daemon）+ 导航本机 Panel。
- `daemon.rs`：`DAEMON_HOST/PORT` 硬编码；裸 TCP HTTP/1.0 `/ready` 探测（无 TLS）。
- `notify.rs`：`WS_URL = "ws://127.0.0.1:18790/ws"` 硬编码 + 启动时从本机 `security.db` 注入 `ALEPH_GATEWAY_TOKEN`。
- `external_link.rs`：`is_internal` 仅放行 loopback host —— **远程 host 会被误判为外链丢给浏览器**（远程场景硬伤）。
- 鉴权：本机走 `bootstrap-url`（nonce，仅 loopback 有效）；远程需 `/pair` 配对码流程（gateway 中间件已对未鉴权浏览器自动跳 `/pair`）。

## 3. 决策摘要（brainstorming 已确认）

| 维度 | 决策 |
|---|---|
| 连接模型 | **互斥切换**：本机 OR 远程，二选一。连远程时不启动/不监管本机 daemon。 |
| 远程鉴权 | **复用配对码流程**：webview 指向远程 → 未鉴权自动跳 `/pair` → 远程端批准 → 设 cookie。notify WS 无凭证时优雅降级。 |
| 连接入口 | **Shell 内置连接页**（静态资源，非 Panel/非 gateway 提供）+ tray 菜单。 |
| 传输层 | **http 与 https 都允许**，不强制（LAN/Tailscale 明文 http 是现实默认）。 |
| 启动策略 | **记住上次目标**；远程不可达 → 显示连接页+错误+重试/返回本机，**不静默 fallback**。 |

## 4. 核心抽象

```
enum ConnectionTarget {
    Local,            // 启动+监管本机 aleph-server，webview→127.0.0.1:18790（=今天）
    Remote(Url),      // 不碰本机 daemon，webview→远程 origin
}
```

- **持久化**：`~/.aleph/.desktop-shell-target`，单行 `local` 或 `http(s)://host:port`。与 `.desktop-shell-autostart` / `.desktop-shell-daemon-version` 同目录同风格。
- **首次运行无文件 = `Local`** —— 完全等同今天的行为，**零回归**。
- **URL 规范化**：接受 `http://host` / `host:port` / `https://host`，解析为 `Url`，缺省端口 18790。

## 5. 组件设计

### 5.1 连接页（新）

- 新增 shell 自带静态资源 `desktop/shell/connect/index.html`（与 `splash/` 同级，`tauri://` 资源 → `external_link::is_internal` 天然放行）。
- 内容：URL/IP 输入框 + "连接 / 返回本机" + 错误显示区（复用 `window.__alephError` 风格）。
- 纯 shell 资产，非 Panel、非 gateway 提供 —— 不触 R2（这是 shell 自身"我是谁的肢体"配置，不是业务 UI）。

### 5.2 最小原生命令面（审慎重开 `invoke_handler`）

`main.rs` 注释当前声明"No invoke_handler"。本期**审慎重开**，仅 3 个命令，严格限于连接配置（非业务逻辑），spec 显式声明此例外：

- `get_connection_target() -> String`
- `set_connection_target(target: String)` —— 校验+规范化+持久化，触发重新分流导航。
- `clear_connection_target()` —— 回 Local。

### 5.3 tray 菜单

- 加「连接到远程…」→ 导航 webview 到 connect 页。
- 加「返回本机」→ `set Local` + 起本机 daemon + 导航本机 Panel。
- 现有项（Quit / Quit & Stop / Show）不变。macOS 菜单同步。

### 5.4 背景分流（`spawn_background` 按 target 分支）

- **Local**：`reconcile_for_version` + `ensure_ready`（启动+监管）+ 导航本机 Panel —— **逐字节等同今天**。
- **Remote(url)**：**跳过** reconcile/launch/relaunch；**裸 TCP 可达性探测**（连得上 host:port 即可达，零 TLS/HTTP 依赖；真正的 HTTP+鉴权+TLS 全交给 webview）；导航 webview 到远程 root（未鉴权自动跳 `/pair`）。

### 5.5 supervisor 扩状态

`Supervisor` / `SupervisorAction` 增 Remote 语义：

- Remote 档健康探测失败（裸 TCP 不可达）→ 动作 `ShowConnectionError`（**不 relaunch** —— 远程 daemon 起不了），导航回 connect 页并附错误 + 重试 + 返回本机。
- 恢复 → `ReloadPanel`。
- Local 档行为完全不变。

### 5.6 `external_link` 动态放行

- `is_internal`：loopback 恒 internal，**外加当前远程 origin**。
- 当前远程 host 存全局 `RwLock<Option<url::Host>>`（或 `ArcSwap`），`set_connection_target` 时更新。
- 否则远程 Panel 内的内部导航会被误判外链丢给浏览器。

### 5.7 `notify` WS

- 指向当前 target 的 `ws(s)://…/ws`。
- **安全边界（关键）**：Remote 档**绝不**把本机 `ALEPH_GATEWAY_TOKEN` 发给远程 —— 防本机凭证泄露给远程 server。connect 帧在 Remote 档省略本机 token。
- 无远程凭证 → 优雅降级，不弹通知（已定决策；远程通知本期不做凭证派生，YAGNI）。

### 5.8 不变项

- `update` checker：关于 app 自身（GitHub releases），与 target 正交，保持运行。
- `hotkey` / `deeplink`：与 target 正交，不变。
- `main()` 启动时注入本机 token 的逻辑保留（Local 档需要）；Remote 档由 notify 自行不使用它。

## 6. 安全

- 互斥：连远程时本机 daemon 不被启动（无意外暴露）。
- 本机 token 绝不外泄给远程（§5.7）。
- 远程默认 chat 档（由 **Spec B** 保证）—— 这是先 B 后 A 的原因。
- 传输不强制加密是显式权衡（LAN/Tailscale 场景），spec 记录之。

## 7. 测试（纯单元，无需真 daemon，延续现有风格）

- `ConnectionTarget` URL 规范化（`host:port` / 带 scheme / 缺省端口）。
- `Supervisor` Remote 档：失败 → `ShowConnectionError`（不 relaunch）；恢复 → `ReloadPanel`；Local 档回归不变。
- `external_link::is_internal` 动态：设远程 origin 后该 origin internal，loopback 仍 internal，其它外链仍 external。
- notify 在 Remote 档不带本机 token（connect 帧断言）。
- target 持久化读写往返。

## 8. 红线对账

| 红线 | 落地 |
|---|---|
| R1 — 大脑/四肢分离 | 纯 `desktop/` 改动，零 `src/` |
| R2 — UI 唯一源 | 连接页是 shell I/O 配置（非业务 UI）；业务 UI 仍在远程 Panel |
| R3/R10/P6 — 薄、笨 | 不引入 TLS/HTTP 客户端依赖；探测复用裸 TCP |
| R6 — 一核多端 | 本 spec 即其字面落地 |

## 9. 范围外（YAGNI）

- 远程 shell 独立安装包。
- 多目标保存/快速切换（账号切换器式）—— 本期只单一持久目标 + 手动切。
- 远程通知凭证派生（notify 远程只降级）。
- https 强制 / 私有网段校验。
