# Pi Durable 源码核验笔记（v2 → v3 优化用）

来源：`/Volumes/TBU4/Github/pi/packages/durable/`
- 入口 `src/index.ts` (181 行) 全部是 export
- 子目录：`harness/`（运行时）、`session/`（核心）、`storage/`（适配）、`tools/`、`testing/`、`env/`
- 文档：`docs/spec.md`（4601 行，Pico5 规范）
- 包名 `@earendil-works/pi-durable` v1.0.0

下文是文档 v2 中可能需要修订/补充/纠正的事实。**所有行号均指源文件**。

## 1. 公开 API（`src/index.ts`）

### 工厂
- `Harness.open(storage, options, context)`（静态方法，返回 `Harness` 实例）
- `createSession(storage, options?)`
- `createRegistry()`

### 文档（document）
内置：
- `AgentDoc` — `RewindableConversationDocToken<AgentState>`
- `InboxDoc` — `ConversationDocToken<InboxState>`
- `LiveDoc` — `ConversationDocToken<LiveState>`
- `UsageDoc` — `ConversationDocToken<UsageState>`
用户自定义：`defineDoc<T>(definition)` / `defineDocFamily<T, I>(definition)`

### 任务（task）
内置：`GenerationTask` / `ToolTask` / `CompactionTask`
用户自定义：`defineTask(input, state, result, phases, ...)`

### 条目（entry）—— 6 种 kind
工厂 `defineEntry(kindName)`，返回 `{kind, is()}` 类型守卫。
- `UserEntry` ("pi.user") — 提交写入，`model: [UserMessage]`
- `AssistantEntry` ("pi.assistant") — generation 写入，含 stop reason
- `SystemEntry` ("pi.system") — 位置性 prompt + 工具变化，`content: ""`；sections/toolsAdded/toolsRemoved 表达
- `ToolResultEntry` ("pi.tool-result") — 工具结果 + `data: {diagnostics: ToolDiagnostic[]}`
- `ResetEntry` ("pi.reset") — 上下文重置，`head: "self"`，可携带 handoff 文本
- `CompactionEntry` ("pi.compaction") — 压缩摘要，`data: {reason: CompactionReason}`

### 扩展 / 钩子
- `defineExtension(extension)` — identity + 类型推断
- `defineTool(tool)` — identity + 类型推断（schema 来自 `tools: TSchema`，details 来自运行时上报）
- `hook(task, handlers)` — 注册对特定 task 的 hooks
- `section(key, render, options?)` — 提示片段（可加 `tag: true` 启用缓存）
- `wrapTool(tool, wrapper)` — 包装现有工具
- `wrapSection(key, wrapper)` — 包装已有 section

### 错误
- `ConversationBusy` — 对话忙时操作被拒
- `ReadAfterWrite` — 同事务内读后写冲突
- `StorageRejected` — 存储层拒绝写入

### 默认策略常量
- `DEFAULT_COMPACTION_POLICY` — `{enabled: true, reserveTokens: 16384, keepRecentTokens: 20000, backgroundTokens: 32768}`
- `DEFAULT_RETRY_POLICY` — `{enabled: true, maxRetries: 3, baseDelayMs: 2000, maxAgentDelayMs: 60000}`

### 事件
- `watchEvents(stream, listener)` — 订阅事件流
- `AgentEvent` / `AgentEventStream` — 智能体事件类型
- `MessageChange` / `SnapshotEvent` — 视图变更事件

---

## 2. 文档系统（`src/documents.ts`）

### 三种 scope
- `session` — 会话单例
- `conversation` — 对话单例
- `task` — 任务单例

### 两种 conversation semantics
- `latest` — 只关心当前值
- `rewindable` — 可回卷到 checkpoint（AgentDoc 是 rewindable 的）

### 三种 fork 行为（**对应 doc §10.1 策略**，且实现确认）
- `initial` — 从初始值开始（LiveDoc、InboxDoc 选这个，确保 fork 时无在跑任务）
- `asOf` — 取分叉那一刻的快照值
- `current` — 沿用父会话当前值

### 文档结构
```ts
{
  kind: string,                              // 类型标识
  version: number,                           // 必须正整数
  scope: 'session' | 'conversation' | 'task',
  history?: 'latest' | 'rewindable',         // 仅 conversation 有
  fork?: 'initial' | 'asOf' | 'current',     // 仅 conversation 有
  family?: true,                             // 多 key 文档族
  initial(seed?): T,                         // 初始值工厂
  migrate?(value, fromVersion): T,           // 升级迁移
  checkpointWhen?(value, ops, info): boolean // 何时打 base
}
```

### 文档族（DocFamily）
`defineDocFamily<T, I>(def with family:true, initial(seed: I))` — 同 kind 多 key（如 todo list 每个 item 一个 key）。`resolveAddress(definition, args)` 用 `addressId(address)` = JSON([kind, scope.kind, owner, key]) 寻址。

### 关键函数
- `resolveAddress(definition, args)` → `{address, id, nextArgument}`
- `addressId(address)` — 稳定字符串身份（用于去重）
- `documentCreate(definition, address, id)` — 构造持久化初始记录
- `checkRecordScope` / `checkRecordVersion` / `materializeDocumentValue` — 类型安全访问

---

## 3. Inbox 机制（`src/harness/inbox.ts`）

### InboxDoc 定义
- scope=conversation, history=latest, fork=initial
- checkpointWhen：`items.length === 0`（空时打 base）
- `initial: () => ({items: []})`

### InboxItem 三种模式
- `{mode: "steer", content}` — 直接中断式输入
- `{mode: "followUp", content}` — 排队到下一轮
- `{mode: "write", entry}` — 被动写入条目（含 reset）

### 队列设置
- `steeringMode: "all" | "one-at-a-time"`（默认 `"one-at-a-time"`）
- `followUpMode: "all" | "one-at-a-time"`（默认 `"one-at-a-time"`）

### Boundary 机制（**核心，比 v2 描述更精细**）
- `prepareBoundary(tx, conversationId, modes)` — 在 commit 开头调用，获取当前边界状态
  - 读 inbox draft（必须先于首次写）
  - 读 latestHeadMarker → 取得当前 head 位置
- `applyBoundary(tx, boundary, at, now)` — 在 `postTools` 或 `final` 边界执行：
  - 所有 `write` 都放行；选到 `head: "self"`（reset）会把 `postTools` 升为 `final`
  - `steer`：按 `steeringMode` 选第一条或全部
  - `followUp`：仅 `final` 时触发，按 `followUpMode` 选第一条或全部
  - 全部按 ID 排序，write 在 user 前面
  - write whose head targets an entry before active range → settle as `unanswered: stale`

### 提交流转
- `submit(type:"input")` → 进入 inbox → 在 boundary 由 scheduler 选择 → `tx.appendEntry()` + `tx.placeSubmission(id, entry.id)`
- `submit(type:"write")` → 同上但携带 `entry: EntryDraft`
- 提交被拒：`tx.settleSubmission(id, {status:"unanswered", reason})`

### withdrawQueuedInputs（abort 触发）
- 遍历 inbox items
- `mode === "write"`：保留
- 其他模式：settle as `unanswered: aborted` + 从 items 移除

---

## 4. Live 状态（`src/harness/live.ts`）

### LiveDoc 定义
- scope=conversation, history=latest, **fork=initial**（关键：fork 时 LiveDoc 一定为空）
- checkpointWhen：`generation === undefined && !tools.some(slot.status === "running")` —— 只在完全空闲时打 base

### LiveState 结构
```ts
{
  run?: { taskId, inputs: SubmissionId[] },     // 运行控制（仅 pi.generation 可拥有）
  generation?: {
    attempt: number,
    message?: JsonRepresentation<AssistantMessage>,  // **committed throttled partial**
    retry?: { at: number, error: string },           // **durable backoff**
    deferred?: { pollAt: number },                   // **provider-side deferred polling**
  },
  tools?: ToolSlot[],
  compactions?: CompactionStatus[],
}
```

### ToolSlot
```ts
{
  callId: string,
  name: string,
  taskId?: TaskId,                // 未启动时为空；模型未提供的调用以"done + entry"展示
  status: "pending" | "running" | "done",
  output?: string,                // 保留的运行中输出
  droppedBytes?: number,          // 超出限额被丢弃的字节
  droppedLines?: number,
  details?: JsonValue,            // 最后一次 details() 调用
  diagnostics?: ToolDiagnostic[], // api.diagnostic() 记录
  entry?: EntryId,                // 完成时的 result entry（faulted/orphaned 时无）
}
```

### CompactionStatus
```ts
{
  taskId, reason,                  // 压缩任务 ID 与原因
  blocking: boolean,               // 是否 generation 在等它
  attempt: number,
  retry?: { at: number, error: string },
}
```

### 关键函数
- `endRun(tx, live, taskId, settlement)` — 终结 run，结算每个 input，移除 generation/tools
- `addCompactionStatus(live, status)` / `removeCompactionStatus(live, taskId)`
- `toolSlot(live, taskId)` / `finishSlot(slot, entry)` / `clearProgress(slot)`
- `settleSchedulerOutcome(tx, record, outcome)` — scheduler 终结 outcome（faulted/orphaned）时清理：
  - `pi.tool`：finishSlot(undefined)，context 推导合成缺失 result
  - `pi.compaction`：移除 status
  - `pi.generation`（含其 variants）：convertPartial → 把 throttled 转为 aborted assistant entry（保留 token 到 `pi.usage`），endRun with reason

**注释强调**："REMINDER: a committed generation partial becomes an aborted assistant entry here, exactly as in the generation abort handler, so the transcript keeps what the model produced and `pi.usage` counts its spend."

---

## 5. Fork 实现（`src/session/forks.ts`）

### 关键发现：**Fork 确实复制文档，不是"不需要复制"**
- `prepareForkDocumentCopies(storage, parentId, at, childId, context)` → `ForkDocumentCopy[]`
- 每条生成新 `DocumentId`，地址从父转换到子
- 策略：
  - `parentConversationId` 对应的 entry 所在 conversation 的所有 conversation scope 文档按 `asOf` 复制（取分叉时刻快照）
  - `parentConversationId` 对应的所有 conversation scope 文档按 `current` 复制（取当前值）
- 去重：按 `addressId` 去重

### 条目不复制的机制
- 条目通过 `parent.at` 链回到父历史
- `entry.entry.conversationId` 可能不等于 `parentConversationId`（继承自更早的对话）
- Fork 遍历 = 子条目 + 父条目（沿 parent.at 边界）

### SCAN_PAGE_SIZE = 256

---

## 6. Harness 类层级（`src/harness/harness.ts`）

### 类层级
```
HarnessImpl extends SessionImpl implements HarnessType
SessionImpl（session/session.ts，session/transaction.ts）
```

### Conversation handle API（**比 v2 列的更丰富**）
- `submit(submission, ctx)` → `Submission`
- `agent(ctx)` — 获取当前 agent
- `configure(change, ctx)` — 在独立 commit 内修改
- `commit(change, ctx)` — 执行任意原子变更
- `context(ctx)` → `ContextView`
- `entries(query, limit, cursor, ctx)` → `Page<EntryRecord, Cursor>`
- `fork(at, options, ctx)` — 在 EntryId 处分叉
- `compact(instructions?, ctx)` → `TaskId<CompactionResult>` —— 手动压缩
- `reset(handoff?, ctx)` — 上下文重置（写一个 head="self" 的 ResetEntry）
- `abort(ctx, options?: {background?})` — `{background:true}` 也中止子任务
- `waitForIdle(ctx)`
- `viewState(ctx)` → `AttachedReplicatedState<ConversationView>`（基于 Chord 复制状态）
- `watch(ctx)` → `ConversationWatch`

### TaskScheduler 能力
- `open(ctx)` — 把 surviving `running` 任务 reconciliation 到 `pending`（崩溃恢复核心）
- `resume()` — 唤醒调度器
- `abort(taskId)` — 标记中止
- `abortConversation(id, background, ctx)`
- `waitForTask(id, ctx)` → `SettledTask`
- `waitForIdle(conversationId?, ctx)`

### Built-in 任务（Harness.open 时校验存在）
- `pi.generation`
- `pi.tool`
- `pi.compaction`
缺失时 open 报错："Registry lacks built-in tasks {names}; create it with createRegistry()"

### Built-in 文档（每个新建/分叉对话自动创建空实例）
- `pi.live`（LiveDoc）
- `pi.inbox`（InboxDoc）
- `pi.usage`（UsageDoc）
- `pi.agent`（AgentDoc）

### Harness 高级 API
- `inspect(ctx)` → `HarnessInspection`：`{scheduling: "paused"|"running"|"closing", tasks, submissions}`
- `usage(ctx)` → 聚合所有对话 `pi.usage` 的 UsageState
- `taskGraph(ctx)` → `AttachedReplicatedState<TaskGraph>`（任务父子关系 + 依赖图）
- `watchTaskGraph(ctx)`
- `getTask(id, ctx)`
- `submission(id, ctx)`
- `abortSubmission(id, ctx, conversationId?)`
- `abortTask(id, ctx)` → `"marked" | "terminal"`
- `waitForTask(id, ctx)`
- `waitForIdle(ctx)`
- `resume()`
- `root(ctx, options?)` — 创建根对话
- `conversation(id, ctx)` — 检索对话句柄
- `createConversation(options, ctx)`

### HarnessOptions
```ts
{
  models: Models,                                // pi-ai 接口
  registry: RegistryReader<Tool>,                // 扩展注册表
  settings?: HarnessSettings,
  env?: (target, ctx) => ExecutionEnv | undefined, // 异步；不在 Session 线上
  conversationCreated?: (tx, record) => ...,     // 创建钩子（每个创建/分叉的 commit 都跑）
  now?: () => number,                            // 可 mock
  onReport?: (error) => void,
}
```

### boundConversation（`ConversationHandle`）
- 绑到 `InvocationBinding`（带 abort signal）
- 每个操作先 `binding.check()`，再 `withAbortSignal(signal, context)`
- 中止时新操作被拒，但持久化工作继续（**v2 应该强调这条**）

---

## 7. 规范核心（`docs/spec.md` 第 1–600 行）

### 项目代号
**Pico5**（之前叫 Pico3）。包名 `@earendil-works/pi-durable`。

### 核心规则（一句话）
> A Session atomically commits immutable entries, full task records, and Chord-tracked documents. Only committed state is observable.

### 8 个 invariants（**应该写进 doc**）
1. 一个 Session commit 在所有记录和文档写上**原子**
2. 文档更新仅在**存储 commit 成功**后发布
3. 所有可见进度都是持久的；**没有易变发布路径**
4. 外部 effects **不在 Session mutation 事务中**执行
5. 条目和 ID 不可变，committed 后永不重用
6. 文档 draft 在事务回调结束时**完全撤销**；值必须 strict JSON
7. mutation line **在 settlement 期间保持**；observer 仅同步捕获不可变状态
8. 不确定的存储失败 = 致命 — 必须**重开 Session**

### 上下文推导（9 步算法，spec §2.1）
1. 找 cutoff 处或之前有 head 的最新可见 entry `H`
2. `from = H.head` 或转录开始
3. 扫从 `from` 到 cutoff 的可见 entry
4. 每个 target 取最新 edit（omit / replace with messages）
5. 如果 H 存在，context = [H] + 非 head 的 range；否则就是 range
7. 工具结果移到 assistant 之后（协议要求）
8. 缺失 result 合成错误；孤立 tool result 丢弃
9. 排除 aboted/error/deferred 的 assistant

### 提交（Submission）状态机
```
queued → placed → done (EntryId)
                  → unanswered (reason)
write 类型：queued → done (EntryId) 或 unanswered
```

### SubmissionDraft 类型
```ts
type SubmissionDraft = {
  requestId?: string,
} & (
  | { type: "input", content: UserInput, whenBusy?: "steer" | "followUp" | "reject" }
  | { type: "write", entry: EntryDraft }
)
```

### ContextEdit
- `{target: EntryId, action: "omit"}` — 不发送
- `{target: EntryId, action: "replace", messages: Message[]}` — 替换为指定消息
（注：原 spec 里写了 `{action:"omit", messages?: never} | {action:"replace", messages}` 的 discriminated union）

### 默认策略值（Settings 已解析后）
- retry: `{enabled: true, maxRetries: 3, baseDelayMs: 2000, maxAgentDelayMs: 60000}`
- compaction: `{enabled: true, reserveTokens: 16384, keepRecentTokens: 20000, backgroundTokens: 32768}`
- toolExecution: `"parallel"`
- steeringMode: `"one-at-a-time"`
- followUpMode: `"one-at-a-time"`

### AgentState（存在 `pi.agent` 文档里）
- model: ModelRef | undefined
- thinkingLevel: ModelThinkingLevel
- extensions: `string[]` 或 `{add?, remove?}` —— `string[]` 是精确选择，`{...}` 是编辑默认
- tools: `string[]` 或 `{remove}` —— 过滤已选扩展的工具
- instructions: string —— 渲染在所有扩展 section 之后，作为 `instructions` section
- cwd: string —— 传给 env builder

### AgentChange
- 每个字段：`set value` / `null: clear` / `undefined: no change`
- tools 字段可以是 ToolRegistration 对象或 `{remove: ToolRegistration[]}`
- extensions 同上

### 任何 Extensions 注册表层级
```ts
type Extension<Tool> = {
  promptSections?: PromptSection<Tool>[],
  hooks?: HookRegistration[],
  wraps?: Wrap<Tool>[],         // 包装其他扩展的 tool/section
}
type RegistryReader<Tool> = {
  snapshot(): RegistrySnapshot<Tool>
  // ... 可订阅变化
}
type RegistrySnapshot<Tool> = {
  task(name): TaskRecord
  // ...
}
```

---

## 8. 与 v2 文档对比的修订清单

### 应改为更精细的表述

1. **§4 八组接口**：v2 描述偏概念。补充：
   - 第 9 组：**任务图订阅**（`Harness.taskGraph()` / `watchTaskGraph()`）—— 第 3 类
   - 第 10 组：**对话视图 / 多端复制状态**（`viewState` / `watch`，Chord `AttachedReplicatedState<ConversationView>`）—— 这是 §10.3 多人协作的物理基础
   - 把"配置任务"重新组织为"每个对话的 AgentState 持久化（`pi.agent` rewindable 文档）"—— 解释了 v2 没有强调的"每个对话可独立改 model/tools/extensions/instructions/cwd"

2. **§8 核心机制**：
   - **8.3/8.5（流式提交 / 持久化运行状态）**：补 `throttled partial` 在 `LiveDoc.generation.message` 里持久化的实现细节（"committed throttled partial"），不只描述 100ms 周期。
   - 新增：8.6 **durable retry backoff** — `generation.retry: {at, error}` 让 retry 跨重启恢复，不靠内存计时器。
   - 新增：8.7 **deferred polling** — `generation.deferred: {pollAt}` 支持 provider-side deferred 响应轮询。
   - 新增：8.8 **8 个 invariants**（直接引用 spec）—— 这才是"一条原子提交"原则的精确含义。

3. **§9 崩溃恢复**：
   - 9.3 之前缺 **Scheduler.open(ctx) 的 running→pending reconciliation** —— 注释里写 "Reconcile surviving `running` tasks to `pending`; part of open"，这是 v2 没说的关键。
   - 9.3 之前缺 **aborted assistant entry** —— 当 generation 中途 crash，partial 转为 aborted assistant entry 入 transcript，`pi.usage` 仍计费。

4. **§10 派生功能**：
   - **10.1 fork —— 关键修正**：v2 说"不需要复制数据"，应改为"**条目不复**（按 ID + parent.at 链回父），**文档必须复制**（`prepareForkDocumentCopies` 生成新 DocumentId），按文档定义的 fork 策略决定值（asOf / current / initial）"。
   - 10.2 inbox —— 补 **stale write**（指向 active range 之前的 head）和 **steeringMode/followUpMode = "all"/"one-at-a-time"**。
   - 10.3 多人协作 —— v2 没提到 `viewState()` 返回 `AttachedReplicatedState`，这是 Chord 的复制状态原语。应补充：多客户端订阅 = `AttachedReplicatedState<ConversationView>`，每端独立操作，没有内存订阅关系（"白嫖" Chord）。
   - 10.4 subagent —— 通过创建 `ConversationCreateOptions{ownership: {kind:"task", taskId: parentTaskId}}` 的子对话 + `Harness.taskGraph()` 观察。
   - 10.5 运行中替换代码 —— 实际上**通过 AgentState.tools/extensions 编辑**或**wrapTool/wrapSection** 生效，不靠 LiveDoc 内置 reload。但 LiveDoc 的 `fork=initial` 含义是在 fork 时清空 in-flight 任务，给新代码一个干净起点。

5. **§12 存储与执行环境**：
   - storage adapter：`MemoryStorage`、`JSONLStorage`、`SQLiteStorage`（可能还有）—— 需确认
   - env：`HarnessOptions.env(target, ctx)` 是 per-conversation builder，**不在 Session 线上调用**（注释里写："Never called on the Session line; may be async."）

6. **§14.5 启发清单** —— 补充：
   - Aleph 需要思考：fork 时文档复制 vs 自己分叉时如何处理 in-flight 任务（`fork=initial` 是个好默认）
   - Aleph 需要思考：是否引入 `throttled partial` —— 让 partial assistant response 在 crash 后可见
   - Aleph 需要思考：durable retry backoff 让长跑任务跨重启不用重做
   - Aleph 需要思考：transaction 里写入 JSON 必须 strict JSON（draft 同步撤销），不能用 class instance

---

## 9. 仍未读取（按优先级）

1. `src/types.ts` 1085 行 —— 完整类型表（已部分知道）
2. `src/session/transaction.ts` 1023 行 —— Tx 接口、document mutation
3. ✅ `src/session/observation.ts` —— **不是** memo 机制；是 **Chord bridge**（`CommittedStateSource` / `CommittedWatch` / `SessionSourceAttachment`）
4. ✅ `src/harness/scheduler.ts` 1-1300 行 —— 任务调度核心 + memo 实现位置
5. `src/harness/generation.ts` —— GenerationTask 细节
6. `src/harness/tool.ts` —— ToolTask 细节（replay 协议在 tools/ 目录的 tool 框架里）
7. `src/harness/compaction.ts` —— CompactionTask 细节
8. `src/harness/context.ts` —— context 读取（ContextView 实现）
9. `src/harness/registry.ts` —— 扩展注册表细节
11. `docs/spec.md` 剩余 4002 行 —— §3 documents 详述 / §4 tasks / §5 hooks / §6 boundary / §7 registry / §8 compaction / §9 multi-client

---

## 10. observation.ts（**Chord bridge，不是 memo**）

`CommittedStateSource<T>` / `CommittedWatch<T>` 把 Session commits 桥接到 Chord 的多端复制状态对象。

- `CommittedStateSource`：一个 document / conversation view 的 source of truth。`#attachments: Set<SessionSourceAttachment>` 持有所有当前订阅者。`advance(value, ops, context)` 在 Session commit 后被调用，递增 `#cursor`、更新 `#value`、把 frame 广播给所有 attachment。value 为 `null` = retirement。
- `SessionSourceAttachment`：单个订阅者，持有当前快照 `{value, cursor}`；frames 进 `#frames: ReplicatedStateSourceFrame<T>[]` 缓冲；通过 `queueMicrotask` 异步投递（避免阻塞 Session line）。`MAX_PENDING_WATCH_FRAMES = 100` buffer 上限；超出后丢弃所有 pending 帧、推一个 replace 帧（`#replace` 回调或最新值）。
- `CommittedWatch<T>`：返回给消费者的 `WatchHandle<T>`。**串行化 exact-frame watch**——`#drain` 一次只投递一帧，等 listener 的 promise 解决后再取下一帧。支持 `observeCancellation(signal)`、`stop()`、`cancel()`。终止 reason：`"stopped" | "cancelled" | "session_closed" | "listener_error" | "retired"`。`RETIREMENT_OPERATIONS = [["r", null]]` 是规范化的 retirement op。
- **含义**：UI 不是从 source 拉数据，而是从自己的 attachment 拿快照 + 异步增量；Session 主线不等 UI。**"白嫖 Chord"在这里变成一个异步订阅桥**。

---

## 11. scheduler.ts（任务调度核心 + memo）

### Task 状态机

5 个状态：
- `pending` — 等待被调度器捡起
- `running` — 当前正在跑 phase handler
- `waiting` — phase handler 主动等待某个外部事件（如 sleep、watchDoc、waitForTask）
- `completing` — phase handler 已返回最终结果，正在收尾（结算 own 的 conversation / 子任务）
- `terminal` — 结算完成（持 `TaskOutcome`：`completed` / `faulted` / `orphaned`）

`LIVE_STATUSES = ["pending", "running", "waiting", "completing"]`——`#live` map 只装这 4 个状态。`terminal` 不在 `#live` 里。

### 所有权树（spec §5.5）

每个 task 有一个 `owner: TaskId?`（归属它的父 task）。每个 conversation 有一个 `owner: TaskId?`（归属它的 task）。
- task.parent = task.owner ?? task.conversationId
- conversation.parent = conversation.owner
- 形成一棵树，根是 ownerless conversation

**abort cascade 沿这棵树向下走**：标 `abortRequested` 后，walk up 检查父链上是否还有未 mark 的祖先；walk down 检查自己 own 的 conversation / 子 task。

### TaskScheduler.open() 的 reconciliation

```ts
async open(context: Context): Promise<void> {
  this.#session.subscribeCommits((publication) => this.#observe(publication));
  this.#session.subscribeClose(() => this.#seal());
  this.#unsubscribeRegistry = this.#registry.subscribe(() => this.#kick());
  await this.#session.commitWith(async (tx) => {
    for (const status of LIVE_STATUSES) {
      const records = await scanAll((cursor) => tx.scanTasks({ status }, SCAN_PAGE_SIZE, cursor));
      for (const record of records) {
        this.#live.set(record.id, record);
        if (record.state.status === "running") {
          // ⭐ running → pending，重启时 surviving task 全部改回 pending
          tx.setTask(withState(record, { status: "pending", checkpoint: record.state.checkpoint }));
        }
        if (record.state.status === "waiting" && record.state.policy === "failFast") {
          this.#failFastChecks.add(record.id);
        }
      }
    }
  }, context);
  this.#cascadePending = true;
  this.#scheduleReconcile();
}
```

**关键点**：
1. 扫描 4 个 LIVE_STATUSES 的所有记录（分页 `SCAN_PAGE_SIZE = 256`）
2. 所有 `running` 任务**原子地**改为 `pending`，checkpoint 保留
3. `waiting` 且 policy = `failFast` 的任务加入 `#failFastChecks`，等 reconcile 时检查兄弟任务是否失败
4. `cascadePending = true` 触发后续 reconciliation，**推算 crash 时未应用的 abort 标记**（沿所有权树向下走）

### Session line 序列化

```ts
/**
 * Invariant: every task transition is decided and written by one callback serialized on the Session line.
 * That covers reservation, marks, runtime commits, finalization, and the synchronous step before each phase.
 * Handlers and joins run off the line.
 */
```

**核心 invariant**：task 状态转换**全部**在 Session 主线上同步进行——reservation、abort mark、runtime commit、finalization、phase 前的 precedence 检查。Handler 和 join 在主线外跑。
**含义**：runtime 写的 commit 也走 `commitWith` 进入主线，所以"phase 还在跑但写了 commit"的中间态被主线序列化保证。

### Invocation（task 的内存执行）

```ts
type Invocation = {
  readonly taskId: TaskId;
  readonly conversationId: ConversationId;
  readonly mode: "run" | "abort";           // 两种模式
  readonly controller: AbortController;       // 自己的 abort 信号
  readonly context: Context;                  // 上下文
  readonly watches: Set<DocumentWatch<...>>;  // runtime 通过 watchDoc 开的订阅
  ended: boolean;                              // handler 是否结束
  readonly done: Promise<void>;               // 完成的 promise
  readonly finish: () => void;
};
```

- 一个 task 同一时间只有一个 `mode === "run"` 的 invocation
- abort 模式 invocation 在 run 完成且 own 工作结束后才启动
- `controller` = invocation 自己的 abort signal；外部 `abort(taskId)` 触发它
- runtime 通过 `watchDoc` 开的订阅会在 invocation 结束时自动 `stop()`（`invocation.watches.delete(watch)` after `watch.closed`）

### #gated — 写 commit 的统一入口

```ts
#gated<T>(invocation, change, context): Promise<T> {
  if (invocation.ended) return Promise.reject(endedError(invocation));
  return this.#session.commitWith(async (tx) => {
    if (invocation.ended) throw endedError(invocation);
    if (this.#closing) throw closedError();
    const found = this.#live.get(invocation.taskId);
    if (found === undefined) throw new Error(`Task ${...} is terminal`);
    if (found.state.status !== "running") throw new Error(`Task ${...} is ${found.state.status}`);
    const current = found as ErasedRunningTask;
    if (invocation.mode === "run" && current.abortRequested) {
      throw new Error(`Task ${...} has a durable abort mark`);
    }
    return change(tx, current);
  }, context);
}
```

每次 commit 之前**重新检查** task 是否还在 running、invocation 是否还没结束、是否已被 abort。**这是 runtime 不能"随便"写 commit 的原因**——所有写都要走这道闸。

### memo 实现

```ts
memo: ((name: string, ...rest: readonly unknown[]) => {
  if (rest.length === 1) {
    // 只读：runtime.memo(name) → 当前 task 的 memo[name] 或 undefined
    return this.#read(invocation, async () => memoOf(this.#live.get(invocation.taskId), name));
  }
  const candidate = rest[0] as JsonValue;
  // 写入或返回：runtime.memo(name, value) → 如果已有 memo[name] 返回它；否则原子写入并返回 candidate
  return this.#gated(invocation, (tx, current) => {
    const winner = memoOf(current, name);
    if (winner !== undefined) return winner;
    tx.setTask({ ...current, memos: { ...current.memos, [name]: candidate } } as AnyTaskRecord);
    return candidate;
  }, rest[1] as Context);
}) as ErasedRuntime["memo"],

function memoOf(record: AnyTaskRecord | undefined, name: string): JsonValue | undefined {
  const memos = record?.memos;
  return memos !== undefined && Object.hasOwn(memos, name) ? memos[name] : undefined;
  // ⭐ Object.hasOwn：防止 "toString" 等继承属性
}
```

### withState — memo 在结算时被剥掉

```ts
/** Replace a live record's state; memos disappear once an outcome is decided. */
function withState(record: AnyTaskRecord, state: TaskState<...>): AnyTaskRecord {
  if (state.status !== "terminal" && state.status !== "completing") return { ...record, state } as AnyTaskRecord;
  const { memos: _memos, ...rest } = record;
  return { ...rest, state };
}
```

**关键事实**：task 进入 `completing` 或 `terminal` 时，`memos` 字段**被剥掉**。
- 这意味着 memo 是"还在跑"的辅助存储，**不能携带进最终 outcome**
- 但**运行中** crash 的话，`running` → `pending` 不触发 `withState` 剥 memo——memo 跨重启还在
- 直到任务真正完成（`completed`）才剥掉

### abort(taskId) 流程

```ts
async abort(id, context): Promise<"marked" | "terminal"> {
  const marked = await this.#session.commitWith(async (tx) => {
    const current = await tx.task(id);
    if (current.state.status === "terminal") return { result: "terminal" };
    const invocation = this.#invocations.get(id);
    if (invocation === undefined && current.state.status !== "completing") {
      await this.#loadScopes(false);
      if (!this.#ownedLive().has(id)) {
        // task 自身不活（无子任务 / 子对话在跑）且没有定义能接 → settle as orphaned
        const resolution = this.#resolve(current, this.#registry.snapshot());
        if (resolution.kind === "blocked") {
          await this.#terminate(tx, current, { status: "orphaned", reason: resolution.reason });
          return { result: "marked" };
        }
      }
    }
    if (!current.abortRequested) tx.setTask({ ...current, abortRequested: true });
    return { result: "marked", run: invocation?.mode === "run" ? invocation : undefined };
  }, context);
  if (marked.run !== undefined) await awaitWithContext(marked.run.done, context);
  return marked.result;
}
```

四种归宿：
1. 已经 terminal → 立即返回 `"terminal"`
2. 有 invocation 在跑 + 无 owned live work → settle as `orphaned`（**直接终止，不走 abort handler**）
3. 有 invocation 在跑 + 有 owned live work → 设 `abortRequested: true`，等 run 自然结束
4. 无 invocation 在跑 → 设 mark，等下次 reconcile 唤醒 abort handler

### MAX_TIMER_DELAY

```ts
const MAX_TIMER_DELAY = 2_147_483_647;  // ~24.8 天
```

`#sleep(until, context)` 把长 sleep 分段：`delay(Math.min(remaining, MAX_TIMER_DELAY), signal)` 每次最多等 24.8 天，循环重检时钟。**含义**：sleep 本身是"被动让步"，跨重启不需要持久化——重启后从头算就好。

---

## 12. 已读 / 待补清单

- ✅ tool.ts（src/harness/tool.ts）
- ✅ generation.ts（src/harness/generation.ts）
- ✅ compaction.ts（src/harness/compaction.ts）
- ❌ transaction.ts（src/session/transaction.ts，1023 行）——下次读
- ❌ spec.md §3-§9 剩余

---

## 13. tool.ts（ToolTask 实现 + replay 协议）

### ToolTask 两阶段

```
phase "call" (initial)            phase "execute" (recovery only)
```

- `call` 阶段：
  - 读 call (assistant entry 的 toolCall by id)
  - 在 agent.tools 里找 tool，找不到 → settle as `tool_unavailable` `completed`
  - 跑 `prepareArguments`（tool 自己的 repair），出错 → settle as `invalid_arguments` `completed`
  - 跑 `beforeTool` hooks；可设 `block: string` 或改 `args`
  - **commit checkpoint** `{phase: "execute", arguments: final, replay: tool.replay ?? "unsafe"}` 到 `pi.live.tools` slot
  - 然后执行
- `execute` 阶段（**仅由恢复路径进入**）：
  - 读 storage 里 stored args + stored replay policy
  - **当且仅当 stored 和 current 都声明 `safe` 才重放**：清空 progress，从头跑
  - 否则 → settle as `failed`，message = "Tool ... was interrupted and may have partially run"
    - **这次 `failed` 状态会触发 cascade abort**——该 tool call own 的 conversations 都自动中止
    - spec 注释："`failed` records cancellation intent, so the call's owned conversations, left unsupervised, are aborted."

### `abort` handler

- 读 call
- settle as `aborted`，从 slot 的 partial output 构造结果

### `run` helper（核心执行逻辑）

构建 `ToolExecutionApi` 传给 tool.execute(args, api, context)：
- `taskId`, `conversationId`, `callId` — 身份
- `registry` — 扩展注册表
- `agent(runtime)` — 当前 agent
- `output(chunk)` — push 到 OutputBuffer，触发 progress.mark()
- `diagnostic(d)` — push 到 reported.diagnostics，触发 progress.mark()
- `details(value)` — set details + `markAndWait()` 等待 commit 完成（**让 details 在 terminal commit 前必然持久化**）
- `commit(change, ctx)` — runtime 写 commit 的入口
- `memo(name)` — runtime.memo
- `createTask(task, input, options, ctx)` — **创建子任务**（subagent 用）；走 runtime.commit
- `getTask` / `waitForTask` / `conversation` / `snapshot` / `snapshotAsOf` / `watchDoc`

**env 在 execute() 调用前才构建**："Built for this call, so a rerun after recovery gets the conversation's environment at that time."——env 不持久化，但每次（包括重放）都从当前 settings 重新取。

### Progress（throttled partial commit）

`new Progress(commit, onError)`：
- `mark()` — 调度一次 throttled commit（不立即执行）
- `markAndWait()` — 调度并返回 promise
- `stop()` — 停止 throttle + 返回所有 pending waiters

commit 函数捕获 reported.output.snapshot() + reported.details + reported.diagnostics，**diff 出来**只写新内容：
```ts
// 文本：shared 部分不写，只写新增。Chord diff 编码为 append / trim+append / replace
const shared = snapshot.text.startsWith(written.text)
    ? written.text.length
    : overlap(written.text, snapshot.text, 65_536);
bytes += utf8ByteLength(snapshot.text.slice(shared));
// details：JSON.stringify 比对
if (detailsChanged) bytes += utf8ByteLength(JSON.stringify(current.details ?? null));
// diagnostics：slice(written.diagnostics, current.diagnostics)
if (added.length > 0) bytes += utf8ByteLength(JSON.stringify(added));
```

**含义**：commit 文本由 Chord diff 编码为 append 或 trim+append——这就是"白嫖 Chord"的另一个具体体现。

### Tool error / abort 三种归宿

```ts
try {
    result = await tool.execute(args, { ...api, env }, context);
} catch (error) {
    if (runtime.signal.aborted) { throw error; }  // abort → re-throw
    result = { isError: true, diagnostics: [...] };
    ending = { status: "failed", message: `Tool ${call.name} threw` };  // cascade abort
}
// 注意：result.isError=true 仍走 COMPLETED（不触发 cascade abort）
```

**三种归宿表**：
| 情况 | ending | 效果 |
|------|--------|------|
| 正常返回 | COMPLETED | 完成 |
| tool 抛错（非 abort） | failed | cascade abort owned conversations |
| 抛错但 runtime.signal.aborted | re-throw | abort handler 接走 |
| result.isError=true | COMPLETED | 完成但带 error |

### `appendToolResult`（结果条目持久化）

```ts
tx.appendEntry(ToolResultEntry, conversationId, { model: [message], data: { diagnostics } });
```

- content 末尾追加 rendered diagnostics 文本
- `data.diagnostics` 保留结构化诊断列表
- `result.usage !== undefined` → `recordUsage(tx, conversationId, "tools", call.name, result.usage)` 在同 commit 内
- `result.control` 用 `copyJson({omitUndefinedProperties: true})` 严格 JSON 化

### `settle`（结束阶段统一入口）

所有终态都走 `settle(runtime, call, ending, build, context)`：
- build(slot) — 用 slot 的 durable partial output 构造 ToolExecutionResult
- commit：
  1. 调 build 构造 result
  2. appendToolResult(tx, conversationId, call, result, runtime.now())
  3. finishSlot(slot, entry.id) — 在 slot 上收尾
  4. 根据 ending 返回 task state：
     - `aborted` → `{status: "terminal", outcome: {status: "aborted", result: {entryId}}}`
     - `failed` → `{status: "terminal", outcome: {status: "failed", error: {message}, result: {entryId}}}`
     - `completed` → `{status: "terminal", outcome: {status: "completed", result: {entryId, ...control}}}`

---

## 14. generation.ts（GenerationTask 5 阶段机）

5 个 phase：`prepare` → `request` → `classify` → (retry | poll) | answer | (tools round)

### prepare

```ts
const view = await runtime.context(conversationId, context);  // ⭐ 全文 context 视图
const shown = replaySections(view.messages);
const input = { conversationId, agent, env, shown, read: runtime };
const desired = await renderSections(agent.sections, input, shown, report, context);
const entries = planSystemEntries(view, desired, agent.tools, runtime.now());
const threshold = thresholdCompaction(view, entries, resolved.contextWindow, settings.compaction);
// threshold = "blocking" | "background" | undefined

if (threshold === "blocking") {
    // ⭐ 生成 blocking compaction（归 generation 自己），等待它
    await runtime.commit(async (tx) => {
        const child = await createCompaction(tx, conversationId, { reason: "threshold" }, runtime.taskId);
        return { status: "waiting", checkpoint: { phase: "prepare", attempt, compacted: child }, on: [child], policy: "allSettled" };
    }, context);
    return;
}

await runtime.commit(async (tx) => {
    let cutoff = (await tx.scanEntries({ conversationId }, 1)).items[0]?.id;
    for (const entry of entries) cutoff = (await tx.appendEntry(SystemEntry, conversationId, entry)).id;
    if (threshold === "background") {
        // 后台 compaction，**仅在没有正在跑的 compaction 时启动**
        if ((await tx.doc(LiveDoc, conversationId)).compactions === undefined) {
            await createCompaction(tx, conversationId, { reason: "threshold" });
        }
    }
    return { status: "running", checkpoint: { phase: "request", ...request } };
}, context);
```

### request

- 设 `live.generation = { attempt }`
- `convertPartial(tx, live, conversationId)` 把之前 crashed 留下的 partial 结算为 `aborted` assistant entry
- `streamResponse` 流式拉模型：
  - **100ms throttled partial commits**（`PARTIAL_THROTTLE_MS = 100`）
  - 每次 flush 把 partial 写进 `live.generation.message`
  - **一次只有一个 in-flight commit**（`inFlight: Promise<void> | undefined`）
  - `finally` 停 throttle + await inFlight，**保证 terminal commit 之前不会有 stale partial**

### classify（分类响应）

| stopReason | 处理 |
|------------|------|
| `deferred` + `deferred: handle` | 切到 `poll` 阶段，`pollAt = max(now + handle.pollAfterMs ?? 5000, lastPollAt + 1)` |
| `toolUse` + calls | `startToolRound` |
| `stop` / `length` / `toolUse` (空) | `answer` |
| `error` + `isContextOverflow` + 可压缩 | `createCompaction(..., {reason: "overflow"})`，从 prepare 重试 |
| `error` + retryable | `retry` 阶段，`until = now + retryDelayMs(...)` |
| 其他 | `failed` terminal |

### retry

```ts
await runtime.sleep(until, context);  // 跨重启 sleep 不持久化——重启从头算
await runtime.commit(async (tx) => {
    (await tx.doc(LiveDoc, ...)).generation = { attempt: attempt + 1 };
    return { status: "running", checkpoint: { phase: "prepare", attempt: attempt + 1, ... } };
}, context);
```

### poll

```ts
await runtime.sleep(pollAt, context);
const message = await runtime.models.fetchDeferred(model, handle, { signal: runtime.signal });
await classify(runtime, request, message, context);
```

### tools（处理 sequential tool round）

```ts
const [next, ...rest] = pending;
await runtime.commit(async (tx) => {
    const taskId = await createToolTask(tx, runtime, assistant, next);
    return { status: "waiting", checkpoint: { phase: "tools", ..., tools: [...tools, taskId], pending: rest }, on: [taskId], policy: "allSettled" };
}, context);
```

每次只起一个 tool task，等它结束再起下一个。

### answer（最终回答）

- 跑 `onYield` hooks（successor 接力）
- `prepareBoundary` + `applyBoundary(tx, boundary, "final", now)` 拿 queued user items + reset
- 若有 continuation hook + 无 boundary user item + 无 reset → append UserEntry、createGeneration、handOver run、terminal
- 否则 → endRun as `done`，若有 users 则 startRun

### abort

- 若 poll 阶段 → `cancelDeferred`
- 对所有 pending（unstarted）tool calls → append `tool_unavailable` 结果
- 设 `live.generation = { status: "unanswered", reason: "aborted" }`
- 终态 aborted

### thresholdCompaction 决策

```ts
function thresholdCompaction(view, planned, contextWindow, policy): "blocking" | "background" | undefined {
    if (!policy.enabled || contextWindow <= 0) return undefined;
    const tokens = estimateContext(view, planned.flatMap(e => e.model ?? []));
    const blocking = contextWindow - policy.reserveTokens;
    const background = blocking - policy.backgroundTokens;
    const over = tokens > blocking ? "blocking"
              : policy.backgroundTokens > 0 && tokens > background ? "background"
              : undefined;
    if (over === undefined || selectCut(view, policy.keepRecentTokens) === undefined) return undefined;
    return over;
}
```

**关键**：
- `blocking` 触发后立即做 → 占用 generation prepare，让 generation 等
- `background` 不阻塞 → 但**仅在没 compaction 在跑时**启动（`live.compactions === undefined`）
- `selectCut === undefined` → 没东西可压，不启动

### `streamResponse` 的 partial 节流细节

```ts
const flush = (): void => {
    timer = undefined;
    const partial = pending;
    pending = undefined;
    if (partial === undefined || stopped) return;
    inFlight = (async () => {
        const message = copyJson(partial, { omitUndefinedProperties: true });
        await runtime.commit(async (tx) => {
            const live = await tx.doc(LiveDoc, runtime.conversationId);
            live.generation ??= { attempt };
            assignJson(live.generation as Draft<Record<string, JsonValue>>, "message", message);
            return undefined;
        }, context);
    })().catch(error => {
        if (!runtime.signal.aborted) runtime.report(error);
    }).finally(() => {
        inFlight = undefined;
        if (pending !== undefined && !stopped) timer = setTimeout(flush, PARTIAL_THROTTLE_MS);
    });
};
```

**保证**：
- 同一时刻最多一个 in-flight commit
- abort 后 partial commit 的 reject 被吞掉（committed state 仍一致）
- terminal commit 之前所有 partial 都已 flush（`finally { stopped = true; clearTimeout(timer); await inFlight; }`）

---

## 15. compaction.ts（CompactionTask 3 阶段机 + 接入 write submission）

### 3 阶段：`select` → `summarize` → `retry` (loop)

### select

- 解析 model + settings.compaction
- `view = runtime.context(conversationId)` — 全文 context 视图
- `cut = selectCut(view, policy.keepRecentTokens)`
- 若 cut === undefined → `complete()` (无总结)
- 跑 `beforeCompact` hooks：hook 可 `decline: true`（拒绝） 或直接返回 `summary: string`（跳过模型）
- 若 decline → complete
- 若 hook 给 summary → 直接 `place`
- 否则 → commit `{phase: "summarize", ...SummaryRequest}`

### summarize

- view = `runtime.context(conversationId, context, tail)` — 以 tail 为 cutoff
- 找 cut = `view.entries.findIndex(e => e.id === firstKept)`
- 构造 messages：`[system: SUMMARIZATION_SYSTEM_PROMPT, user: summaryPrompt(...)]`
- `runtime.models.completeSimple(model, {messages}, {cacheRetention: "none", maxTokens, signal, ...})`
- `summaryText(message)` 提取纯文本（stopReason === "stop" + 无 tool call + 非空文本）
- 若 retryable error → `retry` 阶段
- 否则 commit `placeSummary(tx, runtime, current, live, firstKept, summary)`

### placeSummary（关键：两条路径）

```ts
const result: CompactionResult =
    current.owner === undefined
        ? {
            // ⭐ conversation-owned compaction：走 write submission 协议
            submissionId: await admitSubmission(
                tx,
                runtime.conversationId,
                { type: "write", requestId: `compaction:${runtime.taskId}`, entry },
                runtime.now(),
                runtime.settings,
            ),
        }
        : {
            // ⭐ task-owned (blocking) compaction：直接 append
            entryId: (await tx.appendEntry(runtime.conversationId, entry)).id,
        };
```

**关键不变量**：spec §2.4 "nothing else may append to a busy conversation, so every non-blocking summary goes through admission"——非阻塞 compaction 必须走 Inbox admission 协议。

### createCompaction（任务创建）

```ts
const ownership = owner === undefined
    ? ({ kind: "conversation" } as const)
    : { kind: "task" as const, taskId: owner };
const background = owner === undefined && input.reason !== "manual";
const taskId = await tx.createTask(CompactionTask, input, { ownership, conversationId, background });
const status = { taskId, reason: input.reason, blocking: owner !== undefined, attempt: 1 };
addCompactionStatus(await tx.doc(LiveDoc, conversationId), status);
return taskId;
```

**`background: true` 是任务选项**：标记 task 为"background"，意味着 abort cascade 时它**不被波及**——前台用户按 Esc 不会中止后台 compaction。

### selectCut 算法（cut 选择）

```ts
function selectCut(view: ContextView, keepRecentTokens: number): number | undefined {
    const start = view.head === undefined ? 0 : 1;
    const candidates: number[] = [];
    for (let index = start; index < contributions.length; index++) {
        if (isCandidate(contributions, index)) candidates.push(index);
    }
    let kept = 0;
    let cut: number | undefined;
    for (let index = contributions.length - 1; index >= start; index--) {
        for (const message of contributions[index]!) kept += estimateMessageTokens(message);
        if (kept < keepRecentTokens) continue;
        cut = candidates.find(candidate => candidate >= index) ?? candidates.at(-1);
        break;
    }
    if (cut === undefined) return undefined;
    for (let index = start; index < cut; index++) {
        if (contributions[index]!.length > 0) return cut;
    }
    return undefined;
}
```

**关键**：
- 跳过第一个 entry 当 `view.head !== undefined`（上一个 summary 的 anchor）
- candidates 是 user/assistant 起点位置（不能从 tool result 开始，因为会破坏多轮对应关系）
- 从尾往头累积 token，找到第一个 `kept >= keepRecentTokens` 的位置
- 选 `>= index` 的最小 candidate
- 若 cut 前没有有效贡献 → undefined（不压缩）

### isCandidate（候选判定）

```ts
function isCandidate(contributions, index): boolean {
    const first = contributions[index]![0];
    if (first?.role === "assistant") return true;
    if (first?.role !== "user") return false;
    // user entry 前面若还有未结算 tool results 不能切（否则 orphan tool result）
    let calls = new Set<string>();
    for (let before = index - 1; before >= 0; before--) {
        const assistant = contributions[before]!.findLast(m => m.role === "assistant");
        if (assistant === undefined) continue;
        calls = new Set(assistant.content.flatMap(c => c.type === "toolCall" ? [c.id] : []));
        break;
    }
    if (calls.size === 0) return true;
    for (let after = index; after < contributions.length; after++) {
        for (const [position, message] of contributions[after]!.entries()) {
            if (message.role === "assistant" && (after > index || position > 0)) return true;
            if (message.role === "toolResult" && calls.has(message.toolCallId)) return false;
        }
    }
    return true;
}
```

**含义**：user entry 前面若还有未完成的 tool results 不能切——否则把 orphan tool result 留给 summary 但 user 提示已无对应工具调用。

### estimateContext

```ts
function estimateContext(view, extra): number {
    // 找最新一个 assistant message，usage > 0 且 id > view.head?.id
    let measured: AssistantMessage | undefined;
    const after = view.head?.id ?? Number.NEGATIVE_INFINITY;
    for (let i = view.entries.length - 1; i >= 0 && measured === undefined; i--) {
        if (view.entries[i]!.id <= after) continue;
        measured = view.contributions[i]!.findLast((m): m is AssistantMessage =>
            m.role === "assistant" && calculateContextTokens(m.usage) > 0
        );
    }
    const from = measured === undefined ? 0 : view.messages.lastIndexOf(measured) + 1;
    let tokens = measured === undefined ? 0 : calculateContextTokens(measured.usage);
    for (const message of view.messages.slice(from)) tokens += estimateMessageTokens(message);
    for (const message of extra) tokens += estimateMessageTokens(message);
    return tokens;
}
```

**含义**：从最新一个有 usage 的 assistant 开始算——它之前的内容是已计过费的，不需要重新算 token；之后的内容按消息估算。`extra` 是 `planSystemEntries` 计划要 append 的 system messages（参与阈值判断但不计入 transcript）。

### `serializeConversation`（summary 用的 transcript 序列化）

messages → `[User]: ...` / `[Assistant]: ...` / `[Tool result]: ...`（>2000 字符的截断）— 不显示系统消息，让 summarizer 读 transcript 而不是 continue conversation。

---

## 16. transaction.ts（位于 src/session/transaction.ts）

已读。1023 行。

### Tx 接口：每个 callback 一次事务

`class Transaction implements Tx` 是 Session commit 回调里的那个事务对象：

```
├── #writes: StorageWrite[]            // 原子批次，所有写都先到这里
├── #pendingOperations: Set<Promise<unknown>>  // 追踪未完异步，结算时拒绝/drain
├── #sealed: boolean                    // 一旦结算，所有操作都抛
├── #hasTableWrite: boolean             // 一旦写过，读就抛 ReadAfterWrite
├── #tasksById: Map<TaskId, TransactionTask>
├── #submissions: Map<SubmissionId, SubmissionRecord>
├── #submissionChanges: {id, change}[]  // 按 staging 顺序存
├── #documents: DocumentEntry[]         // 全部文档获取/退休的清单
├── #plans: DocumentPlan[]              // 装配后所有写入计划
└── #latestDocumentByAddress: Map<string, DocumentEntry>
```

### Table reads（包裹在 `#read` 里）

- `conversation(id)`, `entry(id, kind?)`, `task(id)`
- `scanConversations`, `scanEntries`, `scanTasks`
- `latestHeadMarker(conversationId)`
- `submission(id)`, `submissionByRequest(conversationId, requestId)`

**ReadAfterWrite**：`#read` 检查 `#hasTableWrite`，一旦写过 → 抛 `ReadAfterWrite`。**含义**：callback 内读必须早于写；写过之后还想读就是逻辑错误。

### Table writes（包裹在 `#write` 里）

- `createConversation({ownership})` → `#stageConversation(undefined, ownership)` 生成新 conversationId
- `createRootConversation()` → 同上但用 `ROOT_CONVERSATION_ID`（保留的根身份）
- `forkConversation(parentId, at, {ownership})` → `#stageConversation({conversationId: parent, at}, ownership)`
- `appendEntry(conversationId, draft)`：
  - `head: "self"` → 设为该 entry 自己的 id（重置）
  - `byTaskId: scope.taskId` → 记录这条 entry 是哪个 task append 的
- `createTask(task, input, options)`：
  - `ownership.kind === "task"` → 校验 owner 存在 + 子 task 不能 background + conversationId 必须和 owner 一致
  - record： `{id, conversationId, kind, version, input, owner?, background: options.background ?? false, abortRequested: false, state: {status: "pending", checkpoint}}`
- `createSubmission(create)` → 原始提交创建，无 admission
- `settleSubmission(id, settlement)` / `placeSubmission(id, entry)` → 提交变更到 staging
- `setTask(value)` → 内部：task 改自己的 state 走这条
- `stagedTasks()`, `stagedConversations()` → 内部：列出本事务 staging 的 record

### Documents（`doc` 和 `retireDoc`）

四个重载：`SessionDoc`, `ConversationDoc`, `TaskDoc` + 三种 family 变体（带 key 和 seed）。返回值 `Draft<JsonObject>`——Chord 追踪器暴露的可变代理。

**`retireDoc(token, ...)`**——退休一个文档 incarnation。

**文档路径**：
- fork-copy 类型的 target → 直接复用 fork 准备的拷贝记录
- 已存在的地址 → `latest` 缓存或重新加载
- 全新 → `#acquire` 调 `definition.initial()` 起步

### Settlement（结算）

| 方法 | 用途 |
|------|------|
| `settleFailure()` | callback 失败 → abort 所有 change + drain pending |
| `settleSuccess()` | callback 成功 → seal + prepare 所有 change + assemble 原子批次 |
| `discard()` | Storage 失败或不需要写 → abort 已 prepared change |
| `adopt(seq)` | Storage 成功后 → pointer swap 拿 tracker |

**关键不变量**：`adopt(seq)` 只在 storage 写成功后调用。失败走 `discard()`。这正是"提交完成之前不展示任何东西"——tracker swap 才是"可见"。

### `#validateOwners`（spec §5.5）

```ts
/** A task therefore cannot create owned work in the commit that finishes it (spec §5.5). */
async #validateOwners(): Promise<void> {
    // 收集所有写了 owner 的 conversation/task
    for (const { what, taskId } of owners) {
        const task = await this.#currentTask(taskId);
        if (task === undefined) throw new Error(`${what} ${taskId} does not exist`);
        if (task.state.status === "terminal" || task.state.status === "completing") {
            throw new Error(`${what} ${taskId} is ${task.state.status}`);
        }
        if (task.abortRequested) throw new Error(`${what} ${taskId} is abort-marked`);
    }
}
```

**含义**：一个 task 在自己的结算 commit 里不能再创建 owned work（conversation 或 owned task）。避免 "task A 即将完成时又开了个 task B，但 A 死了没人管 B" 的孤儿局面。

### `#rejectForkSourceWrites`（fork transaction 限制）

- 不能改 fork 源文档（`#forkSourceDocumentIds`）
- 不能在 fork transaction 里改 `current`-policy 文档——因为 fork 要拷贝它们

**含义**：fork 是隔离动作——同一 commit 不能又 fork 又改原 conversation 的 current 状态。

### 文档计划（`planDocument`）

每个 `DocumentEntry` 转成 `DocumentPlan`：

| target kind | 写出内容 |
|-------------|----------|
| `created` | `document.create` + base 值 |
| `fork-copy` | `document.copy` + source |
| `loaded` + version 变了 | `document.change` + base 值 |
| `loaded` + 只有 ops | `document.change` + delta |
| `loaded` + 啥都没变 | 不写 content（但可能 retire） |
| `retire-only` | 只 record，无 content |

**`publishes(plan)`**：决定哪些 plan 需要 publish 给 observers：
- 每次创建、拷贝、退休 → publish
- 加载的 incarnation：仅当写了 content 时 publish（包含 version 迁移的 base——让老 shape 的观察者拿到新值）

### Terminal 时任务文档退休

```ts
if (terminalTaskIds.size > 0) {
    const retiring = new Set<DocumentId>();
    for (const plan of plans) {
        const scope = plan.record.scope;
        if (scope.kind !== "task" || !terminalTaskIds.has(scope.taskId)) continue;
        plan.retire = true;
        retiring.add(plan.record.id);
    }
    // 扫存储补齐还没 staged 的 task-scope 文档
    for (const taskId of terminalTaskIds) {
        if (this.#tasksById.get(taskId)?.write?.kind === "create") continue;
        let cursor: Cursor | undefined;
        do {
            const page = await storage.scanDocuments({scope: {kind: "task", taskId}, at: "current"}, 256, cursor, ...);
            for (const record of page.items) {
                if (retiring.has(record.id)) continue;
                plans.push({addressId: ..., record, retire: true});
                retiring.add(record.id);
            }
            cursor = page.next;
        } while (cursor !== undefined);
    }
}
```

**含义**：task 进入 `terminal` → 它拥有的所有 task-scope 文档**自动退休**（不需要每个文档单独写）。`create` 的 task 因为本事务内还未 persist，跳过 scan。

---

## 17. Tx 与 Storage 的分层

读完 transaction.ts 后重画 §8.2 "一条原子提交" 的全貌：

```
┌──────────────────────────────────────────────────────────────┐
│  Session commit (一行串行)                                  │
│                                                              │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │  Transaction (Tx) — callback 的事务对象                 │ │
│  │  - 收集 reads/writes 到 staging                         │ │
│  │  - ReadAfterWrite 保护                                  │ │
│  │  - validation (#validateOwners, #rejectForkSource)     │ │
│  └─────────────────────────────────────────────────────────┘ │
│           │                                                  │
│           ▼ settleSuccess → assemble                         │
│           │                                                  │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │  StorageWrite[] 原子批次                                │ │
│  │  conversation | entry | submission | task               │ │
│  │  document.create | document.copy | document.change     │ │
│  │  document.retire                                        │ │
│  └─────────────────────────────────────────────────────────┘ │
│           │                                                  │
│           ▼ storage.commit()                                  │
│           │                                                  │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │  Storage (SQLite / JSONL / ...pluggable)                │ │
│  │  原子提交                                                  │ │
│  └─────────────────────────────────────────────────────────┘ │
│           │                                                  │
│           ▼ adopt(seq)                                        │
│           │                                                  │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │  Chord Trackers — in-memory mutation observers          │ │
│  │  pointer swap → "可见"                                  │ │
│  └─────────────────────────────────────────────────────────┘ │
│           │                                                  │
│           ▼                                                   │
│  CommittedStateSource.advance() → attachment 收 frame        │
└──────────────────────────────────────────────────────────────┘
```

**关键：
- **Tx 是逻辑事务**（callback 视角），**StorageWrite 是物理批次**（存储视角）
- **Chord tracker swap 是 publish 触发点**——inv observer 只看到已 commit 的不可变快照
- **三层结构让"pluggable storage"自然成立**——Storage 换 SQLite / JSONL / DO 都不影响 Tx 这一层

---

## 18. 已读清单

- ✅ observation.ts
- ✅ scheduler.ts（1-300 + 1080-1310 + memo grep）
- ✅ tool.ts（src/harness/tool.ts）
- ✅ generation.ts（src/harness/generation.ts）
- ✅ compaction.ts（src/harness/compaction.ts）
- ✅ transaction.ts（src/session/transaction.ts）
- ✅ index.ts（公开 API）
- ✅ spec.md（8 invariants + 部分其他节）
- ❌ spec.md §3-§9 剩余部分（只读了 §2 invariants + §5.5 owner 部分）

---

## 19. chord-delta-findings.md（位于 packages/durable/docs/）

已读。19 KB。Chord 选型与基准测量调查。

### 决定

- **放弃了 normalized graph storage**（每个对象/数组 → canonical node + numeric ID，wire 用 `["@", id]` 引用）
- **保留 tree/path delta** + weak-reference cache + spread-first clone

### 放弃 graph 的原因（实测）

工作量：20,000 strokes / 2,000,000 points / ~139.5 MiB heap

| 指标 | Tree delta | Graph |
|------|------------|-------|
| Ready producer heap | 139.5 MiB | 430.4 MiB |
| Snapshot apply | 0.027 ms | 1,417.161 ms |
| Snapshot JSON decode | 159.388 ms | 1,179.166 ms |
| Pipeline 最大 RSS | 852.8 MiB | 4,978.0 MiB |

Graph 在引用重赋值上快（不必遍历子节点），但 hydration / 复制 / 全量读都慢到无法接受。

### 保留 tree-delta 的代价与改善

- weak-reference cache patch：**永久保留**从 3,526 MiB → 203 MiB（after traversal）
- 但 cold traversal 时间增加（6,298 ms vs 2,397 ms）
- 残留 ~1.2 KB/容器 bookkeeping overhead（可接受）
- spread-first clone：V8 对象布局从 56 字节/point 压缩到 48 字节/point（额外保留从 154.7 MiB → 139.5 MiB）

### Batching 优势

baseline 在脏区合并后比较一次最终状态；operation log 在每次赋值时比较。
**含义**：如果应用可以把多次 mutation 放在一个 callback 里做（coalesce），tree-delta 优势明显。如果 mutation 必须分散（每改一次就 commit），operation log 优势明显。Pi Durable 的节流提交（100ms / commit boundary）天然支持 batching。

### 不能从性能通过推导出语义

graph prototype 测试都通过，但有 replica divergences：aliases / held references / reindexing / detach-reinsert。deep equality 不足以保证 identity/topology 正确——保留引用的对象在两个 producer 路径上会被分别更新，必须两路同步。

---

## 20. pico-v5-chord-usage.md（位于 packages/durable/docs/）

已读。18 KB。Pico5 规范的 Chord 使用指南（代码可跑在 `test/chord-guide.test.ts`）。

### 文档模型

`document` = JSON 持久化为 `base + ops + checkpoint`

- base：完整 JSON 快照，每次 checkpoint 重写
- ops：delta operations 数组（`[["set", path, value], ["delete", path]]`）
- checkpointWhen 谓词决定何时把 ops 折叠进 base（例：`info.deltasSinceBase >= 99`）

### 文档定义（defineDoc）的字段

| 字段 | 含义 |
|------|------|
| `kind` | 文档类型（**外部协议的一部分**，必须稳定） |
| `version` | schema 版本（schema 变更需迁移，**不是改名**） |
| `scope` | `session` / `conversation` / `task`（决定寿命 + fork 时是否复制） |
| `initial(seed)` | 创建时的初始值 |
| `checkpointWhen(value, ops, info)` | 何时触发 base 重写 |
| `family: true` + `family(key, seed)` | 同 kind 多实例，逻辑键 `(kind, conversationId, key)` |
| `history: "latest" \| "rewindable"` | 是否支持 `asOf(at)` 读历史 |
| `fork: "initial" \| "current" \| "asOf"` | fork 时如何复制 |

### 文档生命周期

- **incarnation ID**（numeric）：每次创建分配，永不重用
- 第一次 `tx.doc()` 创建实例，后续 seed **被忽略**
- task-scope 文档：task 进入 terminal → **自动 retire**
- retired 源 publish `null`；服务要么撤销、要么 terminal 在 null

### watch 协议

- 一次 Session commit = 一个 watch 帧
- listener 签名：`(value, context, delivery) => void`
- `delivery.kind`：`hydrate` / `update`
- `delivery.sequence`：stream 序列号（**不是 Session commit 序号**）
- 缓冲上限 **100 帧**；超出后用 `[["r", newestValue]]` 替换 pending 后缀
- 退休源 publish `[["r", null]]`

### Chord adoption = 原子快照 + 注册精确帧订阅

```
documentState():
  - atomically capture committed snapshot
  - register for every later exact frame
  - return already-hydrated read-only state
  - operations covered by snapshot are never redelivered
```

### checkpoint 流程

```
addStroke:
  hold Session mutation line
  await tx.doc → mutate tracker change draft
  callback succeeds:
    tracker prepare: immutable candidate + immutable ops
    Session checkpoint predicate selects a base or delta exactly once
    atomic storage commit: persist selected document and record writes
    storage succeeds → adopt candidate + enqueue candidate/ops (still on line)
  release line:
    deliver committed source ops → adapter → local/remote Chord consumers
late subscriber:
  atomically capture committed value + adapter sequence + subscription
```

### 不变量

- 一次 Session commit = 一个发布批（所有变化的 entry/head + 所有变化的 mounted docs）
- 第三方文档**不会自动 mount**到 view；需要自己的 Chord 服务或 trusted invocation watches
- 未确定 storage 失败：publish 什么也不发，**毒化 open Session** → 关闭重开

### Footguns

- 不要跨 commit 持有 draft、nested proxy、bound array method（被 revoke）；赋值的 container **按值拷贝**
- async commit 持有 line 经过 storage settlement + baseline adoption；等待文档访问而不是 model / process / network
- 只有 `tx.doc()` 能创建；`snapshot` / `documentState` / `watchDoc` 不存在则返回 `undefined` 且不写
- family seeds 仅第一次 `tx.doc()` 创建时使用；后续 seed 被忽略
- `watch.value` 在 `start()` 之前从固定快照初始化；100 帧后 pending 后缀被全量替换
- `stop()` 阻止未来回调，但**不中止已在跑的回调**
- 定义负责 checkpoint，**不是存储启发式**；保持 kind/version/fork policy/public path 稳定

---

## 21. 已读清单（最终）

- ✅ observation.ts
- ✅ scheduler.ts（1-300 + 1080-1310 + memo grep）
- ✅ tool.ts（src/harness/tool.ts）
- ✅ generation.ts（src/harness/generation.ts）
- ✅ compaction.ts（src/harness/compaction.ts）
- ✅ transaction.ts（src/session/transaction.ts）
- ✅ index.ts（公开 API）
- ✅ spec.md（8 invariants + §5.5 owner 部分）
- ✅ chord-delta-findings.md（19 KB Chord 选型调查）
- ✅ pico-v5-chord-usage.md（18 KB Chord 使用指南）
- ❌ pico-v5-handoff.md（57 KB，未读）——但是个 implementation handoff，不是架构参考
- ❌ spec.md §3-§9 剩余（只读了 §1 invariants + §12 footguns + 部分 §5.5）

---

## 22. spec.md §12 API footguns（位于 packages/durable/docs/spec.md）

已读。36 条 footgun。规范性强，不是善意提醒——每条都是不可绕过的不变量。

### 跨事务不变量

- **Read after write**：`#read` 在 `#hasTableWrite=true` 后抛错。文档 draft 不受此限。
- **Async commit 持有 Session 主线**：不能 await 模型 / 工具 / 进程 / 网络 / 人类 / 嵌套 commit / session waiter。要等就用 Tx 方法。
- **Detached draft work**：callback settle 后所有 Tx 操作拒绝。fire-and-forget 工作在 callback settle 前还能改 active tx。
- **Long transactions**：同 async commit 警告——line 不能被阻塞 IO 占用。

### Conversation / 提交

- **Writing to a busy conversation**：只有 Harness append 到有 active run 的 conversation。raw entry 在 generation prepare request 时会错放 system prompt entries——用 write submission。
- **Heads and edits during compaction**：摘要反映 compaction 选 cut 的时刻。summarization 期间写的 edit 会丢——必须在 compaction 前放置。
- **Raw head writes into the past**：`tx.appendEntry()` 直接写到 active range 之前的 entry 会改变 model context，但 mounted view 保持原 entries（直到 mount 重建）。用 write submission（自带 stale 检查）。
- **Queued items after a failed run**：failure 和 task abort 保留 inbox——等待 follow-ups 在 `pi.inbox`，直到下一次提交 boundary 放置。compaction summary 是一种 submission；结束时 boundary 放 waiting follow-ups 并开 run。

### Subagent / 用量

- **Double-counted subagent spend**：owned 子会话的 pi.usage 已被 harness 计费，tool result 里**不要**再 report。
- **Owned work holds its owner**：task 有 owned live ordinary work → 保持 completing；前台 Esc 终止不了 background 之外的 owned work。
- **Compensation in abort handlers**：abort handler 跑在被 abort-marked 的 task 里，`#validateOwners` 拒绝新 owned work。补偿要在执行 effect 的那一层做。
- **Work created by hooks**：hook 跑在 asking task 的 invocation 里，hook 创建的 owned work 持有那个 task。run task 的 owned work 不能在 run release 后写 transcript。

### Documents / Schema

- **Schema stability**：`kind` 和可见 mount path 是公开协议的一部分。schema 升级必须迁移，**不是改名**。同 kind 重复定义是 caller misuse，Session 不维护 registry 去查。
- **Checkpoint starvation**：`checkpointWhen()` 永不返 true → replay 和 current-only document storage 无限增长。
- **Wrong fork setting**：`current` / `initial` / `asOf` 是 product semantics 不是优化——改了就改了子会话行为。
- **Family initialization**：第一次 absent family acquisition 选 seed。已有实例和后续调用忽略 seed——seed 既不是 identity 也不是 update。
- **Reserved `pi.` names**：task names / document kinds / entry kinds 以 `pi.` 开头的属于 built-ins。复用会撞 Harness 行为（如 `pi.system` 会被当 system message 重放）。
- **Large terminal results**：terminal task record 仍可查询。大量结果放 entries 或更长寿命的 documents，outcome 只保留 ID。**不能**引用被同 outcome retire 的 task-scope document。

### Watch / Observation

- **Watch activation**：`start()` 前 `watch.value` 是固定 acquisition revision。先用它初始化 consumer；start 后属性跳到每个 delivered immutable revision。
- **Buffered watches**：100 帧上限；超出后未投递后缀被全量替换。需要审计每次 transition 的，落到不可变 entry 里单独查询。
- **Watch stop**：`stop()` 阻止未来回调但不中止/join 已在跑的回调。
- **Durable progress cadence**：client 只看到 committed progress——crash 可能丢当前未 commit 的 throttle 窗口。

### Storage / Lifecycle

- **Fatal storage errors**：不确定 storage 失败 → Session 被毒化。**不要 catch 继续用**，必须 close + reopen。
- **JSONL durability**：默认 JSONL 处理普通 process crash；不保证 power / host failure。
- **Non-cooperative code at close**：close join 每个 invocation。task handler / tool / hook 忽略 signal → close 一直 pending 直到它返回。
- **Services outliving the Harness**：Harness close 之前先 withdraw Chord services 和 detach clients。被 close 终止的 state 保持最后 value，不再更新。

### Extension / Prompt

- **Moving default selections**：conversation on default selection 每次 settings / extensions 变化都 append `pi.system` entries——provider prompt cache miss。
- **Unstable prompt text**：section renderer 输出含时间等非内容变化 → append system deltas → 打败 provider prompt caching。
- **Selection edits and copies**：`{ add, remove }` 永远编辑 host default selection。写到从 owner copy 的子会话 → 子会话拿 host default + edit，不是 owner selection。要从 resolved agent 重算子会话 array。
- **Copies are one-time**：新 task-owned conversation 创建时 copy owner 的 `pi.agent`，owner 后续变化不传过去。fork 保持 `asOf` copy。
- **Environment on recovery**：recovery 后 tool rerun 用 conversation 的**当前** `cwd`——可能和首次尝试不同。
- **Early resource disposal**：uninstall / replace extension 只停止新使用——立即释放资源可能让还在跑的 call 失败。
- **Guards left out of a selection**：permissions hook 等 extension 如果被 array selection 漏掉，conversation 跑在没它的环境里。

### Raw Transcript

- **Raw transcript**：view entries 不是 model context。渲染 edit、display-only entries、model 过滤需要合适 reducer。

---

## 23. 已读清单（更新）

- ✅ observation.ts
- ✅ scheduler.ts（1-300 + 1080-1310 + memo grep）
- ✅ tool.ts（src/harness/tool.ts）
- ✅ generation.ts（src/harness/generation.ts）
- ✅ compaction.ts（src/harness/compaction.ts）
- ✅ transaction.ts（src/session/transaction.ts）
- ✅ index.ts（公开 API）
- ✅ spec.md §1（8 invariants）+ §5.5（owner）+ §12（36 footguns）
- ✅ chord-delta-findings.md（19 KB Chord 选型调查）
- ✅ pico-v5-chord-usage.md（18 KB Chord 使用指南）
- ✅ pico-v5-handoff.md（已读开头 80 行，是 implementation handoff，不是架构参考）
- ✅ tui-plan.md（未读，但可能与 TUI 设计相关）
- ✅ spec.md §3.7（Forks，30 行）
- ❌ spec.md §3.1-§6（除 §3.7）+ §7-§11 剩余

---

## 25. spec.md §3.7 Forks（已读，30 行）

位于 §3 Documents 末尾。规范层 fork 语义：

- **fork 必须指向一个具体可见 entry `E`**——同一 commit 内的不同文档状态需要分别开 commit 才能分别 fork。
- **fork policy 表**（存在 DocumentRecord）：
  - `asOf`：parent value at `E`'s commit
  - `current`：committed parent value selected when fork commit runs
  - `initial`：no copied instance; initializer on first child access
- **`current` 和 `asOf` 复制 singleton + family 实例**，保留未知定义和 stored versions；复制出的实例有**新 DocumentId 和独立 initial base**。
- **`initial` 不复制实例**，首次 child access 用 initializer 创建。
- **Task 文档 / tasks 永不复制**——subagent 的 owned work 不随 fork 走（防止 orphan）。
- **Session 文档保持共享且不可 rewind**——`pi.live` 这类全局 LiveDoc 不参与 fork。
- **Fork copy 读取 committed pre-batch stored values**（不是 typed tracker caches）。一个事务里创建 fork 同时写父文档的 `fork: "current"` → 拒绝，必须先 commit 父变更。
- **Backend-side `document.copy`**：carry child create record + exact source incarnation/point。Storage materializes each source and persists its stored value/version as the child's independent initial base。Remote storage server-side 执行。每次 copy 读 committed pre-batch 源状态，独立于 write-array 顺序。
- **源在同一 batch 内不能 create / change / retire**——否则 fork 看到中间态，违反因果。

---

## 24. spec.md §6 Submissions and inbox（已读，131 行）

Built-in InboxDoc `pi.inbox`（kind/version=`1`, scope=conversation, fork=`initial`）。状态 `{ items: InboxItem[] }`，InboxItem 是有 ID 的 steer / followUp / write 三种 tag。
Built-in LiveDoc `pi.live`：可选 `run: { taskId, inputSubmissionIds }`——`run !== undefined` 即 busy。当前 round 的 tool task 列在 `pi.live.tools`，**tool task 不持有 run**，run 始终是 generation task 的。
Submission 状态机：placed → done | unanswered；queued → placed → done | unanswered。write 路径可直接 settle done（idle + empty inbox 时），不创建 run。
**Stale 检测**：write 的 head 指向 active range 之前 → unanswered: stale。例外：user items 永远不 stale；head:"self" 写永远不 stale。
**Compaction 排序**：compaction B cut 70 + blocking compaction A cut 150 → B stale；A cut 60 → B placed。cut 位置即 happens-before 键。
**Abort 不对称**：`Submission.abort()` 只撤 queued（placed 报 `already_placed`，terminal 报 `settled`）。`Conversation.abort()` 撤 queued user items 但保留 write。
**Boundary 表**：
| boundary | write | steer | follow-up |
| `postTools` | 全部 | first/all by mode | none |
| `final` | 全部 | first/all by mode | first/all by mode |
总是**先写后读**（ID 顺序）。`head:"self"` write 升 `postTools` 为 `final`。`onYield` continuation 在 `final` 无 user item 且无 reset 时维持 conversation 活跃。

---

## 26. spec.md §5.5 Structured concurrency（已读，89 行，spec.md:2031-2120）

**Tree + 2 rules + abort mechanics**——spec §5.5 的完整骨架。

### A. 树与不变性
- Tasks + conversations 一个 ownership tree
- `createTask(task, input, { ownership })` 必传 `TaskOwnership`（conversation 或 task）
- child task 永远活在 owner conversation；只有 owned conversation 跨 conversation 边界
- **Owner edges immutable**

### B. Two rules follow from the tree

**Rule 1：新 owned work 需要 live owner**——`createTask` / `createConversation` 若 owner 在 commit 的 final candidate 里是 `completing` / `terminal` / abort-marked，拒绝（spec §3.3）。
- 注意：conversations 在 owner 死后仍可用——可发起 new run 审讯已完成 subagent，那是 ordinary work，不属于任何 finish。

**Rule 2：Owned work 不会活过 owner 的 finish**——五子规则：
1. **Hold at completing**：task 写 terminal 或 scheduler 写 terminal outcome（`faulted`/`orphaned`）时，若 ordinary owned work 还 live，则存为 `{ status: "completing", outcome }`；scheduler 在 later commit 写最终 `terminal`。每次 commit 后评估（含 hold 后新创的工作）。**Open() 重评每个 completing**（`open()` 的 reconciliation 第二步就是这个）。
2. **Held outcome 终态**：no phase、no runtime commit、no abort handler 再次跑；no definition 必要；never reserved / migrated；`abortTask()` 仅 mark 不 re-run。
3. **Held outcome 语义**：非 completed = abort 意图（下层 cascade + drain）；completed = 等下层正常结束。
4. **Writes split at hold**：task-written terminal commit 把其他 writes（tool result entry + slot）落 hold；只把 record terminal state + task doc retirement + task waiters 推到 final commit。Scheduler-written outcome 在 hold 时只写 record；harness cleanup 在 final commit 跑（所以 faulted run task 持 `pi.live.run` 直到它的 tool drained）。
5. **Waiters 看 completing 是 live 直到 final commit**。

### C. Abort mechanics

**Abort flows down**：
- live owner 的 cancel 意图（abort mark 或 held 非 completed outcome）→ 标 owned ordinary work
- Background tasks 是 boundaries 除非直接 abort 或被 `Conversation.abort(context, { background: true })`

**Abort order is bottom-up**：
- abort invocation 启动前必须等 ordinary owned work 不再 live（这样 abort handler 看到的是 final outcomes below）
- waiting task 上 abort mark → leave wait early for abort handler
- tasks in `on` that it does not own → not awaited
- ordering judged on committed records（child 跑在 terminal commit 后的代码不 ordered）
- **Abort handler 限制**：
  - 不能 return `waiting`
  - 不能 create owned children（task 已 abort-marked）
  - 只能 inline 补偿 或 创建 background conversation-owned tasks（cascade 触不到）+ `runtime.waitForTask()` 持 invocation

### D. Cascade 示例（spec 给出）
`Checkout` task `failFast` 等待 4 个 `Payment` 子。某 payment 卡过期 failed → 3 个 live payment 得 abort marks + 跑 refund（abort handler）；4 个都 terminal → Checkout resume → 读 `outcomes()` → 自己决 outcome。`abortTask(Checkout)`：Checkout mark → cascade 标 live payments → 它们的 abort handler 先跑 → Checkout.abort 看到 final outcomes。

---

## 27. 已读清单（最终 + doc 整合状态）

### Spec.md（4500+ 行）
- §3.7 Fork ✅
- §5.3 Task 状态机 ✅
- §5.5 Structured concurrency ✅
- §6 Inbox & Submissions ✅
- §12 API footguns ✅
- 未读：§1-§2 / §3.1-§3.6 / §4 / §5.1 / §5.2 / §5.4 / §7-§11

### Doc 整合（spec-grounded）
- §8.6 终态 4 步 ✅
- §8.7 ownership tree + spec §5.5（4 块新增）✅
- §10.1 fork 硬约束 ✅
- §10.2 inbox 模型 + 边界表 + stale + compaction 排序 ✅
- §12.2 Chord ✅
- §14.5 #validateOwners + 正面 #10 ✅
- §14.6 10 条 footguns ✅

### Doc 当前状态
- 文件：`/Users/zouguojun/Desktop/pi-durable-overview.md`
- 行数：683（从 675 加 8 行 spec §5.5）
- 阅读地图：7 条指引（含 §12.2 / §14.6 / §8.13 / §9.7 / §9.8）
- 任务：按用户 m00501 同意，spec §5.5 整合后停止