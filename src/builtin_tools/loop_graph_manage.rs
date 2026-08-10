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
use tracing::{info, warn};

use crate::error::{AlephError, Result};
use crate::loop_graph::templates::{AUDIT_DEFAULT_CRON_EXPR, AUDIT_JOB_NAME, AUDIT_TEMPLATE};
use crate::loop_graph::{EdgeKind, GraphEdge, GraphNode, LoopGraphStore, NodeKind, Origin};
use crate::sync_primitives::Arc;
use crate::tasks::cron::{CronJob, ScheduleKind, SharedCronService};
use crate::teams::TeamStore;
use crate::tools::AlephTool;
use crate::utils::text_format::truncate_with_marker;

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
    /// One-line rationale for the edge (prose; code never parses it).
    /// Omitting it on a re-`link` KEEPS the existing rationale (pass `""` to
    /// clear) — same rule as a node's `body`/`cadence`.
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

    // ── Internal (injected by the dispatcher, not LLM-visible) ─────
    /// Source channel id, injected from turn context. Stamped onto the cron
    /// jobs `enable_audit` / `pair` install so their output has somewhere to
    /// go — the audit template's step 7 is "上报: 通知用户", and `cron_manage`
    /// has plumbed this since it was written. Without it both governance loops
    /// run, reach a verdict, and deliver it nowhere (R5).
    #[serde(default, rename = "__channel")]
    #[schemars(skip)]
    pub __channel: Option<String>,

    /// Source conversation id (e.g. a Telegram `chat_id`), injected from turn
    /// context — the channel alone does not identify where to deliver.
    #[serde(default, rename = "__conversation_id")]
    #[schemars(skip)]
    pub __conversation_id: Option<String>,
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

    /// Fan `audits` edges from the audit node onto every optimization loop and
    /// frozen rule. Returns how many landed.
    ///
    /// A per-edge failure is logged and skipped rather than propagated: by the
    /// time this runs the cron job and the marker node are committed, so
    /// returning `Err` reports total failure over a LIVE audit ring — and the
    /// caller's only documented next step ("重装") then hits the idempotency
    /// guard. The edges are advisory wiring the audit template is told to
    /// re-check anyway ("后续新登记的环需手动补 audits 边或由审计环自查接线"),
    /// so a missing one degrades coverage; a bogus error message destroys the
    /// operator's model of what exists.
    fn wire_audit_edges(
        &self,
        agent_id: &str,
        audit_node_id: &str,
        origin: Origin,
    ) -> Result<usize> {
        let targets: Vec<GraphNode> = self
            .store
            .list_nodes(agent_id)?
            .into_iter()
            .filter(|n| {
                n.id != audit_node_id
                    && (n.kind.is_optimization_loop() || n.kind == NodeKind::Frozen)
            })
            .collect();
        let mut wired = 0usize;
        for t in &targets {
            match self.store.upsert_edge(
                &GraphEdge::new(agent_id, audit_node_id, &t.id, EdgeKind::Audits, origin)
                    .with_note("enable_audit 自动接线"),
            ) {
                Ok(()) => wired += 1,
                Err(e) => warn!(target = %t.id, error = %e,
                    "loop_graph enable_audit: audits edge not wired — \
                     the audit ring is live but does not cover this node yet"),
            }
        }
        Ok(wired)
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
        //
        // `None` means "the roster was read and this job is not in it".
        // A failed read must NOT collapse into that: an empty map plus
        // `self.cron.is_some()` printed "⚠ target missing（cron job 已消失）"
        // against EVERY cron node at once, and the audit template feeds
        // exactly that line into its 点名 step — one transient error would
        // manufacture audit findings against a graph of healthy loops.
        let cron_by_id: Option<std::collections::HashMap<String, serde_json::Value>> =
            match &self.cron {
                Some(svc) => {
                    let service = svc.lock().await;
                    match service.list_jobs().await {
                        Ok(jobs) => Some(
                            jobs.into_iter()
                                .filter_map(|j| {
                                    serde_json::to_value(&j).ok().map(|v| (j.id.clone(), v))
                                })
                                .collect(),
                        ),
                        Err(e) => {
                            tracing::warn!(error = %e,
                                "loop_graph status: cron roster unreadable; live joins skipped");
                            None
                        }
                    }
                }
                None => None,
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
                        // Three-way, like the cron arm: "the store answered and
                        // this row is gone" is a finding; "I could not read the
                        // store" is not. Collapsing them let a single
                        // SQLITE_BUSY brand every goal node in one pass as
                        // vanished — and the audit template reads exactly this
                        // line when it calls the roll, so a transient error
                        // manufactured findings against a healthy graph.
                        match store.get(session_id) {
                            Ok(Some(g)) => out.push_str(&format!(
                                "\n    live: status={:?} objective={}",
                                g.status,
                                truncate_with_marker(&g.objective, 80, "…")
                            )),
                            Ok(None) => {
                                out.push_str("\n    live: ⚠ target missing（goal 已消失）");
                            }
                            Err(e) => {
                                tracing::warn!(node = %n.id, error = %e,
                                    "loop_graph status: goal store unreadable — live line omitted");
                            }
                        }
                    }
                }
                NodeKind::LoopCron => {
                    let job_id = n.id.trim_start_matches("cron:");
                    match cron_by_id.as_ref().map(|m| m.get(job_id)) {
                        Some(Some(v)) => {
                            let state = &v["state"];
                            out.push_str(&format!(
                                "\n    live: enabled={} runs={} last={} consecutive_errors={}",
                                v["enabled"],
                                state["run_count"],
                                state["last_run_status"],
                                state["consecutive_errors"]
                            ));
                        }
                        // Roster read, job absent — a real finding.
                        Some(None) => {
                            out.push_str("\n    live: ⚠ target missing（cron job 已消失）");
                        }
                        // Roster unavailable — say nothing rather than accuse.
                        None => {}
                    }
                }
                NodeKind::Anchor | NodeKind::Frozen | NodeKind::Root => {
                    if let Some(b) = &n.body {
                        out.push_str(&format!("\n    {}", truncate_with_marker(b, 120, "…")));
                    }
                }
                NodeKind::Team => {
                    if let Some(ts) = &self.teams {
                        let team_id = n.id.trim_start_matches("team:");
                        // Same three-way discipline as the goal / cron arms.
                        match ts.get_team(team_id).await {
                            Ok(Some(t)) => out.push_str(&format!(
                                "\n    live: status={} leader={} name={}",
                                t.status.as_str(),
                                t.leader_id,
                                truncate_with_marker(&t.name, 40, "…")
                            )),
                            // `Ok(None)` is TWO answers here and this render
                            // cannot tell them apart: the `TeamStore` in hand
                            // is `ScopedTeamStore`, whose `get_team` filters
                            // through `team_visible` → `ambient_owner_visible`,
                            // an owner-keyed and role-BLIND predicate. So a
                            // second admin auditing a ring whose `team:` node
                            // belongs to another admin gets a refusal that used
                            // to read as "记录已消失" — and AUDIT_TEMPLATE step 5
                            // takes exactly this line as a naming-and-shaming
                            // input, so the layer manufactured a standing audit
                            // finding about a perfectly healthy team. Refusal is
                            // deliberately indistinguishable from absence (the
                            // no-oracle rule), which means the honest render is
                            // the one that does not choose.
                            Ok(None) => {
                                out.push_str(
                                    "\n    live: ⚠ 读不到该 team（不存在，或不属于当前调用者）\
                                     ——不要据此点名",
                                );
                            }
                            Err(e) => {
                                tracing::warn!(node = %n.id, error = %e,
                                    "loop_graph status: team store unreadable — live line omitted");
                            }
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
                out.push_str(&format!("  ({})", truncate_with_marker(note, 60, "…")));
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
        let delivery = (args.__channel.clone(), args.__conversation_id.clone());

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
                // Report the origin the STORE now holds, not the one that was
                // passed. `origin` is write-once by design (it is the
                // provenance the audit template checks), so re-registering an
                // existing node with `origin='human'` keeps `llm` — and echoing
                // the argument told the caller a human attested a row that
                // still says otherwise. The write-once rule is the right one;
                // the message was the part that lied.
                let stored_origin = self.store.get_node(&agent_id, &id)?.map_or_else(
                    || origin.as_str().to_string(),
                    |n| n.origin.as_str().to_string(),
                );
                let note = if stored_origin == origin.as_str() {
                    String::new()
                } else {
                    format!(
                        "（本次传入 origin={}，但 provenance 写一次不改写——要改须先 drop_node 再重建）",
                        origin.as_str()
                    )
                };
                Ok(LoopGraphOutput {
                    message: format!(
                        "节点 {id} 已登记（{}，origin={stored_origin}）{note}",
                        kind.as_str()
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
                        // The node is a BINDING to a live entity, never a copy
                        // of it — so dropping it unbinds, it does not stop
                        // anything. For `cron:` that distinction is expensive:
                        // the job keeps running its governance prompt weekly
                        // while `status`/`list`/`lint`/doctor, which all iterate
                        // graph rows, can no longer see it.
                        let mut m = format!(
                            "节点 {id} 已移除；指向它的边将作为悬空审计信号保留，用 gc 清理"
                        );
                        if let Some(job) = id.strip_prefix("cron:") {
                            m.push_str(&format!(
                                "。注意：cron job '{job}' 仍在按原时间表运行——图里只是不再登记它，\
                                 status/list/lint 从此看不见它。确需停掉请用 \
                                 cron_manage(action='delete', id='{job}')"
                            ));
                        }
                        m
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
                // Say so when the edge that was just built cannot carry the
                // one thing a `watches` edge is chiefly for. Such a watcher
                // satisfies `lint_naked_loops` and renders in the prompt
                // exactly like a real one, so the graph LOOKS sound while
                // victory claims go unreviewed until the watcher's own cadence
                // comes round. The model has no way to know that from the graph
                // itself — it learns it here or not at all.
                //
                // Ask BOTH halves (`immediate_review_reaches`), because a
                // promise of immediate review needs a wakeable watcher AND a
                // target that announces a win — and for a non-`goal:` target
                // this is the only surface that could ever ask the second half:
                // `render_session_topology_in` builds its node id as
                // `goal:{session}`, so a `daemon:`/`cron:`/`heartbeat:` target
                // has no prompt surface at all. Round 11 mirrored the target
                // half onto `pair` and did not carry it back here; the flagship
                // dogfood pairing in GRAPH_LAYER §4.1 (a watcher on
                // `daemon:dreaming`) is exactly the shape that fell through.
                let caveat = if kind != EdgeKind::Watches
                    || crate::loop_graph::service::immediate_review_reaches(&from_id, &to_id)
                {
                    ""
                } else if !crate::loop_graph::service::target_has_victory_claim(&to_id) {
                    // Target half first: when it fails, `pair` is not a remedy
                    // either — it builds a `cron:` watcher, and that watcher's
                    // own success message says the same thing. Recommending it
                    // here would send the model to install a real cron job for
                    // a promise no action can keep.
                    "。注意：只有 goal:/team: 目标有胜利宣称触发点，\
                     本目标没有终态时刻可挂，所以任何看守都只按自己的节奏跑，不会有即时评审。"
                } else {
                    "。注意：只有 cron: 看守能被胜利宣称即时唤醒，\
                     本看守只会按它自己的节奏跑（要即时评审就用 action='pair'）。"
                };
                Ok(LoopGraphOutput {
                    message: format!("边已建立: {from_id} -[{}]-> {to_id}{caveat}", kind.as_str()),
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
                let report = self.store.gc(&agent_id)?;
                let mut message = if report.removed.is_empty() {
                    "无悬空边".to_string()
                } else {
                    format!(
                        "已清理 {} 条悬空边: {}",
                        report.removed.len(),
                        report.removed.join("; ")
                    )
                };
                // Say what was deliberately kept. A caller who asked to clear
                // dangling rows and is told "无悬空边" while an owns_reference
                // row still enforces would go looking for a bug that is not
                // there — and the honest line is also the one that names the
                // single sanctioned way out.
                if !report.retained_acl.is_empty() {
                    message.push_str(&format!(
                        "。保留 {} 条治理边（owns_reference 悬空但仍在生效——\
                         被治理环的 objective 写保护不会因为治理者消失而自动解除）: {}。\
                         确需解除请用 action='unlink'（会向用户请示）",
                        report.retained_acl.len(),
                        report.retained_acl.join("; ")
                    ));
                }
                Ok(LoopGraphOutput {
                    message,
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
                // One audit loop per agent scope. The guard keys on a live
                // audit LOOP — a node still carrying `AUDIT_NODE_BODY`, the
                // marker this action stamps — not on "any `audits` edge
                // exists". Two failures the weaker predicate caused:
                // `delete_node` deliberately leaves edges dangling as audit
                // signals, so following this error's own advice (`drop_node`)
                // made the loop permanently un-reinstallable; and any
                // hand-wired `audits` edge (a first-class verb, e.g.
                // `cron:ratchet -[audits]-> frozen:budget`) blocked the
                // installer while naming a node that has nothing to do with
                // the audit ring.
                // Raw-column read, not `list_nodes`: this is an EXISTENCE
                // question and `list_nodes` fail-softs an unparseable row to
                // nothing, which here reads as "no audit ring" and mints a
                // second one — the outcome the guard exists to prevent.
                let live = self
                    .store
                    .node_ids_with_body(&agent_id, crate::loop_graph::AUDIT_NODE_BODY)?;
                if let Some(existing) = live.first() {
                    return Err(AlephError::tool(format!(
                        "审计环已存在（{existing}）。只改时间表用 \
                         cron_manage(action='update', id='{job}', schedule=…)，不必重装。\
                         确需重装：先 cron_manage(action='delete', id='{job}') 删掉它的 cron job，\
                         再 drop_node 它、gc 清悬空边——只 drop_node 会留下一个仍在按周跑、\
                         但 status/list/lint 都看不见的审计环。",
                        job = existing.trim_start_matches("cron:")
                    )));
                }
                let Some(cron) = &self.cron else {
                    return Err(AlephError::tool(
                        "loop_graph enable_audit: cron service unavailable",
                    ));
                };
                // Idempotency against REALITY, not only against the graph.
                // `drop_node` removes the binding but never the cron job, so
                // the graph check alone cannot see an audit ring whose row was
                // dropped — and minting a second one gives two seven-step
                // audits filing verdicts that supersede each other forever
                // (exactly what the rollback below is written to avoid). Adopt
                // the survivor instead: re-register the node and re-wire, which
                // is also what a re-`enable_audit` after a crash should do.
                //
                // Match on the NAME, not on the prompt. `CronJob::new` persists
                // the prompt verbatim at install time and nothing ever rewrites
                // it, while `AUDIT_TEMPLATE` is edited most rounds — so
                // `j.prompt == AUDIT_TEMPLATE` is false for exactly the
                // long-lived rings this scan exists to re-adopt, and true only
                // for one installed since the last template edit. A fresh
                // fixture passes; the production ring installed 2026-07-19 does
                // not, and the reinstall it lets through is the duplicate
                // seven-step audit both rollback paths here are written to
                // prevent. The prompt stays as a secondary match so a ring
                // renamed by hand is still re-adopted rather than duplicated.
                let orphan = {
                    let service = cron.lock().await;
                    match service.list_jobs().await {
                        Ok(jobs) => jobs
                            .into_iter()
                            .find(|j| j.name == AUDIT_JOB_NAME || j.prompt == AUDIT_TEMPLATE)
                            .map(|j| j.id),
                        Err(e) => {
                            // Unreadable roster: do NOT proceed to create. "I
                            // could not find out" must not become "there is
                            // none" on the one path whose duplicate is a
                            // self-perpetuating LLM loop.
                            return Err(AlephError::tool(format!(
                                "loop_graph enable_audit: 读不到 cron 名册（{e}）——\
                                 拒绝安装，否则可能装出第二个审计环。请稍后重试。"
                            )));
                        }
                    }
                };
                let expr = args
                    .cron_expr
                    .unwrap_or_else(|| AUDIT_DEFAULT_CRON_EXPR.to_string());
                crate::tasks::shared::schedule::compute_next_cron(&expr, None, chrono::Utc::now())
                    .map_err(|e| AlephError::tool(format!("Invalid cron schedule: {e}")))?;

                if let Some(job_id) = orphan {
                    let audit_node_id = format!("cron:{job_id}");
                    self.store.upsert_node(
                        &GraphNode::new(
                            &agent_id,
                            &audit_node_id,
                            NodeKind::LoopCron,
                            AUDIT_JOB_NAME,
                            origin,
                        )
                        .with_cadence("weekly")
                        .with_body(crate::loop_graph::AUDIT_NODE_BODY),
                    )?;
                    let rewired = self.wire_audit_edges(&agent_id, &audit_node_id, origin)?;
                    info!(job_id = %job_id, "orphaned audit cron re-adopted");
                    return Ok(LoopGraphOutput {
                        message: format!(
                            "发现一个已在运行、但图里没有对应节点的审计 cron（{job_id}）——\
                             已重新登记并接线 {rewired} 个节点，而不是再装一个（两个审计环会互相 \
                             supersede 裁决）。它的时间表沿用原来的；要改用 \
                             cron_manage(action='update', id='{job_id}', schedule=…)。"
                        ),
                        nodes: None,
                        edges: None,
                        rendered: None,
                    });
                }

                let mut job = CronJob::new(
                    AUDIT_JOB_NAME,
                    &agent_id,
                    AUDIT_TEMPLATE,
                    ScheduleKind::Cron {
                        expr: expr.clone(),
                        tz: None,
                        stagger_ms: None,
                    },
                );
                // Step 7 of AUDIT_TEMPLATE is "上报: 通知用户". A channel-less
                // cron has nowhere to report to.
                stamp_governance_job(&mut job, &delivery);
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
                    AUDIT_JOB_NAME,
                    origin,
                )
                .with_cadence("weekly")
                .with_body(crate::loop_graph::AUDIT_NODE_BODY);
                // The marker row is what makes this action idempotent, and it
                // can only be written after `add_job` mints the id the node is
                // named for. So the ordering cannot be reversed — roll the job
                // back instead. Without this, a failed write (locked DB, full
                // disk) left a real weekly audit cron that the guard above
                // cannot see: the operator retries, gets a SECOND audit ring,
                // and two independent seven-step audits run forever, each
                // filing verdicts about the other.
                if let Err(e) = self.store.upsert_node(&node) {
                    let service = cron.lock().await;
                    if let Err(rollback) = service.delete_job(&job_id).await {
                        warn!(job_id = %job_id, error = %rollback,
                            "loop_graph enable_audit: audit cron left orphaned after a failed \
                             graph write — delete it by hand with cron_manage");
                    }
                    return Err(e);
                }

                let wired = self.wire_audit_edges(&agent_id, &audit_node_id, origin)?;
                info!(job_id = %job_id, targets = wired, "audit loop installed");
                Ok(LoopGraphOutput {
                    message: format!(
                        "审计环已安装（cron {job_id}，{expr}），audits 接线 {wired} 个节点。\
                         后续新登记的环需手动补 audits 边或由审计环自查接线。"
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
                let mut job = CronJob::new(
                    &label,
                    &agent_id,
                    &full_prompt,
                    ScheduleKind::Cron {
                        expr: expr.clone(),
                        tz: None,
                        stagger_ms: None,
                    },
                );
                // WATCH_TEMPLATE_FOOTER: "发现便宜赢法 → 裁决写 note 并简短通知
                // 用户". Same delivery route as the audit ring.
                stamp_governance_job(&mut job, &delivery);
                let job_id = {
                    let service = cron.lock().await;
                    service.add_job(job).await.map_err(|e| {
                        AlephError::tool(format!("Failed to create watcher cron: {e}"))
                    })?
                };
                let watcher_id = format!("cron:{job_id}");
                // Roll the cron back if the graph write fails: a watcher job the
                // graph has no row for is a loop that burns LLM runs on a
                // schedule while being invisible to `list`/`status`/`lint` and
                // unreachable by `unlink` (same rule as `enable_audit`).
                let wired = self
                    .store
                    .upsert_node(
                        &GraphNode::new(&agent_id, &watcher_id, NodeKind::LoopCron, &label, origin)
                            .with_cadence(args.cadence.unwrap_or_else(|| "nightly".to_string()))
                            .with_body(truncate_with_marker(&watch_prompt, 200, "…")),
                    )
                    .and_then(|()| {
                        self.store.upsert_edge(
                            &GraphEdge::new(
                                &agent_id,
                                &watcher_id,
                                &to_id,
                                EdgeKind::Watches,
                                origin,
                            )
                            .with_note(args.note.unwrap_or_else(|| "pair 配对".to_string())),
                        )
                    });
                if let Err(e) = wired {
                    let service = cron.lock().await;
                    if let Err(rollback) = service.delete_job(&job_id).await {
                        warn!(job_id = %job_id, error = %rollback,
                            "loop_graph pair: watcher cron left orphaned after a failed graph \
                             write — delete it by hand with cron_manage");
                    }
                    // `wired` is node-then-edge, so an edge failure leaves the
                    // NODE behind — and the rollback above has just deleted the
                    // job it is named for. That residue is not inert: `status`
                    // renders `⚠ target missing（cron job 已消失）` for it and
                    // `lint_naked_loops` reports it as an unwatched optimisation
                    // loop, forever, and AUDIT_TEMPLATE's roll-call step reads
                    // exactly those two lines. A failed `pair` would then
                    // manufacture a standing audit finding about a loop that
                    // never existed. Roll back both halves or neither.
                    if let Err(rollback) = self.store.delete_node(&agent_id, &watcher_id) {
                        warn!(node = %watcher_id, error = %rollback,
                            "loop_graph pair: watcher node left behind after a failed edge \
                             write — remove it with action='drop_node'");
                    }
                    return Err(e);
                }
                info!(job_id = %job_id, target = %to_id, "watcher paired");
                // `pair` always builds a `cron:` watcher, so the watcher half
                // of the promise always holds — but the TARGET half does not.
                // Only `goal:` and `team:` have a victory-claim call site;
                // pairing onto `daemon:dreaming` or a `cron:` loop produces a
                // perfectly real watcher that this sentence used to advertise
                // as instantly reviewable, and it can never be. Same disclosure
                // `link` makes about unpokeable watchers, mirrored onto the
                // side that was missed.
                let trigger_line = if crate::loop_graph::service::target_has_victory_claim(&to_id) {
                    "被看守目标的胜利宣称（goal 完成 / team 解散）还会即时触发本看守\
                     （post-run 钩子，去抖 60s）。"
                } else {
                    "注意：本看守只按上面的时间表跑——只有 goal:/team: 目标有胜利宣称触发点，\
                     其它环没有终态时刻可挂，所以不会有即时评审。"
                };
                Ok(LoopGraphOutput {
                    message: format!(
                        "看守环已配对: {watcher_id} -[watches]-> {to_id}（{expr}）。{trigger_line}"
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

/// Provenance every cron job this tool installs must carry, in one place
/// because `enable_audit` and `pair` are the two installers and a fact set on
/// only one of them is the kind of asymmetry this layer keeps rediscovering.
///
/// Two things, both of which `cron_manage` — the twin reached from the same
/// tool surface behind the same operator gate — already does:
///
/// 1. **The delivery route**, so the ring can report (AUDIT_TEMPLATE step 7 /
///    WATCH_TEMPLATE_FOOTER).
/// 2. **`owner_user_id` / `scope_id`**, so the scheduled run executes as the
///    admin who installed it. Without them `cron::executor::build_cron_metadata`
///    finds no `ScopeAttribution` to rehydrate and the run is unscoped — which
///    in a multi-user install means the weekly audit reads the graph, the notes
///    and the sessions of nobody in particular, and its `note_manage` verdicts
///    land outside the installer's partition. `CronJob::new` defaults both to
///    `None` and nothing else on this path sets them.
///
/// `current_scope()` being `None` (single-user install, or a pre-P1 path) leaves
/// both columns `None`, which is exactly the legacy shape `from_persisted`
/// already treats as "unscoped" — no behaviour change there.
fn stamp_governance_job(job: &mut CronJob, delivery: &(Option<String>, Option<String>)) {
    job.source_channel_id = delivery.0.clone();
    job.source_conversation_id = delivery.1.clone();
    if let Some(attr) = crate::scope::current_scope() {
        job.owner_user_id = Some(attr.owner_user_id);
        job.scope_id = Some(attr.scope.render());
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
            __channel: None,
            __conversation_id: None,
        }
    }

    /// Register `id` with `kind` so an edge can be built against it.
    async fn seed_node(t: &LoopGraphTool, id: &str, kind: NodeKind) {
        let mut a = args(LoopGraphAction::Node);
        a.id = Some(id.to_string());
        a.kind = Some(kind);
        a.label = Some("x".into());
        t.call(a).await.expect("node");
    }

    /// `link` is the third face asking the immediate-review question, and for a
    /// non-`goal:` target it is the ONLY one that can: `render_session_topology`
    /// keys on `goal:{session}`, so a `daemon:` target has no prompt surface.
    ///
    /// The half that matters most is the REMEDY. When the target has no victory
    /// claim, `pair` cannot help either — it builds a `cron:` watcher whose own
    /// success message says so — and recommending it sends the model to install
    /// a real cron job that burns LLM runs for a promise nothing can keep.
    #[tokio::test]
    async fn link_onto_a_target_with_no_victory_claim_does_not_recommend_pair() {
        let (_d, t) = tool();
        seed_node(&t, "cron:dream-watch", NodeKind::LoopCron).await;
        seed_node(&t, "daemon:dreaming", NodeKind::Daemon).await;

        let mut a = args(LoopGraphAction::Link);
        a.from_id = Some("cron:dream-watch".into());
        a.to_id = Some("daemon:dreaming".into());
        a.edge = Some(EdgeKind::Watches);
        let out = t.call(a).await.expect("link");

        assert!(
            out.message.contains("胜利宣称触发点"),
            "the target half must be disclosed: {}",
            out.message
        );
        assert!(
            !out.message.contains("pair"),
            "`pair` builds a cron: watcher and cannot supply a victory claim — \
             recommending it here installs a real job for a promise nothing keeps: {}",
            out.message
        );
    }

    /// The other two arms, so the fix cannot degenerate into "always warn".
    #[tokio::test]
    async fn link_stays_silent_when_both_halves_hold_and_offers_pair_when_only_the_watcher_fails() {
        let (_d, t) = tool();
        seed_node(&t, "cron:w", NodeKind::LoopCron).await;
        seed_node(&t, "heartbeat:hb", NodeKind::LoopHeartbeat).await;
        seed_node(&t, "goal:s1", NodeKind::LoopGoal).await;

        let mut ok = args(LoopGraphAction::Link);
        ok.from_id = Some("cron:w".into());
        ok.to_id = Some("goal:s1".into());
        ok.edge = Some(EdgeKind::Watches);
        let out = t.call(ok).await.expect("link");
        assert!(
            !out.message.contains("注意"),
            "a fully working pairing must carry no caveat: {}",
            out.message
        );

        let mut unwakeable = args(LoopGraphAction::Link);
        unwakeable.from_id = Some("heartbeat:hb".into());
        unwakeable.to_id = Some("goal:s1".into());
        unwakeable.edge = Some(EdgeKind::Watches);
        let out = t.call(unwakeable).await.expect("link");
        assert!(
            out.message.contains("pair"),
            "here `pair` IS the remedy — it builds the cron: watcher that is missing: {}",
            out.message
        );
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

    /// `Pair` is the highest-usage complex action (creates a watcher cron,
    /// binds it as a node, wires a `watches` edge — and the trigger-line
    /// disclosure differs by target kind via `target_has_victory_claim`).
    /// It was the only one of the nine actions without any unit test: the
    /// regressions that bit `enable_audit` (orphan detection, idempotency vs
    /// reality, dual `audits` ring) all have analogues here, but the harness
    /// did not exercise this path so a next refactor was free to break any
    /// of them silently.
    ///
    /// The unit-test layer cannot attach a `SharedCronService` cheaply (the
    /// happy-path branch touches a real backing store), so the pin is at the
    /// EARLY-FAILURE seam that fires BEFORE any cron job is created — the
    /// cron-handle guard, symmetric with the one already pinned for
    /// `enable_audit`. A pair whose `to_id` does not exist in the graph would
    /// in a real run be the NEXT guard down ("`loop_graph pair: 被看守节点
    /// 'X' 不存在——先用 action='node' 登记它`"); in the no-cron harness the
    /// first guard fires first, and pinning both orderings separately would
    /// require a cron-injecting harness fixture better owned by integration
    /// tests than the unit layer.
    #[tokio::test]
    async fn pair_without_cron_service_fails_gracefully() {
        let (_d, t) = tool();
        let mut a = args(LoopGraphAction::Pair);
        a.to_id = Some("goal:sess-1".into());
        a.label = Some("counter".into());
        a.prompt = Some("probe".into());
        let err = t.call(a).await.unwrap_err().to_string();
        assert!(err.contains("cron service unavailable"), "{err}");
    }

    /// The installer's idempotency guard keys on a live audit **loop** — a node
    /// carrying [`crate::loop_graph::AUDIT_NODE_BODY`] — not on "some `audits`
    /// edge exists".
    ///
    /// `Audits` is a first-class verb (`types.rs`: "audit loop → any node"),
    /// and hand-wiring `cron:ratchet -[audits]-> frozen:budget` is documented
    /// and encouraged. Keying on the verb meant one such edge refused
    /// `enable_audit` forever while telling the operator to `drop_node` a node
    /// with nothing to do with the audit ring — advice that destroys their own
    /// governance edge. Both halves are asserted here: a hand-wired edge does
    /// NOT block, a real installed audit loop DOES.
    #[tokio::test]
    async fn enable_audit_is_blocked_only_by_a_real_audit_loop() {
        let (_d, t) = tool();
        for (id, kind) in [
            ("cron:ratchet", NodeKind::LoopCron),
            ("daemon:dreaming", NodeKind::Daemon),
        ] {
            let mut a = args(LoopGraphAction::Node);
            a.id = Some(id.into());
            a.kind = Some(kind);
            a.label = Some(id.into());
            t.call(a).await.unwrap();
        }
        let mut link = args(LoopGraphAction::Link);
        link.from_id = Some("cron:ratchet".into());
        link.to_id = Some("daemon:dreaming".into());
        link.edge = Some(EdgeKind::Audits);
        t.call(link).await.unwrap();

        // Hand-wired `audits` edge from an ordinary loop: must NOT block. The
        // call gets past the guard and fails only on the absent cron service
        // (this harness attaches none).
        let err = t
            .call(args(LoopGraphAction::EnableAudit))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("cron service unavailable"),
            "a hand-wired audits edge must not block the installer: {err}"
        );

        // Now stand up a node that really IS the audit loop (the marker the
        // installer stamps) — that one blocks, and names itself.
        let mut a = args(LoopGraphAction::Node);
        a.id = Some("cron:aud".into());
        a.kind = Some(NodeKind::LoopCron);
        a.label = Some("循环治理·审计环".into());
        a.body = Some(crate::loop_graph::AUDIT_NODE_BODY.to_string());
        t.call(a).await.unwrap();

        let err = t
            .call(args(LoopGraphAction::EnableAudit))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("审计环已存在"), "{err}");
        assert!(err.contains("cron:aud"), "must name the auditor: {err}");
        assert!(err.contains("gc"), "the remedy must name gc too: {err}");

        // Follow the error's own advice: `drop_node` leaves the edge dangling
        // by design, so reinstall must still be possible afterwards.
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
