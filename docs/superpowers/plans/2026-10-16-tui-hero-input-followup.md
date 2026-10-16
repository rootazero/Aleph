# TUI hero, reset and host input follow-up / TUI 欢迎页、重置与宿主输入修复

## Scope / 范围

- Remove the assistant speaker rail without changing markdown/code-block semantics. User messages retain their existing full-row background. / 移除助手消息侧边装饰，保留 Markdown 与代码块语义、用户消息整行背景。
- Render a full-width, cell-safe welcome card with a five-scanline Aleph logo; keep existing conversations compact. No font/profile modification. / 全宽欢迎卡与五行像素 Aleph 标识，已有对话保持紧凑；不修改字体或终端 profile。
- `/clear` resets display/draft only and preserves live work. `/new [topic]` uses authoritative `sessions.new`, switches only on success, and restores welcome without a success receipt. / `/clear` 仅重置本地显示与草稿并保留任务；`/new [topic]` 经服务端成功轮转后切换新会话、恢复欢迎页，不插入成功回执。
- Filter unconsumed mouse motion/drag/release records and use nonblocking mouse queue delivery; keyboard/paste/resize retain reliable ordering. Saturated mouse queues may drop transient wheel/click records rather than stall input. / 过滤未消费的鼠标移动/拖动/释放事件；鼠标队列非阻塞，键盘/粘贴/resize 保留可靠顺序；鼠标队列满时允许丢弃瞬态滚轮或点击事件。
- Restore raw keyboard mode after capture transitions, including F6 release. Preserve the existing collector and host-native selection; no synthetic clipboard/selection. / 捕获切换后恢复 raw 键盘模式，保留事件采集与宿主原生选区，不伪造剪贴板或选区。
- Disable terminal end-of-line wrapping inside the alternate screen; restore wrapping before leaving it (including setup failure/panic). A controlled legacy-console comparison reproduced bottom-row displacement with wrap enabled and eliminated it with wrap disabled. / alternate screen 内关闭行末自动换行，退出前恢复（含初始化失败/panic）；传统 console 单变量对照复现开启换行时底行移位，关闭后消失。
- Clear the transcript pane on repaint and synchronize the Windows physical display/diff buffer once on the empty/history boundary, not on every streamed frame. / 重绘前清理对话区域；Windows 在空白/历史边界切换时同步物理显示与 diff buffer，不逐流式帧清屏。

## Verification / 验证

All Cargo commands ran serially in the isolated worktree through `qa/terminal/memory_guard.py`, minimum available physical memory 4 GiB, `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0`. / Cargo 全部经内存守卫在隔离工作树串行运行。

- TUI library: **588 passed, 2 ignored**; binary: **8 passed**. / TUI 库 588 通过、2 忽略；可执行文件 8 通过。
- Strict scoped Clippy, build, changed-file rustfmt and `git diff --check`: passed. / 限定范围严格 Clippy、构建、修改文件格式及 diff 检查通过。
- Owned native-console UI probe: welcome, F6 freeze/input gating, resume, actual provider reply, no You label and no transcript rail: passed. / 自有 console 探针验证欢迎页、F6 冻结/输入门控、恢复、真实回复、无 You 和对话侧线。
- Native theme probe: five palettes plus community theme, output colors under `NO_COLOR=1`, local feedback, invalid reload preservation and restart: passed. / 五主题及社区主题、真实颜色、本地反馈、无效重载和重启记忆通过。
- Windows Terminal/ConPTY real `user32.SendInput` wheel probe: wheel-up changes transcript and wheel-down returns to bottom; passed. / Windows Terminal 真实滚轮上下滚动通过。
- Windows Terminal real drag + UI Automation `GetSelection`: selected `F6_UIA_TEXT_PROBE `, raw mode after F6 `496 (0x1F0)`, after resume `152 (0x98)`; subsequent keyboard input passed. No clipboard read/write. / Windows Terminal 真实拖动选中非空文字，F6 后模式 496，恢复后 152，后续输入通过；未读写剪贴板。
- The UIA controller was corrected for 200% DPI and maximizes only its owned window to avoid off-screen targets. An earlier retry with an off-screen target failed; it was not counted as success. / UIA 控制器修复 200% DPI 坐标并仅最大化自有窗口，早先目标越界失败未计入成功。
- `/new` native keyboard + loopback wire observation on an owned QA session: epoch changed, subsequent `/usage` uses the new key, welcome/no receipt passed. `/clear` keeps the key; `/new --help` does not roll. After the EOL-wrap fix, a fresh probe without controller-side mode changes passed every composer/empty-draft check after new, clear and help. The earlier failed snapshot is historical, not counted as a pass. / 自有 QA 会话经真实键盘与回环 wire 验证新 epoch、新 key、欢迎页及无回执；clear 保留 key，help 不轮转。修复行末自动换行后，探针不再代设模式，新建/清屏/help 后输入框与空草稿检查全部通过；此前失败仅作历史记录。

## Boundaries / 未验证与限制

The temporary wrap cleanup restores wrap-enabled mode rather than querying an ANSI host's initial wrap setting; a deliberately wrap-disabled host may return enabled. Windows native alternate buffers isolate the original buffer. / 临时换行清理恢复为开启，未查询 ANSI 宿主初始值，初始主动关闭换行的宿主可能恢复为开启；Windows 原生 alternate buffer 隔离原始 buffer。

No macOS/Linux or legacy-conhost manual drag/copy acceptance, no clipboard integration test, no full workspace suite, and no guarantee for an initial host mode with QuickEdit disabled. F6 deliberately pauses UI interaction while background events continue; selection covers visible cells only. / 未做 macOS/Linux 或传统 conhost 人工拖选复制、未测试剪贴板、未跑全工作区测试、不保证初始关闭 QuickEdit 的宿主。F6 期间 UI 交互暂停但继续接收后台事件，仅选择可见单元格。

No main merge, Gateway binary replacement, credential/keyring/vault changes or terminal profile changes. Normal QA session/RPC/provider writes were permitted; no database/session files were deleted. / 未合并 main、未替换 Gateway、未改凭据或终端 profile；仅正常 QA 会话与 provider 运行写入，未删除数据库或会话文件。
