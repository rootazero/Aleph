//! 群聊广播编排器(telegram 式 multiagent 平等群聊)。spec 2026-06-16。
//!
//! 纯逻辑在 `targets` / `transcript` / `member_prompt`(host 可测,无 IO);
//! IO 编排(fan-out 跑 run + 回流)在 `GroupChatBroadcaster`(批次 B)。
//! 防风暴三道闸全是确定性脚手架,不参与推理、不进 `src/harness/`(守 R10)。

pub mod member_prompt;
pub mod targets;
pub mod transcript;

/// 接话链最大深度(防 A↔B 无限互@)。spec §7。
pub const MAX_CHAIN_DEPTH: u32 = 6;
/// 单轮最多同时唤醒的 agent 数(防 @all 在大群一次炸开)。spec §7。
pub const MAX_FANOUT_WIDTH: usize = 5;
/// 单次用户触发的整棵接话树最多累计唤醒的成员次数(防风暴第三闸)。
///
/// `depth × width` 是 per-branch / per-round 的局部约束:最坏情形(每个成员每轮
/// 都 @ 满 `MAX_FANOUT_WIDTH` 个人、连续 `MAX_CHAIN_DEPTH` 层)累计可达
/// `5^6 ≈ 1.5 万` 次成员 run。本闸为整棵 fan-out 树设一个**全局**唤醒上限,
/// 把单条用户消息能引发的总成员执行数硬封顶,堵住"爱 @人 的模型在深树上炸开
/// 资源"这条路。纯确定性脚手架,不参与推理(守 R10)。
pub const MAX_TOTAL_ACTIVATIONS: usize = 32;
/// 群 transcript 注入的 token 预算(超出从尾保留最近)。
pub const TRANSCRIPT_TOKEN_BUDGET: usize = 8000;
/// 保留 handle:agent 不能 @ 回用户(防自环)。openteams `RESERVED_USER_HANDLE`。
pub const RESERVED_USER_HANDLE: &str = "user";

/// Operator-tunable storm-prevention guards for group-chat broadcast (§4.5),
/// the broadcast-side parallel to [`DispatcherConfig`](crate::teams::dispatcher::DispatcherConfig)
/// (§4.4). The four guards used to be bare `const`s — changing one meant a
/// rebuild. References all expose these as config (hermes `delegation.*`,
/// openclaw `agent-limits.ts`, pi constants); this closes that asymmetry.
///
/// Defaults are read from the `MAX_*` / `TRANSCRIPT_TOKEN_BUDGET` consts above,
/// so the authoritative values live in exactly one place and an unconfigured
/// deployment is byte-identical to prior behaviour. Pure scaffolding — these
/// caps gate fan-out mechanically and never reason (守 R10).
#[derive(Debug, Clone)]
pub struct BroadcastConfig {
    /// 接话链最大深度(防 A↔B 无限互@)。
    pub max_chain_depth: u32,
    /// 单轮最多同时唤醒的 agent 数(防 @all 在大群一次炸开)。
    pub max_fanout_width: usize,
    /// 单次用户触发的整棵接话树最多累计唤醒的成员次数(防风暴第三闸)。
    pub max_total_activations: usize,
    /// 群 transcript 注入的 token 预算(超出从尾保留最近)。
    pub transcript_token_budget: usize,
}

impl Default for BroadcastConfig {
    fn default() -> Self {
        Self {
            max_chain_depth: MAX_CHAIN_DEPTH,
            max_fanout_width: MAX_FANOUT_WIDTH,
            max_total_activations: MAX_TOTAL_ACTIVATIONS,
            transcript_token_budget: TRANSCRIPT_TOKEN_BUDGET,
        }
    }
}

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStatus, CoordTaskStore};
use crate::gateway::context::GatewayContext;
use crate::gateway::event_emitter::team_fanout::{team_event_bus, TeamFanoutEmitter};
use crate::gateway::event_emitter::{CollectingEventEmitter, EventEmitter};
use crate::gateway::execution_engine::RunRequest;
use crate::gateway::reply_emitter::extract_final_response;
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Arc;
use crate::teams::messages::{MessageStore, MessageType, NewMessage};
use crate::teams::TeamStore;

/// 是否已达/超过接话深度上限。`max` 由 [`BroadcastConfig::max_chain_depth`] 提供。
#[must_use]
pub fn over_depth(chain_depth: u32, max: u32) -> bool {
    chain_depth >= max
}

/// 组装被唤醒成员 run 的 metadata。
///
/// **关键**:必须带 `platform = "webchat"`。群聊是 Panel 上实时、面向用户的对话,
/// 不是后台任务。少了 platform,harness 会回退到 `Background` paradigm
/// (`run_loop` 用 `metadata.get("platform")` 推导 manifest,None → Background),
/// 而 `Background` 默认带 `SilentReply` capability → `ProtocolTokensLayer` 教模型用
/// `ALEPH_SILENT_COMPLETE` 表示"不发言"。成员于是把这个字面 token 当整条回复发出,
/// 泄漏进 Panel 气泡和 transcript。`webchat` → `WebRich`(不含 `SilentReply`)从
/// 源头杜绝。与所有 channel 路径 `metadata["platform"] = channel.channel_type()` 一致。
#[must_use]
fn member_run_metadata(
    team_id: &str,
    chain_depth: u32,
) -> std::collections::HashMap<String, String> {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("team_id".to_string(), team_id.to_string());
    metadata.insert("chain_depth".to_string(), chain_depth.to_string());
    metadata.insert("platform".to_string(), "webchat".to_string());
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

/// 群聊广播编排器。无状态:每次 dispatch 现场拉 team/roster/transcript。
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

    /// 入口:用户消息触发(没@时 leader 兜底)。假定 user 消息已由调用方存进 `msg_store`。
    ///
    /// 每次用户触发新建一个 fan-out 树的全局唤醒计数器(`MAX_TOTAL_ACTIVATIONS` 闸),
    /// 计数器随这棵树的整条递归共享、用完即弃,确保上限是"每条用户消息"而非"进程累计"。
    pub async fn dispatch_user(&self, team_id: String, content: String) {
        let leader_first = self.maybe_plan_team_strategy(&team_id, &content).await;
        let budget = Arc::new(AtomicUsize::new(0));
        self.clone()
            .dispatch(
                team_id,
                content,
                RESERVED_USER_HANDLE.to_string(),
                0,
                true,
                leader_first,
                budget,
            )
            .await;
    }

    /// 递归核心。`user_triggered`=false 时没@不兜底(链自然停)。
    /// `leader_first`=true 时硬路由到 leader(strategy 轮次2,TaskActivity 之后激活)。
    /// `budget` 是整棵 fan-out 树共享的累计唤醒计数器(防风暴第三闸)。
    fn dispatch(
        self,
        team_id: String,
        content: String,
        sender: String,
        chain_depth: u32,
        user_triggered: bool,
        leader_first: bool,
        budget: Arc<AtomicUsize>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async move {
            if over_depth(chain_depth, self.config.max_chain_depth) {
                self.post_system(&team_id, "讨论已达深度上限,等你接话。")
                    .await;
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

            let targets = targets::resolve_targets(
                &content,
                &sender,
                &team.leader_id,
                &roster_ids,
                user_triggered,
                leader_first,
                self.config.max_fanout_width,
            );
            if leader_first && user_triggered && content.contains('@') {
                self.post_system(&team_id, "已交由 leader 统筹:先规划任务分配,再分派给成员。")
                    .await;
            }
            if targets.is_empty() {
                return; // 链自然停
            }

            let roster_label = members
                .iter()
                .map(|m| format!("{} ({})", m.agent_id, m.role))
                .collect::<Vec<_>>()
                .join(", ");

            // 并发跑本轮每个目标 agent;各自完成后递归回流。
            let mut handles = Vec::new();
            for agent_id in targets {
                // 防风暴第三闸:整棵树累计唤醒封顶。`fetch_add` 原子领取一个槽位,
                // 越界即跳过本成员;恰好跨越上限的那一次(且仅那一次)发一句系统提示
                // —— `claimed == MAX` 在所有并发分支里只会被命中一次,天然去重不刷屏。
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
                )));
            }
            for h in handles {
                // JoinError 只在成员任务 panic 或被取消时出现。吞掉会让群聊里"某个
                // agent 静默消失"无迹可循,这里降级为 warn 让 panic 可观测(不上抛,
                // 一个成员崩溃不该拖垮整轮广播)。
                if let Err(e) = h.await {
                    tracing::warn!(team_id = %team_id, error = %e, "group-chat member task panicked");
                }
            }
        })
    }

    /// 跑单个成员 agent,拿回复 → 存 transcript → 解析@递归回流。
    /// `budget` 继续随回流递归向下传,让累计唤醒上限覆盖整棵接话树。
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
    ) {
        let Some(agent) = self.ctx.agent_registry().get(&agent_id).await else {
            return;
        };

        // 拉最新 transcript(含刚到的消息)并格式化
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

        // collector 收集回复;TeamFanoutEmitter 同时广播到 team.<id>.*(Panel 气泡)
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
                    owner: Some(agent_id.clone()),
                })
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|t| t.id)
                .collect(),
            _ => Vec::new(),
        };

        let run_id = uuid::Uuid::new_v4().to_string();
        let metadata = member_run_metadata(&team_id, chain_depth);
        let req = RunRequest {
            run_id,
            input,
            session_key: SessionKey::task(&agent_id, "team_chat", &team_id),
            timeout_secs: None,
            metadata,
            attachments: Vec::new(),
            pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
        };

        if let Err(e) = self
            .ctx
            .execution_adapter()
            .execute(req, agent, emitter)
            .await
        {
            tracing::warn!(team_id = %team_id, agent_id = %agent_id, error = %e, "group-chat member run failed");
            return;
        }

        // F2-retrigger: any task this member just moved into WaitingReview ⇒
        // synthetically @-nudge the leader to review it. Goes through `dispatch`
        // directly (an inert team_messages row would never re-trigger anyone),
        // carrying the live budget + depth. Skipped for the leader (self-review).
        if let (Some(cs), false) = (&self.coord_task_store, is_leader) {
            let review_post: Vec<String> = cs
                .list_tasks(CoordTaskFilter {
                    team_id: Some(team_id.clone()),
                    status: Some(CoordTaskStatus::WaitingReview),
                    owner: Some(agent_id.clone()),
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
                    )
                    .await;
            }
        }

        let Some(reply) = extract_final_response(&collector.events().await) else {
            return;
        };
        if reply.trim().is_empty() {
            return;
        }

        // 存回复进 transcript(广播气泡已由 emitter 发出;这里持久化 + 给下一轮注入)。
        // 长 TTL:群 transcript 是持久记录,不走 inbox 的短 TTL。
        let _ = self
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
            .await;

        // 回流:解析回复里的@,递归(agent 触发→没@不兜底)。深度+1。
        // dispatch 返回显式 boxed future(打破 async 递归的 opaque 类型循环)。
        self.dispatch(
            team_id,
            reply,
            agent_id,
            chain_depth + 1,
            false,
            false,
            budget,
        )
        .await;
    }

    async fn post_system(&self, team_id: &str, text: &str) {
        let _ = self
            .msg_store
            .send_message(NewMessage {
                team_id: team_id.to_string(),
                from_agent: "system".to_string(),
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
        assert!(over_depth(max, max), "到上限应阻断");
        assert!(over_depth(max + 1, max), "超上限应阻断");
        assert!(!over_depth(max - 1, max), "未到上限放行");
        assert!(!over_depth(0, max), "初始放行");
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
    }

    #[test]
    fn activation_budget_admits_exactly_max_and_dedups_overflow_note() {
        // 复刻 dispatch 里的领取逻辑:fetch_add 领槽,claimed < MAX 放行,
        // claimed == MAX 恰好一次(发系统提示),claimed > MAX 静默拒。
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
        assert_eq!(admitted, MAX_TOTAL_ACTIVATIONS, "恰好放行 MAX 次成员唤醒");
        assert_eq!(note_posts, 1, "越界提示只发一次(天然去重,不刷屏)");
    }

    #[test]
    fn member_metadata_tags_webchat_platform() {
        // 群聊成员必须以 webchat(→ WebRich)paradigm 运行。少了 platform
        // 会回退 Background → 教模型 ALEPH_SILENT_COMPLETE → 泄漏进气泡/transcript。
        let m = member_run_metadata("team-x", 2);
        assert_eq!(m.get("platform").map(String::as_str), Some("webchat"));
        assert_eq!(m.get("team_id").map(String::as_str), Some("team-x"));
        assert_eq!(m.get("chain_depth").map(String::as_str), Some("2"));
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
        // 守住修复依赖的不变量:webchat 解析出的 paradigm 不含 SilentReply,
        // 因此 ProtocolTokensLayer 永不教 ALEPH_SILENT_COMPLETE。
        use crate::gateway::channel::paradigm_for_channel_type;
        use crate::thinker::interaction::Capability;
        let paradigm = paradigm_for_channel_type("webchat");
        assert!(
            !paradigm
                .default_capabilities()
                .contains(&Capability::SilentReply),
            "WebRich 不得含 SilentReply,否则成员会被教 ALEPH_SILENT_COMPLETE"
        );
    }
}
