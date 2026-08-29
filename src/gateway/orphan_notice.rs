//! Durable user-facing receipt for agent tasks orphaned by a restart.
//!
//! When the daemon is replaced mid-run (config change, hot-swap, crash), the
//! in-progress task is left marked `running`. On the next boot
//! `reconcile_orphaned_tasks` flips it to the terminal `Interrupted` state —
//! but does so silently. From the user's side their conversation simply stops
//! with no result and no error: the symptom that motivated this module.
//!
//! A live channel push at boot is unreliable — channels (Telegram, etc.) may
//! not be connected yet when reconciliation runs. Instead we append a durable
//! notice to the orphaned task's *session history*. Every per-run task records
//! its session key in `parent_session_id` (see
//! `execution_engine::persistence::persist_run_task_started`), so we can
//! reconstruct the [`SessionKey`] and write directly to the session. The user
//! sees the notice the next time they open that conversation in any channel or
//! the Panel.
//!
//! Best-effort by design: a single task that fails to parse or persist is
//! logged and skipped — it must never abort startup.

use crate::gateway::session_store::types::MessageRecord;
use crate::gateway::session_store::SessionStore;
use crate::resilience::{AgentTask, Lane};
use crate::routing::session_key::SessionKey;

/// Append an "interrupted by restart" notice to the session of each Main-lane
/// orphan. Returns the number of notices successfully written.
///
/// Subagent-lane tasks are skipped: they are internal background work with no
/// user-facing conversation to write into.
pub async fn notify_interrupted_tasks(store: &dyn SessionStore, orphans: &[AgentTask]) -> usize {
    let mut written = 0usize;
    for task in orphans {
        if task.lane != Lane::Main {
            continue;
        }
        // `from_key_string` (= `parse().or_else(from_legacy)`), not `parse`.
        // This reads a key string somebody else PERSISTED, so it must accept
        // every spelling the store ever wrote; `parse` alone rejects the legacy
        // form (`agent:x:peer:a:b`) and the only trace was a `warn!` — the
        // orphan whose conversation is oldest is the one most likely to carry
        // it, and it silently got no notice at all. `sessions.new` documents
        // this exact divergence as the bug it just fixed
        // (`db_handlers::create`).
        let Some(key) = SessionKey::from_key_string(&task.parent_session_id) else {
            tracing::warn!(
                task_id = %task.id,
                session = %task.parent_session_id,
                "orphan notice: unparseable session key, skipping"
            );
            continue;
        };

        // The session already exists (the task ran in it), but ensure it so a
        // pruned-session edge case can't turn the append into a hard error.
        if let Err(error) = store.get_or_create(&key).await {
            tracing::warn!(task_id = %task.id, %error, "orphan notice: ensure session failed");
            continue;
        }

        match store
            .append_message(&key, interruption_message(&task.task_prompt))
            .await
        {
            Ok(()) => written += 1,
            Err(error) => {
                tracing::warn!(task_id = %task.id, %error, "orphan notice: append failed")
            }
        }
    }
    written
}

/// Build the assistant-role notice record for an interrupted task.
fn interruption_message(prompt: &str) -> MessageRecord {
    let snippet: String = prompt.chars().take(80).collect();
    let content = format!(
        "⚠️ 上一个任务在执行过程中因服务重启被中断，未能完成：「{snippet}」。\n\
         请重新发送该请求，我会重新开始。"
    );
    MessageRecord {
        id: uuid::Uuid::new_v4().to_string(),
        role: "assistant".to_string(),
        content,
        timestamp: chrono::Utc::now().timestamp(),
        metadata: None,
        input_tokens: 0,
        output_tokens: 0,
        tool_call_id: None,
        tool_name: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::resilience::RiskLevel;

    fn store(temp: &tempfile::TempDir) -> SessionManager {
        let config = SessionManagerConfig {
            db_path: temp.path().join("orphan_notice_test.db"),
            ..Default::default()
        };
        SessionManager::new(config).expect("test session store")
    }

    fn main_orphan(id: &str, session_key: &str) -> AgentTask {
        let mut t = AgentTask::new(
            id,
            session_key,
            "main",
            "搜索比特币走势做投资报告",
            RiskLevel::High,
        );
        t.lane = Lane::Main;
        t
    }

    /// A Main-lane orphan gets a durable assistant notice in its session, and
    /// the count reflects exactly the tasks written.
    #[tokio::test]
    async fn writes_notice_into_orphan_session() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let key = SessionKey::Main {
            agent_id: "main".into(),
            main_key: "main".into(),
            epoch: 22,
        };
        store.get_or_create(&key).await.unwrap();

        let orphan = main_orphan("run-1", &key.to_key_string());
        let written = notify_interrupted_tasks(&store, std::slice::from_ref(&orphan)).await;
        assert_eq!(written, 1);

        let history = store.get_history(&key, None).await.unwrap();
        let last = history.last().expect("a message was appended");
        assert_eq!(last.role, "assistant");
        assert!(last.content.contains("因服务重启被中断"));
        assert!(last.content.contains("比特币")); // prompt snippet echoed
    }

    /// Subagent-lane orphans have no user conversation — they are skipped.
    #[tokio::test]
    async fn skips_subagent_lane_orphans() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let key = SessionKey::Main {
            agent_id: "main".into(),
            main_key: "main".into(),
            epoch: 1,
        };
        store.get_or_create(&key).await.unwrap();

        let mut orphan = main_orphan("run-2", &key.to_key_string());
        orphan.lane = Lane::Subagent;

        let written = notify_interrupted_tasks(&store, std::slice::from_ref(&orphan)).await;
        assert_eq!(written, 0);
        // No assistant notice appended.
        let history = store.get_history(&key, None).await.unwrap();
        assert!(history.iter().all(|m| m.role != "assistant"));
    }

    /// An unparseable session key is skipped without panicking or aborting.
    #[tokio::test]
    async fn skips_unparseable_session_key() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let orphan = main_orphan("run-3", "not-a-valid-session-key");
        let written = notify_interrupted_tasks(&store, std::slice::from_ref(&orphan)).await;
        assert_eq!(written, 0);
    }

    /// A LEGACY-spelled `parent_session_id` must still get its notice.
    ///
    /// This reads a string the store persisted, so it has to accept every
    /// spelling the store ever wrote. `SessionKey::parse` alone rejects the
    /// legacy form, and the only trace was a `warn!` — so the conversations
    /// oldest enough to carry it were exactly the ones that went silent.
    /// Derived from the parser pair rather than from a hand-written key: the
    /// fixture asserts the string is legacy-only before relying on it.
    #[tokio::test]
    async fn a_legacy_spelled_session_key_still_gets_its_notice() {
        let legacy = "agent:legacyorphan:peer:telegram:99";
        assert!(
            SessionKey::parse(legacy).is_none() && SessionKey::from_key_string(legacy).is_some(),
            "fixture no longer exercises the legacy-only branch"
        );

        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let key = SessionKey::from_key_string(legacy).unwrap();
        store.get_or_create(&key).await.unwrap();

        let orphan = main_orphan("run-4", legacy);
        let written = notify_interrupted_tasks(&store, std::slice::from_ref(&orphan)).await;
        assert_eq!(
            written, 1,
            "a legacy-spelled key got no interruption notice at all"
        );
    }
}
