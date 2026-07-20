# 连接形态由「构建」决定,面板连接只读化

**Date**: 2026-06-22
**Status**: Approved (design)
**Scope**: desktop shell (Rust) + webchat panel (WASM) + i18n
**Redlines touched**: R2/R4(I/O 边界,只删 UI 不加业务)、R6(一核多端)、R10(薄壳,删死代码)

---

## 1. 背景与动机 (Why)

### 1.1 触发问题

完整版桌面 App 的 Settings ▸ 服务连接 显示「当前在浏览器中运行,连接切换仅在桌面 App 内可用」,与「用户明明是通过桌面 App 打开」矛盾。

根因(已用 `strings` + mtime 证实,**非代码 bug**):

- `data-shell-variant` 标记由 **Tauri 壳二进制** `aleph-desktop-shell` 注入,功能在提交 `19f38a993`(2026-06-22 18:46)才落地。
- 用户运行的 `Aleph.app` 中:`aleph-server`(内嵌当前 panel WASM)是 06-22 19:59 重编的(panel 新),但 `aleph-desktop-shell` 仍是 06-19 01:38 的旧二进制(`data-shell-variant` 出现 0 次)。
- 新 panel 读不到标记 → `shell_variant()` 返回 `None` → 落到 `Browser` 分支 → 误报。

即:**热替换了 server,漏重编了壳**。壳是独立构建产物,这是一整类「panel/server 与壳不同步」的脆弱点。

### 1.2 设计意图(用户决策)

把「连哪个核心」从**用户在面板里可选**,改成**由构建形态隐式决定**,并让本地/远程的判定来自永远新鲜的 `location.host`,从而:

1. 面板不再承载连接切换(R4:Interface 纯 I/O,连接是壳/地址栏的事,不是面板业务)。
2. 完整版**只能连本地**,纯壳**只能连远程**,语义清晰,符合 PRODUCT_TOPOLOGY(完整 App 单机零配置 / 纯壳瘦客户端)。
3. **弃用 `data-shell-variant` 标记** → 从根上消灭「壳没重编」这类不同步 bug。

---

## 2. 目标模型 (What)

| 形态 | 连哪个核心 | 连接方式 | 面板「服务连接」段 |
|---|---|---|---|
| 完整版 App(`embedded-core`) | **恒本地** loopback | 内嵌 server,无从选 | 只读指示 `已连接 127.0.0.1:18790 [本地]` |
| 纯壳 Panel(lite) | **恒远程** | 壳自己的 connect 页(输 IP / mDNS)或托盘 | 只读指示 `已连接 <ip> [远程]` |
| 浏览器 | 取决于地址栏 | 用户在地址栏输 `http://<ip>:18790` | 只读指示 `已连接 <host> [本地/远程]` |

**单一真相源**:本地/远程一律由 `location.host` 经 `is_loopback_host()` 判定。完整版恒 loopback、纯壳恒远程,所以判定永远准确,**无需任何壳注入标记**。

---

## 3. 改动清单 (How)

### 3.1 Panel(用户可见主体)

**A. `interfaces/webchat/src/views/settings/network/connection.rs`(重写为只读)**

- 删除:`SwitchableConnection` 组件、`ConnectionMode` enum、`ShellVariant` enum、`decide_mode()`、`shell_variant()`、`resolve_apply_target()`、对应单测。
- 保留并简化 `ConnectionSection`:标题 + 描述 + **那行只读连接指示**(`host` / `remote` / `host_present` 来自 `current_host()` + `is_loopback_host()`,徽章 `badge_local`/`badge_remote`)。
- 删除 `{body}`(Switchable/RemoteReadOnly/Browser 三分支)。
- 保留纯函数 `current_host()`、`host_only()`、`is_loopback_host()` + 其单测。
- 更新文件头注释:删去关于 `data-shell-variant` / IPC 切换的描述。

**B. `interfaces/webchat/src/components/connection_status.rs`(dashboard 连接 chip)**

- `resolve_target_label()` 简化为**纯 `location.host`** 路径:删掉 `is_shell()` + `get_connection_target()` IPC 分支,统一走「origin 即核心」逻辑(loopback → `Local`,否则 host)。
- 保留 `host_of()`/`is_loopback_host()` 及其单测(仍被 location.host 路径需要)。

**C. `interfaces/webchat/src/api/tauri_bridge.rs`(清孤儿)**

- 删除变孤儿的 `get_connection_target()`、`set_connection_target()`、`normalize_endpoint_preview()` + 其单测。
- 核对 `is_shell()` 是否仍有其它消费者:若已无人用则一并删,有则保留。

**D. i18n `locales/zh.json` + `locales/en.json`**

- 删死键:`local_service`、`remote_service`、`remote_readonly_lite`、`remote_readonly_full`、`remote_readonly_hint`、`browser_only`、`apply`、`preview`。
- 保留:`section_title`、`description`、`connected_label`、`badge_local`、`badge_remote`。
- 两语言文件键集合保持一致。

### 3.2 Shell(让完整版「真的只能本地」+ 弃标记)

**E. `desktop/shell/src/main.rs`**

- 删除 `SHELL_VARIANT_JS` 常量(两个 cfg 分支)+ `build_main_window` 里那行 `.initialization_script(SHELL_VARIANT_JS)`。
- **保留** `SHELL_MARKER_JS`(`data-shell`/`data-platform`,别处仍在用,如 `is_shell()` / 平台 CSS)。
- `bring_target_online`(`#[cfg(feature = "embedded-core")]`):collapse 成只走原 `Local` 臂逻辑,删除 `Remote` 臂;清理因此变孤儿的 `show_connection_page` 调用(若 embedded-core 路径下不再被引用,删该函数或 cfg 限定到 lite)。
- invoke_handler 的 `#[cfg(feature = "embedded-core")]` 臂:移除 `set_connection_target`、`get_connection_target`、`clear_connection_target` 注册;**保留** `is_lite_shell`。

**F. `desktop/shell/src/connection.rs`**

- `load_target()`:在 `#[cfg(feature = "embedded-core")]` 下**恒返回 `ConnectionTarget::Local`**(cfg 门控函数体,不读 marker 文件)。这是「完整版无法变远程」的**单一真相源** —— 菜单「重载 / 在浏览器打开」(menu.rs 经 `load_target()`)随之自动只走本地,无视任何遗留 marker。
- `set_connection_target` / `get_connection_target` / `clear_connection_target` 的 Rust 函数体**保留**(lite handler + connect.html + menu.rs `clear_connection_target` 仍在用);仅完整版不再经 IPC 暴露 set/get/clear(见 E)。
- `ConnectionTarget` / `parse` / `save_target` / `marker_exists` 保留(纯壳依赖)。

### 3.3 明确不动

- `desktop/shell/src/connect_setup.rs`、`desktop/shell/splash/connect.html`、托盘 `tray.rs` —— 纯壳远程连接全靠它们。
- `desktop/shell/src/menu.rs` —— 靠 F 的 `load_target()→Local` 自动正确,不改。
- `connect.html` 的 `!liteMode`(full-mode)死分支 —— 完整版不再加载 connect.html,留着无害,本次**不清**(可选后续)。
- `interfaces/webchat/src/views/settings/network/cluster.rs`(集群联邦)—— 与本次无关。

---

## 4. 验证 (Verify)

- 编译:`cargo check -p aleph-panel --target wasm32-unknown-unknown --lib`(panel)、`cargo check -p aleph-desktop-shell`(壳两 variant:默认 + `--no-default-features` 或对应 lite cfg)、`cargo check -p alephcore --lib`(若涉及)。极度节制 cargo:每面至多一次。
- 单测:panel 中 `is_loopback_host`/`host_only`/`host_of` 的既有/新增断言;`connection.rs` 新增「loopback→本地、非 loopback→远程」纯函数断言。
- 行为(部署后人工):
  - 完整版 App:服务连接段显示 `127.0.0.1:18790 [本地]`,无切换控件。
  - 纯壳 Panel:显示 `<远程 ip> [远程]`,无切换控件;首连仍走 connect.html。
  - 浏览器访问远程 origin:显示该 host `[远程]`。
- 回归:确认完整版菜单「重载 / 在浏览器打开」走 loopback;确认旧 marker 文件存在时完整版仍恒本地。

## 5. 部署 (Deploy)

- Panel 改:`just wasm` → 重编 `aleph-server`(rust_embed 编译期嵌入)。
- Shell 改:重编 `aleph-desktop-shell` —— 完整版 `just shell-build`、纯壳 `just shell-build-lite`。
- **两个 .app 都需重建重装** —— 本次 bug 正源于漏重编壳。

## 6. 风险与取舍

- **孤儿清理可能触发连锁编译错**(删 enum/函数后残留 import/引用)—— 实施时逐个清,以 rust-analyzer / `cargo check` 诊断为准。
- **`show_connection_page` 归属**:确认它仅在 embedded-core 的 Remote 臂被调用;若 lite 也调用则只 cfg-gate 不删。
- **`is_shell()` 去留**:取决于是否还有别的消费者,实施时 grep 定夺。
- 不引入新依赖、不碰 async runtime、不碰平台 API(守技术栈禁用清单)。
