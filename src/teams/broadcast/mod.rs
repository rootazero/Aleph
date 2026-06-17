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
/// 群 transcript 注入的 token 预算(超出从尾保留最近)。
pub const TRANSCRIPT_TOKEN_BUDGET: usize = 8000;
/// 保留 handle:agent 不能 @ 回用户(防自环)。openteams `RESERVED_USER_HANDLE`。
pub const RESERVED_USER_HANDLE: &str = "user";

use crate::gateway::context::GatewayContext;
use crate::gateway::event_emitter::team_fanout::{team_event_bus, TeamFanoutEmitter};
use crate::gateway::event_emitter::{CollectingEventEmitter, EventEmitter};
use crate::gateway::execution_engine::RunRequest;
use crate::gateway::reply_emitter::extract_final_response;
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Arc;
use crate::teams::messages::{MessageStore, MessageType, NewMessage};
use crate::teams::TeamStore;

/// 是否已达/超过接话深度上限。
#[must_use]
pub fn over_depth(chain_depth: u32) -> bool {
    chain_depth >= MAX_CHAIN_DEPTH
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

/// 群聊广播编排器。无状态:每次 dispatch 现场拉 team/roster/transcript。
#[derive(Clone)]
pub struct GroupChatBroadcaster {
    ctx: Arc<GatewayContext>,
    team_store: Arc<dyn TeamStore>,
    msg_store: Arc<dyn MessageStore>,
}

impl GroupChatBroadcaster {
    #[must_use]
    pub fn new(
        ctx: Arc<GatewayContext>,
        team_store: Arc<dyn TeamStore>,
        msg_store: Arc<dyn MessageStore>,
    ) -> Self {
        Self {
            ctx,
            team_store,
            msg_store,
        }
    }

    /// 入口:用户消息触发(没@时 leader 兜底)。假定 user 消息已由调用方存进 `msg_store`。
    pub async fn dispatch_user(&self, team_id: String, content: String) {
        self.clone()
            .dispatch(team_id, content, RESERVED_USER_HANDLE.to_string(), 0, true)
            .await;
    }

    /// 递归核心。`user_triggered`=false 时没@不兜底(链自然停)。
    fn dispatch(
        self,
        team_id: String,
        content: String,
        sender: String,
        chain_depth: u32,
        user_triggered: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async move {
        if over_depth(chain_depth) {
            self.post_system(&team_id, "讨论已达深度上限,等你接话。").await;
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
        );
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
            let role = members
                .iter()
                .find(|m| m.agent_id == agent_id)
                .map(|m| m.role.clone())
                .unwrap_or_else(|| "member".to_string());
            let this = self.clone();
            let team_id = team_id.clone();
            let leader_id = team.leader_id.clone();
            let roster_label = roster_label.clone();
            handles.push(tokio::spawn(this.run_member(
                team_id,
                agent_id,
                role,
                leader_id,
                roster_label,
                chain_depth,
            )));
        }
        for h in handles {
            let _ = h.await;
        }
        })
    }

    /// 跑单个成员 agent,拿回复 → 存 transcript → 解析@递归回流。
    async fn run_member(
        self,
        team_id: String,
        agent_id: String,
        role: String,
        leader_id: String,
        roster_label: String,
        chain_depth: u32,
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
        let transcript = transcript::format_transcript(&history, TRANSCRIPT_TOKEN_BUDGET);

        let is_leader = agent_id == leader_id;
        let input = member_prompt::build_member_input(
            &team_id,
            &agent_id,
            &role,
            &roster_label,
            &transcript,
            is_leader,
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
        self.dispatch(team_id, reply, agent_id, chain_depth + 1, false)
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
    fn chain_depth_guard_blocks_at_max() {
        assert!(over_depth(MAX_CHAIN_DEPTH), "到上限应阻断");
        assert!(over_depth(MAX_CHAIN_DEPTH + 1), "超上限应阻断");
        assert!(!over_depth(MAX_CHAIN_DEPTH - 1), "未到上限放行");
        assert!(!over_depth(0), "初始放行");
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
