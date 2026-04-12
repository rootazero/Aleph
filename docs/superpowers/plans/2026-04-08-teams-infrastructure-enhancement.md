# Teams 基础设施增强 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add four infrastructure capabilities to the teams module: broadcast messaging, member removal, message peek, and task locking.

**Architecture:** All changes are additive — new methods on existing traits, new parameters on existing tools, one new tool file. No architectural changes. Each feature is independent and can be tested in isolation.

**Tech Stack:** Rust, async-trait, SQLite (rusqlite), serde, schemars, chrono, tokio

**Spec:** `docs/superpowers/specs/2026-04-08-teams-infrastructure-enhancement-design.md`

---

### Task 1: Broadcast messaging

**Files:**
- Modify: `src/builtin_tools/team/message_send.rs`

- [ ] **Step 1: Read the current file**

Read `src/builtin_tools/team/message_send.rs` to understand current structure.

- [ ] **Step 2: Add TeamStore dependency to MessageSendTool**

The tool needs TeamStore to look up team members for broadcast. Update the struct and constructor:

```rust
use crate::teams::TeamStore;

#[derive(Clone)]
pub struct MessageSendTool {
    router: Arc<MessageRouter>,
    team_store: Arc<dyn TeamStore>,
    current_agent_id: String,
}

impl MessageSendTool {
    pub fn new(
        router: Arc<MessageRouter>,
        team_store: Arc<dyn TeamStore>,
        current_agent_id: String,
    ) -> Self {
        Self {
            router,
            team_store,
            current_agent_id,
        }
    }
}
```

- [ ] **Step 3: Add broadcast field to MessageSendArgs**

After the `cc` field (line 31), add:

```rust
    /// If true, send to all team members (excluding sender).
    /// When broadcast is true, `to` is ignored.
    #[serde(default)]
    pub broadcast: bool,
```

- [ ] **Step 4: Update the call() method to handle broadcast**

Replace the validation and send logic in `call()`:

```rust
    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        debug!(
            team_id = %args.team_id,
            to = ?args.to,
            cc = ?args.cc,
            broadcast = args.broadcast,
            msg_type = %args.msg_type.as_str(),
            subject = %args.subject,
            from = %self.current_agent_id,
            "message_send: sending message"
        );

        // Resolve recipients: broadcast overrides explicit to list
        let to = if args.broadcast {
            let members = self
                .team_store
                .get_members(&args.team_id)
                .await
                .map_err(|e| AlephError::other(format!("Failed to get team members: {e}")))?;
            members
                .into_iter()
                .map(|m| m.agent_id)
                .filter(|id| id != &self.current_agent_id)
                .collect::<Vec<_>>()
        } else {
            args.to
        };

        if to.is_empty() && args.cc.is_empty() {
            return Err(AlephError::tool(
                "message_send: at least one recipient (to or cc) is required",
            ));
        }

        let msg = self
            .router
            .send(SendRequest {
                team_id: args.team_id,
                from_agent: self.current_agent_id.clone(),
                to,
                cc: args.cc,
                msg_type: args.msg_type,
                subject: args.subject.clone(),
                content: args.content,
                reply_to: args.reply_to,
                attachments: args.attachments,
            })
            .await
            .map_err(|e| AlephError::other(format!("Failed to send message: {e}")))?;

        Ok(MessageSendOutput {
            message_id: msg.id,
            thread_id: msg.thread_id,
            message: format!("Message '{}' sent successfully", args.subject),
        })
    }
```

- [ ] **Step 5: Update MessageSendTool construction in builder.rs**

In `src/executor/builtin_registry/builder.rs`, find where `MessageSendTool::new` is called and add the `team_store` argument. Search for `MessageSendTool::new` to find the exact location. The team_store should already be available in the builder config.

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: Compiles cleanly.

- [ ] **Step 7: Commit**

```bash
git add src/builtin_tools/team/message_send.rs src/executor/builtin_registry/builder.rs
git commit -m "teams: add broadcast support to message_send tool"
```

---

### Task 2: Member removal

**Files:**
- Modify: `src/teams/store.rs`
- Create: `src/builtin_tools/team/member_remove.rs`
- Modify: `src/builtin_tools/team/mod.rs`
- Modify: `src/executor/builtin_registry/builder.rs`
- Modify: `src/executor/builtin_registry/registry.rs`
- Modify: `src/executor/builtin_registry/definitions.rs`

- [ ] **Step 1: Add remove_member to TeamStore trait**

In `src/teams/store.rs`, add to the `TeamStore` trait (after `get_members`):

```rust
    /// Remove a member from a team. Cannot remove the leader.
    async fn remove_member(&self, team_id: &str, agent_id: &str) -> crate::error::Result<()>;
```

- [ ] **Step 2: Implement remove_member in SqliteTeamStore**

Add inside `impl TeamStore for SqliteTeamStore`, after the `get_members` method:

```rust
    async fn remove_member(&self, team_id: &str, agent_id: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;

        // Check team exists and is active
        let team: Option<Team> = conn
            .prepare_cached(
                "SELECT id, name, description, leader_id, status, created_at, disbanded_at FROM teams WHERE id = ?1",
            )
            .map_err(db_err)?
            .query_row(params![team_id], read_team_row)
            .optional()
            .map_err(db_err)?;

        let team = team.ok_or_else(|| db_err(format!("team not found: {team_id}")))?;

        if team.status == TeamStatus::Disbanded {
            return Err(db_err(format!(
                "cannot remove member from disbanded team: {team_id}"
            )));
        }

        if team.leader_id == agent_id {
            return Err(db_err("cannot remove the team leader"));
        }

        let affected = conn
            .execute(
                "DELETE FROM team_members WHERE team_id = ?1 AND agent_id = ?2",
                params![team_id, agent_id],
            )
            .map_err(db_err)?;

        if affected == 0 {
            return Err(db_err(format!(
                "agent '{agent_id}' is not a member of team '{team_id}'"
            )));
        }

        Ok(())
    }
```

- [ ] **Step 3: Add test for remove_member**

Add to the `#[cfg(test)] mod tests` in `src/teams/store.rs`:

```rust
    #[tokio::test]
    async fn test_remove_member() {
        let store = setup_store().await;

        let team = store
            .create_team(NewTeam {
                name: "Remove Test".into(),
                description: "".into(),
                leader_id: "leader-1".into(),
            })
            .await
            .unwrap();

        store
            .add_member(NewTeamMember {
                team_id: team.id.clone(),
                agent_id: "worker-1".into(),
                role: "worker".into(),
            })
            .await
            .unwrap();

        // Can remove a regular member
        store.remove_member(&team.id, "worker-1").await.unwrap();
        let members = store.get_members(&team.id).await.unwrap();
        assert!(members.iter().all(|m| m.agent_id != "worker-1"));

        // Cannot remove the leader
        let err = store.remove_member(&team.id, "leader-1").await;
        assert!(err.is_err());

        // Cannot remove non-existent member
        let err = store.remove_member(&team.id, "nobody").await;
        assert!(err.is_err());
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib teams::store -- --nocapture`
Expected: All tests pass including the new one.

- [ ] **Step 5: Create member_remove.rs tool**

Create `src/builtin_tools/team/member_remove.rs`:

```rust
//! TeamMemberRemoveTool — remove a member from a team.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::TeamStore;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamMemberRemoveArgs {
    /// ID of the team
    pub team_id: String,
    /// ID of the agent to remove
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TeamMemberRemoveOutput {
    pub message: String,
}

#[derive(Clone)]
pub struct TeamMemberRemoveTool {
    store: Arc<dyn TeamStore>,
    current_agent_id: String,
}

impl TeamMemberRemoveTool {
    pub fn new(store: Arc<dyn TeamStore>, current_agent_id: String) -> Self {
        Self {
            store,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for TeamMemberRemoveTool {
    const NAME: &'static str = "team_member_remove";
    const DESCRIPTION: &'static str =
        "Remove a member from a team. Only the team leader can remove members. \
         The leader cannot be removed.";

    type Args = TeamMemberRemoveArgs;
    type Output = TeamMemberRemoveOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "team_member_remove(team_id='abc123', agent_id='worker-1')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Verify caller is the leader
        let team = self
            .store
            .get_team(&args.team_id)
            .await?
            .ok_or_else(|| AlephError::other(format!("Team '{}' not found", args.team_id)))?;

        if team.leader_id != self.current_agent_id {
            return Err(AlephError::tool(
                "team_member_remove: only the team leader can remove members",
            ));
        }

        self.store
            .remove_member(&args.team_id, &args.agent_id)
            .await
            .map_err(|e| AlephError::other(format!("Failed to remove member: {e}")))?;

        info!(
            team_id = %args.team_id,
            agent_id = %args.agent_id,
            "team_member_remove: member removed"
        );

        Ok(TeamMemberRemoveOutput {
            message: format!(
                "Agent '{}' removed from team '{}'",
                args.agent_id, args.team_id
            ),
        })
    }
}
```

- [ ] **Step 6: Register in mod.rs**

In `src/builtin_tools/team/mod.rs`, add:

```rust
mod member_remove;
```

And in the pub use section:

```rust
pub use member_remove::{TeamMemberRemoveArgs, TeamMemberRemoveOutput, TeamMemberRemoveTool};
```

- [ ] **Step 7: Register in builder.rs, registry.rs, definitions.rs**

Follow the same pattern as other team tools:
1. Add `member_remove_tool: Option<TeamMemberRemoveTool>` field to the registry struct
2. Construct and register it in builder.rs where team_store is available
3. Add a `BuiltinToolDefinition` entry and dispatch arm in definitions.rs
4. Add `"team_member_remove"` to the team tools group in groups.rs

- [ ] **Step 8: Verify compilation and commit**

Run: `cargo check -p alephcore 2>&1 | head -20`

```bash
git add -A
git commit -m "teams: add remove_member to TeamStore + team_member_remove tool"
```

---

### Task 3: Message peek

**Files:**
- Modify: `src/teams/messages/inbox.rs`
- Modify: `src/builtin_tools/team/inbox_read.rs`

- [ ] **Step 1: Add PeekCount struct to inbox.rs**

In `src/teams/messages/inbox.rs`, after the `use` imports, add:

```rust
use serde::Serialize;

/// Unread message counts by recipient role.
#[derive(Debug, Clone, Serialize)]
pub struct PeekCount {
    /// Messages where the agent is a To recipient.
    pub to: u64,
    /// Messages where the agent is a Cc recipient.
    pub cc: u64,
}
```

- [ ] **Step 2: Add peek() and peek_count() methods to Inbox**

In the `impl Inbox` block, after the `get_unread_counts` method:

```rust
    /// Non-destructive read — returns unread messages without marking as read.
    pub async fn peek(
        &self,
        agent_id: &str,
        team_id: &str,
        msg_type: Option<&MessageType>,
    ) -> Result<Vec<TeamMessage>> {
        // Same as read() but with mark_read=false and no event logging
        self.msg_store
            .read_inbox(agent_id, team_id, msg_type)
            .await
    }

    /// Returns unread message counts without reading content.
    pub async fn peek_count(
        &self,
        agent_id: &str,
        team_id: &str,
    ) -> Result<PeekCount> {
        let (to, cc) = self
            .msg_store
            .get_unread_counts(agent_id, team_id)
            .await?;
        Ok(PeekCount {
            to: to as u64,
            cc: cc as u64,
        })
    }
```

- [ ] **Step 3: Add tests for peek**

In the `#[cfg(test)] mod tests` block of inbox.rs:

```rust
    #[tokio::test]
    async fn test_peek_does_not_mark_read() {
        let (inbox, msg_store, event_store) = make_inbox().await;

        msg_store
            .send_message(sample_msg("team-1", "agent-a", "agent-b"))
            .await
            .unwrap();

        // Peek should return message
        let msgs = inbox.peek("agent-b", "team-1", None).await.unwrap();
        assert_eq!(msgs.len(), 1);

        // Peek again — message still unread
        let msgs2 = inbox.peek("agent-b", "team-1", None).await.unwrap();
        assert_eq!(msgs2.len(), 1);

        // No events logged
        let events = event_store.get_events("team-1", None, None).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_peek_count() {
        let (inbox, msg_store, _) = make_inbox().await;

        msg_store
            .send_message(sample_msg("team-1", "agent-a", "agent-b"))
            .await
            .unwrap();

        let count = inbox.peek_count("agent-b", "team-1").await.unwrap();
        assert_eq!(count.to, 1);
        assert_eq!(count.cc, 0);

        // Unknown agent has zero counts
        let empty = inbox.peek_count("nobody", "team-1").await.unwrap();
        assert_eq!(empty.to, 0);
        assert_eq!(empty.cc, 0);
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib teams::messages::inbox -- --nocapture`
Expected: All tests pass.

- [ ] **Step 5: Update InboxReadArgs with peek/count_only parameters**

In `src/builtin_tools/team/inbox_read.rs`, add to `InboxReadArgs` after `mark_read`:

```rust
    /// If true, peek at messages without marking them as read (overrides mark_read).
    #[serde(default)]
    pub peek: bool,

    /// If true, only return unread count (no message content). Fastest option.
    #[serde(default)]
    pub count_only: bool,
```

- [ ] **Step 6: Update InboxReadOutput to support count_only mode**

Replace `InboxReadOutput`:

```rust
/// Output from inbox_read.
#[derive(Debug, Clone, Serialize)]
pub struct InboxReadOutput {
    /// Messages returned (empty when count_only=true).
    pub messages: Vec<TeamMessage>,
    /// Number of messages returned or total unread count.
    pub count: usize,
    /// Unread counts by recipient role (only set when count_only=true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread: Option<crate::teams::messages::inbox::PeekCount>,
}
```

- [ ] **Step 7: Update call() method routing logic**

Replace the message reading logic in `call()`:

```rust
    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        debug!(
            team_id = %args.team_id,
            mode = %args.mode,
            peek = args.peek,
            count_only = args.count_only,
            agent_id = %self.current_agent_id,
            "inbox_read: reading messages"
        );

        // Count-only mode: fastest, returns only counts
        if args.count_only {
            let peek_count = self
                .inbox
                .peek_count(&self.current_agent_id, &args.team_id)
                .await?;
            let total = (peek_count.to + peek_count.cc) as usize;
            return Ok(InboxReadOutput {
                messages: vec![],
                count: total,
                unread: Some(peek_count),
            });
        }

        let messages = match args.mode.as_str() {
            "thread" => {
                let thread_id = args.thread_id.as_deref().ok_or_else(|| {
                    AlephError::tool("inbox_read: thread_id is required when mode='thread'")
                })?;
                self.inbox.read_thread(thread_id).await?
            }
            _ => {
                if args.peek {
                    self.inbox
                        .peek(
                            &self.current_agent_id,
                            &args.team_id,
                            args.msg_type.as_ref(),
                        )
                        .await?
                } else {
                    self.inbox
                        .read(
                            &self.current_agent_id,
                            &args.team_id,
                            args.msg_type.as_ref(),
                            args.mark_read,
                        )
                        .await?
                }
            }
        };

        let count = messages.len();
        Ok(InboxReadOutput {
            messages,
            count,
            unread: None,
        })
    }
```

- [ ] **Step 8: Verify and commit**

Run: `cargo check -p alephcore 2>&1 | head -20`

```bash
git add src/teams/messages/inbox.rs src/builtin_tools/team/inbox_read.rs
git commit -m "teams: add peek/peek_count to Inbox + peek/count_only params to inbox_read tool"
```

---

### Task 4: Task locking

**Files:**
- Modify: `src/agents/swarm/tasks/mod.rs`
- Modify: `src/agents/swarm/tasks/store.rs`
- Modify: `src/builtin_tools/team/delegate.rs`
- Modify: `src/builtin_tools/team/task_submit.rs`

- [ ] **Step 1: Add lock fields to CoordTask struct**

In `src/agents/swarm/tasks/mod.rs`, add two fields to `CoordTask` after `completed_at`:

```rust
    /// Agent currently holding the lock (None = unlocked).
    pub locked_by: Option<String>,
    /// When the lock was acquired (epoch seconds).
    pub locked_at: Option<u64>,
```

- [ ] **Step 2: Add lock methods to CoordTaskStore trait**

In the `CoordTaskStore` trait, add after `get_newly_unblocked`:

```rust
    // --- Locking ---

    /// Acquire a lock on a task. Succeeds if unlocked or already held by the same agent.
    /// Fails if locked by a different agent.
    async fn acquire_lock(&self, task_id: &str, agent_id: &str) -> crate::error::Result<()>;

    /// Release the lock. Only the current holder can release.
    async fn release_lock(&self, task_id: &str, agent_id: &str) -> crate::error::Result<()>;

    /// Release all locks older than `max_age_secs`. Returns count of released locks.
    async fn release_stale_locks(&self, max_age_secs: u64) -> crate::error::Result<usize>;
```

- [ ] **Step 3: Update DB migration in store.rs**

In `src/agents/swarm/tasks/store.rs`, find the `migrate()` method of `SqliteCoordTaskStore`. Add after the existing CREATE TABLE/INDEX statements:

```rust
        // Add lock columns (migration — safe to run multiple times due to IF NOT EXISTS pattern)
        // SQLite doesn't support IF NOT EXISTS for ALTER TABLE, so we check column existence.
        let has_locked_by: bool = conn
            .prepare("SELECT locked_by FROM coord_tasks LIMIT 0")
            .is_ok();
        if !has_locked_by {
            conn.execute_batch(
                r#"
                ALTER TABLE coord_tasks ADD COLUMN locked_by TEXT;
                ALTER TABLE coord_tasks ADD COLUMN locked_at INTEGER;
                "#,
            )
            .map_err(db_err)?;
        }
```

- [ ] **Step 4: Update read_task_row to include lock fields**

In `src/agents/swarm/tasks/store.rs`, update `read_task_row`:

```rust
fn read_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CoordTask> {
    let status_str: String = row.get(4)?;
    let priority_str: String = row.get(6)?;
    let result_val: Option<String> = row.get(7)?;
    let metadata_str: String = row.get(8)?;

    Ok(CoordTask {
        id: row.get(0)?,
        team_id: row.get(1)?,
        subject: row.get(2)?,
        description: row.get(3)?,
        status: CoordTaskStatus::from_stored(&status_str).unwrap_or_default(),
        owner: row.get(5)?,
        priority: Priority::from_stored(&priority_str).unwrap_or_default(),
        result: result_val,
        metadata: serde_json::from_str(&metadata_str)
            .unwrap_or(serde_json::Value::Object(Default::default())),
        dependencies: Vec::new(),
        created_at: row.get(9)?,
        started_at: row.get(10)?,
        completed_at: row.get(11)?,
        locked_by: row.get(12)?,
        locked_at: row.get(13)?,
    })
}
```

Also update ALL SELECT queries in the file that list columns for coord_tasks to include `locked_by, locked_at` after `completed_at`. Search for `"SELECT id, team_id"` to find them all (in `load_task`, `list_tasks`, `create_task`).

- [ ] **Step 5: Implement acquire_lock, release_lock, release_stale_locks**

Add to `impl CoordTaskStore for SqliteCoordTaskStore`:

```rust
    async fn acquire_lock(&self, task_id: &str, agent_id: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        let now = now_epoch();

        let affected = conn
            .execute(
                r#"
                UPDATE coord_tasks
                SET locked_by = ?1, locked_at = ?2
                WHERE id = ?3 AND (locked_by IS NULL OR locked_by = ?1)
                "#,
                params![agent_id, now, task_id],
            )
            .map_err(db_err)?;

        if affected == 0 {
            // Check if task exists
            let exists: bool = conn
                .prepare_cached("SELECT 1 FROM coord_tasks WHERE id = ?1")
                .map_err(db_err)?
                .query_row(params![task_id], |_| Ok(true))
                .optional()
                .map_err(db_err)?
                .unwrap_or(false);

            if !exists {
                return Err(db_err(format!("task not found: {task_id}")));
            }

            // Task exists but locked by someone else
            let holder: Option<String> = conn
                .prepare_cached("SELECT locked_by FROM coord_tasks WHERE id = ?1")
                .map_err(db_err)?
                .query_row(params![task_id], |row| row.get(0))
                .optional()
                .map_err(db_err)?
                .flatten();

            return Err(db_err(format!(
                "task '{task_id}' is locked by '{}'",
                holder.unwrap_or_else(|| "unknown".into())
            )));
        }

        Ok(())
    }

    async fn release_lock(&self, task_id: &str, agent_id: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;

        let affected = conn
            .execute(
                r#"
                UPDATE coord_tasks
                SET locked_by = NULL, locked_at = NULL
                WHERE id = ?1 AND locked_by = ?2
                "#,
                params![task_id, agent_id],
            )
            .map_err(db_err)?;

        if affected == 0 {
            // Silently succeed if already unlocked (idempotent)
            let current_holder: Option<String> = conn
                .prepare_cached("SELECT locked_by FROM coord_tasks WHERE id = ?1")
                .map_err(db_err)?
                .query_row(params![task_id], |row| row.get(0))
                .optional()
                .map_err(db_err)?
                .flatten();

            if current_holder.is_some() {
                return Err(db_err(format!(
                    "task '{task_id}' is locked by '{}', not '{agent_id}'",
                    current_holder.unwrap()
                )));
            }
        }

        Ok(())
    }

    async fn release_stale_locks(&self, max_age_secs: u64) -> crate::error::Result<usize> {
        let conn = self.conn.lock().await;
        let cutoff = now_epoch().saturating_sub(max_age_secs);

        let affected = conn
            .execute(
                r#"
                UPDATE coord_tasks
                SET locked_by = NULL, locked_at = NULL
                WHERE locked_by IS NOT NULL AND locked_at < ?1
                "#,
                params![cutoff],
            )
            .map_err(db_err)?;

        Ok(affected)
    }
```

- [ ] **Step 6: Add lock tests**

Add to the test module in `src/agents/swarm/tasks/store.rs`:

```rust
    #[tokio::test]
    async fn test_acquire_and_release_lock() {
        let store = setup_store().await;
        let task = store
            .create_task(NewCoordTask {
                team_id: Some("team-1".into()),
                subject: "Lock test".into(),
                description: String::new(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();

        // Acquire lock
        store.acquire_lock(&task.id, "agent-a").await.unwrap();

        // Verify lock is held
        let fetched = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(fetched.locked_by.as_deref(), Some("agent-a"));
        assert!(fetched.locked_at.is_some());

        // Same agent can re-acquire (idempotent)
        store.acquire_lock(&task.id, "agent-a").await.unwrap();

        // Different agent cannot acquire
        let err = store.acquire_lock(&task.id, "agent-b").await;
        assert!(err.is_err());

        // Release lock
        store.release_lock(&task.id, "agent-a").await.unwrap();

        // Now agent-b can acquire
        store.acquire_lock(&task.id, "agent-b").await.unwrap();
        let fetched = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(fetched.locked_by.as_deref(), Some("agent-b"));
    }

    #[tokio::test]
    async fn test_release_stale_locks() {
        let store = setup_store().await;
        let task = store
            .create_task(NewCoordTask {
                team_id: Some("team-1".into()),
                subject: "Stale lock test".into(),
                description: String::new(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();

        // Acquire lock and manually backdate it
        store.acquire_lock(&task.id, "agent-a").await.unwrap();
        {
            let conn = store.conn.lock().await;
            let old_time = now_epoch() - 3600; // 1 hour ago
            conn.execute(
                "UPDATE coord_tasks SET locked_at = ?1 WHERE id = ?2",
                params![old_time, task.id],
            )
            .unwrap();
        }

        // Release locks older than 30 minutes
        let released = store.release_stale_locks(1800).await.unwrap();
        assert_eq!(released, 1);

        // Verify lock is released
        let fetched = store.get_task(&task.id).await.unwrap().unwrap();
        assert!(fetched.locked_by.is_none());
    }
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p alephcore --lib agents::swarm::tasks -- --nocapture`
Expected: All tests pass.

- [ ] **Step 8: Auto-acquire lock in team_delegate**

In `src/builtin_tools/team/delegate.rs`, after the task status is set to InProgress (around line 207-215), add lock acquisition:

```rust
        // Acquire lock for the assigned agent
        if let Err(e) = self.coord_store.acquire_lock(&task.id, &args.agent_id).await {
            tracing::warn!(
                task_id = %task.id,
                agent_id = %args.agent_id,
                error = %e,
                "team_delegate: failed to acquire task lock (non-fatal)"
            );
        }
```

Also release lock on completion/failure/timeout. In the `Ok(Ok(Ok(())))` arm (after updating task to Completed), add:

```rust
                let _ = self.coord_store.release_lock(&task.id, &args.agent_id).await;
```

In the `Ok(Ok(Err(e)))` arm (after updating task to Failed):

```rust
                let _ = self.coord_store.release_lock(&task.id, &args.agent_id).await;
```

In the `Ok(Err(join_err))` arm (after updating task to Failed):

```rust
                let _ = self.coord_store.release_lock(&task.id, &args.agent_id).await;
```

In the `Err(_)` timeout arm (after updating task to Failed):

```rust
                let _ = self.coord_store.release_lock(&task.id, &args.agent_id).await;
```

- [ ] **Step 9: Verify and commit**

Run: `cargo check -p alephcore 2>&1 | head -20`

```bash
git add -A
git commit -m "teams: add task locking — acquire/release/stale_locks with auto-management in delegate"
```

---

### Task 5: Final verification

- [ ] **Step 1: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20`
Expected: No warnings.

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: All tests pass.

- [ ] **Step 3: Verify no orphan references**

```bash
grep -rn "review_score\|ReviewScore\|team_explorer\|team_critic\|AgentRole" src/ --include="*.rs" | grep -v target/ | grep -v "AgentRoleLayer\|agent_role" | grep -v "// "
```

Expected: No results related to deleted types.

- [ ] **Step 4: Final commit if needed**

```bash
git add -A
git commit -m "teams: final cleanup after infrastructure enhancement"
```
