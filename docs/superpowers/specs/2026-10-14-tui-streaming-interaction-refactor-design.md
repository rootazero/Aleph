# Aleph TUI 流式输出与交互连线设计 / Streaming and interaction design

状态：待用户批准；仅完成只读研究与设计，尚未实施。
独立 builder check：2026-10-14 原设计及顶部悬浮消息增补均返回 `nothing important left`。这是设计可规划性检查，不是实现正确性或运行验证。悬浮条显示时机仍待用户结合交互示意确认。
日期：2026-10-14。
工作分支：`feat/tui-streaming-refactor`；工作目录：`.worktrees/tui-streaming-refactor`；扫描基线：`ff227d008`。

## 1. 意图 / Intent

本轮把 Aleph TUI 建设成过程可读、流式稳定、交互可恢复的终端工作界面。问题集中在现有能力之间的连线：共享 transcript 核已经具备 Step、reducer、详情档位，但 TUI 仍自行派生；提问提交和恢复未充分消费服务端确认；多个模态的显示与焦点各自维护；部分缓存遗漏显示条件。

用户要求深度参考 pi、pi-claude-code-tui、pi-cc-extensions、pi-ask-user-question，先比较再映射 Rust 架构，优先复用，清理替代路径，在隔离 worktree 双线执行、验证、提交并更新 reference。

既有用户决定以 [stepwise design](2026-09-23-stepwise-transcript-folding-design.md) §2 及其 Phase S 修订为准：按 Think→Act 迭代折叠，最终回答完整显示，实时和重放共用纯 reducer，设备级 `/detail brief|full`，标题来自既有内容，无额外模型调用。这里只设计本轮 TUI 落地及配套修复；不重新决定这些问题。

用户新增明确需求：TUI 顶部悬浮显示最后一条用户消息，并提供字面文案 `[click to jump]` 点击回到原消息。

**成功标准**：用户能看清当前工作、展开中间过程、读到完整最终回答，并随时辨认当前用户要求和跳回原文；断线恢复能取回已记录过程；提问失败保留输入，提交以服务端确认和 pending 快照为准；显示的模态与按键目标始终一致；密集输出期间输入不被长 RPC 阻塞。

## 2. 扫描与比较 / Verified gap analysis

图谱用于定位，再以当前源码验证。以下是静态证据，尚无运行复现或性能测量。

| 维度 | 参考模式及限制 | Aleph 当前事实 | 本轮取舍 |
|---|---|---|---|
| 流式调度 | pi `packages/tui/src/tui.ts:1001,1131` 合并绘制，普通帧间隔约 16ms、输入立即反馈 | 已有 burst drain、按需重绘、Rc 流式前缀及可见窗口；`tui/mod.rs` 主循环仍 await RPC，分支内 drain 无预算 | 保留现有渲染器；限制每轮处理预算，后台执行慢 RPC，结果回 UI 线程 |
| 工具身份 | pi `interactive-mode.ts:3524,3606` 依 toolCallId 更新同一组件 | 已有 tool_id、ToolEnd 补偿、RunComplete 对账及共享 ToolRow | 迁入共享 reducer 后删除 TUI 重复投影，保留状态栏等独立副作用 |
| 过程折叠 | cc-tui `claude-code-tui.ts:158,173` 先 wrap 再限物理行；cc-extensions 组件邻接分组 | 共享 `reducer/step/detail/group` 已实现，`chat_area.rs:820` 的 Step 空臂；TUI `messages` 仍独立维护 | Step 为业务迭代边界，Step 内复用工具折叠、分组、diff；不用组件邻接推断迭代 |
| 重放 | 参考扩展主要是实时组件补丁，不能证明持久化一致性 | `trace.by_runs` 已返回 reasoning 和工具 presentation；attach 当前只映射 chat.history，`app/trace.rs` 忽略 ReasoningEmitted | 历史按 run 去重；完成 run 由共享 reducer 重放，旧无 trace 行保留 history 兜底 |
| 提问确认 | 独立 ask-user 扩展忽略 signal，null/undefined 取消存在版本不兼容风险；questionnaire 的 multi 是多题 | Core 已有多题、多选、超时、终态及 pending；TUI `mod.rs:889` 忽略 resolve 返回并无条件关弹窗 | 类型化确认、失败保留草稿、pending 恢复；答案解释留在 Core |
| 模态一致性 | 参考可借鉴显式生命周期，不能复制其运行策略 | AskUser 单槽覆盖；approval 在 dialog 之上绘制，Focus 可指向 dialog；close_overlay 同时清多类状态 | 单一活动模态控制显示、焦点和关闭；按身份保存排队请求和草稿 |
| 缓存 | cc-extensions 最终态缓存，running 绕过缓存，共用动画时钟 | 静态缓存已有宽度与高亮 generation；verbose 切换未失效，StreamPrefix 缺高亮 generation | 用明确 revision 和布局条件失效；复用既有时钟和终端 diff |
| 顶部消息与跳转 | 本机四个参考仓库当前快照未定位到该确切功能；pi `tui-alt-screen.ts:1642,1037` 的底部跳转控件具备单行覆盖、列宽裁剪与绘制矩形命中 | Aleph 已有 `chat_area.rs` 高度测量/可见窗口、`regions.rs` 每帧命中表、`keys.rs` 鼠标路由，尚无用户消息跳转动作 | 复用这些机制新增用户消息锚点和顶部悬浮条，显示条件作为本设计工程建议，不冒充参考实现 |
| 完整工具输出 | cc-extensions Input/Output 分区可参考 | inline 文本、diff 和展开已工作；`trace.tool_output` 尚无消费者 | 在工具详情中接现有分页 RPC，显示来源、截断、过期状态 |

定位入口：`docs/reference/FEATURE_LOCATOR.md` §5.13、§6.13；`TRANSCRIPT_RENDERING.md` §4、§7；`SUBSYSTEM_ROUTING.md`。

## 3. 架构边界 / Architecture boundaries

Core 继续拥有执行、答案解释、权限、脱敏、持久记录与任务取消。`shared/ui_logic::transcript` 负责纯 data→data 归并和呈现决定。TUI 拥有终端布局、滚动锚点、焦点、草稿、展示队列和 RPC 调度。

复用现有 Ratatui、Tokio、typed StreamEvent、共享 fold/diff/group/affordance/theme tokens。无需引入 Rayon、新 UI 框架、JS 原型补丁或第二套 ANSI 引擎。顺序 reducer 不并行归并；并发用于独立 I/O，避免改变事件顺序。

`AppState` 的 transcript 事实由 `Transcript` 唯一持有；渲染缓存和展开集合是派生状态。保留 metering/context/plan/agents 等事件副作用，但不能再修改另一份 transcript。允许纯粹的历史导入 API，禁止客户端绕过 reducer 编辑其条目。

## 4. Transcript 接入与恢复 / Transcript and recovery

### 4.1 完成 run 的水合

先订阅当前会话事件，再后台获取 history 和按 run 分批的 `trace.by_runs`。按 history 顺序插入用户行与 run 重放块：有非空 trace 的 run 丢弃该 run 的 history assistant 投影；无 trace、无 run_id 的旧记录保留普通历史行。只覆盖本次 history 页，分页边界和 trace 服务端扫描上限必须显式遵守，不能把未加载历史宣称为完整。

新 transcript 在独立实例构建，只有 session/generation 仍匹配才整体替换当前实例。失败保留原画面并显示恢复状态，不清空后再 await。`NeedsResync` 按既有裁定重建整份已加载会话转录；活 run 中先标记过程可能不完整，结束后重建。恢复请求去重且最多一次在途，失败允许用户重试，禁止递归重试风暴。

### 4.2 中途 attach 的边界（本设计的工程选择）

当前 `trace.by_runs` 没有实时 seq watermark，持久 trace 与 live delta 也没有一一对应游标。因此不能声称把任意正在增长的 replay 与缓冲 live 拼接后恰好一次。

本轮采用可证明的保守行为：完成 run 正常重放；正在运行的 run 从订阅后收到的帧接管，提供显式 reducer adopt 入口并允许未知 iteration，显示“接入前过程待恢复”。**不把该 active run 的无游标 replay 与 live 拼接**。任务终结后从持久 trace 重建该会话，恢复订阅前记录。接管时不伪造收到过 RunAccepted，不把未知迭代追认为后到的 TurnStarted。

adopt 入口不能清空此前完成 run，重复采用同一 run 不重开状态，接管另一 run 必须显式终结旧活动状态；`apply_replay` 撞上 live 状态返回 NeedsResync。进行中运行若无法确定归属，保留状态提示而不吞掉当前文本；终局恢复以记录为准。

这是本轮保守默认，代价是中途接入者在任务结束前不能看到完整前半程。无缝进行中快照需要服务端顺序边界协议，作为后续独立设计，不用客户端启发式代替。任务结束前后去重、恢复期间的会话切换与迟到结果必须有行为测试。

### 4.3 Step 与详情档位

沿用既有 Step 语义：live thinking 显示动词、计时和尾部两行；文字流入时打开；下一 TurnStarted 后按档位收折；最终回答提升到顶层完整呈现。展开 Step 内按 thinking、过渡文字、工具/分组、notes 排列。Enter/点击切 Step；Ctrl+O 仍切工具正文。

以下为本次工程选择，不冒充用户已裁定：

- `/detail` 切档时清空 Step 临时 overrides，避免 XOR 使刚展开的条目反向关闭；工具手动展开状态独立。
- 无标题但有 running 工具的 live Step 显示共享动词与计时；只有 notes 的 settled Step 显示中性“过程记录”；全空 Step 不显示。
- 缺 thinking 的旧记录不显示“被脱敏”或“未记录”的猜测；直接使用现有标题退化规则。
- 同一步多条 reasoning 记录保留原次序，无额外 salvage 标签；未被权威记录覆盖的 delta 遵循现有 reducer 终局规则。
- TurnSummary 的缓存使用稳定条目标识或 generation + 位置；实际选择留给 builder，但禁止把不存在的 id 当作缓存键。

`/detail` 使用既有 theme 持久化机制和 `ALEPH_TUI_DETAIL` 覆盖，测试注入临时根。迁移命令帮助与补全，移除旧 `/verbose`、`/tools` 的独立状态；如仓内仍有外部消费者，保留仅转发到 `/detail` 的兼容入口，并在文档列明映射，不保留第二套行为。

## 5. 提问、审批与焦点 / Clarification and modal control

### 5.1 单一展示所有者

引入小型 TUI 交互控制器，活动模态是一个 enum；渲染与 keys 从同一值决定目标。现有 approval 的业务队列继续复用，控制器只协调展示。后到请求不抢正在输入的请求；当前请求结束后按到达顺序选择下一项，同 session supersede 替换旧澄清并显式提示。显式打开本地 picker 时暂停当前展示并保留草稿。

关闭只关闭指定模态，不连带清掉其他请求；外部终态按身份清理对应项。不同 session 的草稿分别保存。secret 内容只进入当前请求的内存草稿，不写日志、history、系统消息或诊断；终态/替代后释放。

### 5.2 提交与恢复

复用 `clarification.pending` 和 `clarification.resolve`，将已有响应形状纳入共享 typed contract，Core 和客户端使用同一形状，避免两份结构互相掩盖错误。

状态最少包含 editing/submitting/reconciling/error：

- 提交期间禁止重复提交；RPC 在后台执行。
- RPC 失败保留题目、选择及草稿，显示可重试错误；连接或响应不确定时先刷新 pending，禁止自动重发。
- `resolved && !stale && pending_questions == 0` 只结束对应请求；剩余题目或 stale 先获取 pending，不依赖下一 AskUser 帧。
- attach、重连、请求完成后和检测到流队列丢失时刷新 pending；快照失败保留可辨认的未知状态。
- 单选、多选、自由文本和多题顺序使用既有 Core 语义，显示 option description/header，不在客户端再解释 value。

### 5.3 请求身份（需要最小 Core 连线）

现有 resolve 只有 session_key，pending 缺 run_id，无法可靠区分同 session 替代请求。仅靠本地 generation 无法保护已发出的旧答案。

为新客户端增加向后兼容的 `request_id`、当前 `question_id` 关联：Core register 生成稳定 request_id，AskUser、PendingClarification、ClarificationEnded 携带它；pending 另外返回 run_id。resolve 可选携带 expected_request_id 和 expected_question_id，Core 在同一锁内核对身份与 cursor 后再解释答案，不匹配返回 stale。旧客户端省略字段保持既有行为。协议、Core producer、Gateway conversion 与客户端解码必须同批接通并有跨 crate 契约测试。

请求身份只用于并发一致性，不取代权限检查。新 TUI 对缺身份旧服务端降级为刷新 pending 后人工确认，不自动恢复提交；文档明确旧协议无法提供同等 supersede 保护。

### 5.4 取消语义

本轮不新增“只取消澄清并继续任务”的业务语义。Esc 收起展示但保留待处理项，提供明确重开入口；Ctrl+C 在当前提问中取消它所属 run，不能落到主屏另一个 run。pending 的 run_id 为缺失时只允许收起，提示无法定位任务，禁止猜取消目标。已有取消/超时 ClarificationEnded 清理指定身份，迟到结果不得关闭新请求。

## 6. 调度、缓存与工具详情 / Scheduling, caching, output

### 6.1 顶部最后一条用户消息 / Sticky last user message

本节实现用户新增需求；下列细节是待确认的工程默认。目标为**当前会话按转录顺序最后一条 UserText**，不取最近 assistant、工具输出或“视口中最近的用户行”。同样的文本可以多次出现，禁止文字搜索定位；当前页没有用户行时不捏造目标，页外历史仍遵守既有分页边界。

- 用户原文完全滚出可视区上方后，在聊天区顶部、应用页眉下绘制单行悬浮摘要；原文仍在可视区、位于视口下方或无用户行时隐藏。用户要求未限定显示条件，这个省行默认需要在示意中确认。
- 悬浮条使用既有 theme token，保留 `[click to jump]` 文案。多行文本只在摘要中折成空格，保留原文；CJK/emoji 按终端列宽裁剪，加省略号，不把截断摘要写回 transcript。
- 悬浮条是终端覆盖层，避让边框/滚动条，复用每帧 region table。点击跳转区的区域优先于被覆盖工具行；其余覆盖区域也不能把点击穿透到工具行。模态展示期间不响应底层跳转。
- 跳转目标携带绘制时的 session/generation 和用户条目稳定身份。物理行起点来自 `build_visible_lines` 同一次高度测量，点击后按当前布局重新解析目标并换算底部 scroll_offset；不能缓存旧行号或用固定消息高度。切会话/水合替换后失效的目标不跳去另一条新消息。
- 跳转后尽可能把原文首行置于可视区顶部；内容不足时遵守实际 scroll clamp。暂停自动跟随，后续流事件、窗口缩放、Step 展开/收折保留身份锚点；身份与行内偏移转换只属于 TUI。用户可使用已有 Back to bottom / Ctrl+End 恢复跟随。
- 新提交或恢复出的最新用户行更换摘要目标；编辑输入框不更换目标。复制、粘贴输入和发送不能由点击跳转触发。无鼠标时提供 `/jump-user` 同动作入口，帮助/补全同步更新。
- 宽度不足以同时容纳摘要和完整跳转文案时，先收缩摘要；连完整文案也放不下或聊天区仅一行时不绘制悬浮条，保留命令入口。渲染尺寸变化不能产生负宽度或陈旧点击矩形。

此功能无需 Core 新 RPC、不持久化第二份消息副本，不修改 reducer 的业务决定。每帧取最新用户条目和行位置是转录的派生状态；如用索引加速，必须由 transcript generation/revision 驱动，不能维护独立的“最后用户消息正文”。

### 6.2 调度与缓存 / Scheduling and caches

主循环接收输入、流事件、topic 和后台任务结果。流与 topic 每轮有有限条数/时间预算，剩余工作在下一轮继续，不丢弃业务事件以限制帧率。普通重绘合并，输入触发及时反馈；阈值由 builder 在现有 tick 机制上选取，并用密集事件场景验证。

所有可能长时间等待的 UI RPC（首先 resolve/pending/approval poll/history/replay/tool detail）交给受限数量后台任务；结果携带 session/request/generation，仅 UI 线程更新状态。独立请求可并发，同一请求提交保持串行。断线/切会话取消或丢弃过期工作；禁止无限 spawn。

`shared/client` 当前 bounded stream 使用 try_send，不能简单改阻塞 send 使 RPC 路由死锁。本轮增加可观察的丢失信号/connection generation，通过独立于该 bounded 队列的 watch 或计数器传递，不能把“丢帧通知”塞回同一个已满队列；客户端据此刷新 pending 和 transcript；终态不能依赖单次事件必达。修复只提高可恢复性，不宣称网络交付恰好一次。

缓存覆盖内容 revision、宽度、主题/高亮 generation、detail/展开状态；running 动画不复用静态结果。StreamPrefix 在高亮 generation 变化时重建。Step 缓存不能只 hash id；避免每帧对所有历史全文重新 hash，优先使用 reducer Change 驱动局部 revision。沿用 cell 宽度、可见窗口、滚动锚点和现有终端 diff。

工具详情按 session + tool_id 调用已有 `trace.tool_output`，沿用 offset/total_bytes/truncated/source 的单位及约束。首次用户打开时请求，显式加载更多；显示 Expired、不可用、读取失败及已加载字节边界；不得把 Expired 当下一页。正文、Input/Output 与 diff 复用现有部件，TUI 不读服务端文件路径。限制在途分页与缓存容量，secret/脱敏完全复用 Core 结果。

## 7. 双线实施顺序与清理 / Parallel ownership and cleanup

每一阶段在独立 worktree 子分支工作，集成目标始终为 `feat/tui-streaming-refactor`，不写 main。用户“线2合并回主分支”解释为本轮集成分支，符合其禁止触碰 main 的约束。

| 批次 | 线1 | 线2 | 集成依赖 |
|---|---|---|---|
| 1 基础修复 | transcript adopt/import 与恢复控制器；建立 Step 测试面 | clarification 身份、共享响应契约、交互控制器；完成 RPC 确认测试 | 先约定 shared protocol 归线2；线1只依赖现有 StreamEvent |
| 2 消费者接入 | live/replay 接线、Step renderer、/detail、缓存、用户消息锚点与顶部悬浮跳转 | keys/dialog/approval 模态接线、pending 恢复、定向取消 | app/mod.rs、tui/mod.rs、commands.rs 由主线程集成，子任务提交独立模块和最小接线补丁 |
| 3 扩展与清理 | 工具详情分页、可读布局与帮助 | 后台 RPC 调度、流丢失信号与恢复接线 | 线2完成后先合入集成分支，再开始依赖任务；禁止两个 Cargo 同时抢 target |
| 4 收尾 | 移除重复投影、更新定位/呈现文档 | 行为复核、适用 QA、检查旧路径引用 | 每 2–3 个相关任务合并验证，失败先修当前批次 |

明确 CUT 落点：Step 消费者接管后删除 `app/trace.rs` 中 transcript 的重复归并（独立 trace inspector 需要的代码迁到明确模块）；移除 `TranscriptEntry::Reasoning` 的 TUI producer/renderer、verbose 状态、旧工具过滤状态、前缀去重与工具对账孪生逻辑。共享 enum 删除前普查 Panel/CLI/测试全部使用者，不能为了 TUI 的清理破坏其他端。保留仍有消费的 trace_result_to_wire、ToolGroup、未缓存 renderer 测试基准及状态栏副作用。

旧路径删除和新调用点在同一批次提交；文档不能写“已接通”而没有生产消费点。

## 8. 验证与资源边界 / Validation and resources

运行任何 Cargo 或测试前先审计所选 target、build runner 与测试的 HOME/USERPROFILE、dirs::home_dir、.aleph、keyring、SQLite/vault、删除和环境覆盖路径；只能运行已证明写删在 owned TempDir/注入临时根内的测试，破坏性清理须有 containment。Windows 改 HOME 不等于隔离。遇到现有不安全测试先在隔离分支修复，不能绕过后假称全绿。

所有 Cargo 串行，由线2/指定验证执行者统一运行；每次启动前查询实际 Windows 可用物理内存。低于 4GiB 时不启动，用非阻塞后续检查等待并汇报；不在会话里长时间 sleep。统一 `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0`；大型链接仍按实际内存和失败情况调整。记录每次检查值与命令。禁止全仓 renormalize；git porcelain 使用 `-c core.autocrlf=true`。

关键行为验证：

1. 同一有 trace run 的 live/replay 得到同一 Step/工具/最终回答；history 不重复；旧无 trace 会话兜底。
2. 中途接入明确显示不完整，终局恢复；恢复请求乱序、会话切换、流队列溢出不污染当前会话。
3. 多工具交错和结果补偿；Step 内工具遍历、泄漏断言能看到嵌套正文；空标题和旧 thinking 不伪造事实。
4. brief/full、手动展开、宽度/主题/高亮变化后缓存正确；CJK/emoji/长单行物理行预算和滚动锚点。
5. approval 与 AskUser 并存时可见目标等于按键目标；两会话请求保留草稿；同 session supersede 的旧答案在 Core 被拒绝。
6. resolve 成功、剩余问题、stale、RPC 失败/断线、超时、取消和迟到终态；secret 不进入 transcript 或日志。
7. 慢 RPC 与密集流并存期间输入继续推进；后台任务数量受限；恢复能补回丢失的提问终态。
8. 完整工具输出分页、过期、读取失败、脱敏与切会话迟到响应。

9. 悬浮条命中跳到最后一条用户原文，同文不同 id 不混淆；原文可见时隐藏；新用户消息、会话切换、重放替换与迟到点击不串线；窗口缩放、Step 收折及持续流不破坏阅读锚点；CJK/emoji、多行/长消息、极窄/极矮窗口、无鼠标命令及模态点击隔离。

按改动运行 `just test-shared` 所覆盖的共享协议/UI逻辑/TUI/CLI targets（共享 UI 含无默认 feature 形态）、相应 clippy/rustfmt；Core 变更至少编译相关 lib/bins/feature-gated integration targets并跑经审计的 clarification/协议/恢复测试。如果安全审计阻止某条整套 recipe，逐项报告跳过范围与替代验证，不能用局部检查冒充 recipe 通过。Panel 源码若改动，追加 Panel lib 和出厂 WASM 验证。

补 `qa/transcript_fold` 验证真实 drain 帧与真实 trace.by_runs，覆盖正常、工具并发、grace、失败/取消；端到端未能运行则记录 UNRUN 原因。不在没有基线测量时声称性能超过参考项目。

## 9. 文档、提交与边界 / Documentation and limits

完成每批次后提交可独立审查的变更；最终更新 FEATURE_LOCATOR §5.13/§6.13、TRANSCRIPT_RENDERING 的 Phase T 状态、SUBSYSTEM_ROUTING 及 QA 入口。更新生产落点、命令、失败/恢复行为、清理位置和实测验证记录，不重复复制用户裁定。

本轮不改 Panel 呈现、不做品牌吉祥物、不另调模型写摘要、不改变任务推理/执行策略、不做无游标 active replay 拼接、不声称修复所有既有服务端终局竞态。先前 spec 中 run 作用域 final_text、失败/取消 live/replay 差异等遗留必须由集成测试核实；若当前仍存在，作为 Step 验收阻塞单独修复，涉及 harness 时遵循 R10 棘轮，不能在 renderer 中增加猜测来掩盖。

留给 builder：模块具体命名、合理的事件预算/缓存上限、后台任务原语、条目 revision 存放方式和兼容命令映射细节。不得改变上述用户可见行为；若证据证明约定不可实现，应写清阻塞与替代设计再继续。
