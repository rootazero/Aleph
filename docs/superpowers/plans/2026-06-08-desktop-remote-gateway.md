# Spec A — 桌面 shell 连接远程 Gateway Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让桌面 shell（crate `aleph-desktop-shell`）能在「本机 daemon」与「远程 Gateway」之间互斥切换：连远程时不启动/不监管本机 daemon、webview 指向远程 origin（未鉴权自动跳 `/pair` 走配对码流程，远程批准后落 chat 档——由已落地的 browser 配对档位化保证）、且**绝不**把本机 token 发给远程。首次运行无配置=本机，逐字节零回归。

**Architecture:** 纯 `desktop/` 改动，零 `src/`。新增 `ConnectionTarget { Local, Remote(Url) }` 抽象 + `~/.aleph/.desktop-shell-target` 持久化；`spawn_background`/`supervise_daemon` 按 target 分支（Local 不变，Remote 跳过 daemon 管理 + 裸 TCP 可达性探测）；`external_link::is_internal` 动态放行当前远程 origin；`notify` 在 Remote 档省略本机 token；新增 3 个严格限于连接配置的 Tauri 命令 + shell 自带连接页 + tray/menu 入口。

**Tech Stack:** Rust（`aleph-desktop-shell`，workspace 内，`cargo check/test -p aleph-desktop-shell`）+ Tauri 2 + 静态 HTML 连接页（放 `desktop/shell/splash/` 下，frontendDist 根）。

**Spec:** `docs/superpowers/specs/2026-06-07-desktop-remote-gateway-design.md`

**Git 约束（全程）:** 共享单分支 main + 并发提交者——只追加式提交、**显式文件路径**暂存（禁 `git add -A/-u/.`）、禁 reset/amend/rebase/push；提交信息英文、无 attribution footer；不 push；提交前 `git status` 确认不卷入他人 WIP（工作区有 `interfaces/webchat/dist/*` 产物未暂存，勿 staged）。

---

## File Structure

- **新建** `desktop/shell/src/connection.rs` — `ConnectionTarget` enum、URL 规范化、`.desktop-shell-target` 读写 helper、3 个 Tauri 命令。
- **新建** `desktop/shell/splash/connect.html` — shell 自带连接页（frontendDist 根下，served 于 `tauri://localhost/connect.html`）。
- **改** `desktop/shell/src/main.rs` — `mod connection;`、Builder 加 `invoke_handler`、`spawn_background` 按 target 分支、`supervise_daemon` 消费新动作。
- **改** `desktop/shell/src/external_link.rs` — 全局当前远程 host + `is_internal` 动态放行。
- **改** `desktop/shell/src/notify.rs` — `connect_request` Remote 档省略本机 token + WS URL 按 target。
- **改** `desktop/shell/src/daemon.rs` — 裸 TCP 可达性探测 helper（host/port 参数化）+ `build_panel_url` 的 target 感知（或新 helper）。
- **改** `desktop/shell/src/tray.rs` + `desktop/shell/src/menu.rs` — 「Connect to Remote…」/「Back to Local」入口 + 分发。

任务顺序：Task 1（connection.rs 核心，纯单元）→ Task 2（external_link 动态放行，纯单元）→ Task 3（notify Remote 省 token，纯单元）→ Task 4（Supervisor Remote 状态，纯单元）→ Task 5（Tauri 命令 + 连接页）→ Task 6（背景分流 + tray/menu wiring）。Task 5/6 是集成 wiring（编译验证；Tauri 运行时部分手动）。

> **行号为快照**：以实现时实际文件为准。Local 档所有路径必须逐字节等同今天（零回归）。

---

### Task 1: `connection.rs` — ConnectionTarget 抽象 + 持久化

**Files:**
- Create: `desktop/shell/src/connection.rs`
- Modify: `desktop/shell/src/main.rs`（加 `mod connection;`）

- [ ] **Step 1: 写失败测试**

新建 `desktop/shell/src/connection.rs`，先写测试模块：
```rust
//! Connection target: local daemon vs remote Gateway.
//!
//! The shell connects to exactly one Gateway at a time — either the
//! same-machine `aleph-server` it launches and supervises (Local, the
//! default and today's behaviour), or a remote Gateway by URL (Remote, which
//! never touches the local daemon). The choice persists in
//! `~/.aleph/.desktop-shell-target`; a missing file means Local (zero
//! regression on first run).

use url::Url;

/// Default Gateway port when the user omits one.
const DEFAULT_PORT: u16 = 18790;

/// Where the chosen target persists. Mirrors the sibling
/// `.desktop-shell-autostart` / `.desktop-shell-daemon-version` markers.
fn target_marker() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".aleph/.desktop-shell-target"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionTarget {
    /// Launch + supervise the local daemon; webview → 127.0.0.1:18790.
    Local,
    /// Connect to a remote Gateway by origin; never touch the local daemon.
    Remote(Url),
}

impl ConnectionTarget {
    pub fn is_local(&self) -> bool {
        matches!(self, ConnectionTarget::Local)
    }

    /// Parse a persisted/user-entered target string. `"local"` (any case) or
    /// empty → Local. Otherwise normalise to a `Remote(Url)`:
    /// accept `host`, `host:port`, `http://host`, `https://host:port`;
    /// default scheme `http`, default port 18790.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let t = raw.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("local") {
            return Ok(ConnectionTarget::Local);
        }
        let with_scheme = if t.contains("://") {
            t.to_string()
        } else {
            format!("http://{t}")
        };
        let mut url = Url::parse(&with_scheme).map_err(|e| format!("invalid target URL: {e}"))?;
        match url.scheme() {
            "http" | "https" => {}
            other => return Err(format!("unsupported scheme: {other}")),
        }
        if url.host().is_none() {
            return Err("target URL has no host".to_string());
        }
        if url.port().is_none() {
            // set_port only errors when the URL cannot have a port (it can here)
            let _ = url.set_port(Some(DEFAULT_PORT));
        }
        Ok(ConnectionTarget::Remote(url))
    }

    /// Serialise for persistence. Local → `"local"`; Remote → the URL origin.
    pub fn to_persisted(&self) -> String {
        match self {
            ConnectionTarget::Local => "local".to_string(),
            ConnectionTarget::Remote(url) => url.as_str().trim_end_matches('/').to_string(),
        }
    }
}

/// Load the persisted target; missing/unreadable/unparsable → Local
/// (fail-safe: a corrupt marker must never strand the user on a broken
/// remote — it falls back to the always-available local daemon).
pub fn load_target() -> ConnectionTarget {
    let Some(marker) = target_marker() else {
        return ConnectionTarget::Local;
    };
    match std::fs::read_to_string(&marker) {
        Ok(s) => ConnectionTarget::parse(&s).unwrap_or(ConnectionTarget::Local),
        Err(_) => ConnectionTarget::Local,
    }
}

/// Persist a target string (already validated by `parse`). Writes the
/// normalised form.
pub fn save_target(target: &ConnectionTarget) -> Result<(), String> {
    let Some(marker) = target_marker() else {
        return Err("home directory not found".to_string());
    };
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create .aleph dir: {e}"))?;
    }
    std::fs::write(&marker, target.to_persisted()).map_err(|e| format!("write target: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_local_parse_to_local() {
        assert_eq!(ConnectionTarget::parse("").unwrap(), ConnectionTarget::Local);
        assert_eq!(ConnectionTarget::parse("  ").unwrap(), ConnectionTarget::Local);
        assert_eq!(ConnectionTarget::parse("local").unwrap(), ConnectionTarget::Local);
        assert_eq!(ConnectionTarget::parse("LOCAL").unwrap(), ConnectionTarget::Local);
    }

    #[test]
    fn bare_host_gets_http_and_default_port() {
        let t = ConnectionTarget::parse("192.168.1.5").unwrap();
        assert_eq!(t.to_persisted(), "http://192.168.1.5:18790");
    }

    #[test]
    fn host_port_gets_http() {
        let t = ConnectionTarget::parse("box.lan:9000").unwrap();
        assert_eq!(t.to_persisted(), "http://box.lan:9000");
    }

    #[test]
    fn explicit_scheme_preserved() {
        let t = ConnectionTarget::parse("https://gw.example.com").unwrap();
        assert_eq!(t.to_persisted(), "https://gw.example.com:18790");
        let t2 = ConnectionTarget::parse("https://gw.example.com:443").unwrap();
        assert_eq!(t2.to_persisted(), "https://gw.example.com:443");
    }

    #[test]
    fn unsupported_scheme_rejected() {
        assert!(ConnectionTarget::parse("ftp://host").is_err());
        assert!(ConnectionTarget::parse("ws://host").is_err());
    }

    #[test]
    fn is_local_flag() {
        assert!(ConnectionTarget::Local.is_local());
        assert!(!ConnectionTarget::parse("10.0.0.1").unwrap().is_local());
    }
}
```

- [ ] **Step 2: 加 `mod connection;`**

在 `desktop/shell/src/main.rs` 顶部模块声明区（约 :1-21，与 `mod daemon;` 等并列）加 `mod connection;`。

- [ ] **Step 3: 运行测试确认通过（先验证核心逻辑）**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p aleph-desktop-shell connection 2>&1 | tail -20
```
Expected: 6 个 connection 测试通过。若 `url`/`dirs` crate 未在 `desktop/shell/Cargo.toml` 依赖里，先确认（`notify.rs`/`external_link.rs` 已用 `url`，`daemon.rs` 已用 `dirs` → 应已有；若缺则加入 `[dependencies]`，并在 commit 里显式 add `Cargo.toml`）。

- [ ] **Step 4: fmt + clippy**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo fmt -p aleph-desktop-shell && cargo clippy -p aleph-desktop-shell 2>&1 | grep -A3 connection.rs | head
```
Expected: 无残留、无新警告。

- [ ] **Step 5: 提交（显式路径）**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git status
git add desktop/shell/src/connection.rs desktop/shell/src/main.rs
git commit -m "desktop: ConnectionTarget abstraction + persisted target marker"
git show --stat HEAD
```
若 Step 3 改了 `Cargo.toml`，一并显式 add。

---

### Task 2: `external_link` 动态放行当前远程 origin

**Files:**
- Modify: `desktop/shell/src/external_link.rs`（`is_internal` :57-72、`LOOPBACK_HOSTS` :31、测试 :110-152）

- [ ] **Step 1: 写失败测试**

在 `desktop/shell/src/external_link.rs` 的 `mod tests`（:110-152）内新增：
```rust
    #[test]
    fn remote_origin_becomes_internal_when_set() {
        // default: a LAN host is external
        assert!(!internal("http://10.0.0.5:18790/chat"));
        set_remote_host(Some(Url::parse("http://10.0.0.5:18790").unwrap()));
        // now the configured remote host is internal, loopback still internal,
        // and an unrelated origin stays external
        assert!(internal("http://10.0.0.5:18790/chat"));
        assert!(internal("http://127.0.0.1:18790/"));
        assert!(!internal("https://github.com/aleph"));
        // clearing reverts
        set_remote_host(None);
        assert!(!internal("http://10.0.0.5:18790/chat"));
    }
```

- [ ] **Step 2: 运行确认失败**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p aleph-desktop-shell external_link 2>&1 | tail -15
```
Expected: 编译失败（`set_remote_host` 未定义）。

- [ ] **Step 3: 实现全局远程 host + 动态放行**

在 `desktop/shell/src/external_link.rs` 顶部（imports 后）加：
```rust
use std::sync::RwLock;

/// The currently-configured remote Gateway host, if any. `is_internal` treats
/// it as internal so in-Panel navigations on the remote origin are not
/// misrouted to the OS browser. Loopback is always internal regardless.
static REMOTE_HOST: RwLock<Option<String>> = RwLock::new(None);

/// Update the remote origin allow-list. Pass the remote target's URL when
/// switching to a remote Gateway, or `None` when returning to Local.
pub fn set_remote_host(url: Option<url::Url>) {
    let host = url.and_then(|u| u.host_str().map(|h| h.trim_start_matches('[').trim_end_matches(']').to_string()));
    if let Ok(mut guard) = REMOTE_HOST.write() {
        *guard = host;
    }
}
```
> `RwLock::new` 在 const 上下文可用（Rust 1.63+，本仓库 MSRV 1.95 满足），故 `static` 直接初始化无需 `OnceLock`/`lazy_static`。

把 `is_internal`（:57-72）的 http/https 分支扩展为也放行当前远程 host：
```rust
        "http" | "https" => match url.host_str() {
            Some(host) => {
                let host = host.trim_start_matches('[').trim_end_matches(']');
                if LOOPBACK_HOSTS.contains(&host) || host == "tauri.localhost" {
                    return true;
                }
                // Allow the currently-configured remote Gateway origin.
                REMOTE_HOST
                    .read()
                    .ok()
                    .and_then(|g| g.clone())
                    .is_some_and(|remote| remote == host)
            }
            None => false,
        },
```

- [ ] **Step 4: 运行确认通过**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p aleph-desktop-shell external_link 2>&1 | tail -15
```
Expected: 新测试 + 既有 `loopback_panel_origins_are_internal` / `outside_origins_are_external` 全绿。
> 注意测试间共享全局 `REMOTE_HOST`——新测试结尾 `set_remote_host(None)` 复位，避免污染其它测试（Rust 测试默认多线程，但本测试自洽地设/清；若出现 flaky 用 `serial_test` 或把断言收进单一测试，本计划已合进一个测试避免跨测试顺序依赖）。

- [ ] **Step 5: fmt + clippy + 提交**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo fmt -p aleph-desktop-shell && cargo clippy -p aleph-desktop-shell 2>&1 | grep -A3 external_link | head
git status
git add desktop/shell/src/external_link.rs
git commit -m "desktop: external_link allows the configured remote origin"
git show --stat HEAD
```

---

### Task 3: `notify` Remote 档省略本机 token + WS URL 按 target

**Files:**
- Modify: `desktop/shell/src/notify.rs`（`WS_URL` :27、`connect_request` :106-124、`run_notification_bridge` 入口、测试 :295-318）

- [ ] **Step 1: 写失败测试**

先 Read `notify.rs` 确认 `run_notification_bridge` 签名与 `WS_URL` 用法、`connect_request` 调用点。在 `mod tests`（:295-318）内新增：
```rust
    use crate::connection::ConnectionTarget;

    #[test]
    fn local_connect_request_includes_token_when_present() {
        std::env::set_var("ALEPH_GATEWAY_TOKEN", "tok-local");
        let v: Value = serde_json::from_str(&connect_request(&ConnectionTarget::Local)).unwrap();
        assert_eq!(v["params"]["shared_token"], "tok-local");
        std::env::remove_var("ALEPH_GATEWAY_TOKEN");
    }

    #[test]
    fn remote_connect_request_never_sends_local_token() {
        std::env::set_var("ALEPH_GATEWAY_TOKEN", "tok-local");
        let remote = ConnectionTarget::parse("10.0.0.5").unwrap();
        let v: Value = serde_json::from_str(&connect_request(&remote)).unwrap();
        assert!(
            v["params"].get("shared_token").is_none(),
            "remote connect must NOT leak the local token"
        );
        std::env::remove_var("ALEPH_GATEWAY_TOKEN");
    }
```
> 两测试都设/清同一 env var；若并行 flaky，合进一个测试顺序执行。本计划保留两测试但都在结尾 remove；如 CI flaky 可合并。

- [ ] **Step 2: 运行确认失败**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p aleph-desktop-shell notify 2>&1 | tail -15
```
Expected: 编译失败（`connect_request` 现无参数）。

- [ ] **Step 3: 改 `connect_request` 接收 target**

把 `connect_request`（:106-124）改签名并门控 token：
```rust
fn connect_request(target: &crate::connection::ConnectionTarget) -> String {
    let mut params = json!({
        "device_name": "Aleph Desktop",
        "device_type": "desktop",
        "device_id": "aleph-desktop-shell",
    });
    // Security: only the LOCAL daemon may receive the auto-provisioned local
    // shared token. A remote Gateway must never see it (it would hand a remote
    // server full local operator control). Remote auth rides the /pair cookie
    // flow in the webview; the notify WS gracefully degrades without creds.
    if target.is_local() {
        if let Ok(token) = std::env::var("ALEPH_GATEWAY_TOKEN") {
            if !token.is_empty() {
                params["shared_token"] = json!(token);
            }
        }
    }
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "connect",
        "params": params,
    })
    .to_string()
}
```

- [ ] **Step 4: WS URL 按 target + 调用点传 target**

- 把硬编码 `WS_URL`（:27）的使用改为按 target 计算。新增 helper：
```rust
/// Build the EventBus WS URL for a target. Local → the loopback default;
/// Remote → `ws(s)://host:port/ws` derived from the target origin (https→wss).
fn ws_url(target: &crate::connection::ConnectionTarget) -> String {
    match target {
        crate::connection::ConnectionTarget::Local => WS_URL.to_string(),
        crate::connection::ConnectionTarget::Remote(url) => {
            let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
            let host = url.host_str().unwrap_or("127.0.0.1");
            // port_or_known_default(): the url crate elides scheme-default ports
            // (443/80) from `.port()`, so use the known-default form to recover
            // an https-on-443 remote correctly; parse() always set a port so
            // this is effectively always Some, the unwrap_or is belt-and-braces.
            let port = url.port_or_known_default().unwrap_or(18790);
            format!("{scheme}://{host}:{port}/ws")
        }
    }
}
```
- 改 `run_notification_bridge` 接收/读取当前 target（Read 该函数确认现签名 `pub async fn run_notification_bridge(handle: tauri::AppHandle)`）。最小改动：在它内部 `let target = crate::connection::load_target();`，把 `WS_URL` 用处换成 `ws_url(&target)`，把 `connect_request()` 调用换成 `connect_request(&target)`。
> Local 档：`ws_url(Local)==WS_URL` 且 token 注入逻辑不变 → 逐字节等同今天。

- [ ] **Step 5: 运行确认通过**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p aleph-desktop-shell notify 2>&1 | tail -20
```
Expected: 新 2 测 + 既有 `connect_request_is_well_formed`（现需传 `&ConnectionTarget::Local`——一并更新该既有测试调用）/ `subscribe_request_carries_notify_topics` 全绿。

- [ ] **Step 6: fmt + clippy + 提交**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo fmt -p aleph-desktop-shell && cargo clippy -p aleph-desktop-shell 2>&1 | grep -A3 notify.rs | head
git status
git add desktop/shell/src/notify.rs
git commit -m "desktop: notify omits local token + targets remote WS in Remote mode"
git show --stat HEAD
```

---

### Task 4: `Supervisor` Remote 档语义（ShowConnectionError，不 relaunch）

**Files:**
- Modify: `desktop/shell/src/main.rs`（`SupervisorAction` :357-365、`Supervisor` :367-414、测试 :492-545）

- [ ] **Step 1: 写失败测试**

在 `main.rs` 的 `mod tests`（:492-545）内新增（Remote 档失败应 `ShowConnectionError` 不 `Relaunch`；恢复仍 `ReloadPanel`）：
```rust
    #[test]
    fn supervisor_remote_shows_error_instead_of_relaunch() {
        let mut sup = Supervisor::new_remote(true);
        // sustained failure on a remote target must NOT try to relaunch a
        // daemon we don't own — it surfaces a connection error instead.
        let mut action = SupervisorAction::Idle;
        for _ in 0..FAILURES_TO_DECLARE_DOWN {
            action = sup.tick(false);
        }
        assert_eq!(action, SupervisorAction::ShowConnectionError);
        assert_eq!(sup.health, DaemonHealth::Down);
    }

    #[test]
    fn supervisor_remote_reloads_on_recovery() {
        let mut sup = Supervisor::new_remote(false);
        assert_eq!(sup.tick(false), SupervisorAction::ShowConnectionError);
        assert_eq!(sup.tick(true), SupervisorAction::ReloadPanel);
    }

    #[test]
    fn supervisor_local_behaviour_unchanged() {
        // regression guard: local mode still relaunches
        let mut sup = Supervisor::new(true); // existing ctor = Local
        let mut action = SupervisorAction::Idle;
        for _ in 0..FAILURES_TO_DECLARE_DOWN {
            action = sup.tick(false);
        }
        assert_eq!(action, SupervisorAction::Relaunch);
    }
```

- [ ] **Step 2: 运行确认失败**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p aleph-desktop-shell supervisor 2>&1 | tail -15
```
Expected: 编译失败（`new_remote`、`ShowConnectionError` 未定义）。

- [ ] **Step 3: 扩 enum + Supervisor 模式**

在 `SupervisorAction`（:357-365）加变体：
```rust
    /// Remote target unreachable; we don't own that daemon — surface a
    /// connection error and offer retry / back-to-local instead of relaunch.
    ShowConnectionError,
```
给 `Supervisor` 加一个 `remote: bool` 字段 + `new_remote` 构造，并让 `tick` 在 Down 转移时按模式选动作：
```rust
struct Supervisor {
    health: DaemonHealth,
    consecutive_failures: u32,
    remote: bool,
}

impl Supervisor {
    fn new(daemon_up: bool) -> Self {
        Self { health: if daemon_up { DaemonHealth::Up } else { DaemonHealth::Down }, consecutive_failures: 0, remote: false }
    }
    fn new_remote(reachable: bool) -> Self {
        Self { health: if reachable { DaemonHealth::Up } else { DaemonHealth::Down }, consecutive_failures: 0, remote: true }
    }

    fn down_action(&self) -> SupervisorAction {
        if self.remote { SupervisorAction::ShowConnectionError } else { SupervisorAction::Relaunch }
    }

    fn tick(&mut self, ready: bool) -> SupervisorAction {
        match (self.health, ready) {
            (DaemonHealth::Up, true) => { self.consecutive_failures = 0; SupervisorAction::Idle }
            (DaemonHealth::Up, false) => {
                self.consecutive_failures += 1;
                if self.consecutive_failures >= FAILURES_TO_DECLARE_DOWN {
                    self.health = DaemonHealth::Down;
                    self.down_action()
                } else { SupervisorAction::Idle }
            }
            (DaemonHealth::Down, false) => self.down_action(),
            (DaemonHealth::Down, true) => {
                self.health = DaemonHealth::Up;
                self.consecutive_failures = 0;
                SupervisorAction::ReloadPanel
            }
        }
    }
}
```
> Local 档（`new`，`remote:false`）`down_action()==Relaunch` → 行为逐字节不变。

- [ ] **Step 4: 运行确认通过**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p aleph-desktop-shell supervisor 2>&1 | tail -20
```
Expected: 新 3 测 + 既有 4 个 supervisor 测试全绿。

- [ ] **Step 5: fmt + clippy + 提交**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo fmt -p aleph-desktop-shell && cargo clippy -p aleph-desktop-shell 2>&1 | grep -A3 "main.rs" | head
git status
git add desktop/shell/src/main.rs
git commit -m "desktop: Supervisor remote mode surfaces connection error, no relaunch"
git show --stat HEAD
```

---

### Task 5: Tauri 命令 + 连接页

**Files:**
- Modify: `desktop/shell/src/connection.rs`（加 3 个命令）
- Create: `desktop/shell/splash/connect.html`
- Modify: `desktop/shell/src/main.rs`（Builder 加 `invoke_handler`）
- Modify: `desktop/shell/src/daemon.rs`（`build_panel_url` target 感知或新 helper）

- [ ] **Step 1: 加 3 个 Tauri 命令到 `connection.rs`**

严格限于连接配置（spec §5.2 显式声明此 invoke_handler 例外）。追加：
```rust
/// Return the current target as a string (`"local"` or the remote URL).
#[tauri::command]
pub fn get_connection_target() -> String {
    load_target().to_persisted()
}

/// Validate + persist a new target, update the external-link allow-list, and
/// ask the shell to re-route (navigate + supervise) for it. `raw` accepts the
/// same forms as `ConnectionTarget::parse`.
#[tauri::command]
pub fn set_connection_target(app: tauri::AppHandle, raw: String) -> Result<(), String> {
    let target = ConnectionTarget::parse(&raw)?;
    save_target(&target)?;
    match &target {
        ConnectionTarget::Remote(url) => crate::external_link::set_remote_host(Some(url.clone())),
        ConnectionTarget::Local => crate::external_link::set_remote_host(None),
    }
    crate::reroute_for_target(&app, target);
    Ok(())
}

/// Reset to Local (launch + supervise the local daemon).
#[tauri::command]
pub fn clear_connection_target(app: tauri::AppHandle) -> Result<(), String> {
    set_connection_target(app, "local".to_string())
}
```
> `crate::reroute_for_target` 在 Task 6 定义（导航 + 重启背景分流）。本 Task 先让命令编译——若 Task 6 尚未实现 `reroute_for_target`，可先在 main.rs 加一个最小桩 `pub(crate) fn reroute_for_target(_app: &tauri::AppHandle, _target: connection::ConnectionTarget) {}` 并在 Task 6 充实（commit 信息注明桩）。**为避免半成品，本计划把 `reroute_for_target` 的完整实现也放在 Task 6；Task 5 引入最小桩仅为编译。**

- [ ] **Step 2: 注册 `invoke_handler`**

在 `main.rs` Builder 链（:82-113）`.manage(...)` 附近加：
```rust
        .invoke_handler(tauri::generate_handler![
            connection::get_connection_target,
            connection::set_connection_target,
            connection::clear_connection_target,
        ])
```
并把 :107-110 的 "No invoke_handler" 注释更新为说明此处只暴露连接配置命令（非业务逻辑，spec §5.2 例外）。加最小桩 `reroute_for_target`（见 Step 1 注）。

- [ ] **Step 3: 连接页 `splash/connect.html`**

新建 `desktop/shell/splash/connect.html`（served 于 `tauri://localhost/connect.html`，`is_internal` 的 `tauri` scheme 天然放行）。纯静态、无框架，调用 3 个命令：
```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>Connect Aleph</title>
<style>
  body { font: 14px -apple-system, system-ui, sans-serif; background:#0d0d10; color:#e8e8ea;
         display:flex; min-height:100vh; align-items:center; justify-content:center; margin:0; }
  .card { width:340px; padding:28px; background:#17171c; border-radius:14px; box-shadow:0 10px 40px #0008; }
  h1 { font-size:18px; margin:0 0 4px; }
  p { color:#9a9aa2; margin:0 0 18px; font-size:13px; }
  input { width:100%; box-sizing:border-box; padding:10px 12px; border-radius:8px;
          border:1px solid #2a2a32; background:#0d0d10; color:#e8e8ea; font-size:14px; }
  .row { display:flex; gap:8px; margin-top:14px; }
  button { flex:1; padding:10px; border:0; border-radius:8px; font-size:14px; cursor:pointer; }
  .primary { background:#4f46e5; color:#fff; }
  .ghost { background:#23232b; color:#cfcfd6; }
  .err { color:#ff6b6b; font-size:12px; margin-top:10px; min-height:14px; }
</style>
</head>
<body>
  <div class="card">
    <h1>Connect to a Gateway</h1>
    <p>Enter a remote Aleph address (e.g. <code>192.168.1.5</code> or <code>https://gw.example.com</code>), or go back to the local daemon.</p>
    <input id="addr" placeholder="host, host:port, or http(s)://host" autofocus />
    <div class="row">
      <button class="primary" id="connect">Connect</button>
      <button class="ghost" id="local">Back to Local</button>
    </div>
    <div class="err" id="err"></div>
  </div>
  <script>
    const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
    const err = (m) => { document.getElementById('err').textContent = m || ''; };
    document.getElementById('connect').addEventListener('click', async () => {
      err('');
      const raw = document.getElementById('addr').value.trim();
      if (!raw) { err('Enter an address'); return; }
      try { await invoke('set_connection_target', { raw }); }
      catch (e) { err(String(e)); }
    });
    document.getElementById('local').addEventListener('click', async () => {
      err('');
      try { await invoke('clear_connection_target'); }
      catch (e) { err(String(e)); }
    });
    // prefill with the current target
    invoke('get_connection_target').then(t => {
      if (t && t !== 'local') document.getElementById('addr').value = t;
    }).catch(() => {});
    // shell calls this to surface routing errors (mirrors window.__alephError)
    window.__alephError = (m) => err(m);
  </script>
</body>
</html>
```
> 确认 Tauri 2 的 invoke 全局：本仓库 webview 是否注入 `window.__TAURI__`（需 `app.withGlobalTauri` 或 capabilities）。**实现时先 Read `desktop/shell/capabilities/default.json` 确认 `core:default`/invoke 权限**；若未启用 global Tauri，改用 `import { invoke } from '@tauri-apps/api/core'` 不可行（静态页无打包），则需在 `tauri.conf.json` 设 `app.withGlobalTauri: true`（一并显式 add 该文件）。这是连接页能调命令的前提，务必验证。

- [ ] **Step 4: `daemon::build_panel_url` target 感知**

Read `daemon::build_panel_url`（grep 确认签名）。Local 档保持现有本机 URL 行为。新增 target 感知 helper（供 Task 6 导航 Remote 用），例如 `pub fn target_root_url(target: &ConnectionTarget) -> Option<url::Url>`：Local → 现有本机 root；Remote → `url.clone()`（webview 直接打远程 root，未鉴权自动跳 `/pair`）。不破坏 `build_panel_url` 既有 Local 调用。

- [ ] **Step 5: 编译验证**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p aleph-desktop-shell 2>&1 | tail -20
```
Expected: 整 crate 编译通过（含 invoke_handler 宏展开、命令签名）。Tauri 命令的运行时行为本步不验证（无 Tauri runtime）。

- [ ] **Step 6: fmt + clippy + 提交**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo fmt -p aleph-desktop-shell && cargo clippy -p aleph-desktop-shell 2>&1 | tail -15
git status
git add desktop/shell/src/connection.rs desktop/shell/src/main.rs desktop/shell/src/daemon.rs desktop/shell/splash/connect.html
git commit -m "desktop: connection-config Tauri commands + shell connect page"
git show --stat HEAD
```
若改了 `tauri.conf.json`（withGlobalTauri），一并显式 add。

---

### Task 6: 背景分流 + tray/menu wiring（集成）

**Files:**
- Modify: `desktop/shell/src/main.rs`（`spawn_background` :231-277、`supervise_daemon` :421-440、新增 `reroute_for_target`）
- Modify: `desktop/shell/src/tray.rs`（菜单项 + 分发）
- Modify: `desktop/shell/src/menu.rs`（macOS 菜单项 + 分发）

- [ ] **Step 1: `spawn_background` 按 target 分支**

Read `spawn_background`（:231-277）。在读 version 后 `let target = connection::load_target();`，按 target 分支：
- **Local**（`target.is_local()`）：**逐字节保留**现有 `reconcile_for_version` + `ensure_ready` + `reveal_panel` + `supervise_daemon(handle, daemon_up)`。另调 `external_link::set_remote_host(None)`（显式清空，幂等）。
- **Remote(url)**：跳过 reconcile/launch；`external_link::set_remote_host(Some(url.clone()))`；裸 TCP 可达性探测（`daemon::tcp_reachable(host, port).await` —— 见 Step 2；host=`url.host_str()`，port=`url.port_or_known_default().unwrap_or(18790)`，因 url crate 省略 scheme-default 端口）；导航 webview 到远程 root（`window.navigate(url)`，未鉴权自动跳 `/pair`）；启动 `supervise_daemon_remote`（或给 `supervise_daemon` 传 target + 用 `Supervisor::new_remote`）。notify/update 任务照常启动（notify 在 Task 3 已按 target 省 token）。

- [ ] **Step 2: 裸 TCP 可达性探测 helper（`daemon.rs`）**

新增（参数化 host/port，复用现有 `TcpStream` + `PROBE_TIMEOUT`）：
```rust
/// Bare TCP reachability for a remote Gateway — connect only, no HTTP/TLS.
/// True reachability + auth + TLS are the webview's job; the supervisor only
/// needs "is the port answering".
pub async fn tcp_reachable(host: &str, port: u16) -> bool {
    tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect((host, port)))
        .await
        .ok()
        .and_then(|r| r.ok())
        .is_some()
}
```

- [ ] **Step 3: `supervise_daemon` 消费 ShowConnectionError**

让监管循环按 target 用 `Supervisor::new`（Local）或 `Supervisor::new_remote`（Remote），探测函数 Local 用 `daemon::is_ready()`、Remote 用 `daemon::tcp_reachable(host,port)`。新增动作消费：
```rust
            SupervisorAction::ShowConnectionError => {
                tracing::warn!("remote Gateway unreachable — showing connection page");
                show_connection_page(&handle, "Remote Gateway unreachable. Retry or go back to local.");
            }
```
其中 `show_connection_page` 导航到 `tauri://localhost/connect.html` 并（可选）eval `window.__alephError(msg)`（复用 `show_daemon_error` 模式，但目标是 connect 页）。`Idle`/`Relaunch`/`ReloadPanel` 分支不变。

- [ ] **Step 4: `reroute_for_target`（替换 Task 5 的桩）**

```rust
/// Re-route the shell for a freshly-chosen target: update the link allow-list,
/// then navigate + (re)start supervision. Called by the connection commands.
pub(crate) fn reroute_for_target(app: &tauri::AppHandle, target: connection::ConnectionTarget) {
    match &target {
        connection::ConnectionTarget::Remote(url) => {
            external_link::set_remote_host(Some(url.clone()));
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.navigate(url.clone());
            }
        }
        connection::ConnectionTarget::Local => {
            external_link::set_remote_host(None);
            // bring the local daemon up and reveal the Panel
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                let version = handle.package_info().version.to_string();
                daemon::reconcile_for_version(&version).await;
                let _ = daemon::ensure_ready().await;
                reveal_panel(&handle);
            });
        }
    }
}
```
> 简化决策（spec 允许）：切换 target 后**重新导航**即可；常驻 supervisor 在下一 tick 按持久化的新 target 自适应（supervise 循环每轮 `connection::load_target()` 重读，或持有 target 句柄）。为避免双 supervisor，本计划让 `supervise_daemon` 每轮 `load_target()` 重读 target 决定探测方式与 Supervisor 模式重建——实现时确认循环结构，最简方案：循环顶部读 target，若与上轮不同则重置 `Supervisor`。

- [ ] **Step 5: tray 菜单项 + 分发（`tray.rs`）**

在 `build`（:13-30）menu items 加两项；在 `on_menu_event`（:36-52）加分发：
```rust
    // items
    let connect_remote = MenuItem::with_id(app, "connect_remote", "Connect to Remote…", true, None::<&str>)?;
    let connect_local = MenuItem::with_id(app, "connect_local", "Back to Local", true, None::<&str>)?;
    // include them in Menu::with_items(...) around the separator
    // event:
    "connect_remote" => {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.navigate(url::Url::parse("tauri://localhost/connect.html").unwrap());
            crate::focus_window(app);
        }
    }
    "connect_local" => { let _ = crate::connection::clear_connection_target(app.clone()); }
```

- [ ] **Step 6: macOS 菜单项 + 分发（`menu.rs`）**

在 app_menu（:28-60）加同样两项（带 `ID_CONNECT_REMOTE`/`ID_CONNECT_LOCAL` 常量），`on_event`（:139-160）加同样两分支（导航 connect 页 / 调 `clear_connection_target`）。

- [ ] **Step 7: 编译 + 全 shell 测试 + clippy**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p aleph-desktop-shell 2>&1 | tail -10
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p aleph-desktop-shell 2>&1 | tail -20
cd /Volumes/TBU4/Workspace/Aleph && cargo clippy -p aleph-desktop-shell 2>&1 | tail -10
```
Expected: 编译通过；Task 1-4 的所有单测全绿；clippy 无新警告。

- [ ] **Step 8: 提交（显式路径）**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git status
git add desktop/shell/src/main.rs desktop/shell/src/daemon.rs desktop/shell/src/tray.rs desktop/shell/src/menu.rs
git commit -m "desktop: route background+tray+menu by connection target (local/remote)"
git show --stat HEAD
```

---

## 最终验证（全任务完成后）

- [ ] `cargo check -p aleph-desktop-shell` 绿
- [ ] `cargo test -p aleph-desktop-shell` 全绿（ConnectionTarget 规范化/持久化、external_link 动态放行、notify Remote 省 token、Supervisor Remote 语义）
- [ ] `cargo clippy -p aleph-desktop-shell` 无新警告
- [ ] `git diff <base>..HEAD --stat` 只含 `desktop/` 下文件 + docs，**零 `src/` 改动**（R1 对账），无 `interfaces/dist` 产物
- [ ] 派 final code reviewer 审整体：①Local 档全路径逐字节零回归（首次无 marker=Local）②**本机 token 绝不发远程**（notify Remote connect 帧无 shared_token——已单测）③Remote 不启动/监管本机 daemon（spawn_background Remote 分支不调 ensure_ready/relaunch）④external_link 动态放行只放当前远程 origin、loopback 恒 internal、其它仍 external ⑤Remote 不可达→ShowConnectionError 导航 connect 页（不 relaunch）、恢复→ReloadPanel ⑥连接页能调命令（withGlobalTauri/capabilities 已验证）⑦invoke_handler 仅暴露 3 个连接配置命令（无业务逻辑，R2/R4 边界）
- [ ] **手动冒烟**（用户在真机/`just shell-dev`）：本机启动如常；tray「Connect to Remote…」→连接页→输入远程地址→webview 跳远程（未鉴权落 /pair）→远程批准落 chat 档；「Back to Local」回本机；远程不可达显示连接页+错误。

## 部署

纯 `desktop/`：`just shell-build` 打三平台安装包（或 `just shell-dev` 本地跑）。与 `src/` 的 Phase 3a/3b/browser 配对档位化正交，可独立或一起上线。
