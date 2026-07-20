# Graph 层 × 多智能体融合 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按 spec `docs/superpowers/specs/2026-07-19-graph-multiagent-fusion-design.md` 落地四工作包：WP1 治理环独立视角化（loop-auditor builtin agent + 模板改造）、WP2 Team 显式入图（NodeKind::Team + disband 胜利宣称触发）、WP3 Grounding 进 task_review（锚点证据 + require_grounding 开关）、WP4 编排智慧与文档修正。

**Architecture:** 全部改动落在工具层 / prompt 层 / agent 定义层；`src/harness/` 零触碰（R10）。治理表持久慢环，Team 显式 pair 才入图；审计取证默认 spawn 独立 context 的 `loop-auditor`；grounding 走 acceptance metadata 既有通道（零迁移）+ task comment 通道存证（零迁移）。

**Tech Stack:** Rust (tokio + serde + schemars + rusqlite)，测试 `cargo test -p alephcore --lib`。

## Global Constraints

- **R10**: 不触碰 `src/harness/` 任何文件（12 文件锁 + 行数棘轮）。
- **零新依赖**（R3）；无 schema migration（metadata JSON 通道 + comment 通道）。
- **⚠️ 本仓库禁止裸 `cargo fmt`**（repo 非 fmt-clean，会污染 131 个未触碰文件）。只允许 `cargo fmt -- <你改过的文件>` 或不 fmt。
- 提交规范：`<scope>: <description>` 英文小写 scope；单分支 main 直接提交。
- 代码注释英文；面向用户的工具消息/模板文本沿用现有中文风格。
- grounding kind 闭集与 loop_graph anchor truth 闭集同词表：`exit_code | numeric | line_count`。
- 每个 task 结束跑该模块测试；最后 Task 9 跑全量 lib 测试。

---

### Task 1: WP1a — `loop-auditor` builtin agent

**Files:**
- Modify: `src/agents/registry.rs`（`builtin_agents()`，在 `explore` agent 条目之后插入）

**Interfaces:**
- Produces: builtin agent id `"loop-auditor"`（AgentMode::SubAgent、ContextMode::Fresh、READ_ONLY 工具集 + bash、denied file_write/file_edit/search/web_fetch）。Task 2/7/8 的模板与 prompt 文本引用 `subagent(agent_type="loop-auditor")`。
- Consumes: `AgentDef` builder（`with_allowed_tool_sets` 后链 `with_allowed_tools` = union 语义，见 `src/agents/types.rs:198-215`）；`ContextMode::Fresh`（`src/agents/types.rs:85-91`，默认值，但此处显式声明以自文档）。

- [ ] **Step 1: Write the failing test**

在 `src/agents/registry.rs` 的 `#[cfg(test)] mod tests` 中追加：

```rust
#[test]
fn loop_auditor_is_independent_and_measure_only() {
    let agents = builtin_agents();
    let auditor = agents
        .iter()
        .find(|a| a.id == "loop-auditor")
        .expect("loop-auditor builtin must exist");
    assert!(matches!(auditor.mode, AgentMode::SubAgent));
    // Independent context is the whole point of this agent.
    assert!(matches!(auditor.context_mode, ContextMode::Fresh));
    // Can measure (bash for real exit codes) and read, cannot rewrite.
    assert!(auditor.is_tool_allowed("bash"));
    assert!(auditor.is_tool_allowed("file_read"));
    assert!(!auditor.is_tool_allowed("file_write"));
    assert!(!auditor.is_tool_allowed("file_edit"));
    // Audit doctrine forbids network search — deny at the definition level.
    assert!(!auditor.is_tool_allowed("search"));
    assert!(!auditor.is_tool_allowed("web_fetch"));
    // SubAgent mode structurally cannot recurse.
    assert!(!auditor.is_tool_allowed("subagent"));
}
```

注意：test 模块若未 import `ContextMode`，在测试文件头部现有 `use super::*;` 已覆盖（`registry.rs` 顶部已 `use crate::agents::types::...`；如编译报 `ContextMode` 未找到，在测试模块加 `use crate::agents::types::ContextMode;`）。

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agents::registry::tests::loop_auditor_is_independent_and_measure_only -- --nocapture`
Expected: FAIL with `loop-auditor builtin must exist`

- [ ] **Step 3: Write minimal implementation**

在 `src/agents/registry.rs` 的 `builtin_agents()` 中，`explore` agent 条目（`.with_max_iterations(20),` 行）之后插入：

```rust
        // Loop-auditor — the governance layer's independent evidence collector
        // (spec 2026-07-19-graph-multiagent-fusion). Fresh context by design:
        // an auditor that shares the auditee's memory/context can only confirm
        // the auditee's own story ("agents reading the same data prove each
        // other right"). Can measure (bash → real exit codes / counts) and
        // read; cannot rewrite; network search denied per audit doctrine.
        AgentDef::new("loop-auditor", AgentMode::SubAgent)
            .with_description("Independent-context evidence collector for governance loops")
            .with_when_to_use(
                "When an audit/watcher governance turn needs independently gathered \
                 evidence: run anchor probes, re-measure claimed numbers, verify a \
                 reviewed deliverable against reality. Read-and-measure only.",
            )
            .with_context_mode(ContextMode::Fresh)
            .with_allowed_tool_sets(vec!["READ_ONLY".into()])
            .with_allowed_tools(vec!["bash".into()])
            .with_denied_tools(vec![
                "file_write".into(),
                "file_edit".into(),
                "search".into(),
                "web_fetch".into(),
            ])
            .with_max_iterations(15),
```

若 `registry.rs` 顶部尚未 import `ContextMode`（现有代码已用 `ContextMode::Summary`，应已 import），保持现状即可。

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib agents::registry -- --nocapture`
Expected: 全部 PASS（含既有 registry 测试——若有测试断言 builtin agent 总数，按新增 1 个修正该断言）

- [ ] **Step 5: Commit**

```bash
git add src/agents/registry.rs
git commit -m "agents: add loop-auditor builtin for independent governance evidence"
```

---

### Task 2: WP1b — 审计/看守模板默认独立取证

**Files:**
- Modify: `src/loop_graph/templates.rs`

**Interfaces:**
- Consumes: Task 1 的 agent id `"loop-auditor"`；spawn 工具真名 `subagent`，参数名 `agent_type`（见 `src/agents/subagent_tool/parse.rs:178`）。
- Produces: 模板文本含 `loop-auditor` 字样（Task 9 全量测试依赖模板测试更新后的断言）。

- [ ] **Step 1: Write the failing test**

在 `src/loop_graph/templates.rs` 测试模块 `audit_template_covers_the_seven_steps_and_iron_rules` 的 needle 数组中追加两个元素：

```rust
            "loop-auditor",
            "agent_type",
```

并在 `governance_templates_carry_their_disciplines` 中追加：

```rust
        assert!(WATCH_TEMPLATE_HEADER.contains("loop-auditor"));
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib loop_graph::templates -- --nocapture`
Expected: FAIL with `audit template missing: loop-auditor`

- [ ] **Step 3: Write minimal implementation**

编辑 `AUDIT_TEMPLATE` 第 2 步。用 Edit 把这段旧文本：

```text
2)【锚点取证·真实执行，不信报表】对图中每个 anchor 节点：
```

替换为：

```text
2)【锚点取证·真实执行，不信报表】取证默认派独立审计员：subagent(agent_type="loop-auditor", task="<探针清单：每个探针的 probe 命令与 truth 类型，要求只返回测量值>")——它以全新上下文执行探针，防「与被审计者共读同一套记忆互证正确」；你只接收测量值并裁决。图很小（≤2 个锚点）时可自行执行。对图中每个 anchor 节点：
```

编辑 `WATCH_TEMPLATE_HEADER`。把这段旧文本：

```text
再按下面的看守指令真实取证（bash 只读，mode=ro；不信自我报告）。
```

替换为：

```text
再按下面的看守指令真实取证（bash 只读，mode=ro；不信自我报告）。取证优先派独立审计员 subagent(agent_type="loop-auditor", task="<反指标探针+返回测量值>")——独立上下文测量，防与被看守环共读同套数据互证正确。
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib loop_graph::templates -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/loop_graph/templates.rs
git commit -m "loop_graph: default audit/watch templates to independent-context evidence"
```

---

### Task 3: WP2a — `NodeKind::Team` 变体

**Files:**
- Modify: `src/loop_graph/types.rs`
- Modify: `src/builtin_tools/loop_graph_manage.rs`（仅补 2 个 exhaustive match 臂让编译通过；真正的 live-join 在 Task 4）

**Interfaces:**
- Produces: `NodeKind::Team`，`as_str() == "team"`，`parse("team")`，`is_optimization_loop() == true`（team 优化其使命→注册进图即应被看守，"裸奔优化环" lint 顺势覆盖）。id 前缀 `team:<team_id>`。
- Consumes: 无。

- [ ] **Step 1: Write the failing test**

`src/loop_graph/types.rs` 测试模块：在 `kind_roundtrip_through_strings` 的 NodeKind 数组中加入 `NodeKind::Team,`（`NodeKind::Daemon,` 之后）；在 `optimization_loop_classification` 中追加：

```rust
        assert!(NodeKind::Team.is_optimization_loop());
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib loop_graph::types -- --nocapture`
Expected: 编译错误 `no variant named Team`（编译失败即本步的 RED）

- [ ] **Step 3: Write minimal implementation**

`src/loop_graph/types.rs` 三处：

enum 中 `Daemon` 变体之后插入：

```rust
    /// An explicitly governed multi-agent team (`team:<team_id>`). Teams enter
    /// the graph only by explicit pairing — never automatically; fast-loop
    /// coord tasks never become nodes.
    Team,
```

`as_str` 的 `Self::Daemon => "daemon",` 之后插入：

```rust
            Self::Team => "team",
```

`parse` 的 `"daemon" => Some(Self::Daemon),` 之后插入：

```rust
            "team" => Some(Self::Team),
```

`is_optimization_loop` 的 matches! 改为：

```rust
        matches!(
            self,
            Self::LoopGoal | Self::LoopCron | Self::LoopHeartbeat | Self::Daemon | Self::Team
        )
```

`src/builtin_tools/loop_graph_manage.rs` 两处 exhaustive match 补臂：

`expected_prefix` 的 `NodeKind::Daemon => "daemon:",` 之后插入：

```rust
            NodeKind::Team => "team:",
```

`render_status` 的 live-join match：把 `NodeKind::LoopHeartbeat | NodeKind::Daemon => {}` 改为：

```rust
                NodeKind::LoopHeartbeat | NodeKind::Daemon | NodeKind::Team => {}
```

（Task 4 会把 Team 拆出为真 live-join 臂。）

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib loop_graph -- --nocapture`
Expected: PASS（若还有别处 NodeKind exhaustive match 编译报错，同样补 Team 臂为对既有行为最保守的选择，并在 commit message 中列出）

- [ ] **Step 5: Commit**

```bash
git add src/loop_graph/types.rs src/builtin_tools/loop_graph_manage.rs
git commit -m "loop_graph: add team node kind"
```

---

### Task 4: WP2b — status live-join Team + pair 支持 team 目标

**Files:**
- Modify: `src/builtin_tools/loop_graph_manage.rs`
- Modify: `src/executor/builtin_registry/builder/constructor/mod.rs:409-410`（接线）

**Interfaces:**
- Consumes: `crate::teams::TeamStore` trait（`get_team(&str) -> Result<Option<Team>>`，`Team.status: TeamStatus{Active,Disbanded}` 带 `as_str()`、`Team.leader_id`、`Team.name`）；constructor 的 `config.team_store: Option<Arc<dyn TeamStore>>`（同文件 `coord_team_tools.rs` 已在用）。
- Produces: `LoopGraphTool::with_team_store(Option<Arc<dyn TeamStore>>)` builder；status 输出中 team 节点的 live 行。pair 本就只校验节点存在（`get_node`），team 节点注册后 pair 自动可用——只更新提示文案。

- [ ] **Step 1: Write the failing test**

`src/builtin_tools/loop_graph_manage.rs` 测试模块追加（无 TeamStore 时的降级路径 + team 节点注册/配对可用性）：

```rust
    #[tokio::test]
    async fn team_node_registers_and_renders_without_team_store() {
        let (_d, t) = tool();
        let mut a = args(LoopGraphAction::Node);
        a.id = Some("team:release-crew".into());
        a.kind = Some(NodeKind::Team);
        a.label = Some("发版小队".into());
        t.call(a).await.unwrap();

        let out = t.call(args(LoopGraphAction::Status)).await.unwrap();
        let rendered = out.rendered.unwrap();
        assert!(rendered.contains("team:release-crew"));
        // No team store attached → no live line, no panic, degraded gracefully.
        assert!(!rendered.contains("live:") || !rendered.contains("team 记录已消失"));
        // A registered team without watchers is a naked optimization loop.
        assert!(rendered.contains("裸奔优化环"));
    }

    #[tokio::test]
    async fn team_node_prefix_enforced() {
        let (_d, t) = tool();
        let mut a = args(LoopGraphAction::Node);
        a.id = Some("release-crew".into());
        a.kind = Some(NodeKind::Team);
        a.label = Some("发版小队".into());
        assert!(t.call(a).await.is_err(), "team id must carry team: prefix");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib loop_graph_manage -- --nocapture`
Expected: `team_node_prefix_enforced` PASS（Task 3 已给前缀），`team_node_registers_and_renders_without_team_store` 在「裸奔优化环」断言上 PASS——如两个测试全 PASS，说明 Task 3 的保守臂已覆盖此降级路径，本步为确认性 RED（直接进 Step 3 加 live-join 能力，测试保持为回归护栏）

- [ ] **Step 3: Write implementation**

`src/builtin_tools/loop_graph_manage.rs`：

顶部 import 区追加：

```rust
use crate::teams::TeamStore;
```

struct 与构造器改为：

```rust
#[derive(Clone)]
pub struct LoopGraphTool {
    store: Arc<LoopGraphStore>,
    cron: Option<SharedCronService>,
    teams: Option<Arc<dyn TeamStore>>,
}

impl LoopGraphTool {
    pub const fn new(store: Arc<LoopGraphStore>) -> Self {
        Self {
            store,
            cron: None,
            teams: None,
        }
    }
```

`with_cron_service` 之后追加 builder：

```rust
    /// Attach the team store handle (unlocks `team:<id>` live joins in
    /// `status`). Absent = team nodes render without a live line.
    #[must_use]
    pub fn with_team_store(mut self, teams: Option<Arc<dyn TeamStore>>) -> Self {
        self.teams = teams;
        self
    }
```

`render_status` 中把 Task 3 的保守臂 `NodeKind::LoopHeartbeat | NodeKind::Daemon | NodeKind::Team => {}` 拆回：

```rust
                NodeKind::Team => {
                    if let Some(ts) = &self.teams {
                        let team_id = n.id.trim_start_matches("team:");
                        match ts.get_team(team_id).await {
                            Ok(Some(t)) => out.push_str(&format!(
                                "\n    live: status={} leader={} name={}",
                                t.status.as_str(),
                                t.leader_id,
                                truncate(&t.name, 40)
                            )),
                            _ => out
                                .push_str("\n    live: ⚠ target missing（team 记录已消失）"),
                        }
                    }
                }
                NodeKind::LoopHeartbeat | NodeKind::Daemon => {}
```

`Pair` 臂的成功 message 改为通用措辞。旧：

```rust
                        "看守环已配对: {watcher_id} -[watches]-> {to_id}（{expr}）。\
                         被看守 goal 的胜利宣称还会即时触发本看守（post-run 钩子，去抖 60s）。"
```

新：

```rust
                        "看守环已配对: {watcher_id} -[watches]-> {to_id}（{expr}）。\
                         被看守 goal/team 的胜利宣称（goal 完成 / team 解散）还会即时触发本看守（post-run 钩子，去抖 60s）。"
```

工具 `DESCRIPTION` 中 `register self-improvement loops (goal/cron/heartbeat/daemon)` 改为 `register self-improvement loops (goal/cron/heartbeat/daemon/team)`。

`LoopGraphArgs.id` 的 doc-comment 前缀清单加入 `team:<team_id>`（`daemon:<name>` 之后）。

接线 `src/executor/builtin_registry/builder/constructor/mod.rs`，旧：

```rust
        let loop_graph_tool = crate::builtin_tools::LoopGraphTool::new(loop_graph_store)
            .with_cron_service(config.cron_service.clone());
```

新：

```rust
        let loop_graph_tool = crate::builtin_tools::LoopGraphTool::new(loop_graph_store)
            .with_cron_service(config.cron_service.clone())
            .with_team_store(config.team_store.clone());
```

（若该作用域 `config` 无 `team_store` 字段则说明 config 类型不同——用 `grep -n "team_store" src/executor/builtin_registry/builder/constructor/config*.rs` 找到正确字段名后接线；`coord_team_tools.rs:116` 证明同一 `config` 值可达。）

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib loop_graph_manage -- --nocapture && cargo check -p alephcore`
Expected: 全 PASS，编译干净

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/loop_graph_manage.rs src/executor/builtin_registry/builder/constructor/mod.rs
git commit -m "loop_graph: live-join teams in status and accept team pair targets"
```

---

### Task 5: WP2c — team 解散 = 胜利宣称触发看守

**Files:**
- Modify: `src/loop_graph/service.rs`（DRY 重构 + `notify_team_settled`）
- Modify: `src/builtin_tools/team/disband.rs`（挂钩）

**Interfaces:**
- Produces: `pub async fn notify_team_settled(team_id: &str)`（best-effort no-op 契约同 `notify_goal_settled`）；内部纯函数 `fn watcher_jobs_for(store, node_id) -> Vec<String>`（单测目标）。
- Consumes: `crate::loop_graph::global()`、`CRON_TRIGGER`、`debounce_pass`（全部既有）；disband 侧仅一行调用，零构造器改动。

- [ ] **Step 1: Write the failing test**

`src/loop_graph/service.rs` 测试模块追加（`seeded_store` 之后）：

```rust
    #[test]
    fn watcher_jobs_resolve_for_goal_and_team_nodes() {
        let (_dir, store) = seeded_store();
        store
            .upsert_node(&GraphNode::new(
                DEFAULT_AGENT,
                "team:release-crew",
                NodeKind::Team,
                "发版小队",
                Origin::Llm,
            ))
            .unwrap();
        store
            .upsert_node(
                &GraphNode::new(
                    DEFAULT_AGENT,
                    "cron:team-watch",
                    NodeKind::LoopCron,
                    "小队看守",
                    Origin::Llm,
                )
                .with_cadence("nightly"),
            )
            .unwrap();
        store
            .upsert_edge(&GraphEdge::new(
                DEFAULT_AGENT,
                "cron:team-watch",
                "team:release-crew",
                EdgeKind::Watches,
                Origin::Llm,
            ))
            .unwrap();

        assert_eq!(
            watcher_jobs_for(&store, "team:release-crew"),
            vec!["team-watch".to_string()],
            "watches edge on a team node must surface its cron watcher"
        );
        assert!(
            watcher_jobs_for(&store, "goal:sess-1").is_empty(),
            "owns_reference edge is not a watcher"
        );
        assert!(watcher_jobs_for(&store, "team:nonexistent").is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib loop_graph::service -- --nocapture`
Expected: 编译错误 `cannot find function watcher_jobs_for`

- [ ] **Step 3: Write implementation**

`src/loop_graph/service.rs`：

在 `debounce_pass` 之后新增纯函数：

```rust
/// Cron job ids of every `watches` watcher pointed at `node_id`. Pure lookup
/// (unit-testable); empty on store errors.
fn watcher_jobs_for(store: &crate::loop_graph::LoopGraphStore, node_id: &str) -> Vec<String> {
    store
        .list_edges(DEFAULT_AGENT)
        .map(|edges| {
            edges
                .iter()
                .filter(|e| e.kind == EdgeKind::Watches && e.to_id == node_id)
                .filter_map(|e| e.from_id.strip_prefix("cron:").map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
```

把 `notify_goal_settled` 重构为共享内核 + 两个薄入口（替换现有整个 `notify_goal_settled` 函数体）：

```rust
/// Poke every cron watcher paired (via `watches`) to `node_id`. Best-effort
/// and bounded: no graph / no store / no cron handle / no watchers → no-op.
async fn notify_node_settled(node_id: &str) {
    let Some(store) = crate::loop_graph::global() else {
        return;
    };
    let watcher_jobs = watcher_jobs_for(&store, node_id);
    if watcher_jobs.is_empty() {
        return;
    }
    let Some(cron) = CRON_TRIGGER.get() else {
        info!(node = %node_id, "loop_graph: watchers paired but no cron trigger handle");
        return;
    };
    for job_id in watcher_jobs {
        if !debounce_pass(&job_id) {
            continue;
        }
        let service = cron.lock().await;
        match service.run_job(&job_id).await {
            Ok(()) => {
                info!(node = %node_id, watcher = %job_id,
                    "loop_graph: victory claim — watcher cron poked");
            }
            Err(e) => {
                warn!(node = %node_id, watcher = %job_id, error = %e,
                    "loop_graph: failed to poke watcher cron");
            }
        }
    }
}

/// Goal victory-claim entry. Call sites: the goal continuation hook's
/// gate-less terminal complete and gate-pass commit moments.
pub async fn notify_goal_settled(session: &str) {
    notify_node_settled(&format!("goal:{session}")).await;
}

/// Team victory-claim entry — a disband is the team's "we're done" moment.
/// Call site: `team_disband` success path.
pub async fn notify_team_settled(team_id: &str) {
    notify_node_settled(&format!("team:{team_id}")).await;
}
```

（原 `notify_goal_settled` 内联的 watcher 收集逻辑删除，由 `watcher_jobs_for` 承担——行为等价：原实现同样对 `list_edges` 错误静默返回。）

`src/builtin_tools/team/disband.rs` 的 `call()`：在 `info!(team_id = %args.team_id, "team_disband: team disbanded");` 之后插入：

```rust
        // Victory-claim trigger: a disband is the team's "we're done" moment —
        // poke any watcher paired to `team:<id>` in the governance graph
        // (best-effort; a no-op when the team was never explicitly paired).
        crate::loop_graph::service::notify_team_settled(&args.team_id).await;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib loop_graph::service -- --nocapture && cargo test -p alephcore --lib builtin_tools::team -- --nocapture`
Expected: 全 PASS（`governing_owner_and_topology_render` / `debounce_collapses_bursts` 回归不破）

- [ ] **Step 5: Commit**

```bash
git add src/loop_graph/service.rs src/builtin_tools/team/disband.rs
git commit -m "loop_graph: poke watchers on team disband victory claim"
```

---

### Task 6: WP3a — `require_grounding` acceptance metadata 通道

**Files:**
- Modify: `src/agents/swarm/tasks/acceptance.rs`

**Interfaces:**
- Produces: `pub const REQUIRE_GROUNDING_METADATA_KEY: &str = "require_grounding"`；`pub fn require_grounding(&Value) -> bool`；`pub fn with_require_grounding(Value, bool) -> Value`（完全镜像 `lead_review_required` 三件套：容忍读、false 直通、非对象提升、不可变）。Task 7 消费。
- Consumes: 无新依赖。

- [ ] **Step 1: Write the failing test**

`src/agents/swarm/tasks/acceptance.rs` 测试模块追加：

```rust
    #[test]
    fn require_grounding_reads_false_when_absent_or_wrong_shape() {
        assert!(!require_grounding(&json!({})));
        assert!(!require_grounding(
            &json!({ REQUIRE_GROUNDING_METADATA_KEY: "yes" })
        ));
        assert!(!require_grounding(&json!(42)));
    }

    #[test]
    fn require_grounding_roundtrips_preserves_siblings_and_false_is_passthrough() {
        let original = json!({ "managed_by": "dispatcher" });
        let merged = with_require_grounding(original.clone(), true);
        assert!(original.get(REQUIRE_GROUNDING_METADATA_KEY).is_none()); // immutable
        assert_eq!(merged["managed_by"], json!("dispatcher"));
        assert!(require_grounding(&merged));

        let untouched = with_require_grounding(json!({ "k": 1 }), false);
        assert!(untouched.get(REQUIRE_GROUNDING_METADATA_KEY).is_none());
        assert!(with_require_grounding(json!("scalar"), true).is_object());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib acceptance -- --nocapture`
Expected: 编译错误 `cannot find function require_grounding`

- [ ] **Step 3: Write minimal implementation**

在 `STALE_REVIEW_WARNED_AT_METADATA_KEY` 块之前（即 `with_lead_review_required` 之后）插入：

```rust
/// Metadata key requiring the reviewer to attach grounding evidence — a real
/// measurement (exit_code / numeric / line_count, the same closed truth
/// vocabulary as loop_graph anchors) — before an approve verdict is accepted.
pub const REQUIRE_GROUNDING_METADATA_KEY: &str = "require_grounding";

/// Whether this task's approval requires reviewer grounding evidence.
/// Tolerant like [`lead_review_required`]: missing / non-bool reads `false`.
pub fn require_grounding(metadata: &Value) -> bool {
    metadata
        .get(REQUIRE_GROUNDING_METADATA_KEY)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Return a new metadata value with the require-grounding flag merged in.
/// Mirrors [`with_lead_review_required`]: non-object input promoted,
/// `required = false` is a pass-through, original untouched.
#[must_use]
pub fn with_require_grounding(metadata: Value, required: bool) -> Value {
    let mut value = match metadata {
        Value::Object(_) => metadata,
        _ => Value::Object(serde_json::Map::new()),
    };
    if !required {
        return value;
    }
    if let Some(obj) = value.as_object_mut() {
        obj.insert(REQUIRE_GROUNDING_METADATA_KEY.to_string(), Value::Bool(true));
    }
    value
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib acceptance -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/agents/swarm/tasks/acceptance.rs
git commit -m "swarm: add require_grounding acceptance metadata helpers"
```

---

### Task 7: WP3b — task_review grounding 闸 + task_create 开关入口

**Files:**
- Modify: `src/builtin_tools/team/task_review.rs`
- Modify: `src/builtin_tools/task_manage/create.rs`

**Interfaces:**
- Consumes: Task 6 的 `require_grounding` / `with_require_grounding`（`crate::agents::swarm::tasks::acceptance`）；Task 1 的 `loop-auditor`（指导文案引用）。
- Produces: `GroundingEvidence { kind, source, value, note }`（JsonSchema）；`TaskReviewArgs.grounding: Option<GroundingEvidence>`；`TaskCreateArgs.require_grounding: Option<bool>`；输出 status 新值 `"grounding_required"`；grounding 证据以 `[grounding] ...` 结构化 comment 记入 task（零迁移存证通道，审计环经 task 历史可读）。

- [ ] **Step 1: Write the failing tests**

`src/builtin_tools/team/task_review.rs` 测试模块追加（四格校验矩阵 + kind 闭集，纯函数风格与现有测试一致）：

```rust
    #[test]
    fn grounding_bounce_matrix() {
        use serde_json::json;
        let gated = json!({ "require_grounding": true });
        let open = json!({});
        // require on + approve + no evidence → bounce
        assert!(needs_grounding_bounce(ReviewDecision::Approve, &gated, false));
        // require on + approve + evidence → pass
        assert!(!needs_grounding_bounce(ReviewDecision::Approve, &gated, true));
        // require on + reject + no evidence → pass (rejection is conservative)
        assert!(!needs_grounding_bounce(ReviewDecision::Reject, &gated, false));
        // require off + approve + no evidence → pass
        assert!(!needs_grounding_bounce(ReviewDecision::Approve, &open, false));
    }

    #[test]
    fn grounding_kind_is_closed_vocabulary() {
        for k in ["exit_code", "numeric", "line_count"] {
            assert!(grounding_kind_valid(k));
        }
        assert!(!grounding_kind_valid("vibes"));
        assert!(!grounding_kind_valid(""));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib task_review -- --nocapture`
Expected: 编译错误 `cannot find function needs_grounding_bounce`

- [ ] **Step 3: Write implementation — task_review.rs**

import 区：`use crate::error::Result;` 改为 `use crate::error::{AlephError, Result};`，并追加：

```rust
use crate::agents::swarm::tasks::acceptance::require_grounding;
```

`ReviewDecision` 定义之后插入：

```rust
/// Reviewer-side anchor evidence backing an approval — a real measurement the
/// reviewer performed (not the submitter's self-report). `kind` uses the same
/// closed truth vocabulary as loop_graph anchor nodes (one anchor language
/// system-wide).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GroundingEvidence {
    /// exit_code | numeric | line_count
    pub kind: String,
    /// The real command run / independent data source consulted
    pub source: String,
    /// The measured value (exit code, number, line count)
    pub value: String,
    /// Optional context note
    #[serde(default)]
    pub note: Option<String>,
}

/// Closed grounding vocabulary — aligned with the loop_graph anchor truth set.
fn grounding_kind_valid(kind: &str) -> bool {
    matches!(kind, "exit_code" | "numeric" | "line_count")
}

/// Whether this review call must be bounced for missing grounding evidence.
/// Only approvals are gated: a rejection is naturally conservative. Pure /
/// host-testable.
#[must_use]
fn needs_grounding_bounce(
    decision: ReviewDecision,
    metadata: &serde_json::Value,
    has_grounding: bool,
) -> bool {
    matches!(decision, ReviewDecision::Approve)
        && require_grounding(metadata)
        && !has_grounding
}
```

`TaskReviewArgs` 的 `feedback` 字段之后追加：

```rust
    /// Grounding evidence backing an approval (a measurement you ran yourself,
    /// or one collected via subagent(agent_type='loop-auditor')). Required to
    /// approve when the task carries `require_grounding: true`.
    #[serde(default)]
    pub grounding: Option<GroundingEvidence>,
```

`call()` 中，authz 检查块（`if !is_authorized(...) { ... }`）之后、`let status = target_status(args.decision);` 之前插入：

```rust
        if let Some(g) = &args.grounding {
            if !grounding_kind_valid(&g.kind) {
                return Err(AlephError::tool(format!(
                    "task_review: grounding.kind '{}' invalid — must be one of \
                     exit_code | numeric | line_count",
                    g.kind
                )));
            }
        }
        if needs_grounding_bounce(args.decision, &task.metadata, args.grounding.is_some()) {
            return Ok(TaskReviewOutput {
                task_id: args.task_id,
                status: "grounding_required".into(),
                newly_unblocked: Vec::new(),
                message: "this task requires grounding evidence to approve: run a real \
                          measurement yourself (test/probe → exit_code, count → numeric, \
                          output size → line_count) — or spawn \
                          subagent(agent_type='loop-auditor') to collect it independently \
                          — then re-call task_review with the `grounding` field filled"
                    .into(),
            });
        }
```

`call()` 中，feedback comment 块（`if let Some(fb) = args.feedback...` 结束）之后插入存证：

```rust
        if let Some(g) = &args.grounding {
            // Durable, migration-free evidence trail: ride the task-comment
            // channel so auditors / loop-auditor can later verify the review
            // touched reality.
            let line = format!(
                "[grounding] kind={} source={} value={}{}",
                g.kind,
                g.source,
                g.value,
                g.note
                    .as_deref()
                    .map(|n| format!(" note={n}"))
                    .unwrap_or_default()
            );
            let _ = self
                .coord_store
                .add_task_comment(&args.task_id, &self.current_agent_id, &line)
                .await;
        }
```

`DESCRIPTION` 末尾（`verify the handle it returned (path, URL, id) yourself.` 之后）追加一句：

```text
 Tasks created with require_grounding=true bounce approvals lacking the \
 `grounding` evidence field (kind: exit_code | numeric | line_count).
```

`examples()` 追加一条：

```rust
            "task_review(task_id='task-3', decision='approve', grounding={kind:'exit_code', source:'cargo test -p alephcore --lib', value:'0'})".to_string(),
```

- [ ] **Step 4: Write implementation — create.rs**

import：`use crate::agents::swarm::tasks::acceptance::with_acceptance_criteria;` 改为：

```rust
use crate::agents::swarm::tasks::acceptance::{with_acceptance_criteria, with_require_grounding};
```

`TaskCreateArgs` 的 `acceptance_criteria` 字段之后追加：

```rust
    /// Require the reviewer to attach grounding evidence (a real measurement:
    /// exit_code / numeric / line_count) before this task can be approved.
    /// Use for tasks with verifiable side effects (tests, builds, published
    /// output); leave off for tasks with nothing measurable (prose review).
    #[serde(default)]
    pub require_grounding: Option<bool>,
```

metadata 组装链中把：

```rust
                with_acceptance_criteria(
                    with_managed_marker(args.metadata),
                    args.acceptance_criteria.unwrap_or_default(),
                ),
```

改为：

```rust
                with_require_grounding(
                    with_acceptance_criteria(
                        with_managed_marker(args.metadata),
                        args.acceptance_criteria.unwrap_or_default(),
                    ),
                    args.require_grounding.unwrap_or(false),
                ),
```

`task_create` 的 `DESCRIPTION` 末尾追加一句：

```text
 Set `require_grounding` to demand reviewer-side measurement evidence at the \
 approval gate.
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib task_review -- --nocapture && cargo test -p alephcore --lib task_manage -- --nocapture && cargo check -p alephcore`
Expected: 全 PASS（若 create.rs / 其它测试用字面量构造 `TaskCreateArgs` / `TaskReviewArgs` 报缺字段 E0063，补 `require_grounding: None` / `grounding: None`）

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/team/task_review.rs src/builtin_tools/task_manage/create.rs
git commit -m "teams: grounding evidence gate on task_review approvals"
```

---

### Task 8: WP4 — 编排智慧 + 文档修正 + skill 增补

**Files:**
- Modify: `src/teams/leader_prompt.rs`
- Modify: `docs/reference/GRAPH_LAYER.md`
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md`
- Modify: `/Volumes/TBU4/Workspace/Aleph-skills/loop-governance/SKILL.md`（兄弟仓）+ 同步 `~/.aleph/skills/loop-governance/SKILL.md`

**Interfaces:**
- Consumes: Task 1 `loop-auditor`、Task 7 `require_grounding`/`grounding`（prompt 文本引用它们，须与实现名精确一致）。
- Produces: 无代码接口；三条教义文本 + 文档。

- [ ] **Step 1: Write the failing test**

`src/teams/leader_prompt.rs` 测试模块追加：

```rust
    #[test]
    fn build_carries_orchestration_doctrine() {
        let out = build("t1", "Squad", "a (x)", None, "req");
        assert!(out.contains("防过度编排"));
        assert!(out.contains("require_grounding"));
        assert!(out.contains("loop-auditor"));
        assert!(out.contains("局部重跑") || out.contains("原地重做"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib leader_prompt -- --nocapture`
Expected: FAIL on `防过度编排`

- [ ] **Step 3: Implement leader_prompt doctrine**

在 `build()` 的 format! 字符串中，`不要自己闷头做完所有事…` 段之前插入编排纪律段。旧：

```text
         4. 全部子任务验收通过后，汇总成员产出，给用户一个清晰的最终答复。\n\n\
         不要自己闷头做完所有事，
```

新：

```text
         4. 全部子任务验收通过后，汇总成员产出，给用户一个清晰的最终答复。\n\n\
         编排纪律：\n\
         - 防过度编排：目标明确的短活别拆成任务网——一次委派（一个成员或一个 subagent）就够；只有出现并行、审批门、回滚、跨工具依赖时才值得任务 DAG。\n\
         - 审查要独立触地：成员的 task_submit 是自我报告，审查者不能只读它自证。验收有可测量产出的任务时自己跑测量，或派 subagent(agent_type='loop-auditor') 独立取证；创建这类任务时设 require_grounding=true，approve 时附 grounding 证据（kind: exit_code|numeric|line_count）。\n\
         - 失败局部重跑：reject 只把该任务退回原地重做（依赖图自动挡住下游），不要解散重建团队。\n\n\
         不要自己闷头做完所有事，
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib leader_prompt -- --nocapture`
Expected: PASS

- [ ] **Step 5: Update GRAPH_LAYER.md**

在 `docs/reference/GRAPH_LAYER.md` 末尾（或「实现参考」主体之后）追加一节：

```markdown
## 多智能体融合（2026-07-19 第二轮，spec: specs/2026-07-19-graph-multiagent-fusion-design.md）

- **独立视角**：审计/看守模板默认 `subagent(agent_type="loop-auditor")` 独立取证（builtin agent：`ContextMode::Fresh` 零继承、READ_ONLY+bash、denied file_write/file_edit/search/web_fetch）——治「共读同套数据互证正确」。落点 `src/agents/registry.rs` + `src/loop_graph/templates.rs`。
- **Team 显式入图**：`NodeKind::Team`（`team:<id>`）只经显式 node/pair 进图，快环 coord task 永不进表；status live-join `TeamStore`；`team_disband` 成功即胜利宣称 → `notify_team_settled` poke 看守（60s 去抖，与 goal 同内核 `notify_node_settled`）。落点 `src/loop_graph/{types,service}.rs` + `src/builtin_tools/{loop_graph_manage.rs,team/disband.rs}`。
- **Grounding 进执行层**：`task_create(require_grounding=true)`（acceptance metadata 通道，零迁移）→ `task_review` approve 无 `grounding` 证据即 bounce（`grounding_required`）；证据 kind 闭集与 anchor truth 同词表（exit_code|numeric|line_count），以 `[grounding]` comment 存证供审计环核验。reject 永不要求锚（拒绝天然保守）。落点 `src/agents/swarm/tasks/acceptance.rs` + `src/builtin_tools/team/task_review.rs`。
- **编排智慧**：leader prompt 三教义（防过度编排 / 审查独立触地 / 失败局部重跑），`src/teams/leader_prompt.rs`。
```

- [ ] **Step 6: Fix MULTI_AGENT_SYSTEM.md drift**

在 `docs/reference/MULTI_AGENT_SYSTEM.md` 中定位 `### Role Mechanism` 小节（从该标题起，到下一个同级或更高级标题之前的全部内容，含 `ReviewScore`、`TeamRoleConfig`、`Explorer-Critic Interaction Flow` 等描述），整段替换为：

```markdown
### Review & Acceptance Mechanism

> Doc-drift fix (2026-07-19): an earlier revision of this section described
> `review_score` / `ReviewScore` / `TeamRoleConfig` / `min_challenges`, none of
> which exist in the implementation. What follows is the real mechanism.

Roles are prompt-level only (leader orchestration preamble in
`src/teams/leader_prompt.rs` + per-member handoff context); there is no
code-level role enum for members.

**Review**: the leader accepts/rejects a submitted deliverable with the
`task_review` tool (`src/builtin_tools/team/task_review.rs`) — approve →
`Completed` (dependents unblock), reject → back to `InProgress` (redo in
place, feedback rides along). Verdicts are recorded on the task run
(`ReviewVerdict` / `ReviewerKind` in `src/agents/swarm/tasks/mod.rs`).

**Acceptance contract**: per-task policy lives in the task `metadata` JSON
channel (`src/agents/swarm/tasks/acceptance.rs`) — `acceptance_criteria`
(definition-of-done checklist rendered into the handoff prompt and the review
gate), `lead_review_required` (route successful runs to `WaitingReview`), and
`require_grounding` (approvals must carry reviewer-side measurement evidence).

**Grounding evidence** (2026-07-19): `task_review` accepts a structured
`grounding` field (`kind: exit_code | numeric | line_count` — the same closed
truth vocabulary as loop_graph anchors — plus `source`/`value`/`note`). When
the task metadata carries `require_grounding: true`, an approve without
evidence bounces with status `grounding_required`. Evidence is persisted as a
`[grounding]` task comment for later audit. Reviewers may collect evidence
independently via `subagent(agent_type="loop-auditor")` (fresh-context
measure-only builtin). See `docs/reference/GRAPH_LAYER.md` §多智能体融合.
```

- [ ] **Step 7: Update loop-governance skill (sibling repo)**

在 `/Volumes/TBU4/Workspace/Aleph-skills/loop-governance/SKILL.md` 末尾追加一节：

```markdown
## Graph × 多智能体（何时上 team，怎样独立取证）

**防过度编排**：单 Loop 适合目标明确的短任务；出现并行、审批门、回滚、跨工具依赖才值得 Team/任务 DAG。把简单任务过度编排是最常见的坑。

**独立视角铁律**：Graph 里的监督者若与被监督者共读同一套数据，只会互证正确。审计/看守取证默认派 `subagent(agent_type="loop-auditor")`——全新上下文、只测量不改写（READ_ONLY+bash，denied 写与网络搜索）。

**执行层 grounding**：有可测量产出的任务创建时设 `task_create(require_grounding=true)`；leader approve 必须附 `grounding` 证据，kind 闭集与 anchor truth 同词表：`exit_code`（真实跑过的命令退出码）| `numeric`（独立测得的数字）| `line_count`（产出行数）。无锚可举的任务（文案/设计评审）不开开关——逼造假锚是 Goodhart 反噬治理自身。

**Team 入图**：默认 Team 运行不碰治理图；需要被看守的长跑 Team 显式 `loop_graph(action="node", id="team:<id>", kind="team", ...)` + `pair`。`team_disband` 即胜利宣称，会即时戳中看守环复核。

**失败局部重跑**：reject 只退回该任务原地重做，依赖图自动挡住下游；不解散重建。
```

然后同步运行时 skill 副本并提交兄弟仓：

```bash
cp /Volumes/TBU4/Workspace/Aleph-skills/loop-governance/SKILL.md ~/.aleph/skills/loop-governance/SKILL.md
cd /Volumes/TBU4/Workspace/Aleph-skills && git add loop-governance/SKILL.md && git commit -m "loop-governance: add graph x multi-agent section" && cd /Volumes/TBU4/Workspace/Aleph
```

- [ ] **Step 8: Commit（主仓）**

```bash
git add src/teams/leader_prompt.rs docs/reference/GRAPH_LAYER.md docs/reference/MULTI_AGENT_SYSTEM.md
git commit -m "teams/docs: orchestration doctrine and multi-agent doc drift fix"
```

---

### Task 9: 全量验证收尾

**Files:** 无新改动（只验证；如出回归按最小 diff 修复）

- [ ] **Step 1: Run the full lib test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -5`
Expected: 全部 PASS（基线 13,937+，本计划新增 ~10 个测试）

- [ ] **Step 2: Clippy on touched modules**

Run: `cargo clippy -p alephcore --lib 2>&1 | grep -E "warning|error" | head -20`
Expected: 触碰文件无新 warning（既有仓库级 warning 不属本计划管辖，不修）

- [ ] **Step 3: 如有回归**

按失败输出最小修复（改一处→重跑该模块测试→再全量），修复与原 task 同 scope 时 amend 对应提交不可行（已连续提交）→ 独立小提交 `fix: <what>`。

- [ ] **Step 4: 汇报**

向用户汇报：提交清单（主仓 8 笔 + 兄弟仓 1 笔）、全量测试数字、遗留事项（运行时 QA 需重编部署 daemon；Aleph-skills gitlink bump 待推送时一并处理）。
