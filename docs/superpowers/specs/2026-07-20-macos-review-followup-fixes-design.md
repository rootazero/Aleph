# macOS Review-Follow-Up 修复设计（4 组：region 裁剪 + 录制完成校验 + 真·窗口定位 + 桥错误类型保真）

- **日期**: 2026-07-20
- **来源**: `review-results/desktop.md` 2026-07-20 静态评审中的 **macOS-specific** critical/high/medium 条目
- **环境**: 本轮开发机为 **macOS (Darwin 27)**，解锁了前一批（`2026-07-20-review-followup-fixes-design.md`）§延后 的 macOS 项 #6，并借机把 `desktop.md` 里其余仍存活的 macOS 缺陷一并做透。**#7（Linux webview 腿）仍延后**，Linux 上才能编译校验。
- **承接**: 前一批（Windows/core，`70446bebe`）已落地 #1–#5；本批是其 macOS 侧的彻底跟进。

## 背景：先做减法（triage）

`desktop.md` 点名 7 项 macOS 相关缺陷。**逐一核对当前源码后，3 项已被后续提交修复**（评审快照早于修复）：

| 评审条目 | 现状 |
|----------|------|
| `media_types.rs:47/104` camera/audio duration NaN → panic | ✅ 已修（`clamped()` 已含 `is_finite()` 守卫 + 注释） |
| `perm_monitor.rs:126` `aleph-bridge` vs `AlephBridge` 助手命名 | ✅ 已修（先找 `AlephBridge`，再退 `aleph-bridge`，再 PATH） |

**仍存活的 4 项**构成本批，按"清晰、可运行时验证"排序分为 4 组：

| 组 | 评审条目 | 定位 | 严重度 | crate |
|----|----------|------|--------|-------|
| A | region 被忽略、录全屏（PII） | `perception/screen_record.rs:220`（`sc_recording_output_record`，仅 macOS）| HIGH | `aleph-desktop` |
| B | 忽略 `didFinishRecording` 超时、未验产物即报成功 | `perception/screen_record.rs:371` | MEDIUM | `aleph-desktop` |
| C | `focus_window` 只激活 App；move/resize 按标题匹配窗口 | `action/window.rs:506` / `:565`（`macos_focus_window` / `macos_set_window_bounds`）| MEDIUM×2 | `aleph-desktop` |
| D | media 转发把已定型错误压平成通用 `BridgeFailed` | `desktop/macos/src/lib.rs:225`（`bridge_err`）| MEDIUM | `aleph-desktop-macos` |

---

## ① A · #6 region 裁剪（macOS SCK 录制）

**问题**：`sc_recording_output_record` 用 `SCContentFilter::initWithDisplay_excludingWindows` 抓整块 display，并 `setWidth(display_width * 2)` / `setHeight(display_height * 2)`——**从不读 `config.region`**。请求子区域时静默录全屏（PII 泄露）。Linux（x11grab）/ Windows（gdigrab）已正确按 region 裁剪。

**方案**：在 `stream_config` 配置段按 region 分支：
- `region == None`：维持现状（全屏，`display_points × scale`）。
- `region == Some(r)`：`stream_config.setSourceRect(CGRect)` 从 region 裁剪，输出 `setWidth/Height` 设为 **region 尺寸 × scale**。
- 依赖：`SCStreamConfiguration::setSourceRect(CGRect)`（已核实存在于 `objc2-screen-capture-kit 0.3.2`）。`CGRect/CGPoint/CGSize` **必须**取自 `objc2-core-foundation 0.3.2`——objc2 SCK 方法签名要求带 `Encode` 的 objc2 版 `CGRect`，`core-graphics 0.25` 的同名类型是**不同类型、不通用**。仅需在 `desktop/shared/Cargo.toml` 的 macos target 段加一行 `objc2-core-foundation = { version = "0.3", features = ["objc2"] }`（已在 Cargo.lock 树中经 SCK 传递引入——**非新增重依赖，R3 合规**）。此依赖**仅 A（`screen_record.rs`）用**。

**坐标空间约定（关键 · Task 1 评审修正）**：`sourceRect` 在 display 的 **point** 空间（左上原点），但 `ScreenRegion{x,y,w,h: u32}` 是**物理像素**（`lib.rs:58` 明确 "in physical pixels"；`coord_resolve.rs::resolve_viewport` 产出 `dim × scale`；Linux/Windows 同胞录制器 `build_x11grab_args`/`build_gdigrab_args` 把 region 原样当像素喂 ffmpeg）。故 SCK 裁剪需换算：`sourceRect(points) = region(pixels) ÷ scale`，输出 `setWidth/Height(pixels) = region(pixels)` **原值**（不再 ×scale）。全屏分支仍为 `display_points × scale`；一个等于全屏的 region 由此复现全屏输出（正确性锚点）。**早稿曾误写 "region 采用 points 语义 = sourceRect 原值"——已按评审改正。此约定由运行时录制验证兜底**（录已知 region，核对输出像素尺寸 == region 且内容为裁剪区）。

**纯函数 + clamp（可单测）**：抽一个 `#[cfg(any(target_os="macos", test))]` 纯函数（仿 `build_x11grab_args` 的可测模式），入参 `(region, display_w_pt, display_h_pt)`，出参"裁剪到 display 边界后的 rect（points, f64）+ 输出像素 `(w,h): usize`"；region 与 display 交集为空 → 返回错误（映射 `DesktopError::ScreenCapture`）。macOS 代码把纯函数结果转成 `CGRect` 喂 `setSourceRect`。
- 单测：region 全在界内（原样）、越界（裁到边界）、完全在界外（Err）、零尺寸（Err）。

**验证**：`cargo check -p aleph-desktop`；纯函数单测绿；**真机录制**一个已知 region，`ffprobe`/播放核对裁剪尺寸与内容。

---

## ② B · 录制完成校验（SCRecordingOutput 路径）

**问题**：`sc_recording_output_record` 第 12 步 `cvar.wait_timeout_while(..., 15s, ...)` 把返回值绑成 `let _result` 丢弃——**不判是否超时**；随后只查 `error_slot`，若无错即返回 `Ok`。于是"等待超时（delegate 从未回调）且 error_slot 为空"时，会在**产物可能缺失/不完整**的情况下谎报成功。CLI（`screencapture`）与 ffmpeg 两条路径都已 `output_path.exists()` 校验，唯独此路径没有。

**方案**：捕获 `WaitTimeoutResult`，`timed_out()` 为真 → `Err(ScreenCapture("recording did not signal completion within 15s"))`；否则再校验 `output_path` 存在且**非零字节**，缺失/空 → `Err`。两道都廉价、就地，且与另两条录制路径的 exists 校验对齐。

**验证**：`cargo check -p aleph-desktop`；真机正常短录制仍返回 `Ok`（产物存在）。

---

## ③ C · 真·按窗口定位（focus / move / resize）

**问题**：
- `macos_focus_window`（:506）由 `window_id` 查到 PID 后 `NSRunningApplication.activateWithOptions`——**只激活 App**，多窗口时最前的不一定是目标窗口。
- `macos_set_window_bounds`（:565）用 osascript/System Events **按标题** `name of w is t` 匹配窗口——**标题重复**时改错窗口，无标题退 `window 1`。

**根因**：macOS 无干净公开 API 从 `CGWindowID` 直达具体 AX 窗口。`window_list` 用 `CGWindowListCopyWindowInfo`，`WindowInfo.id` 即 `kCGWindowNumber`（真 CGWindowID），且 `WindowInfo.bounds` 已带该窗口的全局 bounds（`kCGWindowBounds`）。

**方案（公开几何匹配，无私有 API）**：在 `window.rs` 加一个**进程内 AX 解析器**（`#[link(name="ApplicationServices", kind="framework")]` + 一小段 `extern "C"`，**仅公开符号**；`#[link` 框架 FFI 已有先例：`desktop/macos/src/sleep_inhibitor.rs` 链 IOKit）：
1. `AXUIElementCreateApplication(pid)` 拿 App 元素；`AXUIElementCopyAttributeValue(app, kAXWindowsAttribute)` 取窗口 CFArray。
2. 逐窗读 `kAXPositionAttribute`/`kAXSizeAttribute`（`AXValueGetValue` → `CGPoint`/`CGSize`），与目标 CGWindowID 的 `WindowInfo.bounds` **按 (pos,size) 容差匹配**（AX 与 CGWindowList 均为**左上原点全局 points**，直接对齐；容差 ≈1pt 吸收取整）。
3. 命中：
   - **focus**：`AXUIElementPerformAction(win, kAXRaiseAction)` + 保留 `NSRunningApplication.activate`（把 App 带前 + 把该窗口抬到 App 内最前）。
   - **bounds**：`AXUIElementSetAttributeValue(win, kAXPositionAttribute/kAXSizeAttribute, AXValueCreate(...))`。
4. **降级**（解析器无命中 / AX 权限缺失 / 符号异常）：回到**现状行为**——focus 退"只激活 App"，bounds 退现有 osascript-by-title。降级路径原样保留，几何匹配是叠加的精确层。

**CF 处理**：**复用 `window.rs` 已在用的 `core-foundation 0.10`**（`CFString`/`CFArray`/`CFType`/`TCFType`，`macos_window_list` 已用其解 `CFDictionary`）+ `core-graphics 0.25`（`CGPoint`/`CGSize` 喂 `AXValueCreate`）——两者都已是 `desktop/shared` 的 macos target 依赖，**C 零新依赖**。`AXUIElement*`/`AXValue*` 函数与 `kAX*` `CFStringRef` 常量、`AXValueType`（`kAXValueCGPointType=1`/`kAXValueCGSizeType=2`）走 `extern "C"`（无 objc2 AX 绑定 crate，raw extern 是限肢层标准做法）。注意 C 用 `core-foundation`/`core-graphics` 生态（与 window.rs 一致），A 用 `objc2-core-foundation` 生态（SCK 要求），两者不混用。

**权限说明**：精确路径需 Accessibility(TCC) 权限（现状 osascript 路径本就需要）。focus 的**降级**（激活 App）无需 AX 权限，故权限缺失不会让 focus 比现状更差。AX 调用返回 `kAXErrorAPIDisabled`/`NotAuthorized` → 走降级。

**依据 R1**：`desktop/*` 是平台限肢层，`window.rs` 已直接用 `objc2_app_kit`；AX FFI 与既有代码同源。**非 R2**（无业务 UI）。

**验证**：`cargo check -p aleph-desktop`；几何匹配纯函数（bounds 容差相等）单测；**真机**：开同一 App 的两个窗口（如 TextEdit），按 id `focus`/`move` 其一，核对**正确**窗口被抬起/移动（而非最前那个或同标题那个）。

---

## ④ D · 桥错误类型保真（media 转发）

**问题**：桥协议**已定型错误**——`bridge/client.rs::map_bridge_error` 在 reader 任务里把 `ERR_PERMISSION_DENIED → PermissionDenied{kind,guide}`、`ERR_TIMEOUT → BridgeTimeout`、`ERR_PLATFORM → PlatformError` 等映射好，`bridge.call()` **已返回正确定型的 `DesktopError`**。但 `desktop/macos/src/lib.rs` 的 media 方法 `.map_err(|e| bridge_err(&format!("... RPC: {e}")))` **把已定型错误重新压平**回 `BridgeFailed(String)`，丢掉 `PermissionDenied` 携带的 `guide`（LLM 需要它给用户深链/步骤）与 `BridgeTimeout` 语义。这是**纯 Rust 侧**缺陷，与 Swift/协议无关。

**方案**：加一个小助手，**定型变体原样穿透，仅给不透明的 `BridgeFailed` 补方法上下文 + 轨道类别**：
```rust
fn preserve_typed(method: &str, e: DesktopError, wrap: impl Fn(String) -> DesktopError) -> DesktopError {
    match e {
        DesktopError::BridgeFailed(m) => wrap(format!("{method}: {m}")), // 仅不透明失败被包裹
        other => other, // PermissionDenied / BridgeTimeout / BridgeBackoff / ... 穿透
    }
}
```
- media（`lib.rs`）：`wrap = DesktopError::BridgeFailed`。**彻底版**同款施于并行的压平点——`screen.rs::call_input`（`wrap = InputFailed`）、`screen.rs::screenshot_via_bridge`（`wrap = ScreenCapture`）、`pim.rs::call_pim`（其 context 变体）——既保 recovery 语义，又保各轨道类别。
- 净删除 `lib.rs` 里的 `bridge_err`（被 `preserve_typed` 取代）。

**验证**：`cargo check -p aleph-desktop-macos`；单测：喂 `PermissionDenied{..}`/`BridgeTimeout` 断言变体穿透不变；喂 `BridgeFailed("x")` 断言得到 `wrap("<method>: x")`。

---

## 坐标空间附录

| 空间 | 原点 | 单位 | 用处 |
|------|------|------|------|
| SCK `sourceRect` / `SCDisplay.width` | 左上 | points | A 裁剪矩形 |
| SCK `setWidth/Height` | — | pixels | A 输出尺寸（= points × scale） |
| AX `kAXPosition/kAXSize` | 左上（全局） | points | C 读/写窗口几何 |
| CGWindowList `kCGWindowBounds` | 左上（全局） | points | C 匹配基准（`WindowInfo.bounds`） |

A 的 `ScreenRegion` 是**物理像素**（非 points）：裁剪矩形 `sourceRect(points) = region ÷ scale`，输出尺寸 = region 像素原值。C 的 AX/CG 都在 points 左上空间，彼此自洽（C 不涉及 region，不受此修正影响）。沿用现有 `scale=2` 假设（region 与全屏分支同用同一 scale，内部一致；非 Retina 实机 scale≠2 的准确性为既有局限，由运行时验证兜底，超出本轮范围）。

## 落地与验证策略

- 单分支开发（直接 `main`），每组独立 commit：`A`（region）、`B`（完成校验，可与 A 合成一 commit）、`C`（窗口定位）、`D`（错误保真）。
- Cargo（项目 CLAUDE.md 节制原则）：组 A/B/C 收尾各一次 `cargo check -p aleph-desktop`；组 D 一次 `cargo check -p aleph-desktop-macos`；纯函数/映射单测按组 `cargo test -p <crate> <filter> --lib`。
- 真机验证（本 macOS）：A 录 region 核对裁剪；B 正常录制仍 Ok；C 双窗口定位正确窗口；D 由单测覆盖（权限/超时错误难可靠触发）。
- **不触碰**工作树里与本任务无关的既有改动（如有）。

## 红线合规

- **R1**：A/B/C 的平台 FFI 全在 `desktop/*` 限肢层，与既有 `objc2`/`#[link]` 同源；无业务 UI（非 R2）。
- **R3**：A 仅引入 `objc2-core-foundation`（已在 lock 树的轻量 geometry 类型，A 专用）；C 复用现有 `core-foundation`/`core-graphics` + raw `extern "C"`，零新依赖。无新重依赖。
- **R7/P8**：D 不引入规则引擎；C 的匹配是几何等值判定，非语义匹配。
- 无 `src/harness/` 触碰（R10 无关）。

## 范围外 / 延后

- **#7 Linux webview 音频-only + origin 门**：Linux-only，本机不能编译校验，**继续延后**（见前批 spec §延后 ⑦）。
- **C 私有 API 精确升级**：`_AXUIElementGetWindow`（yabai/Hammerspoon 同款）可给出更精确的 CGWindowID↔AX 映射，但依赖未公开符号、有随 macOS 失效风险；本批**用公开几何匹配**，私有 API 仅作未来可选升级记录，不落地。
- **D 跨轨道协议级重构**：桥协议已带错误码，无需 Swift/协议改动；本批止于 Rust 侧保真。
