# Review-Results 遗留修复设计（本轮 5 项 + 延后 2 项）

- **日期**: 2026-07-20
- **来源**: `review-results/` 2026-07-20 六模块静态评审的 critical/high + R3/R4 条目中，需更深架构改动或运行时验证的部分
- **环境**: 开发机为 Windows；本轮只落地并真机验证 Windows 可验证项，macOS/Linux 项设计先行、实现留待对应系统
- **范围锁定**（用户已确认三岔路）:
  - #6 uuid → **只改 `shared/protocol`**（不整包移除，alephcore 仍依赖）
  - #4 tui/cost + #5 cli routing → **两项都下沉 daemon**（shell 变纯 I/O）
  - 跨平台 → **先做 Windows 可验证的**，#2 macOS 与 #1 Linux 腿延后

## 背景：为什么这批单列

前一批 review 修复以“单文件补丁、逐模块单独 commit”落地。本批六项需要更深的架构改动（跨 crate 边界下沉、复用既有 SSOT）或运行时验证（平台 GUI），故单列。其中 **“uuid 整包移除”前提有误**：`uuid` 在 ~60 处生产代码使用（含 `tools/turn_budget.rs` 的 `TurnId(pub uuid::Uuid)` 类型字段、多处 serde 域类型线字段），声明在 5 个 crate。移除 `shared/protocol` 的用法**不能**让 crate 离开构建树——alephcore/cli/tui/ui_logic 仍依赖。故本轮收窄为“让 protocol crate 变瘦”，满足 R3 那一条。

## 精确定位

| # | 说法 | 定位 | 本轮 |
|---|------|------|------|
| 1 | CMD shell 注入 | `desktop/shared/src/action/open_path.rs:72` + `app_launch.rs:70`（`cmd /C start "" <target>` 走 cmd.exe） | ✅ |
| 2 | webview_perms Windows mic origin | `desktop/shell/src/webview_perms.rs:88`（麦克风授给任意 origin） | ✅ |
| 3 | tui/cost R4 | `interfaces/tui/src/tui/cost.rs` 自带 `PRICING_TABLE`；`commands.rs:241` 本地算价 | ✅ |
| 4 | cli routing R4 | `interfaces/cli/src/main.rs:583` marketplace-vs-direct 判据在 shell | ✅ |
| 5 | uuid R3 | `shared/protocol/src/jsonrpc.rs:303` + `auth.rs:154/184/207` | ✅ |
| 6 | screen_record region | `desktop/shared/src/perception/screen_record.rs:220`（**仅 macOS** `SCContentFilter` 抓全屏） | ⏸ macOS |
| 7 | webview_perms Linux 腿 | `desktop/shell/src/webview_perms.rs:53`（授全部 UserMedia 含摄像头、无 origin 校验） | ⏸ Linux |

---

## ① CMD shell 注入（Windows）— `open_path.rs` + `app_launch.rs`

**问题**：`cmd /C start "" <target>` 经 cmd.exe 解析，`&` `|` `^` `%VAR%` 可注入命令。

**方案**：改用 Win32 `ShellExecuteW`（verb `"open"`），字符串直接交给 shell 关联解析器，**不经 cmd.exe**，注入面消失。
- `open_path::open(target)` Windows 分支：`ShellExecuteW(HWND(0), w!("open"), <target 宽字符>, NULL, NULL, SW_SHOWNORMAL)`，文件/URL 按“双击”语义，等价原 `start`。
- `app_launch::launch_app` Windows 分支：同一模式，按 App Paths / PATH 解析应用名。
- 返回 `HINSTANCE`，`as isize <= 32` 视为失败 → 映射 `DesktopError::InputFailed`（对 `ERROR_FILE_NOT_FOUND` 等给清晰消息）。

**依据 R1**：`desktop/shared` 是四肢平台层，`app_launch.rs` 已直接用 windows-rs（`windows_quit_app`），ShellExecuteW 与既有代码同源。

**风险**：`windows` crate 需 `Win32_UI_Shell` feature（可能要在 `desktop/shared/Cargo.toml` 补，编译时确认）。

**验证（Windows 真机）**：`open("https://x?a=1&b=2")`、`open("C:\含空格 文件.pdf")`、`launch_app("notepad")` 冒烟；含 `&` 的 target 确认不再执行注入命令。保留既有 `rejects_empty_target`。

---

## ② webview_perms Windows 麦克风 origin 门

**问题**：`grant_windows`（`:88`）对任意 origin 的麦克风请求 `SetState(ALLOW)`。

**方案**：命中 `MICROPHONE` 后取 `args.Uri()` → 解析 `Url` → 用 **`crate::external_link::is_internal(&url)`** 判定，仅 Panel origin（loopback / `tauri.localhost` / 已配置 remote）才 `ALLOW`，否则留默认。

**依据**：`external_link::is_internal` 已是“是否 Panel 面”的 SSOT，同 crate 直接复用（DRY，P2/P5），不新造 origin 名单。

**风险/验证**：webview 本被 `external_link::route` 钉在 Panel origin，此门为纵深防御。Windows 真机：Panel 语音按钮仍能取麦克风（无回归）即通过。

---

## ③ tui/cost R4 下沉 daemon

**问题**：`cost.rs` 自带 `PRICING_TABLE` + `estimate_cost`，`commands.rs:241` 在 shell 本地算价——违 R4，且与 core `src/pricing.rs`（更全）重复。

**方案**：算价搬进 daemon。
- `session.usage`（`handle_usage_db`）返回 token 计数的同时，用 `crate::pricing::estimate(provider, model, breakdown)` 算 `cost_usd: Option<f64>` + `cost_status`，加进响应。provider/model 优先由 session 记录解析（实现时读 `handle_usage_db` 确认可得；不可得则由 TUI 传 `model_name` 参数，算价逻辑仍在服务端，R4 违规仍消除）。
- TUI：`UsageReply` 加 cost 字段，`format_usage` 直接渲染，**整删 `cost.rs`**（表 + `estimate_cost` + 测试）。

**收益**：R4 消除 + 去重；TUI 顺带获得缓存/长上下文分档/更多厂商，去掉 core 没有的过期 `t8star` 条目。

**验证**：`cargo check` core + tui；`session.usage` 响应含 cost；`/usage` 渲染正确。

---

## ④ cli 路由 heuristic R4 下沉 daemon

**问题**：`main.rs:583` 的 `!contains('/')&&!contains('.')&&!contains(':')` 在 shell 判 marketplace vs 直装——违 R4。

**方案**：新增统一 daemon 方法 **`plugin.install { source, scope }`**，把分类判据原样搬进服务端：bare-name → marketplace 安装；git-url → clone 安装。CLI 删 `looks_like_marketplace`，非本地文件安装一律转发 `plugin.install`。
- **边界（防蔓延）**：本地 `.zip`（读本地文件 → base64 → `plugins.installFromZip`）与 `github:`（CLI 取 release）属 CLI 合法 I/O，保持现状；只移动被点名的 name-vs-url 判据。既有 `plugin.marketplace.install`/`plugins.install` 保留（后向兼容）。

**验证**：`cargo check` core + cli；服务端分类加单测（name→marketplace、url→clone）；`plugin install <name>` / `<git-url>` 走通。

---

## ⑤ uuid 收窄至 `shared/protocol`

**问题**：`shared/protocol` 为生成线 ID 拉入 `uuid`（v4→rand）——违 R3。

**方案**：4 个调用点（`jsonrpc.rs:303` + `auth.rs:154/184/207`）改用进程级 `AtomicU64` 计数器生成字符串 ID（如 `req-<n>`）。JSON-RPC id 仅需进程内相关性唯一；`IdentityContext.request_id` 是审计相关性 id 非密钥，计数器安全（KISS）。加 crate 内 `next_id()` 小助手供两文件共用。删除 `shared/protocol/Cargo.toml` 的 `uuid`。

**验证**：`cargo check -p aleph-protocol`；既有 `test_request_creation`（断言 `id.is_some()`）仍绿；已核实 crate 内仅此 4 处 uuid。

---

## 延后 2 项（设计先行）

**⑥ screen_record region（macOS-only）**：`SCContentFilter` 抓全屏、忽略 `config.region`。方案：`SCStreamConfiguration::setSourceRect(CGRect)` 从 `region` 裁剪，`width/height` 设为 region 尺寸×scale。Linux/Windows 已正确处理 region。留待 macOS 实现+真机验证。

**⑦ webview_perms Linux 腿**：`grant_linux` 授全部 UserMedia（含摄像头）、无 origin 校验。方案：仅当 `is_for_audio_device() && !is_for_video_device()` 且 `webview.uri()` 经 `is_internal` 校验时 `allow()`。Linux-only，在 Windows 上连编译都排除。**代码亦留待 Linux 机器**（避免落地本机无法编译校验的代码；代价是 Linux 侧暂留“授摄像头”缺口）。

---

## 落地与验证策略

- 沿用 review 批次约定：本批各项作为 `main` 上独立 commit（单分支开发模式），不逐项 `cargo check`。
- 全部改完后一次编译校验：`cargo check -p alephcore`（core 侧 #3/#4/#5 的服务端）+ 相关 crate（`desktop/shared`、`desktop/shell`、cli、tui、protocol）。
- Windows 真机冒烟：#1（注入闭合）、#2（语音按钮无回归）。
- 单测：#4 服务端 cost 字段、#5 服务端分类、#5 CLI 转发、#6 protocol id 生成。

## 未决小点

⑦ Linux 腿代码是否现在盲写：倾向**留到 Linux 机器**（与“Windows 可验证优先”一致，不落地无法编译校验的代码）。
