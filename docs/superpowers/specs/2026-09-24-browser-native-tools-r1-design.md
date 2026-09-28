# 浏览器原生工具深度重构 Round 1 — 设计定稿

> 日期：2026-09-24 · 分支：`browser-native-r1`（worktree `../Aleph-browser-r1`）
> 对标：`pi-agent-browser-native`（权威副本 = 挂载 v0.6.15；能力参照取 v0.7.1 超集；**不采用其工具面结构**）
> 前置阅读：CLAUDE.md（红线+判据索引）· FL §3.12 · FL 附录 E.9/E.10 · `qa/README.md` browser 条目

## 1. 背景与目标

Gap Analysis 结论（双扫描 agent 盘点，全文见会话记录）：

- **Aleph 已领先/等价**：refs 世代语义、五层安全咽喉、`browser_exec`（⊇ batch --bail）、能力矩阵+真机 caps 探测、延迟披露成本控制、per-principal 多租户、双引擎热切换。
- **本轮要补的真实缺口**：B1 结构化错误契约+nextActions、B2 效果到达探针、B3 tab 身份 targetId 持久化（修 re-attach 旧债）、B4 ref 派发前预检+批内失效闩锁、B5 高价值控件提示。
- **用户裁定**：不做 JS 沙箱（维持 hermes 轮裁定），改为 exec DSL 受控增强；范围全要但拆两轮，本 spec 只覆盖 Round 1。

## 2. 范围

**In scope（Round 1）**：B1–B5 + exec DSL 增强（repeat/if）+ 旧债（Task 14 session_key 归一；评估 manager.rs:1171、cookies.rs:666）。
**Out of scope（Round 2，另行 spec）**：C1 网络拦截/mock/HAR、C2 录制（ffmpeg）、C3 QA 验证工具。

**刻意不做（本轮已评估，勿重提）**：
- `agent_browser_code` 式 JS 沙箱——绕过五层咽喉 + R3 重型依赖；exec DSL 是受控替代。
- pi 的单工具八模式/三工具面结构——Aleph 的 26 工具 + BROWSER_RESIDENT_CORE 是已结算设计。
- `for_each_ref` 集合语义步骤——YAGNI，模型可 snapshot+多轮达成。
- Electron 工具组、source/network-source 归属、transcript 即状态——无客户端（判据 §9）或宿主特定。
- 跨 Aleph 重启的 tab 身份持久化——引擎进程已死，身份表随 EngineRegistry 重建。

## 3. 两线契约（先钉死，并行实现的接口）

```rust
// browser_tools/recovery.rs — 单一作者
pub enum BrowserFailureCategory {
    NavigateBlocked,      // SSRF / post_nav 拒绝
    SecretInInput,        // 输入 secret 扫描拦截
    StaleRef,             // ref 代际失效或 URL 漂移（细分在 message）
    TabGone,              // targetId 失效（tab 关闭/引擎重启）
    EngineBusy,           // obscura command barrier
    UnsupportedByEngine,  // 能力表拒绝
    UnsupportedByDriver,  // driver 单侧能力（save_state 等）
    WaitTimeout,          // wait_for / poll_wait_for 超时
    SelectorMiss,         // ref/坐标找不到元素
    DialogPending,        // 未处理 JS dialog 阻塞
    Transport,            // CDP/MCP/CLI 传输层失败
    ApprovalRequired,     // 审批门拒绝/待决
    BudgetExhausted,      // 步数/墙钟/字符预算
    Unknown,              // 「我不知道」——判据 §8，不许硬塞相近类
}

pub struct NextAction {
    pub tool: &'static str,             // 必须是真实注册工具（census 钉住）
    pub reason: String,                 // 给模型的一句话
    pub params_hint: serde_json::Value, // 静态模板形状
    pub safety: NextActionSafety,       // SafeToRetry | NeedsUserDecision | ChangesPage
}
```

**Wire 形状（向后兼容）**：工具错误输出保留 `{success:false, message}`，追加
`"recovery": {"category": "...", "next_actions": [...]}`。

**核心层新错误形状**（线 2 产出，线 1 消费）：
- `BrowserError::TabGone { tab_id: String, last_url: Option<String> }`
- `StaleReason`（page_state/refs.rs 既有）→ 工具层映射 `StaleRef`。

## 4. 任务分解与文件所有权（双线）

### 线 2（核心/引擎层，先合并）

| # | 任务 | 独占文件 |
|---|---|---|
| T1 | **B3 tab 身份**：TabRegistry 升级为唯一身份真源（targetId 优先、marker/枚举序仅首发现、`TabGone` 结构化错误、删「取最后一行」兜底、登记 `last_url`）；`EngineHandle::TabTable` 降级为可重建 session 附着缓存；老驱动维持现状但判定收口同一函数 | `tab_registry.rs` `manager.rs` `engine/mod.rs` `cdp_backend/*` |
| T2 | **旧债**：Task 14 session_key/profile 名归一（`cdp_backend/mod.rs:93-104`）；评估 `manager.rs:1171` / `cookies.rs:666`——超范围只记录 | `cdp_backend/mod.rs` 等 |
| T3 | **B2 效果探针**：cdp 后端 click/type/fill 派发前装一次性捕获监听、派发后读回；未到达→结构化失败+recovery；能力门控，不支持引擎诚实标注 `effect_verification: skipped(engine)`；trait 默认 no-op | `cdp_backend/actions.rs` `engine/capability.rs` `backend.rs` |
| T4 | **exec DSL**：`repeat{actions,until,max_iterations≤20}` + `if{condition,then,else?}`；plan 递归展开映射既有 ActionType、展开后计入 MAX_EXEC_ACTIONS=50、600s 墙钟不变；批内 ref 失效闩锁（navigate/dialog 后 ref 步骤派发前拒绝+nextAction=重拍） | `exec.rs`（**线 1 不碰**） |

### 线 1（工具/呈现层，线 2 合并后 rebase）

| # | 任务 | 独占文件 |
|---|---|---|
| T5 | **B1**：`recovery.rs`（枚举+注册表+`classify()` 单一源）+ 26 工具接线；守卫 `recovery_registry_entries_name_real_tools` census + classify 穷尽性测试 | `browser_tools/recovery.rs` `browser_tools/*`（除 exec.rs） |
| T6 | **B4 单工具层**：带 ref 工具在 `make_backend_and_tab_guarded` 后、副作用前跑 `RefTable::resolve` 预检；fail-closed；StaleReason→recovery 映射；仅 cdp 后端（老后端无 RefTable，明示不对称），能力表加 `ref_precheck` 行 | `browser_tools/mod.rs` `click.rs` `type_text.rs` 等 |
| T7 | **B5**：render.rs 截断时追加 `Omitted high-value controls`（≤20 条 ≤2KB、refs 仍有效、角色集从 `roles.rs` 派生、末尾如实报 `…and N more`） | `page_state/render.rs` |
| T8 | **文档**：FL §3.12 本轮记录 + 附录 D/E 新判据（若有新形状）+ `qa/README.md` 阶段更新 + 能力表两行（`ref_precheck`/`effect_probe`） | `docs/` `qa/` |

**依赖**：T5 消费 T1 的 `TabGone` 形状（§3 已钉死，可并行开工）；T6 消费 T5 的 category。
**合并序**：线 2（T1+T2 合并编译一次 → T3+T4 合并编译一次）→ 合主线 → 线 1 rebase → T5–T7 合并编译一次 → T8 + 全量验证。

## 5. 宪法核对

- **R1/R3**：零新依赖；CDP 扩展只走 `crates/aleph-cdp` 的 `methods::` 包装。
- **R7**：nextActions 是数据不是路由——不自动执行、不排序推荐、不隐藏原始错误。
- **R9**：不新增常驻描述字节；新行为写进既有工具的 DESCRIPTION 时过两把尺，注意 `CATALOG_DESCRIPTION_CEILING_BYTES` 棘轮（headroom 0 B——加字节必须先修剪）。
- **判据 §2**：每条新守卫必须能被证伪（删掉被守的行→红）。
- **判据 §5**：B5 角色集从 `roles.rs` 派生；能力表行带 `measured_on`。
- **判据 §6**：T1 开工先数 tab 身份写者（TabTable/tab_registry/CLI 会话）grep 剥注释。
- **判据 §8**：`Unknown` category 独立；`effect_verification: skipped` 诚实标注。
- **判据 §9**：新能力必须挂到 browser_* 动词 + 能力表 + qa caps 探测。
- **判据 §16**：孪生——能力门控/降级标注两个引擎同一推导。
- **E.10**：线 2 每次 cargo 前查 `MemAvailable ≥ 4G`；`CARGO_BUILD_JOBS=2`。

## 6. 测试与 QA

**单测**（每条守卫先证伪）：recovery census（注册表每条指向真实工具）；classify 穷尽（新 BrowserError variant 无归属即红）；ref 预检 fail-closed（FakeBackend）；repeat/if 展开计数+硬帽+条件评估错误即中止；B5 区段预算与 `…and N more` 诚实计数；TabGone 不回落末行猜测。

**真机 QA**：`qa/browser_managed attach` 扩展 re-attach 身份断言（改动前应先红——证明断言有效再修绿）；`qa/browser_dual caps` 加 `ref_precheck`/`effect_probe` 两行对真二进制探测。

**全量验证集（收尾七条）**：
```
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
just test-shared
cargo clippy --workspace --all-targets（先 just _stage-shell-placeholders）
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo test -p alephcore --lib   # 全量真跑
```

## 7. 文档更新义务（T8）

代码话术进 FL：§3.12 追本轮记录（对标 pi-agent-browser-native 的 Gap Analysis 结论 + 刻意不做清单）；若产出新判据形状进附录 D/E；CLAUDE.md 不动（无新红线/新形状索引行）。qa/README.md 的阶段清单与证明目标同步。
