# browser_record 会话录制（C2）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新工具 `browser_record`（start/stop/status）：cdp 后端经 CDP `Page.startScreencast` 帧流 → ffmpeg image2pipe 实时编码落盘 → stop 返回 verified 回执。

**Architecture:** aleph-cdp 手写 screencast 三方法 + 事件类型（fetch.rs 先例）；`cdp_backend/recording.rs` 新建 `RecordingRegistry`（ProfileManager 持有、键 `(profile, tab_id)`、CdpBackend 持 Arc——RouteRegistry 先例）；帧 → bounded channel → ffmpeg stdin，**ack pacing 背压**（写入成功才 `screencastFrameAck`）；verified stop = 退出码 + 文件存在 + size>0 + mtime 新鲜。

**Tech Stack:** Rust / tokio / aleph-cdp / 系统 ffmpeg（`/usr/bin/ffmpeg` n9.0.1 已在；playwright 自带 `~/.cache/ms-playwright/ffmpeg-1011/ffmpeg-linux` 兜底）/ FakeCdpServer（`push_event` 已有）。

**Spec:** `docs/superpowers/specs/2026-09-27-browser-recording-design.md`

## Global Constraints

- **唯一 CDP 真源**：screencast 只进 `crates/aleph-cdp/src/methods/screencast.rs` 手写包装；禁第二份 CDP 客户端。
- **ack pacing 方向**：先写入 ffmpeg stdin（或 bounded channel），成功后才发 `screencastFrameAck`。反过来（先 ack 后写）在编码慢时让帧在内存堆积——16GB 内存红线机器上是 OOM 种子。
- **wire 帧是 base64**：解码只发生在 `screencast.rs` 的帧事件 decode 内，下游只见 `Vec<u8>`（fetch.rs fulfill 的同构纪律）。
- **verified stop**：「文件在 ≠ 写完」。回执 `complete=true` 当且仅当：ffmpeg 退出码 0 + 文件存在 + size>0 + mtime 在录制窗口内。
- **订阅先于 enable**：EventStream 必须在 `Page.startScreencast` 之前订阅（events.rs 模块 doc；C1 InterceptLoop 同款纪律）。
- **backend.rs census**：`fake_backend_implements_every_backend_method` 按 `async fn ` 字面量扫描 backend.rs 全文——**backend.rs 内禁任何 async 测试函数，注释里也不许出现 `` `async fn ` `` 字面量**（C1 T3 踩过两次）；默认臂测试放 `playwright_cli_backend.rs`。
- **新工具注册七处**（以 `ln`/BrowserDialogTool 为样板全数照抄）：`browser_tools/mod.rs` 导出、`definitions.rs`（BUILTIN_TOOL_DEFINITIONS + census 断言 + session-less fall-through 列表）、`groups.rs`、`builder/constructor/mod.rs`（构造+注册+definition）、`registry/struct_def.rs`、`registry/tool_registry_impl.rs`（dispatch 臂）。
- **能力台账**：恢复 screencast 行须同步三处——`no_capability_row_exists_without_a_verb_that_dispatches_it`（capability.rs:585-610，把 screencast 从裁清单摘除）、`CAP_FIELDS` 加行、`EngineCapabilities` 加字段；doc comment 点名 `browser_record` 三 action。
- **R9 字节纪律**：DESCRIPTION ≤80 字符；ceiling 棘轮（现 116,114 B）超限时走三问流程记账（C1 先例）。
- **内存守卫**：每次 cargo 前 `awk '/MemAvailable/{exit ($2<4194304)?1:0}' /proc/meminfo`；`CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1`。
- **TDD + 每条守卫先证伪**（变异→红→恢复）；英文 commit `<scope>: <description>`。
- 已知既存红 7 条 + pty flaky（memory 有清单），不算回归。注意：**main 在 C1 后有其他提交**（`browser_dialog` 已改名 `ln` 等）——以 worktree 实码为准。
- 工作区：`/home/zou/data/workspace/Aleph-browser-r2`，分支 `browser-native-r2`（已 rebase 到最新 main）。禁碰其他检出。

## Review Focus

1. **ack pacing 方向搞反**：先 ack 后写 → 编码慢时帧在内存无界堆积。期望：channel 满 → dropped_frames++，内存有界，页面不卡。→ T2 Step 6 钉（慢 ffmpeg 桩）。
2. **tab 录制途中死亡**：事件流随 session 断 → 自动收尾出**截断回执**（`complete=false`），文件是诚实的前半截，绝不标 complete。→ T2 Step 7。
3. **ffmpeg 退出码非 0 但文件存在非空**（部分写入/未知扩展名）：回执 `complete=false`，不拿文件存在当成功。→ T2 Step 8。
4. **start 路径不可写**：fail-fast 用创建临时文件**实测**，不靠权限位猜测（root 写 /proc、只读挂载等边角）。→ T3 Step 3。
5. **base64 漏解码**：screencast 帧原样喂 ffmpeg → 产出 0 帧视频且 ffmpeg 可能退出码 0（垃圾输入不总是报错）——静默完整谎言。→ T1 Step 1 钉 wire 解码 + T2 Step 5 钉 `captured==encoded` 对账与 ffprobe 可解。

---

### Task 1: aleph-cdp screencast 域包装

**Files:**
- Create: `crates/aleph-cdp/src/methods/screencast.rs`
- Modify: `crates/aleph-cdp/src/methods/mod.rs`（挂 `pub mod screencast;`）
- Test: `crates/aleph-cdp/tests/methods.rs`

**Interfaces:**
- Consumes: `CdpConnection::call`、`super::{decode, field}`、`decode_base64` 模式（page.rs:160 `capture_screenshot` 先例——`Page.captureScreenshot` 回包 base64 解码成 `Vec<u8>` 的既有形状）
- Produces（T2 消费这些精确签名）:
  - `pub enum ScreencastFormat { Jpeg { quality: u8 }, Png }`
  - `pub async fn start_screencast(conn: &CdpConnection, session: Option<&SessionId>, format: ScreencastFormat, max_width: Option<u32>, max_height: Option<u32>, every_nth_frame: Option<u32>) -> Result<()>`
  - `pub async fn screencast_frame_ack(conn: &CdpConnection, session: Option<&SessionId>, session_id: u64) -> Result<()>`
  - `pub async fn stop_screencast(conn: &CdpConnection, session: Option<&SessionId>) -> Result<()>`
  - `pub struct ScreencastFrame { pub data: Vec<u8>, pub session_id: u64, pub timestamp: Option<f64> }`（**data 已解码**——wire 上的 base64 在此消失）
  - `pub fn screencast_frame(params: &serde_json::Value) -> Result<ScreencastFrame>`（事件名 `Page.screencastFrame`）

- [x] **Step 1: 写失败测试**（追加到 `crates/aleph-cdp/tests/methods.rs`，沿用 `replying()` 形状 + `received_for`/`last_params` 断言）

```rust
#[tokio::test]
async fn screencast_frame_event_decodes_base64_into_bytes() {
    // Review Focus #5: wire 上 data 是 base64；漏解码 = ffmpeg 吃垃圾。
    // "e30=" 是 "{}" 的 base64 —— 断言解出来的是原始字节 [0x7b, 0x7d]。
    let frame = screencast::screencast_frame(&json!({
        "data": "e30=", "sessionId": 7,
        "metadata": {"timestamp": 1234.5, "deviceWidth": 800, "deviceHeight": 600}
    })).expect("decode");
    assert_eq!(frame.data, b"{}");
    assert_eq!(frame.session_id, 7);
    assert_eq!(frame.timestamp, Some(1234.5));
}

#[tokio::test]
async fn start_screencast_sends_format_and_quality_only_for_jpeg() {
    // png 带 quality 会被 Chrome 拒（capture_screenshot 同款纪律，page.rs:154 注释先例）。
    // 两臂各钉一次 wire 形状。
}

#[tokio::test]
async fn screencast_frame_ack_carries_the_session_id() { /* 钉 {"sessionId": n} */ }

#[tokio::test]
async fn stop_screencast_sends_no_params() { /* 钉 json!({}) */ }
```

- [x] **Step 2: 跑确认红** — `cargo test -p aleph-cdp --test methods screencast 2>&1 | tail -3`（编译错即红）
- [x] **Step 3: 实现 `methods/screencast.rs`**（签名见 Produces；`start_screencast` 只在 Jpeg 臂放 `quality`；`max_width/max_height/every_nth_frame` 为 None 时不出现在 wire 上——CDP 对显式 null 与缺省语义不同）
- [x] **Step 4: 跑确认绿** — 同上命令，4 条 PASS
- [x] **Step 5: 证伪 + commit**：把帧 decode 改成 `data.as_str().bytes().collect()`（不解 base64）→ Step 1 测试红 → 恢复。`git commit -m "aleph-cdp: hand-written Page screencast wrappers for session recording"`

---

### Task 2: RecordingRegistry + 编码管线 + verified stop

**Files:**
- Create: `src/browser/cdp_backend/recording.rs`
- Modify: `src/browser/cdp_backend/mod.rs`（挂 `pub(crate) mod recording;` + CdpBackend 持 `recording_registry: Arc<RecordingRegistry>` 字段——routes 同款）
- Modify: `src/browser/manager.rs`（`recording_registry` 字段 + 构造（manager.rs:311 先例）+ 传入 CdpBackend（:557 先例）+ 访问器（:581 先例））
- Test: `src/browser/cdp_backend/recording.rs` 的 `#[cfg(test)]` + FakeCdpServer 集成

**Interfaces:**
- Consumes: T1 的 `screencast::*`；`CdpConnection::events()`（订阅先于 start_screencast）；RouteRegistry 全套先例（`routes.rs:155` 结构、`:555 ensure_intercept_loop` 惰性启动形状）；`CdpBackend` 的 `command_timeout`
- Produces（T3 消费）:
  - `pub struct RecordingReceipt { pub recording_id: String, pub path: PathBuf, pub duration_ms: u64, pub captured_frames: u64, pub encoded_frames: u64, pub dropped_frames: u64, pub encoder_exit: String, pub size_bytes: u64, pub complete: bool, pub truncation_reason: Option<String> }`
  - `pub struct RecordingStatus { pub recording_id: String, pub elapsed_ms: u64, pub captured_frames: u64, pub dropped_frames: u64 }`
  - `pub enum RecordStartError { AlreadyRecording { existing_id: String }, NoFfmpeg { searched: Vec<String> }, PathNotWritable { path: PathBuf }, DiskLow { available_bytes: u64 }, Engine(#[from] ...) }`（形状随 error.rs 家族定，此处语义是契约）
  - `impl RecordingRegistry`: `start(profile, tab_id, session, conn, path: Option<PathBuf>, fps_cap, format) -> Result<RecordingStatus, RecordStartError>`（`path=None` 时生成受管默认 `~/.aleph/recordings/<profile>/<utc>-<tab_id>.<ext>`——扩展名来自 format）、`stop(profile, tab_id) -> Result<RecordingReceipt, BrowserError>`、`stop_by_id(recording_id) -> Result<RecordingReceipt, BrowserError>`、`status(profile, tab_id) -> Option<RecordingStatus>`
  - `pub(crate) fn resolve_ffmpeg() -> Option<PathBuf>`（env `ALEPH_FFMPEG` → PATH → `~/.cache/ms-playwright/ffmpeg-1011/ffmpeg-linux`）
  - `pub(crate) fn default_recording_path(profile: &str, tab_id: &str, ext: &str) -> PathBuf`（纯函数，可单测）

- [x] **Step 1: 纯函数半失败测试**

```rust
#[test]
fn ffmpeg_resolution_prefers_env_then_path_then_playwright_cache() { /* 三个来源各钉一次优先级 */ }

#[test]
fn a_second_start_on_the_same_tab_is_refused_with_the_existing_id() { /* RecordStartError::AlreadyRecording */ }

#[test]
fn start_without_extension_is_refused() { /* .webm/.mp4/其他扩展名可过；无扩展名拒 */ }

#[test]
fn the_default_path_lands_under_the_managed_dir_with_profile_and_tab() {
    // ~/.aleph/recordings/<profile>/<utc>-<tab_id>.webm 形状；home 由 dirs 族 crate 或
    // ALEPH_HOME 既有惯例解析——看仓库里 vault/runtimes 路径怎么拼，跟同一个惯例。
}
```

- [x] **Step 2: 跑确认红 → Step 3: 实现纯函数半 → Step 4: 绿 + commit `browser: recording registry skeleton with ffmpeg resolution and reservations`**

- [x] **Step 5: 管线集成失败测试**（FakeCdpServer `push_event` 推 `Page.screencastFrame`；**真 ffmpeg**——PATH 可用是本机事实，测试环境同机器；帧内容：生成合法 JPEG 字节，可用 image crate 若已有依赖，否则内嵌一张最小合法 JPEG 常量）

```rust
#[tokio::test]
async fn start_then_frames_then_stop_produces_a_playable_file_and_an_honest_receipt() {
    // FakeCdpServer 推 30 帧 → stop → 断言：
    // receipt.complete == true；captured_frames == 30 == encoded_frames；
    // encoder_exit == "0"；size_bytes > 0；
    // ffprobe 输出可解析出 ≥1 个视频流（ffprobe 随 ffmpeg 同包，PATH 已有）。
    // Review Focus #5 的对账钉：captured == encoded，不等即谎言。
}

#[tokio::test]
async fn ack_is_sent_only_after_the_frame_is_written() {
    // ack pacing 方向钉：FakeCdpServer 记录每帧到达与收到 ack 的相对序——
    // 第 N 帧的 ack 之前，stdin 必须已收到第 N 帧字节。
}
```

- [x] **Step 6: Review Focus #1 背压测试**

```rust
#[tokio::test]
async fn a_slow_encoder_drops_frames_instead_of_buffering_them() {
    // ffmpeg 桩：一个 read 极慢的 shell 脚本（测试内创建，先读 shebang 再 sleep-read）。
    // 推 200 帧 → 断言 dropped_frames > 0 且录制进程 RSS 有界（或 channel 深度有界断言），
    // 且页面侧（FakeCdpServer 视角）没有因为缺 ack 之外的机制卡死。
}
```

- [x] **Step 7: Review Focus #2 对抗测试**（tab 中途死）

```rust
#[tokio::test]
async fn tab_death_mid_recording_auto_finalizes_a_truncated_receipt() {
    // 推 10 帧 → FakeCdpServer 关闭 session（事件流 Closed）→
    // registry 检出 loop 死 → 自动关 stdin、等 ffmpeg、回执入库；
    // 之后的 stop(profile, tab) 返回该截断回执（complete=false, truncation_reason=Some(...)），
    // 文件存在且 ffprobe 可解前半截。
}
```

- [x] **Step 8: Review Focus #3 对抗测试**

```rust
#[tokio::test]
async fn a_nonzero_encoder_exit_is_never_reported_as_complete() {
    // ffmpeg 桩：exit 1。stop → complete=false, encoder_exit="1"，即使文件存在非空。
}

#[tokio::test]
async fn stop_kills_a_wedged_ffmpeg_and_says_so() {
    // ffmpeg 桩：stdin 关闭后不退。stop 有界超时 → kill → complete=false, encoder_exit="killed"。
}

#[tokio::test]
async fn ffmpeg_death_mid_recording_is_detected_on_the_next_frame_write() {
    // 录制中 kill ffmpeg 子进程 → 推下一帧 → 写 stdin 失败即检出 →
    // 录制标记 failed；随后的 stop 返回残留文件的截断回执（complete=false, encoder_exit=<code>）。
}
```

- [x] **Step 9: 实现管线 + verified stop**

核心形状（签名与测试已定的部分不重复）：

```rust
// start 的序（每一步失败都 fail-fast 且不残留）：
// 1. resolve_ffmpeg()?  2. 路径可写实测（创建-写-删临时文件）  3. 磁盘余量 ≥500MB
// 4. 订阅 conn.events()（先于一切起因）  5. Page.startScreencast
// 6. spawn ffmpeg -y -f image2pipe -framerate <fps> -i - <编码参数> <path>
//    —— .webm→-c:v libvpx-vp8，.mp4→-c:v libx264，其他扩展名无 -c:v（交 ffmpeg 猜）
// 7. spawn 消费 task：frame → bounded channel(32) → stdin write 成功 → screencast_frame_ack
//    channel 满 → dropped_frames += 1 且【仍要 ack】（不 ack 引擎会停发——丢帧是我们
//    的选择，不是引擎的停流理由；ack 是「这帧我处理完了（处理了=丢弃）」）
//
// stop 的序：
// 1. Page.stopScreencast  2. 关 channel/ 消费 task 收尾  3. 关 ffmpeg stdin
// 4. 等退出（command_timeout 有界；超时 → kill，exit="killed"）
// 5. 校验四重：退出码 0 + exists + size>0 + mtime ∈ 录制窗口 → complete
```

- [x] **Step 10: 全绿 + 证伪**（变异：ack 移到写入前 → Step 5/6 红；删 tab 死亡自动收尾 → Step 7 红；complete 判据砍掉 mtime → 加一个「旧文件复用路径」测试红）+ commit `browser: screencast-to-ffmpeg recording pipeline with verified stop`

---

### Task 3: browser_record 工具面 + 台账恢复

**Files:**
- Create: `src/builtin_tools/browser_tools/record.rs`
- Modify: `src/builtin_tools/browser_tools/mod.rs`（导出）
- Modify: `src/browser/backend.rs`（trait 默认方法 ×3——**禁 async 测试函数、禁 `` `async fn ` `` 字面量进注释**）
- Modify: `src/browser/cdp_backend/mod.rs`（覆盖三方法，委托 RecordingRegistry）
- Modify: `src/browser/engine/capability.rs`（screencast 行恢复三处 + OBSCURA=Unsupported NOT_PROBED + CHROMIUM=Supported）
- Modify: `src/executor/builtin_registry/definitions.rs`（注册 + census 断言 + fall-through 列表）、`groups.rs`、`builder/constructor/mod.rs`、`registry/struct_def.rs`、`registry/tool_registry_impl.rs`
- Modify: `src/approval/config.rs`（ActionType 新变体 + 默认 Ask——network.rs:293/599 与 config.rs:415 的 C1 先例）
- Test: record.rs `#[cfg(test)]` + capability.rs 防护测试 + 默认臂测试放 `playwright_cli_backend.rs`（C1 T3 先例）

**Interfaces:**
- Consumes: T2 的 `RecordingRegistry`（经 `manager.recording_registry()`）与 `RecordingReceipt/RecordingStatus`；`make_backend_and_tab_guarded`；`check_browser_approval`；dialog-gate census（`the_dialog_gate_records_every_verb_as_gated_or_not`——新动词须放置，ungated：录制是控制面操作）
- Produces: `browser_record` 工具（RecordAction::{Start, Stop, Status}）；`screencast` 能力行

- [x] **Step 1: 工具失败测试**

```rust
#[tokio::test]
async fn start_rejects_fps_out_of_range() { /* fps=0 与 fps=61 各拒一次 */ }

#[tokio::test]
async fn start_on_a_text_driver_profile_says_which_driver_can() {
    // playwright_cli 后端 → 默认臂 unsupported_by_driver 点名 cdp（pdf 先例的措辞形状）
}

#[tokio::test]
async fn stop_without_a_recording_says_so_plainly() { /* 不是通用错误，是「此 tab 无进行中录制」*/ }
```

- [x] **Step 2: Review Focus #4 测试**（路径不可写 fail-fast：测试内建只读目录，start → `PathNotWritable`，且**不产生**半成品文件与注册表残留）—— **落在注册表层**：T2 的 `start_on_an_unwritable_path_fails_fast_without_residue` 已钉（无残留、不到引擎、不毒化后续 start）；工具层够不到 `registry.start`（无活 backend 时在 `make_backend_and_tab` 就停了），工具层钉的是第二道纵深——无扩展名拒 + validation-before-gate
- [x] **Step 3: 实现工具**（七处注册全数照抄 `ln`/BrowserDialogTool 样板；`RecordAction` **无默认**——action 必填，避免 `browser_record{}` 空调用歧义；start 过 approval gate（Ask 默认）、stop/status 不进；stop 双入口：`tab_id` 走 `registry.stop`、`recording_id` 走 `registry.stop_by_id`；DESCRIPTION ≤80 字符）
- [x] **Step 4: trait 默认方法 + 台账恢复**

```rust
// backend.rs，紧挨 pdf 的默认臂风格：
/// One active recording per tab … the default names the driver that can
/// (only the CDP backend owns a screencast pipe).
// 注意：注释内禁出现 `async fn ` 字面量（census 扫描全文）。
```

capability.rs 三处同步：裁清单摘除 screencast（守卫测试更新措辞为「已有 browser_record 动词派发」）、CAP_FIELDS 加行、EngineCapabilities 加字段。

- [x] **Step 5: 跑 + 证伪 + commit**（`cargo test -p alephcore --lib browser_tools::record` + `--lib capability` + `--lib browser::backend` + `--lib executor::builtin_registry`；证伪：摘 CAP_FIELDS 行 → census 红；DESCRIPTION 超限 → ratchet 红；漏一处注册 → definitions census 红）`git commit -m "browser: browser_record tool; screencast capability row restored"`

---

### Task 4: 真机探针 + caps + 文档 + 全量验证

**Files:**
- Create: `docs/superpowers/specs/2026-09-27-browser-recording-design/probes/screencast-probe.mjs`（C1 的 spec-scoped probes/ 先例）
- Modify: `qa/browser_dual/caps.py`（加 `screencast` 行探测）
- Modify: `src/browser/engine/capability.rs`（obscura 行按实测翻转或维持，注释写测量日期/方法/版本）
- Modify: `docs/reference/FEATURE_LOCATOR.md` §3.12 + `qa/README.md`

- [x] **Step 1: obscura 探针**（v0.2.2 本机已装）：raw CDP → `Page.startScreencast{jpeg}` → navigate 动画页 → 断言 `screencastFrame` 事件到达且 data 可 base64 解码且帧头是 JPEG magic（FFD8）→ ack → `stopScreencast`。三结局如实记（事件不到/帧不可解/全通）。
- [x] **Step 2: chromium 端到端冒烟**（**本项目首个 chromium 真机验证点**）：真 chrome-stable 起 CDP → 完整 start→推帧→stop→ffprobe 可解 → 绿则 chromium 行的 Supported 从「同连接推断」升级为实测，注释更新。
- [x] **Step 3: 表值按实测更新** + `cargo test -p alephcore --lib capability` 绿
- [x] **Step 4: caps.py 加 screencast 行**（判据：事件到达**且**帧可解码——C1 教训：到达 ≠ 可用）；`./qa/browser_dual/run.sh caps` 绿；**证伪**：翻转表值 → diff 红 → 恢复
- [x] **Step 5: 文档**：FL §3.12 Round 2 (C2) 段 + qa/README.md + 勾选计划 checkbox
- [x] **Step 6: 七条全量验证集**（同 C1 T4 清单；基线 19,617 passed / 7 既存红 + pty flaky——集合不许变大；T4 新增测试使 passed 数**变大是正常的**，记录实测数字）+ commit `docs: browser recording — locator entry, caps probe, plan ledger`

---

## 合并与验收

单线单分支（`browser-native-r2`）。检查点：Task 2 后 `--lib --no-run` + `--lib browser`；Task 4 后七条全量。合并前确认：无新增红、clippy 净、caps 真机绿、chromium 冒烟绿。
