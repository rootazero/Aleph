//! `loop_graph` tool — the R8 face of the loop-graph governance layer.
//!
//! One multiplexed tool (cron_manage style): register loops/anchors/frozen
//! rules/roots as nodes, wire the six governance verbs as edges, render a
//! live status (topology + structural lint + best-effort joins against the
//! goal/cron stores), and install the audit loop. All semantic judgment —
//! which counter-metric to pair, whether a win was cheap, how to arbitrate —
//! stays with the LLM (R7); this tool only stores structure and moves facts.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::{AlephError, Result};
use crate::loop_graph::templates::{AUDIT_DEFAULT_CRON_EXPR, AUDIT_TEMPLATE};
use crate::loop_graph::{EdgeKind, GraphEdge, GraphNode, LoopGraphStore, NodeKind, Origin};
use crate::sync_primitives::Arc;
use crate::tasks::cron::{CronJob, ScheduleKind, SharedCronService};
use crate::teams::TeamStore;
use crate::tools::AlephTool;

/// Action to perform on the governance graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoopGraphAction {
    /// Register or update a node (loop binding / anchor / frozen / root).
    Node,
    /// Remove a node (its edges dangle as audit signals until `gc`).
    DropNode,
    /// Create or update an edge (six-verb closed vocabulary).
    Link,
    /// Remove an edge.
    Unlink,
    /// Raw nodes + edges dump.
    List,
    /// Rendered topology + structural lint + live loop state joins.
    Status,
    /// Remove dangling edges (explicit only, never automatic).
    Gc,
    /// Install the weekly audit loop: creates a cron with the audit template,
    /// registers it as a node, and wires `audits` edges to every existing
    /// optimization loop and frozen rule.
    EnableAudit,
    /// Goodhart-pairing sugar: create a watcher cron (watch template + your
    /// counter-metric instructions in `prompt`), register it as a node, and
    /// wire a `watches` edge to `to_id` in one call. WHICH counter-metric to
    /// watch is your judgment — the tool never generates it.
    Pair,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LoopGraphArgs {
    /// Action to perform
    pub action: LoopGraphAction,

    // ── node / drop_node ───────────────────────────────────────────
    /// Node id, prefixed by kind: `goal:<session_id>` | `cron:<job_id>` |
    /// `heartbeat:<task_id>` | `daemon:<name>` | `team:<team_id>` |
    /// `anchor:<slug>` | `frozen:<slug>` | `root:<slug>`
    #[serde(default)]
    pub id: Option<String>,
    /// Node kind (required for `node`)
    #[serde(default)]
    pub kind: Option<NodeKind>,
    /// One-line human-readable label (required for `node`)
    #[serde(default)]
    pub label: Option<String>,
    /// Anchor: `{probe, truth}` declaration where truth ∈ exit_code |
    /// numeric | line_count. Frozen: rule text + enforcement pointer.
    /// Root: the human-authored reference text.
    #[serde(default)]
    pub body: Option<String>,
    /// Declared pace: per_turn | hourly | nightly | weekly | monthly | free text
    #[serde(default)]
    pub cadence: Option<String>,
    /// Provenance: human | llm (default llm). Root nodes REQUIRE human —
    /// only pass origin="human" when the user explicitly instructed this.
    #[serde(default)]
    pub origin: Option<Origin>,

    // ── link / unlink ──────────────────────────────────────────────
    #[serde(default)]
    pub from_id: Option<String>,
    #[serde(default)]
    pub to_id: Option<String>,
    /// Edge verb: watches | owns_reference | arbitrates | audits |
    /// anchored_by | feeds (closed vocabulary — anything else belongs in
    /// notes, not in the graph)
    #[serde(default)]
    pub edge: Option<EdgeKind>,
    /// One-line rationale for the edge (prose; code never parses it)
    #[serde(default)]
    pub note: Option<String>,

    // ── enable_audit / pair ────────────────────────────────────────
    /// 6-field cron expression (enable_audit default: Monday 10:00;
    /// pair default: daily 09:30)
    #[serde(default)]
    pub cron_expr: Option<String>,
    /// pair: the counter-metric watch instructions (what to probe, from
    /// which adversarial angle) — appended to the watch template
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LoopGraphOutput {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<GraphNode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edges: Option<Vec<GraphEdge>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered: Option<String>,
}

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

    /// Attach the cron service handle (unlocks `enable_audit` and cron live
    /// joins in `status`). Absent = those degrade gracefully.
    #[must_use]
    pub fn with_cron_service(mut self, cron: Option<SharedCronService>) -> Self {
        self.cron = cron;
        self
    }

    /// Attach the team store handle (unlocks `team:<id>` live joins in
    /// `status`). Absent = team nodes render without a live line.
    #[must_use]
    pub fn with_team_store(mut self, teams: Option<Arc<dyn TeamStore>>) -> Self {
        self.teams = teams;
        self
    }

    fn expected_prefix(kind: NodeKind) -> &'static str {
        match kind {
            NodeKind::LoopGoal => "goal:",
            NodeKind::LoopCron => "cron:",
            NodeKind::LoopHeartbeat => "heartbeat:",
            NodeKind::Daemon => "daemon:",
            NodeKind::Team => "team:",
            NodeKind::Anchor => "anchor:",
            NodeKind::Frozen => "frozen:",
            NodeKind::Root => "root:",
        }
    }

    async fn render_status(&self, agent_id: &str) -> Result<String> {
        let nodes = self.store.list_nodes(agent_id)?;
        let edges = self.store.list_edges(agent_id)?;
        let lint = self.store.lint(agent_id)?;

        if nodes.is_empty() && edges.is_empty() {
            return Ok(
                "治理图为空。用 action='node' 登记第一个环（如 daemon:dreaming），\
                       用 action='enable_audit' 安装审计环。参考 skill `loop-governance`。"
                    .to_string(),
            );
        }

        // One cron list_jobs call, keyed by job id, for live joins.
        let cron_by_id: std::collections::HashMap<String, serde_json::Value> = match &self.cron {
            Some(svc) => {
                let service = svc.lock().await;
                match service.list_jobs().await {
                    Ok(jobs) => jobs
                        .into_iter()
                        .filter_map(|j| serde_json::to_value(&j).ok().map(|v| (j.id.clone(), v)))
                        .collect(),
                    Err(_) => std::collections::HashMap::new(),
                }
            }
            None => std::collections::HashMap::new(),
        };
        let goal_store = crate::goal::global();

        let mut out = String::new();
        out.push_str(&format!(
            "循环治理图（agent={agent_id}）: {} 节点 / {} 边\n\n== 节点 ==\n",
            nodes.len(),
            edges.len()
        ));
        for n in &nodes {
            out.push_str(&format!("• [{}] {} — {}", n.kind.as_str(), n.id, n.label));
            if let Some(c) = &n.cadence {
                out.push_str(&format!("  (cadence: {c})"));
            }
            out.push_str(&format!("  (origin: {})", n.origin.as_str()));
            // Live join — read the EXECUTING entity's own record, never a
            // graph-cached copy of it ("report vs reality" discipline).
            match n.kind {
                NodeKind::LoopGoal => {
                    if let Some(store) = &goal_store {
                        let session_id = n.id.trim_start_matches("goal:");
                        match store.get(session_id) {
                            Ok(Some(g)) => out.push_str(&format!(
                                "\n    live: status={:?} objective={}",
                                g.status,
                                truncate(&g.objective, 80)
                            )),
                            _ => out.push_str("\n    live: ⚠ target missing（goal 已消失）"),
                        }
                    }
                }
                NodeKind::LoopCron => {
                    let job_id = n.id.trim_start_matches("cron:");
                    match cron_by_id.get(job_id) {
                        Some(v) => {
                            let state = &v["state"];
                            out.push_str(&format!(
                                "\n    live: enabled={} runs={} last={} consecutive_errors={}",
                                v["enabled"],
                                state["run_count"],
                                state["last_run_status"],
                                state["consecutive_errors"]
                            ));
                        }
                        None if self.cron.is_some() => {
                            out.push_str("\n    live: ⚠ target missing（cron job 已消失）");
                        }
                        None => {}
                    }
                }
                NodeKind::Anchor | NodeKind::Frozen | NodeKind::Root => {
                    if let Some(b) = &n.body {
                        out.push_str(&format!("\n    {}", truncate(b, 120)));
                    }
                }
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
                            _ => out.push_str("\n    live: ⚠ target missing（team 记录已消失）"),
                        }
                    }
                }
                NodeKind::LoopHeartbeat | NodeKind::Daemon => {}
            }
            out.push('\n');
        }

        out.push_str("\n== 边 ==\n");
        for e in &edges {
            out.push_str(&format!(
                "• {} -[{}]-> {}",
                e.from_id,
                e.kind.as_str(),
                e.to_id
            ));
            if let Some(note) = &e.note {
                out.push_str(&format!("  ({})", truncate(note, 60)));
            }
            out.push('\n');
        }

        if lint.is_empty() {
            out.push_str("\n== 结构 lint ==\n（无发现）\n");
        } else {
            out.push_str("\n== 结构 lint ==\n");
            for f in &lint {
                out.push_str(&format!("⚠ {f}\n"));
            }
        }
        out.push_str(
            "\n历史裁决：检索 tags 含 `graph-audit` 的记忆 note（审计判决书）与 \
             `reference-proposal`（待裁参照提案）。",
        );
        Ok(out)
    }
}

#[async_trait]
impl AlephTool for LoopGraphTool {
    const NAME: &'static str = "loop_graph";
    const DESCRIPTION: &'static str = "Manage the loop-graph governance topology: register \
        self-improvement loops (goal/cron/heartbeat/daemon/team), anchors (irrefutable measurements), \
        frozen rules and human root references as nodes; wire the six governance verbs \
        (watches/owns_reference/arbitrates/audits/anchored_by/feeds) as edges; render live \
        status with structural lint; install the weekly audit loop (enable_audit). Use when the \
        user says 配看守/建审计环/治理循环/loop graph, when creating a goal or cron that \
        optimizes a metric (pair it with a counter-metric watcher), or at the start of an audit \
        tick. Root nodes require origin='human' and an explicit user instruction. See skill \
        `loop-governance` for doctrine.";

    type Args = LoopGraphArgs;
    type Output = LoopGraphOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // The graph is scoped to the default agent, full stop. `agent_id` used
        // to be a model-facing arg, but every READ path hardcodes "main" —
        // `service.rs` (watcher pokes / objective ACL / prompt injection) and
        // the doctor lint — so a graph registered elsewhere enforced nothing
        // while `pair` still promised the poke. Withdrawn rather than left as a
        // knob for a consumer that does not exist (P6). The store column stays;
        // wiring real scoping means teaching the readers, not re-adding an arg.
        let agent_id = crate::routing::DEFAULT_AGENT_ID.to_string();
        let origin = args.origin.unwrap_or(Origin::Llm);

        match args.action {
            LoopGraphAction::Node => {
                let id = require(args.id, "node", "id")?;
                let kind = args
                    .kind
                    .ok_or_else(|| AlephError::tool("loop_graph node: 'kind' is required"))?;
                let label = require(args.label, "node", "label")?;
                let prefix = Self::expected_prefix(kind);
                if !id.starts_with(prefix) {
                    return Err(AlephError::tool(format!(
                        "loop_graph node: id '{id}' must start with '{prefix}' for kind {}",
                        kind.as_str()
                    )));
                }
                if kind == NodeKind::Anchor {
                    let ok = args.body.as_deref().is_some_and(|b| {
                        ["exit_code", "numeric", "line_count"]
                            .iter()
                            .any(|t| b.contains(t))
                    });
                    if !ok {
                        return Err(AlephError::tool(
                            "loop_graph node: anchor 节点必须在 body 声明 {probe, truth}，\
                             truth ∈ exit_code | numeric | line_count（\"不可辩驳\"要可校验）",
                        ));
                    }
                }
                let mut node = GraphNode::new(&agent_id, &id, kind, &label, origin);
                node.body = args.body;
                node.cadence = args.cadence;
                self.store.upsert_node(&node)?;
                info!(id = %id, kind = %kind.as_str(), "loop_graph node upserted");
                Ok(LoopGraphOutput {
                    message: format!(
                        "节点 {id} 已登记（{}，origin={}）",
                        kind.as_str(),
                        origin.as_str()
                    ),
                    nodes: None,
                    edges: None,
                    rendered: None,
                })
            }

            LoopGraphAction::DropNode => {
                let id = require(args.id, "drop_node", "id")?;
                let removed = self.store.delete_node(&agent_id, &id)?;
                Ok(LoopGraphOutput {
                    message: if removed {
                        format!("节点 {id} 已移除；指向它的边将作为悬空审计信号保留，用 gc 清理")
                    } else {
                        format!("节点 {id} 不存在")
                    },
                    nodes: None,
                    edges: None,
                    rendered: None,
                })
            }

            LoopGraphAction::Link => {
                let from_id = require(args.from_id, "link", "from_id")?;
                let to_id = require(args.to_id, "link", "to_id")?;
                let kind = args
                    .edge
                    .ok_or_else(|| AlephError::tool("loop_graph link: 'edge' is required"))?;
                if kind == EdgeKind::AnchoredBy {
                    let target = self.store.get_node(&agent_id, &to_id)?;
                    if target.map(|n| n.kind) != Some(NodeKind::Anchor) {
                        return Err(AlephError::tool(format!(
                            "loop_graph link: anchored_by 的 to_id ('{to_id}') 必须是 anchor 节点"
                        )));
                    }
                }
                let mut edge = GraphEdge::new(&agent_id, &from_id, &to_id, kind, origin);
                edge.note = args.note;
                self.store.upsert_edge(&edge)?;
                Ok(LoopGraphOutput {
                    message: format!("边已建立: {from_id} -[{}]-> {to_id}", kind.as_str()),
                    nodes: None,
                    edges: None,
                    rendered: None,
                })
            }

            LoopGraphAction::Unlink => {
                let from_id = require(args.from_id, "unlink", "from_id")?;
                let to_id = require(args.to_id, "unlink", "to_id")?;
                let kind = args
                    .edge
                    .ok_or_else(|| AlephError::tool("loop_graph unlink: 'edge' is required"))?;
                let removed = self.store.delete_edge(&agent_id, &from_id, &to_id, kind)?;
                Ok(LoopGraphOutput {
                    message: if removed {
                        format!("边已移除: {from_id} -[{}]-> {to_id}", kind.as_str())
                    } else {
                        "该边不存在".to_string()
                    },
                    nodes: None,
                    edges: None,
                    rendered: None,
                })
            }

            LoopGraphAction::List => {
                let nodes = self.store.list_nodes(&agent_id)?;
                let edges = self.store.list_edges(&agent_id)?;
                Ok(LoopGraphOutput {
                    message: format!("{} 节点 / {} 边", nodes.len(), edges.len()),
                    nodes: Some(nodes),
                    edges: Some(edges),
                    rendered: None,
                })
            }

            LoopGraphAction::Status => {
                let rendered = self.render_status(&agent_id).await?;
                Ok(LoopGraphOutput {
                    message: "治理图状态".to_string(),
                    nodes: None,
                    edges: None,
                    rendered: Some(rendered),
                })
            }

            LoopGraphAction::Gc => {
                let removed = self.store.gc(&agent_id)?;
                Ok(LoopGraphOutput {
                    message: if removed.is_empty() {
                        "无悬空边".to_string()
                    } else {
                        format!("已清理 {} 条悬空边: {}", removed.len(), removed.join("; "))
                    },
                    nodes: None,
                    edges: None,
                    rendered: None,
                })
            }

            LoopGraphAction::EnableAudit => {
                // Idempotency BEFORE the external dependency: "it is already
                // installed" is the accurate answer and stays accurate whether
                // or not a cron handle happens to be attached. Checking cron
                // first reported "cron service unavailable" for a graph whose
                // real problem was that it already had an auditor.
                //
                // One audit loop per agent scope. The guard keys on a LIVE
                // audit node, not on "any audits edge exists": `delete_node`
                // deliberately leaves edges dangling as audit signals, so
                // following this error's own advice (`drop_node`) used to make
                // the audit loop permanently un-reinstallable — and any
                // hand-wired `audits` edge (a first-class verb) blocked the
                // installer while naming an unrelated node.
                let edges = self.store.list_edges(&agent_id)?;
                let nodes = self.store.list_nodes(&agent_id)?;
                let live = edges.iter().find(|e| {
                    e.kind == EdgeKind::Audits && nodes.iter().any(|n| n.id == e.from_id)
                });
                if let Some(existing) = live {
                    return Err(AlephError::tool(format!(
                        "审计环已存在（{}）。如需重装：先 drop_node 它，再 gc 清掉它留下的悬空 audits 边。",
                        existing.from_id
                    )));
                }
                let Some(cron) = &self.cron else {
                    return Err(AlephError::tool(
                        "loop_graph enable_audit: cron service unavailable",
                    ));
                };
                let expr = args
                    .cron_expr
                    .unwrap_or_else(|| AUDIT_DEFAULT_CRON_EXPR.to_string());
                crate::tasks::shared::schedule::compute_next_cron(&expr, None, chrono::Utc::now())
                    .map_err(|e| AlephError::tool(format!("Invalid cron schedule: {e}")))?;

                let job = CronJob::new(
                    "循环治理·审计环",
                    &agent_id,
                    AUDIT_TEMPLATE,
                    ScheduleKind::Cron {
                        expr: expr.clone(),
                        tz: None,
                        stagger_ms: None,
                    },
                );
                let job_id = {
                    let service = cron.lock().await;
                    service.add_job(job).await.map_err(|e| {
                        AlephError::tool(format!("Failed to create audit cron: {e}"))
                    })?
                };

                let audit_node_id = format!("cron:{job_id}");
                let node = GraphNode::new(
                    &agent_id,
                    &audit_node_id,
                    NodeKind::LoopCron,
                    "循环治理·审计环",
                    origin,
                )
                .with_cadence("weekly")
                .with_body("唯一职责：验证其他环的测量仍触到现实。模板见 loop_graph::templates::AUDIT_TEMPLATE");
                self.store.upsert_node(&node)?;

                let targets: Vec<GraphNode> = self
                    .store
                    .list_nodes(&agent_id)?
                    .into_iter()
                    .filter(|n| {
                        n.id != audit_node_id
                            && (n.kind.is_optimization_loop() || n.kind == NodeKind::Frozen)
                    })
                    .collect();
                for t in &targets {
                    self.store.upsert_edge(
                        &GraphEdge::new(&agent_id, &audit_node_id, &t.id, EdgeKind::Audits, origin)
                            .with_note("enable_audit 自动接线"),
                    )?;
                }
                info!(job_id = %job_id, targets = targets.len(), "audit loop installed");
                Ok(LoopGraphOutput {
                    message: format!(
                        "审计环已安装（cron {job_id}，{expr}），audits 接线 {} 个节点。\
                         后续新登记的环需手动补 audits 边或由审计环自查接线。",
                        targets.len()
                    ),
                    nodes: None,
                    edges: None,
                    rendered: None,
                })
            }

            LoopGraphAction::Pair => {
                let Some(cron) = &self.cron else {
                    return Err(AlephError::tool(
                        "loop_graph pair: cron service unavailable",
                    ));
                };
                let to_id = require(args.to_id, "pair", "to_id")?;
                let label = require(args.label, "pair", "label")?;
                let watch_prompt = require(args.prompt, "pair", "prompt")?;
                if self.store.get_node(&agent_id, &to_id)?.is_none() {
                    return Err(AlephError::tool(format!(
                        "loop_graph pair: 被看守节点 '{to_id}' 不存在——先用 action='node' 登记它"
                    )));
                }
                let expr = args.cron_expr.unwrap_or_else(|| {
                    crate::loop_graph::templates::WATCH_DEFAULT_CRON_EXPR.to_string()
                });
                crate::tasks::shared::schedule::compute_next_cron(&expr, None, chrono::Utc::now())
                    .map_err(|e| AlephError::tool(format!("Invalid cron schedule: {e}")))?;

                let full_prompt = format!(
                    "{}{watch_prompt}{}",
                    crate::loop_graph::templates::WATCH_TEMPLATE_HEADER,
                    crate::loop_graph::templates::WATCH_TEMPLATE_FOOTER
                );
                let job = CronJob::new(
                    &label,
                    &agent_id,
                    &full_prompt,
                    ScheduleKind::Cron {
                        expr: expr.clone(),
                        tz: None,
                        stagger_ms: None,
                    },
                );
                let job_id = {
                    let service = cron.lock().await;
                    service.add_job(job).await.map_err(|e| {
                        AlephError::tool(format!("Failed to create watcher cron: {e}"))
                    })?
                };
                let watcher_id = format!("cron:{job_id}");
                self.store.upsert_node(
                    &GraphNode::new(&agent_id, &watcher_id, NodeKind::LoopCron, &label, origin)
                        .with_cadence(args.cadence.unwrap_or_else(|| "nightly".to_string()))
                        .with_body(truncate(&watch_prompt, 200)),
                )?;
                self.store.upsert_edge(
                    &GraphEdge::new(&agent_id, &watcher_id, &to_id, EdgeKind::Watches, origin)
                        .with_note(args.note.unwrap_or_else(|| "pair 配对".to_string())),
                )?;
                info!(job_id = %job_id, target = %to_id, "watcher paired");
                Ok(LoopGraphOutput {
                    message: format!(
                        "看守环已配对: {watcher_id} -[watches]-> {to_id}（{expr}）。\
                         被看守 goal/team 的胜利宣称（goal 完成 / team 解散）还会即时触发本看守（post-run 钩子，去抖 60s）。"
                    ),
                    nodes: None,
                    edges: None,
                    rendered: None,
                })
            }
        }
    }
}

fn require(v: Option<String>, action: &str, field: &str) -> Result<String> {
    v.filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AlephError::tool(format!("loop_graph {action}: '{field}' is required")))
}

fn truncate(s: &str, max_chars: usize) -> String {
    // UTF-8 safe truncation (P7).
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> (tempfile::TempDir, LoopGraphTool) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(LoopGraphStore::open(&dir.path().join("g.db")).unwrap());
        (dir, LoopGraphTool::new(store))
    }

    fn args(action: LoopGraphAction) -> LoopGraphArgs {
        LoopGraphArgs {
            action,
            id: None,
            kind: None,
            label: None,
            body: None,
            cadence: None,
            origin: None,
            from_id: None,
            to_id: None,
            edge: None,
            note: None,
            cron_expr: None,
            prompt: None,
        }
    }

    #[tokio::test]
    async fn node_requires_prefix_matching_kind() {
        let (_d, t) = tool();
        let mut a = args(LoopGraphAction::Node);
        a.id = Some("dreaming".into());
        a.kind = Some(NodeKind::Daemon);
        a.label = Some("夜巡".into());
        assert!(t.call(a).await.is_err());

        let mut a = args(LoopGraphAction::Node);
        a.id = Some("daemon:dreaming".into());
        a.kind = Some(NodeKind::Daemon);
        a.label = Some("夜巡".into());
        assert!(t.call(a).await.is_ok());
    }

    #[tokio::test]
    async fn anchor_requires_truth_declaration() {
        let (_d, t) = tool();
        let mut a = args(LoopGraphAction::Node);
        a.id = Some("anchor:corrections".into());
        a.kind = Some(NodeKind::Anchor);
        a.label = Some("用户真实纠正".into());
        a.body = Some("查一下纠正数".into());
        assert!(t.call(a).await.is_err(), "anchor without truth must fail");

        let mut a = args(LoopGraphAction::Node);
        a.id = Some("anchor:corrections".into());
        a.kind = Some(NodeKind::Anchor);
        a.label = Some("用户真实纠正".into());
        a.body = Some("probe: sqlite3 …count(*)…; truth: numeric".into());
        assert!(t.call(a).await.is_ok());
    }

    #[tokio::test]
    async fn root_via_tool_requires_human_origin() {
        let (_d, t) = tool();
        let mut a = args(LoopGraphAction::Node);
        a.id = Some("root:aleph".into());
        a.kind = Some(NodeKind::Root);
        a.label = Some("什么算更好".into());
        // default origin = llm → store invariant rejects
        assert!(t.call(a.clone()).await.is_err());
        a.origin = Some(Origin::Human);
        assert!(t.call(a).await.is_ok());
    }

    #[tokio::test]
    async fn anchored_by_must_target_anchor_node() {
        let (_d, t) = tool();
        for (id, kind, label) in [
            ("daemon:dreaming", NodeKind::Daemon, "夜巡"),
            ("anchor:tests", NodeKind::Anchor, "真实测试"),
        ] {
            let mut a = args(LoopGraphAction::Node);
            a.id = Some(id.into());
            a.kind = Some(kind);
            a.label = Some(label.into());
            if kind == NodeKind::Anchor {
                a.body = Some("probe: cargo test; truth: exit_code".into());
            }
            t.call(a).await.unwrap();
        }
        let mut bad = args(LoopGraphAction::Link);
        bad.from_id = Some("anchor:tests".into());
        bad.to_id = Some("daemon:dreaming".into());
        bad.edge = Some(EdgeKind::AnchoredBy);
        assert!(
            t.call(bad).await.is_err(),
            "anchored_by must point AT an anchor"
        );

        let mut good = args(LoopGraphAction::Link);
        good.from_id = Some("daemon:dreaming".into());
        good.to_id = Some("anchor:tests".into());
        good.edge = Some(EdgeKind::AnchoredBy);
        assert!(t.call(good).await.is_ok());
    }

    #[tokio::test]
    async fn status_renders_lint_and_empty_graph_hint() {
        let (_d, t) = tool();
        let out = t.call(args(LoopGraphAction::Status)).await.unwrap();
        assert!(out.rendered.unwrap().contains("治理图为空"));

        let mut a = args(LoopGraphAction::Node);
        a.id = Some("daemon:dreaming".into());
        a.kind = Some(NodeKind::Daemon);
        a.label = Some("夜巡".into());
        t.call(a).await.unwrap();
        let out = t.call(args(LoopGraphAction::Status)).await.unwrap();
        let rendered = out.rendered.unwrap();
        assert!(rendered.contains("daemon:dreaming"));
        assert!(
            rendered.contains("裸奔优化环"),
            "naked loop must surface: {rendered}"
        );
    }

    #[tokio::test]
    async fn enable_audit_without_cron_service_fails_gracefully() {
        let (_d, t) = tool();
        let err = t
            .call(args(LoopGraphAction::EnableAudit))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("cron service unavailable"));
    }

    #[tokio::test]
    async fn enable_audit_is_blocked_by_a_live_auditor_but_not_by_a_dangling_edge() {
        let (_d, t) = tool();
        for (id, kind) in [
            ("cron:aud", NodeKind::LoopCron),
            ("daemon:dreaming", NodeKind::Daemon),
        ] {
            let mut a = args(LoopGraphAction::Node);
            a.id = Some(id.into());
            a.kind = Some(kind);
            a.label = Some(id.into());
            t.call(a).await.unwrap();
        }
        let mut link = args(LoopGraphAction::Link);
        link.from_id = Some("cron:aud".into());
        link.to_id = Some("daemon:dreaming".into());
        link.edge = Some(EdgeKind::Audits);
        t.call(link).await.unwrap();

        // A LIVE auditor blocks reinstall — that part is intended.
        let err = t
            .call(args(LoopGraphAction::EnableAudit))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("审计环已存在"), "{err}");
        assert!(err.contains("gc"), "the remedy must name gc too: {err}");

        // Follow the error's own advice: drop_node leaves the edge dangling by
        // design. The guard used to key on "any audits edge", so this made the
        // audit loop permanently un-reinstallable. Now it gets past the guard
        // and fails only on the absent cron service.
        let mut drop = args(LoopGraphAction::DropNode);
        drop.id = Some("cron:aud".into());
        t.call(drop).await.unwrap();
        let err = t
            .call(args(LoopGraphAction::EnableAudit))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("cron service unavailable"),
            "a dangling audits edge must not block reinstall: {err}"
        );
    }

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
}
