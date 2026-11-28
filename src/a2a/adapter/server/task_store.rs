use std::collections::HashMap;

use chrono::Utc;
use tokio::sync::{watch, RwLockWriteGuard};

use crate::a2a::domain::{
    A2AError, A2AMessage, A2ARole, A2ATask, Artifact, ListTasksParams, ListTasksResult, TaskState,
    TaskStatus, TaskStatusUpdateEvent,
};
use uuid::Uuid;

use crate::a2a::port::authenticator::TaskOwner;
use crate::a2a::port::streaming::TaskUpdateStream;
use crate::a2a::port::{A2AResult, A2AStreamingHandler};
use crate::a2a::service::notification::PushNotificationConfig;
use crate::gateway::execution_adapter::ChatRunCancellation;
use crate::sync_primitives::AsyncRwLock;

/// Upper bound on retained tasks, including completed snapshots.
/// Reclaim the oldest terminal tasks at capacity and keep active tasks.
const MAX_TASKS: usize = 10_000;
const MAX_PUSH_CONFIGS: usize = 1_024;

pub struct TaskStore {
    tasks: AsyncRwLock<RetainedTasks>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TaskScope {
    Legacy(TaskOwner),
    Standard(TaskOwner),
}

#[derive(Default)]
struct RetainedTasks {
    by_instance: HashMap<String, TaskRecord>,
    aliases: HashMap<(TaskScope, String), String>,
}

struct TaskRecord {
    scope: TaskScope,
    task: A2ATask,
    push: Option<PushNotificationConfig>,
    active: Option<ActiveRun>,
}

struct ActiveRun {
    id: Uuid,
    cancellation: ChatRunCancellation,
    settlement: watch::Sender<Option<A2ATask>>,
}

/// A successfully admitted task, with public identifiers separated from the instance.
pub struct TaskClaim {
    pub(crate) instance_id: String,
    pub(crate) session_id: String,
    pub(crate) run_id: Uuid,
    pub(crate) cancellation: ChatRunCancellation,
    pub(crate) pending_question: Option<String>,
    pub(crate) task: A2ATask,
}

impl RetainedTasks {
    fn insert(&mut self, instance: String, scope: TaskScope, task: A2ATask) {
        self.aliases
            .insert((scope, task.id.clone()), instance.clone());
        self.by_instance.insert(
            instance,
            TaskRecord {
                scope,
                task,
                push: None,
                active: None,
            },
        );
    }

    fn resolve(&self, scope: TaskScope, alias: &str) -> A2AResult<&str> {
        self.aliases
            .get(&(scope, alias.to_string()))
            .map(String::as_str)
            .ok_or_else(|| A2AError::TaskNotFound(alias.to_string()))
    }

    fn check_push_capacity(&self, instance: &str) -> A2AResult<()> {
        if self
            .by_instance
            .get(instance)
            .is_some_and(|record| record.push.is_some())
            || self
                .by_instance
                .values()
                .filter(|record| record.push.is_some())
                .count()
                < MAX_PUSH_CONFIGS
        {
            return Ok(());
        }
        Err(A2AError::InvalidRequest(
            "push notification config store is at capacity; delete an existing config to free a slot or retry later".to_string(),
        ))
    }
}

/// Evict the oldest terminal tasks until the map is back under `MAX_TASKS`
/// (or no further terminal tasks remain). Only terminal tasks are reclaimed
/// so an in-flight task is never dropped out from under its handler.
///
/// Returns `true` when the store is below `MAX_TASKS` afterwards. `false`
/// means every retained task is still in flight: the store cannot accept
/// more work without dropping a live task, so the caller must refuse the
/// new task instead of growing the map without bound.
fn evict_terminal_tasks(tasks: &mut RetainedTasks) -> bool {
    if tasks.by_instance.len() < MAX_TASKS {
        return true;
    }
    let mut terminal: Vec<(String, chrono::DateTime<Utc>)> = tasks
        .by_instance
        .iter()
        .filter(|(_, record)| record.active.is_none() && record.task.status.state.is_terminal())
        .map(|(id, record)| (id.clone(), record.task.status.timestamp))
        .collect();
    terminal.sort_by_key(|entry| entry.1);
    let target = MAX_TASKS * 9 / 10;
    for (id, _) in terminal {
        if tasks.by_instance.len() <= target {
            break;
        }
        if let Some(record) = tasks.by_instance.remove(&id) {
            tasks.aliases.remove(&(record.scope, record.task.id));
        }
    }
    tasks.by_instance.len() < MAX_TASKS
}

/// Error returned when the in-memory task store has no room for a new task.
fn at_capacity_error() -> A2AError {
    A2AError::InvalidRequest("task store is at capacity; retry later".to_string())
}

impl TaskStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tasks: AsyncRwLock::new(RetainedTasks::default()),
        }
    }

    async fn lock_for_task_action(
        &self,
        scope: TaskScope,
        alias: Option<&str>,
    ) -> RwLockWriteGuard<'_, RetainedTasks> {
        loop {
            let tasks = self.tasks.write().await;
            let settling = alias
                .and_then(|alias| tasks.resolve(scope, alias).ok())
                .map(|instance| &tasks.by_instance[instance])
                .filter(|record| record.task.status.state == TaskState::InputRequired)
                .and_then(|record| record.active.as_ref());
            let Some(run) = settling else {
                return tasks;
            };
            // The paused snapshot can be visible before its stream is closed.
            // Recheck ownership and state after cleanup releases the old run.
            let mut settlement = run.settlement.subscribe();
            drop(tasks);
            let _ = settlement.changed().await;
        }
    }

    pub(crate) async fn claim_legacy(
        &self,
        owner: TaskOwner,
        alias: &str,
        requested_context: Option<&str>,
        push: Option<PushNotificationConfig>,
    ) -> A2AResult<TaskClaim> {
        let scope = TaskScope::Legacy(owner);
        let mut tasks = self.lock_for_task_action(scope, Some(alias)).await;
        let previous = tasks.resolve(scope, alias).ok().map(str::to_owned);
        let instance = if let Some(instance) = previous.as_ref() {
            let record = &tasks.by_instance[instance];
            if record.active.is_some()
                || !matches!(
                    record.task.status.state,
                    TaskState::Submitted | TaskState::InputRequired
                )
            {
                return Err(A2AError::InvalidRequest(format!(
                    "Task {alias} cannot accept another message"
                )));
            }
            if requested_context.is_some_and(|context| context != record.task.context_id) {
                return Err(A2AError::TaskNotFound(alias.to_string()));
            }
            instance.clone()
        } else {
            Uuid::new_v4().to_string()
        };

        if push.is_some() {
            tasks.check_push_capacity(&instance)?;
        }
        if previous.is_none() {
            if !evict_terminal_tasks(&mut tasks) {
                return Err(at_capacity_error());
            }
            tasks.insert(
                instance.clone(),
                scope,
                A2ATask::new(alias, requested_context.unwrap_or(alias)),
            );
        }
        let record = tasks
            .by_instance
            .get_mut(&instance)
            .expect("admitted task is retained");
        Ok(Self::admit(record, instance.clone(), instance, push))
    }

    pub(crate) async fn claim_standard(
        &self,
        owner: TaskOwner,
        alias: Option<&str>,
        requested_context: Option<&str>,
        references: &[String],
        message: &A2AMessage,
    ) -> A2AResult<TaskClaim> {
        let scope = TaskScope::Standard(owner);
        let mut tasks = self.lock_for_task_action(scope, alias).await;
        let resolve = |id: &str| tasks.resolve(scope, id).map(str::to_owned);
        for reference in references {
            resolve(reference)?;
        }
        let previous = alias.map(resolve).transpose()?;
        let context_id = match previous.as_ref() {
            Some(instance) => {
                let task = &tasks.by_instance[instance].task;
                if requested_context.is_some_and(|context| context != task.context_id) {
                    return Err(A2AError::TaskNotFound(task.id.clone()));
                }
                task.context_id.clone()
            }
            None if requested_context.is_some() => {
                let context = requested_context.expect("present context");
                if !tasks
                    .by_instance
                    .values()
                    .any(|record| record.scope == scope && record.task.context_id == context)
                {
                    return Err(A2AError::TaskNotFound(context.to_owned()));
                }
                context.to_owned()
            }
            None => Uuid::new_v4().to_string(),
        };
        if tasks.by_instance.values().any(|record| {
            record.scope == scope && record.task.context_id == context_id && record.active.is_some()
        }) {
            return Err(A2AError::ContextBusy);
        }
        let instance = if let Some(instance) = previous {
            let task = &tasks.by_instance[&instance].task;
            if task.status.state != TaskState::InputRequired {
                return Err(A2AError::UnsupportedOperation(
                    "Task cannot accept another message",
                ));
            }
            instance
        } else {
            if !evict_terminal_tasks(&mut tasks) {
                return Err(at_capacity_error());
            }
            let instance = Uuid::new_v4().to_string();
            tasks.insert(
                instance.clone(),
                scope,
                A2ATask::new(&instance, &context_id),
            );
            instance
        };
        let record = tasks
            .by_instance
            .get_mut(&instance)
            .expect("admitted task is retained");
        record.task.history.push(message.clone());
        Ok(Self::admit(record, instance, context_id, None))
    }

    fn admit(
        record: &mut TaskRecord,
        instance_id: String,
        session_id: String,
        push: Option<PushNotificationConfig>,
    ) -> TaskClaim {
        if let Some(push) = push {
            record.push = Some(push);
        }
        let pending_question = (record.task.status.state == TaskState::InputRequired)
            .then(|| {
                record
                    .task
                    .status
                    .message
                    .as_ref()
                    .map(A2AMessage::text_content)
            })
            .flatten();
        record.task.status = TaskStatus {
            state: TaskState::Working,
            message: None,
            timestamp: Utc::now(),
        };
        let run_id = Uuid::new_v4();
        let cancellation = ChatRunCancellation::for_remote_task();
        record.active = Some(ActiveRun {
            id: run_id,
            cancellation: cancellation.clone(),
            settlement: watch::channel(None).0,
        });
        TaskClaim {
            instance_id,
            session_id,
            run_id,
            cancellation,
            pending_question,
            task: record.task.clone(),
        }
    }

    pub(crate) async fn observe_owned(
        &self,
        scope: TaskScope,
        alias: &str,
        streaming: &dyn A2AStreamingHandler,
    ) -> A2AResult<(A2ATask, Option<TaskUpdateStream>)> {
        let tasks = self.tasks.read().await;
        let instance = tasks.resolve(scope, alias)?;
        let record = &tasks.by_instance[instance];
        // Settlement requires the write lock. An observer either attaches before
        // settlement can publish or reads the settled snapshot without a channel.
        let updates = if record.active.is_some() && record.task.status.state == TaskState::Working {
            Some(streaming.subscribe_all(instance).await?)
        } else {
            None
        };
        Ok((record.task.clone(), updates))
    }

    pub(crate) async fn resolve_legacy(&self, owner: TaskOwner, alias: &str) -> A2AResult<String> {
        self.tasks
            .read()
            .await
            .resolve(TaskScope::Legacy(owner), alias)
            .map(str::to_owned)
    }

    pub(crate) async fn set_push_config(
        &self,
        instance: &str,
        config: PushNotificationConfig,
    ) -> A2AResult<PushNotificationConfig> {
        let mut tasks = self.tasks.write().await;
        if !tasks.by_instance.contains_key(instance) {
            return Err(A2AError::TaskNotFound(config.task_id));
        }
        tasks.check_push_capacity(instance)?;
        tasks
            .by_instance
            .get_mut(instance)
            .expect("task is retained")
            .push = Some(config.clone());
        Ok(config)
    }

    pub(crate) async fn push_config(&self, instance: &str) -> Option<PushNotificationConfig> {
        self.tasks
            .read()
            .await
            .by_instance
            .get(instance)
            .and_then(|record| record.push.clone())
    }

    pub(crate) async fn get_owned_push_config(
        &self,
        owner: TaskOwner,
        alias: &str,
    ) -> A2AResult<Option<PushNotificationConfig>> {
        let tasks = self.tasks.read().await;
        let instance = tasks.resolve(TaskScope::Legacy(owner), alias)?;
        Ok(tasks.by_instance[instance].push.clone())
    }

    pub(crate) async fn delete_push_config(&self, owner: TaskOwner, alias: &str) -> A2AResult<()> {
        let mut tasks = self.tasks.write().await;
        let instance = tasks.resolve(TaskScope::Legacy(owner), alias)?.to_string();
        tasks
            .by_instance
            .get_mut(&instance)
            .expect("task is retained")
            .push = None;
        Ok(())
    }

    pub(crate) async fn list_push_configs(&self, owner: TaskOwner) -> Vec<PushNotificationConfig> {
        let tasks = self.tasks.read().await;
        let mut configs: Vec<_> = tasks
            .by_instance
            .values()
            .filter(|record| record.scope == TaskScope::Legacy(owner))
            .filter_map(|record| record.push.clone())
            .collect();
        configs.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        configs
    }

    pub(crate) async fn list_owned(
        &self,
        scope: TaskScope,
        params: ListTasksParams,
    ) -> ListTasksResult {
        let tasks = self.tasks.read().await;
        let mut result: Vec<_> = tasks
            .by_instance
            .values()
            .filter(|record| record.scope == scope)
            .map(|record| &record.task)
            .filter(|task| {
                params
                    .context_id
                    .as_ref()
                    .is_none_or(|context| task.context_id == *context)
                    && params
                        .state_filter
                        .as_ref()
                        .is_none_or(|states| states.contains(&task.status.state))
            })
            .cloned()
            .collect();
        result.sort_by(|a, b| {
            b.status
                .timestamp
                .cmp(&a.status.timestamp)
                .then_with(|| a.id.cmp(&b.id))
        });
        result.truncate(params.limit.unwrap_or(100));
        ListTasksResult {
            tasks: result,
            next_cursor: None,
        }
    }
}

impl Default for TaskStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskStore {
    #[cfg(test)]
    pub(crate) async fn create_task(&self, task_id: &str, context_id: &str) -> A2AResult<A2ATask> {
        let task = A2ATask::new(task_id, context_id);
        let mut tasks = self.tasks.write().await;
        if tasks.by_instance.contains_key(task_id) {
            return Err(A2AError::InvalidRequest(format!(
                "Task already exists: {task_id}"
            )));
        }
        if !evict_terminal_tasks(&mut tasks) {
            return Err(at_capacity_error());
        }
        tasks.insert(
            task_id.to_string(),
            TaskScope::Legacy(TaskOwner::Loopback),
            task.clone(),
        );
        Ok(task)
    }

    #[cfg(test)]
    pub(crate) async fn get_task(
        &self,
        instance: &str,
        history_length: Option<usize>,
    ) -> A2AResult<A2ATask> {
        let tasks = self.tasks.read().await;
        let task = &tasks
            .by_instance
            .get(instance)
            .ok_or_else(|| A2AError::TaskNotFound(instance.to_string()))?
            .task;
        Ok(Self::history_snapshot(task, history_length))
    }

    pub(crate) async fn get_owned(
        &self,
        scope: TaskScope,
        alias: &str,
        history_length: Option<usize>,
    ) -> A2AResult<A2ATask> {
        let tasks = self.tasks.read().await;
        let instance = tasks.resolve(scope, alias)?;
        Ok(Self::history_snapshot(
            &tasks.by_instance[instance].task,
            history_length,
        ))
    }

    fn history_snapshot(task: &A2ATask, history_length: Option<usize>) -> A2ATask {
        let mut snapshot = task.clone();
        if let Some(limit) = history_length {
            let start = snapshot.history.len().saturating_sub(limit);
            snapshot.history.drain(..start);
        }
        snapshot
    }

    #[cfg(test)]
    pub(crate) async fn update_status(
        &self,
        instance: &str,
        state: TaskState,
        message: Option<A2AMessage>,
    ) -> A2AResult<A2ATask> {
        self.update_snapshot(instance, None, state, message, None)
            .await
    }

    pub(crate) async fn finish_run(
        &self,
        claim: &TaskClaim,
        state: TaskState,
        message: Option<A2AMessage>,
        artifact: Option<Artifact>,
    ) -> A2AResult<A2ATask> {
        self.update_snapshot(
            &claim.instance_id,
            Some(claim.run_id),
            state,
            message,
            artifact,
        )
        .await
    }

    async fn update_snapshot(
        &self,
        instance: &str,
        run_id: Option<Uuid>,
        state: TaskState,
        message: Option<A2AMessage>,
        artifact: Option<Artifact>,
    ) -> A2AResult<A2ATask> {
        let mut tasks = self.tasks.write().await;
        let record = tasks
            .by_instance
            .get_mut(instance)
            .ok_or_else(|| A2AError::TaskNotFound(String::new()))?;
        if run_id.is_some_and(|id| record.active.as_ref().map(|run| run.id) != Some(id)) {
            return Err(A2AError::InternalError(
                "A2A run is no longer active".into(),
            ));
        }
        let task = &mut record.task;
        if !task.status.state.can_transition_to(&state) {
            return Err(A2AError::InvalidRequest(format!(
                "Cannot transition task {} from {:?} to {:?}",
                task.id, task.status.state, state
            )));
        }
        task.status = TaskStatus {
            state,
            message: message.clone(),
            timestamp: Utc::now(),
        };
        if let Some(msg) = message {
            task.history.push(msg);
        }
        if let Some(artifact) = artifact {
            task.artifacts.push(artifact);
        }
        if run_id.is_none() && state != TaskState::Working {
            if let Some(active) = record.active.take() {
                active.settlement.send_replace(Some(task.clone()));
            }
        }
        Ok(task.clone())
    }

    /// Release the run after its last broadcast and channel cleanup. A resumed
    /// task can then acquire a new channel without inheriting any old events.
    pub(crate) async fn release_run(&self, claim: &TaskClaim) {
        let mut tasks = self.tasks.write().await;
        if let Some(record) = tasks.by_instance.get_mut(&claim.instance_id) {
            if record
                .active
                .as_ref()
                .is_some_and(|run| run.id == claim.run_id)
            {
                let run = record.active.take().expect("matched active run");
                run.settlement.send_replace(Some(record.task.clone()));
            }
        }
    }

    pub(crate) async fn cancel_owned(
        &self,
        scope: TaskScope,
        alias: &str,
        streaming: &dyn A2AStreamingHandler,
    ) -> A2AResult<A2ATask> {
        let (cancellation, mut settlement) = {
            let mut tasks = self.lock_for_task_action(scope, Some(alias)).await;
            let instance = tasks.resolve(scope, alias)?.to_owned();
            let record = tasks
                .by_instance
                .get_mut(&instance)
                .expect("resolved task is retained");
            if record.task.status.state == TaskState::InputRequired && record.active.is_none() {
                let message = A2AMessage::text(A2ARole::Agent, "Task canceled");
                record.task.status = TaskStatus {
                    state: TaskState::Canceled,
                    message: Some(message.clone()),
                    timestamp: Utc::now(),
                };
                record.task.history.push(message);
                let task = record.task.clone();
                drop(tasks);
                let event = TaskStatusUpdateEvent::new(
                    task.id.clone(),
                    task.context_id.clone(),
                    task.status.clone(),
                    None,
                );
                if let Err(error) = streaming.broadcast_status(&instance, event).await {
                    tracing::warn!(%error, "A2A cancellation update could not be published");
                }
                return Ok(task);
            }
            if record.task.status.state != TaskState::Working {
                return Err(A2AError::TaskNotCancelable(record.task.status.state));
            }
            let run = record
                .active
                .as_ref()
                .ok_or(A2AError::TaskNotCancelable(record.task.status.state))?;
            (run.cancellation.clone(), run.settlement.subscribe())
        };
        let task = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            // The request records stop intent before it waits on inline Core work.
            let _ = cancellation.request().await;
            settlement
                .wait_for(Option::is_some)
                .await
                .map(|value| value.as_ref().unwrap().clone())
        })
        .await
        .map_err(|_| A2AError::CancellationPending)?
        .map_err(|_| A2AError::InternalError("A2A run ended without settlement".into()))?;
        if task.status.state == TaskState::Canceled {
            return Ok(task);
        }
        Err(A2AError::TaskNotCancelable(task.status.state))
    }

    #[cfg(test)]
    pub(crate) async fn list_tasks(&self, params: ListTasksParams) -> A2AResult<ListTasksResult> {
        Ok(self
            .list_owned(TaskScope::Legacy(TaskOwner::Loopback), params)
            .await)
    }

    #[cfg(test)]
    pub(crate) async fn add_artifact(&self, instance: &str, artifact: Artifact) -> A2AResult<()> {
        let mut tasks = self.tasks.write().await;
        let task = &mut tasks
            .by_instance
            .get_mut(instance)
            .ok_or_else(|| A2AError::TaskNotFound(instance.to_string()))?
            .task;
        task.artifacts.push(artifact);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::{A2ARole, Part};

    #[tokio::test]
    async fn create_and_get_task() {
        let store = TaskStore::new();
        let task = store.create_task("t1", "ctx1").await.unwrap();
        assert_eq!(task.id, "t1");
        assert_eq!(task.context_id, "ctx1");
        assert_eq!(task.status.state, TaskState::Submitted);

        let fetched = store.get_task("t1", None).await.unwrap();
        assert_eq!(fetched.id, "t1");
        assert_eq!(fetched.context_id, "ctx1");
    }

    #[tokio::test]
    async fn get_nonexistent_task_returns_not_found() {
        let store = TaskStore::new();
        let err = store.get_task("nonexistent", None).await.unwrap_err();
        assert!(matches!(err, A2AError::TaskNotFound(id) if id == "nonexistent"));
    }

    #[tokio::test]
    async fn update_status_transitions() {
        let store = TaskStore::new();
        store.create_task("t1", "ctx1").await.unwrap();

        let msg = A2AMessage::text(A2ARole::Agent, "Working on it");
        let task = store
            .update_status("t1", TaskState::Working, Some(msg))
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::Working);
        assert!(task.status.message.is_some());
        assert_eq!(task.history.len(), 1);

        // Transition to completed without message
        let task = store
            .update_status("t1", TaskState::Completed, None)
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::Completed);
        assert!(task.status.message.is_none());
        assert_eq!(task.history.len(), 1); // No new message added
    }

    #[tokio::test]
    async fn cancellation_before_registration_waits_for_the_bound_run() {
        let store = TaskStore::new();
        let claim = store
            .claim_legacy(TaskOwner::Loopback, "t1", None, None)
            .await
            .unwrap();
        let settle = async {
            claim.cancellation.token().cancelled().await;
            assert!(claim.cancellation.check().is_err());
            assert_eq!(
                store
                    .get_owned(TaskScope::Legacy(TaskOwner::Loopback), "t1", None)
                    .await
                    .unwrap()
                    .status
                    .state,
                TaskState::Working
            );
            store
                .finish_run(&claim, TaskState::Canceled, None, None)
                .await
                .unwrap();
            store.release_run(&claim).await;
        };
        let streaming = crate::a2a::adapter::server::StreamHub::new();
        let (task, ()) = tokio::join!(
            store.cancel_owned(TaskScope::Legacy(TaskOwner::Loopback), "t1", &streaming),
            settle
        );
        assert_eq!(task.unwrap().status.state, TaskState::Canceled);
    }

    #[tokio::test]
    async fn earlier_run_cannot_settle_or_release_a_resumed_task() {
        let store = TaskStore::new();
        let owner = TaskOwner::Credential(Uuid::new_v4());
        let request = A2AMessage::text(A2ARole::User, "choose a color");
        let initial = store
            .claim_standard(owner, None, None, &[], &request)
            .await
            .unwrap();
        let question = A2AMessage::text(A2ARole::Agent, "Which color?");
        store
            .finish_run(&initial, TaskState::InputRequired, Some(question), None)
            .await
            .unwrap();
        let answer = A2AMessage::text(A2ARole::User, "blue");
        let resume = store.claim_standard(owner, Some(&initial.task.id), None, &[], &answer);
        tokio::pin!(resume);
        assert!(futures::poll!(resume.as_mut()).is_pending());
        store.release_run(&initial).await;
        let resumed = resume.await.unwrap();
        assert_ne!(initial.run_id, resumed.run_id);
        assert!(matches!(
            store
                .finish_run(&initial, TaskState::Failed, None, None)
                .await,
            Err(A2AError::InternalError(_))
        ));
        store.release_run(&initial).await;
        let current = store
            .get_owned(TaskScope::Standard(owner), &resumed.task.id, None)
            .await
            .unwrap();
        assert_eq!(current.status.state, TaskState::Working);
        assert_eq!(
            store
                .finish_run(&resumed, TaskState::Completed, None, None)
                .await
                .unwrap()
                .status
                .state,
            TaskState::Completed
        );
        store.release_run(&resumed).await;
    }

    #[tokio::test]
    async fn cancel_terminal_task_returns_error() {
        let store = TaskStore::new();
        store.create_task("t1", "ctx1").await.unwrap();
        store
            .update_status("t1", TaskState::Completed, None)
            .await
            .unwrap();

        let err = store
            .cancel_owned(
                TaskScope::Legacy(TaskOwner::Loopback),
                "t1",
                &crate::a2a::adapter::server::StreamHub::new(),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            A2AError::TaskNotCancelable(TaskState::Completed)
        ));
    }

    #[tokio::test]
    async fn list_with_context_filter() {
        let store = TaskStore::new();
        store.create_task("t1", "ctx-a").await.unwrap();
        store.create_task("t2", "ctx-b").await.unwrap();
        store.create_task("t3", "ctx-a").await.unwrap();

        let result = store
            .list_tasks(ListTasksParams {
                context_id: Some("ctx-a".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(result.tasks.len(), 2);
        assert!(result.tasks.iter().all(|t| t.context_id == "ctx-a"));
    }

    #[tokio::test]
    async fn list_with_state_filter() {
        let store = TaskStore::new();
        store.create_task("t1", "ctx1").await.unwrap();
        store.create_task("t2", "ctx1").await.unwrap();
        store
            .update_status("t2", TaskState::Working, None)
            .await
            .unwrap();

        let result = store
            .list_tasks(ListTasksParams {
                state_filter: Some(vec![TaskState::Working]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(result.tasks.len(), 1);
        assert_eq!(result.tasks[0].id, "t2");
    }

    #[tokio::test]
    async fn add_artifact_to_task() {
        let store = TaskStore::new();
        store.create_task("t1", "ctx1").await.unwrap();

        let artifact = Artifact {
            artifact_id: "art-1".to_string(),
            kind: "code".to_string(),
            parts: vec![Part::Text {
                text: "fn main() {}".to_string(),
                metadata: None,
            }],
            metadata: None,
        };
        store.add_artifact("t1", artifact).await.unwrap();

        let task = store.get_task("t1", None).await.unwrap();
        assert_eq!(task.artifacts.len(), 1);
        assert_eq!(task.artifacts[0].artifact_id, "art-1");
    }

    #[tokio::test]
    async fn add_artifact_nonexistent_task() {
        let store = TaskStore::new();
        let artifact = Artifact {
            artifact_id: "art-1".to_string(),
            kind: "code".to_string(),
            parts: vec![],
            metadata: None,
        };
        let err = store
            .add_artifact("nonexistent", artifact)
            .await
            .unwrap_err();
        assert!(matches!(err, A2AError::TaskNotFound(_)));
    }

    #[tokio::test]
    async fn history_length_truncation() {
        let store = TaskStore::new();
        store.create_task("t1", "ctx1").await.unwrap();

        // Add 5 messages to history
        for i in 0..5 {
            let msg = A2AMessage::text(A2ARole::Agent, format!("msg-{}", i));
            store
                .update_status("t1", TaskState::Working, Some(msg))
                .await
                .unwrap();
        }

        // Full history
        let task = store.get_task("t1", None).await.unwrap();
        assert_eq!(task.history.len(), 5);

        // Truncated to last 2
        let task = store.get_task("t1", Some(2)).await.unwrap();
        assert_eq!(task.history.len(), 2);
        assert_eq!(task.history[0].text_content(), "msg-3");
        assert_eq!(task.history[1].text_content(), "msg-4");

        // Limit larger than history returns all
        let task = store.get_task("t1", Some(100)).await.unwrap();
        assert_eq!(task.history.len(), 5);
    }

    #[tokio::test]
    async fn list_respects_limit() {
        let store = TaskStore::new();
        for i in 0..10 {
            store.create_task(&format!("t{}", i), "ctx1").await.unwrap();
        }

        let result = store
            .list_tasks(ListTasksParams {
                limit: Some(3),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(result.tasks.len(), 3);
        assert!(result.next_cursor.is_none());
    }
}
