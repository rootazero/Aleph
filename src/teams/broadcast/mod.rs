//! Group-chat broadcast orchestrator (telegram-style multiagent equal group chat). spec 2026-06-16.
//!
//! Pure logic lives in `targets` / `transcript` / `member_prompt` (host-testable, no IO);
//! IO orchestration (fan-out run + backflow) in `GroupChatBroadcaster` (batch B).
//! The three storm-prevention gates are all deterministic scaffolding, never participate
//! in reasoning, never enter `src/harness/` (keeps R10).

pub mod member_prompt;
pub mod targets;
pub mod transcript;

/// Maximum reply-chain depth (prevents A↔B infinite mutual @). spec §7.
pub const MAX_CHAIN_DEPTH: u32 = 6;
/// Max agents awakened concurrently in one round (prevents @all from exploding a large group). spec §7.
pub const MAX_FANOUT_WIDTH: usize = 5;
/// Max cumulative member activations across the entire reply tree triggered by a single
/// user message (storm-prevention third gate).
///
/// `depth × width` is a per-branch / per-round local constraint: worst case (every member @'s
/// up to `MAX_FANOUT_WIDTH` people every round, for `MAX_CHAIN_DEPTH` consecutive levels)
/// would accumulate ~`5^6 ≈ 15k` member runs. This gate imposes a **global** activation cap
/// on the entire fan-out tree, hard-limiting the total member executions one user message can
/// trigger, closing the "a mention-happy model explodes resources on a deep tree" path.
/// Pure deterministic scaffolding, never participates in reasoning (keeps R10).
pub const MAX_TOTAL_ACTIVATIONS: usize = 32;
/// Token budget for injected group transcript (truncated from tail to keep most recent when exceeded).
pub const TRANSCRIPT_TOKEN_BUDGET: usize = 8000;
/// Wall-clock timeout (seconds) for a single member run. Mirrors the default value
/// (600) of `DispatcherConfig::task_timeout_secs`. Previously `run_member` passed
/// `timeout_secs: None`, inheriting `ExecutionEngine::default_timeout_secs`'s 48-hour
/// fallback — one wedged provider call would pin a member run (and the fan-out branch
/// + activation slot waiting on it) for two full days with no stop button.
///
/// Note: the group-chat leader's run is subject to the same cap:
/// if the leader synchronously calls `team_delegate` inside the group chat,
/// the entire delegation must fit within this window (when killed by the cap,
/// the delegate's settle-on-drop fence immediately marks the coord task Failed
/// and releases the lock); for long delegation scenarios, tune up via
/// `[team_broadcast] member_run_timeout_secs`.
pub const MEMBER_RUN_TIMEOUT_SECS: u64 = 600;
/// Reserved handle: agents must not @-reply to the user (prevents self-loop).
/// openteams `RESERVED_USER_HANDLE`.
pub const RESERVED_USER_HANDLE: &str = "user";
/// Reserved `from_agent` for in-chat system notices posted by the broadcaster
/// (guard explanations, member-failure traces). The Panel keys its centered
/// system-chip rendering off this handle, so it must stay in one place —
/// `teams.chat.history` classifies replayed rows by comparing against it.
pub const SYSTEM_HANDLE: &str = "system";

/// Operator-tunable storm-prevention guards for group-chat broadcast (§4.5),
/// the broadcast-side parallel to [`DispatcherConfig`](crate::teams::dispatcher::DispatcherConfig)
/// (§4.4). The four guards used to be bare `const`s — changing one meant a
/// rebuild. References all expose these as config (hermes `delegation.*`,
/// openclaw `agent-limits.ts`, pi constants); this closes that asymmetry.
///
/// Defaults are read from the `MAX_*` / `TRANSCRIPT_TOKEN_BUDGET` consts above,
/// so the authoritative values live in exactly one place and an unconfigured
/// deployment is byte-identical to prior behaviour. Pure scaffolding — these
/// caps gate fan-out mechanically and never reason (keeps R10).
#[derive(Debug, Clone)]
pub struct BroadcastConfig {
    /// Max reply-chain depth (prevents A↔B infinite mutual @).
    pub max_chain_depth: u32,
    /// Max agents awakened concurrently in one round (prevents @all from exploding a large group).
    pub max_fanout_width: usize,
    /// Max cumulative member activations across the entire reply tree (storm-prevention third gate).
    pub max_total_activations: usize,
    /// Token budget for injected group transcript (truncated from tail to keep most recent).
    pub transcript_token_budget: usize,
    /// Wall-clock timeout (seconds) for a single member run. See [`MEMBER_RUN_TIMEOUT_SECS`].
    pub member_run_timeout_secs: u64,
}

impl Default for BroadcastConfig {
    fn default() -> Self {
        Self {
            max_chain_depth: MAX_CHAIN_DEPTH,
            max_fanout_width: MAX_FANOUT_WIDTH,
            max_total_activations: MAX_TOTAL_ACTIVATIONS,
            transcript_token_budget: TRANSCRIPT_TOKEN_BUDGET,
            member_run_timeout_secs: MEMBER_RUN_TIMEOUT_SECS,
        }
    }
}

use std::sync::atomic::{AtomicUsize, Ordering};

use tokio_util::sync::CancellationToken;

use crate::agents::background_tracker::{BackgroundAgentTracker, RunningRegistration, SpawnMeta};
use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStatus, CoordTaskStore};
use crate::gateway::context::GatewayContext;
use crate::gateway::event_emitter::team_fanout::{
    publish_team_event, team_event_bus, TeamFanoutEmitter,
};
use crate::gateway::event_emitter::{CollectingEventEmitter, EventEmitter};
use crate::gateway::execution_engine::{ExecutionError, RunRequest};
use crate::gateway::reply_emitter::extract_final_response;
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Arc;
use crate::teams::messages::{MessageStore, MessageType, NewMessage};
use crate::teams::types::TeamMemberKind;
use crate::teams::TeamStore;

/// Whether the chain depth has reached/exceeded its cap. `max` comes from
/// [`BroadcastConfig::max_chain_depth`].
#[must_use]
pub fn over_depth(chain_depth: u32, max: u32) -> bool {
    chain_depth >= max
}

/// Build metadata for a woken member's run.
///
/// **Critical**: must carry `platform = "webchat"`. Group chat is a real-time,
/// user-facing conversation on the Panel, not a background task. Without
/// `platform`, the harness falls back to the `Background` paradigm
/// (`run_loop` uses `metadata.get("platform")` to derive the manifest, `None` → Background),
/// and `Background` defaults to `SilentReply` capability → `ProtocolTokensLayer` teaches the
/// model to use `ALEPH_SILENT_COMPLETE` to signal "I'm not speaking". The member then
/// emits this literal token as the entire reply, leaking it into Panel bubbles and
/// the transcript. `webchat` → `WebRich` (without `SilentReply`) nips this at the source.
/// Consistent with all channel paths' `metadata["platform"] = channel.channel_type()`.
///
/// Deliberately NOT marked `UNATTENDED_KEY`, unlike cron / heartbeat / A2A /
/// goal continuations: a member run carries no channel, so its approvals resolve
/// through `OperatorApprovalRequester` to a Panel card — and the user who is
/// talking to the team in team chat is the operator watching that Panel. Marking
/// it would auto-deny a human-in-the-loop path that works.
///
/// Also carries the pinned usage mode (`teams::run_mode`). Without it the run
/// falls through to the global `[policies] mode`, and a chat-mode install
/// defers the `task`/`team` families this member's prompt tells it to call.
#[must_use]
fn member_run_metadata(
    team_id: &str,
    chain_depth: u32,
) -> std::collections::HashMap<String, String> {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("team_id".to_string(), team_id.to_string());
    metadata.insert("chain_depth".to_string(), chain_depth.to_string());
    metadata.insert("platform".to_string(), "webchat".to_string());
    crate::teams::run_mode::stamp(&mut metadata);
    metadata
}

/// Fold the user's request + the team roster into the planner objective. The
/// planner is tool-free; the "capability surface" it reasons about for a team
/// is the member roster, so we surface it here (PlannerContext has no roster
/// field). Pure / host-testable.
#[must_use]
fn build_team_planner_objective(user_request: &str, roster_label: &str) -> String {
    format!(
        "你在为一个 agent 团队制定整体战略,团队要完成下面的用户请求。\n\
         团队成员名册(agent_id (role)):{roster_label}\n\
         用户请求:{user_request}"
    )
}

/// Set-difference for the WaitingReview snapshot diff: ids present in `post`
/// but not in `pre` — i.e. tasks this member just moved into WaitingReview this
/// turn. Pure / host-testable.
#[must_use]
fn newly_waiting_review(pre: &[String], post: &[String]) -> Vec<String> {
    post.iter()
        .filter(|id| !pre.contains(id))
        .cloned()
        .collect()
}

/// Identity + kill switch for ONE user-triggered fan-out tree, cloned down the
/// recursion. `run_id` is the `teams.chat.send`-minted run_id (returned to the
/// Panel) — it names the tree's tracker node, and every member run registers
/// under it as `SpawnMeta.parent_id`. `cancel` is the tree-level poison:
/// `teams.chat.cancel` fires it (via `BackgroundAgentTracker::cancel` on the
/// tree node) so no NEW members spawn while the RPC walks
/// `running_children_of` to abort the in-flight ones through the engine's
/// per-run CancellationToken path. Pure scaffolding — no reasoning (keeps R10).
#[derive(Clone)]
struct FanoutTree {
    run_id: Arc<String>,
    cancel: CancellationToken,
}

/// Pre-registered fan-out tree handle. Constructed (and tracker-registered)
/// by [`GroupChatBroadcaster::register_fanout`] BEFORE the dispatch task is
/// spawned and before the RPC returns the `run_id` — otherwise
/// `teams.chat.cancel` can race the spawned registration and NOT_FOUND while
/// the fan-out proceeds uncancelled. Dropping this without dispatching
/// delists the tree node (harmless).
pub struct RegisteredFanout {
    tree: FanoutTree,
    _tree_reg: RunningRegistration,
}

/// Group-chat broadcast orchestrator. Stateless: each dispatch fetches team/roster/transcript
/// on the spot.
#[derive(Clone)]
pub struct GroupChatBroadcaster {
    ctx: Arc<GatewayContext>,
    team_store: Arc<dyn TeamStore>,
    msg_store: Arc<dyn MessageStore>,
    /// Strategic-planner provider for the StraTA team planner (round 2). `None`
    /// (default config off, or `[strategy].plan_team=false`) keeps the
    /// first-message team planner + leader-first hard gate dormant — group chat
    /// behaves exactly as before. Gated to `Some` at boot in agent_init.
    planner_provider: Option<Arc<dyn crate::providers::AiProvider>>,
    /// Read-only coordination-task store (strategy round 2). `Some` lets
    /// `run_member` diff this member's WaitingReview tasks across a turn and
    /// re-trigger the leader to review fresh submissions. `None` keeps group
    /// chat exactly as before (no re-trigger).
    coord_task_store: Option<Arc<dyn CoordTaskStore>>,
    /// Operator-tunable storm-prevention guards (§4.5). `Default` reproduces the
    /// historical hardcoded caps, so an unconfigured deployment is unchanged.
    config: BroadcastConfig,
}

impl GroupChatBroadcaster {
    #[must_use]
    pub fn new(
        ctx: Arc<GatewayContext>,
        team_store: Arc<dyn TeamStore>,
        msg_store: Arc<dyn MessageStore>,
        planner_provider: Option<Arc<dyn crate::providers::AiProvider>>,
        coord_task_store: Option<Arc<dyn CoordTaskStore>>,
        config: BroadcastConfig,
    ) -> Self {
        Self {
            ctx,
            team_store,
            msg_store,
            planner_provider,
            coord_task_store,
            config,
        }
    }

    /// Fire-once team planner. Returns `true` iff THIS call minted + stored a
    /// non-empty team Strategy (⇒ the leader should run first this turn). Returns
    /// `false` when the planner is disabled, the slot is already filled, or the
    /// plan self-gated to nothing. Fail-soft throughout (P7): any miss ⇒ false ⇒
    /// today's equal-broadcast. Plumbing only — the strategy CONTENT is the LLM's.
    async fn maybe_plan_team_strategy(&self, team_id: &str, content: &str) -> bool {
        let Some(provider) = self.planner_provider.clone() else {
            return false;
        };
        let Some(store) = crate::strategy::global() else {
            return false;
        };
        let norm = crate::routing::session_key::normalize_agent_id(team_id);
        let key = crate::strategy::team_key(&norm);
        // Cheap fast-path: already planned ⇒ no paid LLM call, no leader-first.
        if store.get(&key).ok().flatten().is_some() {
            return false;
        }
        let members = self
            .team_store
            .get_members(team_id)
            .await
            .unwrap_or_default();
        let roster_label = members
            .iter()
            .map(|m| format!("{} ({})", m.agent_id, m.role))
            .collect::<Vec<_>>()
            .join(", ");
        let objective = build_team_planner_objective(content, &roster_label);
        let ctx = crate::strategy::planner::PlannerContext {
            tool_descriptions: Vec::new(),
            env_summary: crate::strategy::planner::env_summary(),
            lessons: Vec::new(),
        };
        let Some(strategy) =
            crate::strategy::planner::plan_strategy(&provider, &objective, &ctx, None).await
        else {
            return false;
        };
        // Atomic guard against concurrent first messages: only the winner stores
        // (and reports leader_first); the loser's plan is discarded.
        store.put_if_absent(&key, &strategy).unwrap_or(false)
    }

    /// Entry point: triggered by a user message (leader fallback when no @).
    /// Assumes the user message has already been stored in `msg_store` by the caller.
    ///
    /// Each user trigger creates a fresh global activation counter for the fan-out tree
    /// (`MAX_TOTAL_ACTIVATIONS` gate), shared across the tree's entire recursion and
    /// discarded afterward — the cap is "per user message", not "cumulative across the
    /// process".
    ///
    /// Construct + tracker-register the fan-out tree for an RPC-minted
    /// `teams.chat.send` run_id. Synchronous — call BEFORE spawning the
    /// dispatch task and before returning the run_id, so the id the Panel
    /// receives is cancellable the instant it is visible.
    /// `teams.chat.cancel` fires the stored token via `tracker.cancel(run_id)`
    /// (poisoning new member spawns), then walks `running_children_of(run_id)`
    /// to abort in-flight member runs.
    pub fn register_fanout(tree_run_id: String, team_id: &str) -> RegisteredFanout {
        let tree = FanoutTree {
            run_id: Arc::new(tree_run_id.clone()),
            cancel: CancellationToken::new(),
        };
        let _tree_reg = RunningRegistration::register(
            BackgroundAgentTracker::global(),
            tree_run_id,
            tree.cancel.clone(),
            format!("team chat fan-out (team {team_id})"),
            SpawnMeta {
                parent_id: None,
                depth: 0,
                root_session: tree.run_id.as_ref().clone(),
                model: None,
            },
        );
        RegisteredFanout { tree, _tree_reg }
    }

    /// The `fanout` handle carries the fan-out tree's identity (the RPC-minted
    /// `teams.chat.send` run_id) plus its running-only tracker node holding
    /// the tree kill switch — pre-registered by [`Self::register_fanout`], so
    /// the run_id the Panel receives is honest and immediately cancellable.
    pub async fn dispatch_user(&self, team_id: String, content: String, fanout: RegisteredFanout) {
        // Destructure keeps the RAII registration alive until the whole tree
        // settles — no completed entry, no announce.
        let RegisteredFanout { tree, _tree_reg } = fanout;
        // Tree lifecycle → Panel. Without this the group-chat surface has no
        // honest "the team is working / the team is done" signal: member
        // `.activity` only fires per-member and a fan-out that ends with every
        // member declining to speak emits nothing at all, leaving the composer
        // stuck showing a Stop button for a tree that already settled.
        publish_team_event(
            &team_id,
            "fanout",
            serde_json::json!({ "run_id": tree.run_id.as_str(), "status": "started" }),
        );
        let leader_first = self.maybe_plan_team_strategy(&team_id, &content).await;
        let budget = Arc::new(AtomicUsize::new(0));
        self.clone()
            .dispatch(
                team_id.clone(),
                content,
                RESERVED_USER_HANDLE.to_string(),
                0,
                true,
                leader_first,
                budget,
                tree.clone(),
            )
            .await;
        // `dispatch` awaits the whole recursive tree, so reaching here means
        // every member run has settled (completed, failed, or been cancelled).
        publish_team_event(
            &team_id,
            "fanout",
            serde_json::json!({ "run_id": tree.run_id.as_str(), "status": "settled" }),
        );
    }

    /// Recursive core. `user_triggered`=false means no fallback when no @ (chain naturally stops).
    /// `leader_first`=true means hard-route to the leader (strategy round 2, activated after
    /// TaskActivity).
    /// `budget` is the shared cumulative activation counter for the entire fan-out tree
    /// (storm-prevention third gate).
    fn dispatch(
        self,
        team_id: String,
        content: String,
        sender: String,
        chain_depth: u32,
        user_triggered: bool,
        leader_first: bool,
        budget: Arc<AtomicUsize>,
        tree: FanoutTree,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async move {
            // Tree poisoned by teams.chat.cancel: spawn no further members at
            // any recursion level. In-flight member runs are aborted
            // separately by the RPC through the engine's per-run token.
            if tree.cancel.is_cancelled() {
                return;
            }
            let Some(team) = self.team_store.get_team(&team_id).await.ok().flatten() else {
                return;
            };
            let members = self
                .team_store
                .get_members(&team_id)
                .await
                .unwrap_or_default();
            let roster_ids: Vec<String> = members.iter().map(|m| m.agent_id.clone()).collect();

            let targets::ResolvedTargets {
                targets,
                suppressed,
            } = targets::resolve_targets(
                &content,
                &sender,
                &team.leader_id,
                &roster_ids,
                user_triggered,
                leader_first,
                self.config.max_fanout_width,
            );
            if over_depth(chain_depth, self.config.max_chain_depth) {
                // Post the boundary notice only when the gate actually
                // suppressed someone — a reply with no @ ends its chain
                // naturally at any depth and a "depth cap reached" message for it is
                // false-alarm noise (the activation gate's exactly-once
                // discipline, applied to the depth gate).
                if !targets.is_empty() {
                    self.post_system(&team_id, "讨论已达深度上限,等你接话。")
                        .await;
                }
                return;
            }
            if leader_first && user_triggered && content.contains('@') {
                self.post_system(&team_id, "已交由 leader 统筹:先规划任务分配,再分派给成员。")
                    .await;
            }
            // The width gate held back members that were explicitly addressed.
            // Say so once, in the same place the depth and activation gates
            // announce themselves — a silent drop reads downstream (and to the
            // user watching) as "that member ignored you".
            if !suppressed.is_empty() {
                let named = suppressed
                    .iter()
                    .map(|a| format!("@{a}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                self.post_system(
                    &team_id,
                    &format!("本轮发言人数已达上限,{named} 未被唤醒,可稍后单独 @ 他们。"),
                )
                .await;
            }
            if targets.is_empty() {
                return; // chain naturally stops
            }

            let roster_label = members
                .iter()
                .map(|m| format!("{} ({})", m.agent_id, m.role))
                .collect::<Vec<_>>()
                .join(", ");

            // Run each target agent concurrently; after each finishes, recursively backflow.
            let mut handles = Vec::new();
            for agent_id in targets {
                // ACP members (external CLI) are only executable via the dispatcher's
                // coord-task path (`runner.rs` via AcpAdapterManager); AgentRegistry
                // cannot resolve them — skip before spending a storm-budget slot,
                // otherwise `@all` would burn an activation slot on a forever-silent
                // member with zero logging.
                if members
                    .iter()
                    .any(|m| m.agent_id == agent_id && m.kind == TeamMemberKind::AcpSession)
                {
                    tracing::debug!(team_id = %team_id, agent_id = %agent_id,
                        "group-chat: ACP member is dispatcher-only (not chat-capable); skipped without consuming an activation slot");
                    continue;
                }
                // Storm-prevention third gate: hard cap on cumulative activations
                // across the whole tree. `fetch_add` atomically claims a slot;
                // out-of-bounds members are skipped; the one that exactly crosses
                // the line (and only that one) posts a system notice —
                // `claimed == MAX` is hit exactly once across all concurrent
                // branches, naturally deduplicated without spamming.
                let claimed = budget.fetch_add(1, Ordering::Relaxed);
                if claimed >= self.config.max_total_activations {
                    if claimed == self.config.max_total_activations {
                        self.post_system(&team_id, "群聊活动已达单轮上限,等你接话。")
                            .await;
                    }
                    continue;
                }
                let role = members
                    .iter()
                    .find(|m| m.agent_id == agent_id)
                    .map(|m| m.role.clone())
                    .unwrap_or_else(|| "member".to_string());
                let this = self.clone();
                let team_id_spawn = team_id.clone();
                let leader_id = team.leader_id.clone();
                let roster_label = roster_label.clone();
                let team_name = team.name.clone();
                let protocol = team.protocol.clone();
                let user_request = content.clone();
                handles.push(tokio::spawn(this.run_member(
                    team_id_spawn,
                    agent_id,
                    role,
                    leader_id,
                    roster_label,
                    team_name,
                    protocol,
                    user_request,
                    chain_depth,
                    budget.clone(),
                    tree.clone(),
                )));
            }
            for h in handles {
                // JoinError only surfaces when a member task panics or is cancelled.
                // Swallowing it would leave a silently-vanished agent in the group
                // chat with no trace; we downgrade to warn so the panic is
                // observable (no rethrow — one member crashing must not kill the
                // entire broadcast round).
                if let Err(e) = h.await {
                    tracing::warn!(team_id = %team_id, error = %e, "group-chat member task panicked");
                }
            }
        })
    }

    /// Run a single member agent, capture reply → persist transcript → parse @-mentions
    /// and recursively backflow.
    /// `budget` is passed down through the backflow recursion so the cumulative activation
    /// cap covers the entire reply tree.
    async fn run_member(
        self,
        team_id: String,
        agent_id: String,
        role: String,
        leader_id: String,
        roster_label: String,
        team_name: String,
        protocol: Option<String>,
        user_request: String,
        chain_depth: u32,
        budget: Arc<AtomicUsize>,
        tree: FanoutTree,
    ) {
        let Some(agent) = self.ctx.agent_registry().get(&agent_id).await else {
            // The dispatch loop already filters ACP-kind members, so reaching
            // here means a roster member SHOULD be resolvable — a silent
            // return made such a member vanish from the conversation with no
            // trace (while its activation slot was already spent).
            tracing::warn!(team_id = %team_id, agent_id = %agent_id,
                "group-chat: roster member not resolvable in the agent registry; skipped");
            return;
        };

        // Pull latest transcript (including the just-arrived message) and format it
        let history = self
            .msg_store
            .list_team_messages(&team_id, 200)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|m| (m.from_agent, m.content))
            .collect::<Vec<_>>();
        let transcript =
            transcript::format_transcript(&history, self.config.transcript_token_budget);

        let is_leader = agent_id == leader_id;
        let input = member_prompt::build_member_input(
            &team_id,
            &agent_id,
            &role,
            &roster_label,
            &transcript,
            is_leader,
            &team_name,
            protocol.as_deref(),
            &user_request,
        );

        // collector captures the reply; TeamFanoutEmitter simultaneously broadcasts
        // to team.<id>.* (Panel bubbles)
        let collector = Arc::new(CollectingEventEmitter::new());
        let emitter: Arc<dyn EventEmitter + Send + Sync> = match team_event_bus() {
            Some(bus) => Arc::new(TeamFanoutEmitter::new(
                bus,
                team_id.clone(),
                agent_id.clone(),
                Some(collector.clone()),
            )),
            None => collector.clone(),
        };

        // F2-retrigger: snapshot this member's WaitingReview tasks before the
        // turn so we can detect fresh submissions afterward. Skipped for the
        // leader (a self-review needs no @leader nudge) and when no coord store
        // is wired.
        let review_pre: Vec<String> = match (&self.coord_task_store, is_leader) {
            (Some(cs), false) => cs
                .list_tasks(CoordTaskFilter {
                    team_id: Some(team_id.clone()),
                    status: Some(CoordTaskStatus::WaitingReview),
                })
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|t| t.id)
                .collect(),
            _ => Vec::new(),
        };

        let run_id = uuid::Uuid::new_v4().to_string();
        // Record this member run under the fan-out tree so teams.chat.cancel
        // can enumerate it (`running_children_of(tree.run_id)`) and abort it
        // via the engine's per-run token (keyed by this run_id). Running-only
        // RAII: delists on drop (normal exit, panic, task cancel) — no
        // completed entry, no announce. The stored token is a child of the
        // tree token, so a tracker-level cancel of this entry never poisons
        // the whole tree.
        let _member_reg = RunningRegistration::register(
            BackgroundAgentTracker::global(),
            run_id.clone(),
            tree.cancel.child_token(),
            format!("team-chat member {agent_id} (team {team_id})"),
            SpawnMeta {
                parent_id: Some(tree.run_id.as_ref().clone()),
                depth: chain_depth.saturating_add(1),
                root_session: tree.run_id.as_ref().clone(),
                model: None,
            },
        );
        // Member is live NOW. `TeamFanoutEmitter` only reports "working" on the
        // first ToolStart, so a member that answers straight from the model
        // (no tool call) never showed as active at all — the roster sat Idle
        // from send to reply and the group chat looked frozen. Emitting the
        // same `activity` shape here makes presence honest for every member.
        publish_team_event(
            &team_id,
            "activity",
            serde_json::json!({ "run_id": run_id, "agent_id": agent_id, "status": "working" }),
        );
        let metadata = member_run_metadata(&team_id, chain_depth);
        let req = RunRequest {
            run_id,
            input,
            session_key: SessionKey::task(
                &agent_id,
                crate::teams::run_mode::TEAM_CHAT_TASK_TYPE,
                &team_id,
            ),
            // Bounded wall-clock timeout. `None` would inherit the engine's
            // 48h `default_timeout_secs` — one wedged provider call pinning a
            // member run (and its awaiting fan-out branch + activation slot)
            // for two days with no stop button.
            timeout_secs: Some(self.config.member_run_timeout_secs),
            metadata,
            attachments: Vec::new(),
            pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
        };

        // Last-instant tree poison check: a member spawned concurrently with
        // teams.chat.cancel may have registered after the RPC walked the
        // children; the poison flag (fired BEFORE the walk) still stops it
        // here. A residual race — poison landing between this check and the
        // engine registering the run — leaves a bounded set of stragglers
        // (up to the members concurrently inside this window, ≤ the fan-out
        // width of one level), each capped by `member_run_timeout_secs`.
        if tree.cancel.is_cancelled() {
            return;
        }
        if let Err(e) = self
            .ctx
            .execution_adapter()
            .execute(req, agent, emitter)
            .await
        {
            // A run the USER stopped is not a member failure. `teams.chat.cancel`
            // aborts every in-flight member, so treating `Cancelled` like a
            // fault wrote one permanent "@X 执行失败或超时" row per member into
            // the durable transcript — a record that every later turn (and the
            // leader's own review) then reads as teammates who could not do
            // their job. Nobody failed; the user pressed Stop. Same for
            // `AgentBusy`: that member is already speaking in this round.
            let stopped_by_user = matches!(e, ExecutionError::Cancelled);
            let already_speaking = matches!(e, ExecutionError::AgentBusy(_));
            if stopped_by_user || already_speaking {
                tracing::debug!(team_id = %team_id, agent_id = %agent_id, error = %e,
                    "group-chat member run ended without a reply (cancelled or already running)");
            } else {
                tracing::warn!(team_id = %team_id, agent_id = %agent_id, error = %e, "group-chat member run failed");
            }
            // Release the roster badge this member claimed above. An adapter-level
            // failure (spawn refused, timeout) never reaches the emitter's
            // `RunError` arm, so without this the avatar stays amber "working"
            // for the rest of the session. A stopped/busy member goes back to
            // idle rather than showing an error the user did not cause.
            let activity = if stopped_by_user || already_speaking {
                serde_json::json!({ "agent_id": agent_id, "status": "idle" })
            } else {
                serde_json::json!({ "agent_id": agent_id, "status": "error", "error": e.to_string() })
            };
            publish_team_event(&team_id, "activity", activity);
            // Failure/timeout must leave a trace in the group transcript — otherwise
            // an @'ed member vanishes silently, leaving a traceless hole in
            // subsequent rounds and Panel history (mirroring the depth/width gate
            // system notices). A cancellation deliberately leaves none: the
            // transcript is read back into every later prompt, so a durable row
            // is a durable lie.
            if !stopped_by_user && !already_speaking {
                self.post_system(
                    &team_id,
                    &format!("@{agent_id} 的发言执行失败或超时,本轮未能回复。"),
                )
                .await;
            }
            return;
        }

        // Extract and persist this member's reply FIRST — must happen before the
        // F2-retrigger: the leader's review reads the msg_store transcript; if the
        // nudge comes first, the leader reviews a record missing the submitter's
        // closing statement (self-report/warning/artifact pointer), and the review
        // verdict ends up BEFORE the triggering submission in the transcript
        // (causality reversal, permanently stored).
        let reply =
            extract_final_response(&collector.events().await).filter(|r| !r.trim().is_empty());
        if let Some(reply) = &reply {
            // Long TTL: group transcript is a durable record, not subject to inbox's
            // short TTL. A persistence failure must be visible — silent loss creates
            // a traceless hole in subsequent rounds and Panel history (P7).
            if let Err(e) = self
                .msg_store
                .send_message_with_ttl(
                    NewMessage {
                        team_id: team_id.clone(),
                        from_agent: agent_id.clone(),
                        msg_type: MessageType::Message,
                        subject: String::new(),
                        content: reply.clone(),
                        recipients: Vec::new(),
                        reply_to: None,
                        attachments: Vec::new(),
                    },
                    chrono::Duration::days(3650),
                )
                .await
            {
                tracing::warn!(team_id = %team_id, agent_id = %agent_id, error = %e,
                    "group-chat: failed to persist member reply into the team transcript");
            }
        }

        // F2-retrigger: any task this member just moved into WaitingReview ⇒
        // synthetically @-nudge the leader to review it. Goes through `dispatch`
        // directly (an inert team_messages row would never re-trigger anyone),
        // carrying the live budget + depth. Skipped for the leader (self-review).
        // Runs AFTER the reply persistence above so the leader's review sees
        // the member's closing message.
        if let (Some(cs), false) = (&self.coord_task_store, is_leader) {
            let review_post: Vec<String> = cs
                .list_tasks(CoordTaskFilter {
                    team_id: Some(team_id.clone()),
                    status: Some(CoordTaskStatus::WaitingReview),
                })
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|t| t.id)
                .collect();
            for new_id in newly_waiting_review(&review_pre, &review_post) {
                self.clone()
                    .dispatch(
                        team_id.clone(),
                        format!("@{leader_id} task `{new_id}` 已提交,待你 review(用 task_review 验收)。"),
                        agent_id.clone(),
                        chain_depth + 1,
                        false,
                        false,
                        budget.clone(),
                        tree.clone(),
                    )
                    .await;
            }
        }

        let Some(reply) = reply else {
            return;
        };

        // Backflow: parse @-mentions in the reply, recurse (agent-triggered → no
        // fallback when no @). Depth+1.
        // dispatch returns an explicit boxed future (breaking async recursion's
        // opaque type cycle).
        self.dispatch(
            team_id,
            reply,
            agent_id,
            chain_depth + 1,
            false,
            false,
            budget,
            tree,
        )
        .await;
    }

    /// Post an in-chat system notice (guard explanations, member failure
    /// traces). Durable in the transcript AND fanned out live on
    /// `team.<id>.system` — without the live leg the storm guards fire, the
    /// conversation stops dead, and the user sees nothing at all until they
    /// leave and re-enter the group (the notice was history-only).
    async fn post_system(&self, team_id: &str, text: &str) {
        publish_team_event(team_id, "system", serde_json::json!({ "text": text }));
        let _ = self
            .msg_store
            .send_message(NewMessage {
                team_id: team_id.to_string(),
                from_agent: SYSTEM_HANDLE.to_string(),
                msg_type: MessageType::SystemNotification,
                subject: String::new(),
                content: text.to_string(),
                recipients: Vec::new(),
                reply_to: None,
                attachments: Vec::new(),
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_planner_objective_includes_request_and_roster() {
        let o =
            super::build_team_planner_objective("做个市场调研", "alice (researcher), bob (writer)");
        assert!(o.contains("做个市场调研"));
        assert!(o.contains("alice (researcher), bob (writer)"));
    }

    #[test]
    fn chain_depth_guard_blocks_at_max() {
        let max = MAX_CHAIN_DEPTH;
        assert!(over_depth(max, max), "at cap = blocked");
        assert!(over_depth(max + 1, max), "over cap = blocked");
        assert!(!over_depth(max - 1, max), "under cap = admitted");
        assert!(!over_depth(0, max), "zero = admitted");
    }

    #[test]
    fn broadcast_config_default_mirrors_legacy_consts() {
        // Default config must reproduce the historical hardcoded caps exactly,
        // so an unconfigured deployment is byte-identical to prior behaviour.
        let c = BroadcastConfig::default();
        assert_eq!(c.max_chain_depth, MAX_CHAIN_DEPTH);
        assert_eq!(c.max_fanout_width, MAX_FANOUT_WIDTH);
        assert_eq!(c.max_total_activations, MAX_TOTAL_ACTIVATIONS);
        assert_eq!(c.transcript_token_budget, TRANSCRIPT_TOKEN_BUDGET);
        assert_eq!(c.member_run_timeout_secs, MEMBER_RUN_TIMEOUT_SECS);
    }

    #[test]
    fn activation_budget_admits_exactly_max_and_dedups_overflow_note() {
        // Replicate the dispatch slot-claiming logic: fetch_add claims a slot,
        // claimed < MAX is admitted, claimed == MAX hits exactly once (posts
        // system notice), claimed > MAX is silently rejected.
        let budget = Arc::new(AtomicUsize::new(0));
        let mut admitted = 0usize;
        let mut note_posts = 0usize;
        for _ in 0..(MAX_TOTAL_ACTIVATIONS + 10) {
            let claimed = budget.fetch_add(1, Ordering::Relaxed);
            if claimed >= MAX_TOTAL_ACTIVATIONS {
                if claimed == MAX_TOTAL_ACTIVATIONS {
                    note_posts += 1;
                }
                continue;
            }
            admitted += 1;
        }
        assert_eq!(
            admitted, MAX_TOTAL_ACTIVATIONS,
            "exactly MAX member activations admitted"
        );
        assert_eq!(
            note_posts, 1,
            "overflow notice fired exactly once (natural dedup, no spam)"
        );
    }

    #[test]
    fn member_metadata_tags_webchat_platform() {
        // Group-chat members must run with the webchat (→ WebRich) paradigm.
        // Missing platform falls back to Background → teaches model
        // ALEPH_SILENT_COMPLETE → leaks into bubbles/transcript.
        let m = member_run_metadata("team-x", 2);
        assert_eq!(m.get("platform").map(String::as_str), Some("webchat"));
        assert_eq!(m.get("team_id").map(String::as_str), Some("team-x"));
        assert_eq!(m.get("chain_depth").map(String::as_str), Some("2"));
    }

    /// A member run must declare its mode rather than inherit the global
    /// `[policies] mode` — chat defers the `task`/`team` families the member
    /// and leader prompts contract these agents to call.
    #[test]
    fn member_metadata_pins_the_team_run_mode() {
        let metadata = member_run_metadata("team-1", 0);
        assert_eq!(
            metadata
                .get(crate::config::types::policies::MODE_SESSION_KEY)
                .map(String::as_str),
            Some("work")
        );
    }

    #[test]
    fn newly_waiting_review_is_post_minus_pre() {
        let pre = vec!["a".to_string(), "b".to_string()];
        let post = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(
            super::newly_waiting_review(&pre, &post),
            vec!["c".to_string()]
        );
        // nothing new
        assert!(super::newly_waiting_review(&post, &post).is_empty());
        // a task leaving WaitingReview is not "new"
        assert!(super::newly_waiting_review(&post, &pre).is_empty());
    }

    #[test]
    fn webchat_paradigm_never_teaches_silent_token() {
        // Guard the invariant this fix depends on: webchat-resolved paradigm
        // must not include SilentReply, therefore ProtocolTokensLayer never
        // teaches ALEPH_SILENT_COMPLETE.
        use crate::gateway::channel::paradigm_for_channel_type;
        use crate::thinker::interaction::Capability;
        let paradigm = paradigm_for_channel_type("webchat");
        assert!(
            !paradigm
                .default_capabilities()
                .contains(&Capability::SilentReply),
            "WebRich must not include SilentReply, otherwise members would be taught ALEPH_SILENT_COMPLETE"
        );
    }
}
