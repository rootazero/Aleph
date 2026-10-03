# C2：browser_record 会话录制 — 设计 Spec

**日期**：2026-09-27 | **状态**：已批准（§1–§6 逐节获批） | **轮次**：Round 2 / C2

## §1 意图与硬约束

`browser_record`（第 27 个 browser 工具）：`start`（tab 级，可选路径/fps/格式）→ 后端经 CDP `Page.startScreencast` 帧流 → ffmpeg 实时编码落盘 → `stop` 返回 **verified 回执**（帧计数/编码器退出态/文件校验——「文件在≠写完」）；`status` 查进行中状态。

主用途：**调试回看自动化会话 + 证据/审计留存**（用户裁定）。直播流裁出本轮。

硬约束：
- **唯一 CDP 真源**：aleph-cdp 手写 `Page.startScreencast/screencastFrameAck/stopScreencast` + `ScreencastFrame` 事件类型（fetch.rs 先例：手写、decode 模式；crate 拒绝 codegen）。CDP 规范层面 Page.startScreencast 系列是 Page 域稳定成员（非 experimental）。
- **cdp 后端独占**：chrome_mcp / playwright_cli 是文本驱动无帧管道——不 override，走 `unsupported_by_driver` 默认臂（backend.rs:176 pdf 先例）。obscura 引擎 v0.2.2 strings 证实 screencast 全家实现完整（含 ack pacing、自研渲染器帧源），行值以真机探针实测为准（C1 教训：strings ≠ 行为）。
- **能力台账**：恢复 screencast 行——capability.rs:578-600 有「裁行」守卫测试（裁因 "nothing calls Page.startScreencast"），有动词后按规程恢复；行值逐引擎实测。
- **R9 字节纪律**：新工具 DESCRIPTION ≤80 字符；catalog ceiling 棘轮按三问流程记账（C1 已走过一次，ceiling 现为 116,114 B）。
- **参考实现**：pi-agent-browser-native 的 record 是 wrapper（真捕获在 vercel-labs/agent-browser，源码本机不可考），但其 verified-stop 回执模型完整可见（recording-recovery.ts:131-147）——编码器退出态 + 帧计数 + 文件新鲜度/size 三重校验 + 单次有界恢复查询，照搬其判绿形状。

## §2 架构组件

| 组件 | 位置 | 职责 |
|---|---|---|
| screencast 包装 | `crates/aleph-cdp/src/methods/screencast.rs`（新） | `start_screencast{format,quality,max_width,max_height,every_nth_frame}` / `screencast_frame_ack` / `stop_screencast` + `ScreencastFrame` 事件 decode |
| RecordingRegistry | `src/browser/cdp_backend/recording.rs`（新），ProfileManager 持有 | 键 `(profile, tab_id)` 每 tab 一路（RouteRegistry 先例：后端每次调用重建，长命状态住 manager）；持 ffmpeg 子进程 handle、帧计数（captured/encoded/dropped）、起始时刻、路径 reservation |
| 编码管线 | recording.rs 内 | 帧 → bounded channel → ffmpeg stdin（`-f image2pipe`）；**ack pacing 背压**：帧写入 channel 成功后才发 `screencastFrameAck`——编码慢则少要帧，dropped 计数诚实记录 |
| verified stop | recording.rs 内 | `stop_screencast` → 关 stdin → 等 ffmpeg 退出（有界超时）→ 校验退出码 0 + 文件存在 + size>0 + mtime 新鲜 → 回执 |

**编码方案（用户裁定 A）**：实时管道编码。B（帧落盘后离线编码）出局：IO 放大一个数量级、stop 延迟=全片编码时长（毁证据场景「停录即取」）、临时目录清理债。C（内存缓冲）出局：16GB 内存红线。

**fps 语义**：screencast 是 repaint-driven（无重绘无帧），fps 参数是上限不是承诺；回执里 `captured_frames/duration` 才是实测帧率。默认 fps=10（调试+证据场景甜点，30 是浪费）。

## §3 存储与并发

- **路径**：可选 `path` 参数；不给则落受管默认 `~/.aleph/recordings/<profile>/<utc>-<tab_id>.<ext>`。文件 **host 自有，永不自动删**。相对路径锚定 Aleph 进程 cwd，回执回显解析后的绝对路径（模型对「文件在哪」零猜测）。
- **格式**：按扩展名——`.webm`→libvpx-vp8、`.mp4`→libx264、其他扩展名直接交 ffmpeg、无扩展名拒绝（参考实现同形状；ffmpeg 不认的扩展名在 stop 回执的编码器退出态里诚实暴露）。
- **ffmpeg 解析链**（start 时 fail-fast）：`ALEPH_FFMPEG` env → PATH `ffmpeg` → playwright 自带 `~/.cache/ms-playwright/ffmpeg-1011/`（chromium_resolve.rs:599-610 先例）→ 报错附安装指引 nextActions。
- **并发**：每 `(profile, tab_id)` 一路；同 tab 二次 start → 结构化错误 + nextActions（附现有 recording_id）。跨 tab 并发不设全局上限；每路 start 前查磁盘余量（<500MB 拒录）。进程内 reservation 即够（单实例红线已保证无跨进程争抢）。

## §4 工具面 wire

| action | 参数 | 返回 |
|---|---|---|
| `start` | `tab_id?`（默认 active）、`path?`、`fps?`（1-60，默认 10）、`quality?` | `{recording_id, resolved_path, fps_cap, ffmpeg_source}` |
| `stop` | `tab_id?` 或 `recording_id` | verified 回执：`{recording_id, path, duration_ms, captured_frames, encoded_frames, dropped_frames, encoder_exit, size_bytes, complete}`；`complete=false` 附截断原因 |
| `status` | `tab_id?` | 进行中 `{recording_id, elapsed_ms, captured_frames, dropped_frames}`；无录制明确说无 |

- **approval**：`start` 写文件 → `check_browser_approval`（ActionType 新变体，Ask 默认，BrowserEvaluate 同族）；`stop`/`status` 是读/收尾不进 gate（C1 T3 先例：写进 gate、读不进）。
- **错误**：走 `backend_error_text` 咽喉自动继承 recovery 拖车。
- **trait**：`record_start/record_stop/record_status` 默认方法返回 `unsupported_by_driver`。
- **台账**：screencast 行恢复；chromium=Supported；obscura=探针实测。

## §5 错误处理与测试

| 故障 | 时机 | 行为 |
|---|---|---|
| ffmpeg 三处都解析不到 | start | fail-fast + 安装指引（不开录；比参考实现的「开录后警告」更严） |
| 输出路径不可写 / 磁盘 <500MB | start | fail-fast（写权限用创建临时文件实测，不靠猜） |
| ffmpeg 录制中途死亡 | 录制中 | 下一帧写 stdin 失败即检出 → 标记 failed + 残留文件进回执（`complete=false`） |
| tab 录制途中死亡 | 录制中 | 事件流随 session 断 → 自动收尾（关 stdin、等 ffmpeg、截断回执）——文件是诚实的前半截 |
| stop 时 ffmpeg 不退 | stop | 有界超时后 kill → 回执 `complete=false, encoder_exit=killed`，文件保留 |
| 丢帧 | 录制中 | channel 满 → dropped_frames++，不阻塞页面（ack pacing 天然减速） |

**测试**（FakeCdpServer 已有 `push_event`——C1 T2 扩的）：
- 纯函数半：路径解析链、格式映射、reservation 冲突
- 管线半：FakeCdpServer 推 N 帧 → **真 ffmpeg**（本机 /usr/bin/ffmpeg n9.0.1）→ stop → 帧数对账 + ffprobe 可解
- 对抗半：tab 中途死自动收尾回执；ffmpeg 中途被杀下一帧检出；ack pacing 背压（慢编码 → ack 间隔变大）
- 真机半：obscura 探针（screencastFrame 到达且帧可解码 → Supported）+ caps 行 + **chromium 端到端录制冒烟**（Round 1/2 首个 chromium 真机验证点——录制是低风险的首次实测对象）

## §6 刻意不做

- 直播流（stream）——用户裁：订阅者管理+背压推送是独立设计
- 自动录制开关——用户裁：显式按需
- 音频——screencast 无音频轨，要音频是另一套捕获
- PII scrub / 区域打码——host 自有文件，隐私责任在使用者
- 帧率自适应——fps 是静态上限
- 视频后处理（剪辑/压缩/转码）——落盘即终态
- HAR——C1 已裁，不复活

## §7 执行形状

单线单分支：续用 `Aleph-browser-r2` worktree / `browser-native-r2` 分支（先 rebase 到含 C1 的 main）。任务切分预估：T1=aleph-cdp screencast 包装 → T2=RecordingRegistry+编码管线+verified stop → T3=工具面+台账恢复 → T4=obscura/chromium 真机探针+caps+文档+七条验证集。

**用户可见形状补充**：`recording_id` 为 `rec-<n>`（profile 内单调递增不复用，与 C1 规则 id `r1,r2…` 同族）。
