//! Test-owned gates around the canonical metadata store, and a runtime owner
//! for the real handlers' detached queue tasks. No network provider is installed.
#![allow(dead_code)] // R2 consumes the second handler and closed-gate controls.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::thread::JoinHandle;

use anyhow::{anyhow, ensure, Context};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot, watch, Mutex as AsyncMutex, RwLock};
use tokio::task::{Id, JoinSet, LocalSet};
use tokio_util::sync::CancellationToken;

use alephcore::executor::ToolRegistry;
use alephcore::gateway::agent_instance::AgentInstanceConfig;
use alephcore::gateway::event_bus::GatewayEventBus;
use alephcore::gateway::event_visibility::EventVisibilityIndex;
use alephcore::gateway::execution_engine::{set_mcp_tool_registry, ExecutionEngineConfig};
use alephcore::gateway::router::{AgentRouter, SessionKey};
use alephcore::gateway::session_manager::{SessionManager, SessionManagerConfig};
use alephcore::gateway::session_store::error::SessionStoreError;
use alephcore::gateway::session_store::types::*;
use alephcore::gateway::session_store::{RunBill, SessionStore, StampOutcome};
use alephcore::gateway::{
    AgentInstance, AgentRegistry, ExecutionEngine, JsonRpcRequest, JsonRpcResponse,
};
use alephcore::providers::adapter::{ProviderResponse, RequestPayload};
use alephcore::providers::AiProvider;
use alephcore::scope::{with_scope, ScopeAttribution};
use alephcore::thinker::SingleProviderRegistry;
use alephcore::tool_metadata::UnifiedTool;
use alephcore::tools::ToolHandlerRegistry;

static FIXTURE_PROCESS_BOOT: Once = Once::new();

tokio::task_local! {
    // Spawned engine tasks do not inherit this real RPC future's marker.
    static RPC_HANDLER: ();
}

#[derive(Debug)]
struct Effects {
    provider_requests: Mutex<Vec<ProviderObservation>>,
    provider_entered: watch::Sender<usize>,
    provider_gate: watch::Sender<bool>,
    tools: AtomicUsize,
}

#[derive(Clone, Debug)]
pub struct ProviderObservation {
    pub name: String,
    pub user_texts: Vec<String>,
}

struct LocalProvider(Arc<Effects>);

impl LocalProvider {
    const NAME: &'static str = "readiness-local";
}

impl AiProvider for LocalProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = alephcore::Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            let user_texts = payload
                .messages
                .iter()
                .map(aleph_slot_api::ai_message::UnifiedMessage::text_content)
                .collect::<Vec<_>>();
            {
                self.0
                    .provider_requests
                    .lock()
                    .expect("provider request observations")
                    .push(ProviderObservation {
                        name: self.name().to_owned(),
                        user_texts,
                    });
            }
            self.0.provider_entered.send_modify(|count| *count += 1);
            self.0
                .provider_gate
                .subscribe()
                .wait_for(|open| *open)
                .await
                .expect("fixture owns provider gate");
            Ok(ProviderResponse::text_only("readiness local reply".into()))
        })
    }
    fn name(&self) -> &str {
        Self::NAME
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

struct EmptyTools(Arc<Effects>);

impl ToolRegistry for EmptyTools {
    fn get_tool(&self, _name: &str) -> Option<&UnifiedTool> {
        None
    }
    fn execute_tool(
        &self,
        _name: &str,
        _arguments: Value,
    ) -> Pin<Box<dyn Future<Output = alephcore::Result<Value>> + Send + '_>> {
        Box::pin(async move {
            self.0.tools.fetch_add(1, Ordering::SeqCst);
            Err(alephcore::AlephError::other(
                "no tools installed in readiness fixture",
            ))
        })
    }
}

type Engine = ExecutionEngine<SingleProviderRegistry, EmptyTools>;

/// send_replace retains gate state even before anyone subscribes. watch also
/// retains entry/commit counts, so a notification cannot be missed by a waiter.
struct BarrierStore {
    inner: Arc<dyn SessionStore>,
    prepare: watch::Sender<bool>,
    committed_prepare: watch::Sender<bool>,
    entries: watch::Sender<usize>,
    commits: watch::Sender<usize>,
    metadata: Mutex<Vec<SessionMetadata>>,
    fail_next: Mutex<Option<String>>,
    active: AtomicUsize,
    active_rpc: AtomicUsize,
}

struct StoreCall<'a> {
    all: &'a AtomicUsize,
    rpc: Option<&'a AtomicUsize>,
}

impl<'a> StoreCall<'a> {
    fn new(all: &'a AtomicUsize, rpc: Option<&'a AtomicUsize>) -> Self {
        all.fetch_add(1, Ordering::SeqCst);
        if let Some(rpc) = rpc {
            rpc.fetch_add(1, Ordering::SeqCst);
        }
        Self { all, rpc }
    }
}

impl Drop for StoreCall<'_> {
    fn drop(&mut self) {
        self.all.fetch_sub(1, Ordering::SeqCst);
        if let Some(rpc) = self.rpc {
            rpc.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl BarrierStore {
    fn new(inner: Arc<dyn SessionStore>) -> Self {
        Self {
            inner,
            prepare: watch::channel(false).0,
            committed_prepare: watch::channel(false).0,
            entries: watch::channel(0).0,
            commits: watch::channel(0).0,
            metadata: Mutex::new(Vec::new()),
            fail_next: Mutex::new(None),
            active: AtomicUsize::new(0),
            active_rpc: AtomicUsize::new(0),
        }
    }

    async fn wait_count(sender: &watch::Sender<usize>, count: usize) {
        sender
            .subscribe()
            .wait_for(|seen| *seen >= count)
            .await
            .expect("fixture owns condition sender");
    }
}

struct HandlerContext {
    engine: Arc<Engine>,
    events: Arc<GatewayEventBus>,
    router: Arc<AgentRouter>,
    agents: Arc<AgentRegistry>,
    config: Arc<RwLock<alephcore::Config>>,
    providers: Arc<SingleProviderRegistry>,
    store: Arc<BarrierStore>,
}

struct Call {
    method: &'static str,
    user: String,
    key: String,
    input: String,
    response: oneshot::Sender<JsonRpcResponse>,
}

enum Control {
    Cancel {
        key: String,
        settled: oneshot::Sender<anyhow::Result<()>>,
    },
    Join {
        key: String,
        settled: oneshot::Sender<anyhow::Result<()>>,
    },
}

pub struct ReadinessRig {
    calls: mpsc::UnboundedSender<Call>,
    controls: mpsc::UnboundedSender<Control>,
    shutdown: CancellationToken,
    worker: Option<JoinHandle<anyhow::Result<()>>>,
    context: Arc<HandlerContext>,
    effects: Arc<Effects>,
    frames: Mutex<broadcast::Receiver<String>>,
    terminal_frames: AsyncMutex<broadcast::Receiver<String>>,
    event_visibility: Arc<EventVisibilityIndex>,
    visibility_store: Arc<dyn SessionStore>,
    stream_effects: AtomicUsize,
    keys: Arc<Mutex<HashSet<String>>>,
    _directory: tempfile::TempDir,
}

impl ReadinessRig {
    pub async fn new() -> anyhow::Result<Self> {
        // Process bootstrap only; metadata readiness remains controlled by the store gates.
        FIXTURE_PROCESS_BOOT.call_once(|| {
            set_mcp_tool_registry(Arc::new(ToolHandlerRegistry::new()));
        });
        let directory = tempfile::tempdir()?;
        let inner: Arc<dyn SessionStore> = Arc::new(SessionManager::new(SessionManagerConfig {
            db_path: directory.path().join("sessions.db"),
            max_messages: 100,
            compaction_keep: 50,
            auto_reset_hour: None,
            session_expiry_secs: 0,
        })?);
        let store = Arc::new(BarrierStore::new(inner));
        let effects = Arc::new(Effects {
            provider_requests: Mutex::new(Vec::new()),
            provider_entered: watch::channel(0).0,
            provider_gate: watch::channel(true).0,
            tools: AtomicUsize::new(0),
        });
        let providers = Arc::new(SingleProviderRegistry::new(Arc::new(LocalProvider(
            effects.clone(),
        ))));
        let events = Arc::new(GatewayEventBus::new());
        let frames = Mutex::new(events.subscribe());
        let terminal_frames = AsyncMutex::new(events.subscribe());
        let config = Arc::new(RwLock::new(alephcore::Config::default()));
        let engine = Arc::new(
            ExecutionEngine::new(
                ExecutionEngineConfig {
                    default_timeout_secs: 10,
                    mid_turn_steering: false,
                    ..Default::default()
                },
                providers.clone(),
                Arc::new(EmptyTools(effects.clone())),
                vec![],
            )
            .with_app_config(config.clone())
            .with_session_topic_support(store.clone(), events.clone()),
        );
        let agents = Arc::new(AgentRegistry::new());
        agents
            .register(AgentInstance::new(
                AgentInstanceConfig {
                    agent_id: "main".into(),
                    display_name: Some("Readiness fixture".into()),
                    workspace: directory.path().join("workspace"),
                    model: "readiness-local".into(),
                    max_loops: 2,
                    max_tokens: None,
                    tool_whitelist: vec![],
                    tool_blacklist: vec![],
                    agent_dir: directory.path().join("agent"),
                    allowed_links: None,
                    allowed_users: None,
                    tool_permissions: None,
                    timeout_secs: Some(10),
                },
                store.clone(),
            )?)
            .await;
        let mut router = AgentRouter::new();
        router.set_session_store(store.clone());
        let context = Arc::new(HandlerContext {
            engine,
            events,
            router: Arc::new(router),
            agents,
            config,
            providers,
            store,
        });
        let (calls, incoming) = mpsc::unbounded_channel();
        let (controls, control_commands) = mpsc::unbounded_channel();
        let shutdown = CancellationToken::new();
        let keys = Arc::new(Mutex::new(HashSet::new()));
        let (ready_tx, ready_rx) = oneshot::channel();
        let worker_context = context.clone();
        let worker_shutdown = shutdown.clone();
        let worker_keys = keys.clone();
        let worker_effects = effects.clone();
        let worker = std::thread::Builder::new()
            .name("session-readiness-fixture".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let local = LocalSet::new();
                let result = runtime.block_on(local.run_until(async move {
                    let init = install_orchestrator(&worker_context.engine, worker_effects);
                    let ok = init.is_ok();
                    let _ = ready_tx.send(init);
                    if !ok {
                        return Ok(());
                    }
                    serve_calls(
                        worker_context,
                        incoming,
                        control_commands,
                        worker_shutdown,
                        worker_keys,
                    )
                    .await
                }));
                // Production drops the queue JoinHandles. Owning its entire runtime
                // makes their cancellation and destruction part of this thread join.
                drop(local);
                drop(runtime);
                result
            })?;
        match ready_rx.await {
            Ok(Ok(())) => Ok(Self {
                calls,
                controls,
                shutdown,
                worker: Some(worker),
                context: context.clone(),
                effects,
                frames,
                terminal_frames,
                event_visibility: Arc::new(EventVisibilityIndex::new()),
                visibility_store: context.store.clone(),
                stream_effects: AtomicUsize::new(0),
                keys,
                _directory: directory,
            }),
            result => {
                shutdown.cancel();
                let joined = tokio::task::spawn_blocking(move || worker.join()).await?;
                if let Ok(Err(error)) = joined {
                    return Err(error);
                }
                Err(anyhow!(
                    "readiness runtime initialization failed: {result:?}"
                ))
            }
        }
    }

    pub async fn precreate(&self, user: &str, key: &str) -> anyhow::Result<SessionMetadata> {
        let key = SessionKey::from_key_string(key).ok_or_else(|| anyhow!("invalid session key"))?;
        // Setup bypasses only the test gate; attribution and persistence remain
        // the actual backend's CREATE operation under the actual scope.
        Ok(with_scope(
            Some(ScopeAttribution::personal(user)),
            self.context.store.inner.get_or_create(&key),
        )
        .await?)
    }

    pub async fn metadata(&self, key: &str) -> anyhow::Result<Option<SessionMetadata>> {
        let key = SessionKey::from_key_string(key).ok_or_else(|| anyhow!("invalid session key"))?;
        Ok(self.context.store.get_metadata(&key).await?)
    }

    pub fn committed_metadata(&self) -> Vec<SessionMetadata> {
        self.context
            .store
            .metadata
            .lock()
            .expect("metadata observations")
            .clone()
    }

    pub fn fail_next_get_or_create(&self, message: impl Into<String>) {
        *self
            .context
            .store
            .fail_next
            .lock()
            .expect("failure injection") = Some(message.into());
    }

    pub fn track_key(&self, key: &str) {
        self.keys.lock().expect("fixture keys").insert(key.into());
    }

    pub fn active_store_operations(&self) -> usize {
        self.context.store.active.load(Ordering::SeqCst)
    }

    pub fn active_rpc_preparations(&self) -> usize {
        self.context.store.active_rpc.load(Ordering::SeqCst)
    }

    fn start_call_with_input(
        &self,
        method: &'static str,
        user: &str,
        key: &str,
        input: &str,
    ) -> oneshot::Receiver<JsonRpcResponse> {
        let (response, receive) = oneshot::channel();
        self.keys.lock().expect("fixture keys").insert(key.into());
        self.calls
            .send(Call {
                method,
                user: user.into(),
                key: key.into(),
                input: input.into(),
                response,
            })
            .expect("fixture runtime is open");
        receive
    }

    fn start_call(
        &self,
        method: &'static str,
        user: &str,
        key: &str,
    ) -> oneshot::Receiver<JsonRpcResponse> {
        self.start_call_with_input(
            method,
            user,
            key,
            &format!("readiness-synthetic-caller:{user}"),
        )
    }

    async fn call(&self, method: &'static str, user: &str, key: &str) -> JsonRpcResponse {
        self.start_call(method, user, key)
            .await
            .expect("real handler must finish or fixture must report failure")
    }

    pub fn start_chat_send_with_input(
        &self,
        user: &str,
        key: &str,
        input: &str,
    ) -> oneshot::Receiver<JsonRpcResponse> {
        self.start_call_with_input("chat.send", user, key, input)
    }

    pub fn start_chat_send(&self, user: &str, key: &str) -> oneshot::Receiver<JsonRpcResponse> {
        self.start_call("chat.send", user, key)
    }

    pub fn start_agent_run(&self, user: &str, key: &str) -> oneshot::Receiver<JsonRpcResponse> {
        self.start_call("agent.run", user, key)
    }

    async fn settle_handler(&self, control: Control) -> anyhow::Result<()> {
        let (settled, receive) = oneshot::channel();
        let control = match control {
            Control::Cancel { key, .. } => Control::Cancel { key, settled },
            Control::Join { key, .. } => Control::Join { key, settled },
        };
        self.controls
            .send(control)
            .map_err(|_| anyhow!("fixture runtime is closed"))?;
        receive
            .await
            .map_err(|_| anyhow!("handler settlement command was dropped"))?
    }

    pub async fn cancel_handler(&self, key: &str) -> anyhow::Result<()> {
        self.settle_handler(Control::Cancel {
            key: key.into(),
            settled: oneshot::channel().0,
        })
        .await?;
        Ok(())
    }

    pub async fn join_handler(&self, key: &str) -> anyhow::Result<()> {
        self.settle_handler(Control::Join {
            key: key.into(),
            settled: oneshot::channel().0,
        })
        .await?;
        Ok(())
    }

    pub async fn chat_send(&self, user: &str, key: &str) -> JsonRpcResponse {
        self.call("chat.send", user, key).await
    }
    pub async fn agent_run(&self, user: &str, key: &str) -> JsonRpcResponse {
        self.call("agent.run", user, key).await
    }
    pub async fn wait_prepare_entered(&self) {
        self.wait_prepare_entries(1).await;
    }
    pub async fn wait_prepare_entries(&self, count: usize) {
        BarrierStore::wait_count(&self.context.store.entries, count).await;
    }
    pub async fn wait_metadata_committed(&self) {
        self.wait_metadata_committed_count(1).await;
    }
    pub async fn wait_metadata_committed_count(&self, count: usize) {
        BarrierStore::wait_count(&self.context.store.commits, count).await;
    }
    pub fn release_prepare(&self) {
        self.context.store.prepare.send_replace(true);
    }
    pub fn release_committed_prepare(&self) {
        self.context.store.committed_prepare.send_replace(true);
    }
    pub async fn configure_busy_queue(&self, max_per_session: usize, max_wait_secs: u64) {
        let mut config = self.context.config.write().await;
        config.execution.busy_queue_max_per_session = max_per_session;
        config.execution.busy_queue_max_wait_secs = max_wait_secs;
        config.execution.busy_queue_wake_fallback_secs = max_wait_secs.max(1);
    }

    pub async fn wait_run_frame(&self, run_id: &str, method: &str) -> Value {
        let mut frames = self.terminal_frames.lock().await;
        loop {
            let frame = frames
                .recv()
                .await
                .expect("fixture must receive real event frames");
            let value: Value = serde_json::from_str(&frame).expect("actual event JSON");
            if value["method"].as_str() == Some(method)
                && value["params"]["run_id"].as_str() == Some(run_id)
            {
                return value;
            }
        }
    }

    pub async fn wait_run_error(&self, run_id: &str) -> Value {
        self.wait_run_frame(run_id, "stream.run_error").await
    }

    pub async fn wait_run_queued(&self, run_id: &str) -> Value {
        self.wait_run_frame(run_id, "stream.run_queued").await
    }

    pub async fn wait_visible_run_error(&self, run_id: &str, caller: &str) -> Value {
        let mut frames = self.terminal_frames.lock().await;
        loop {
            let frame = frames
                .recv()
                .await
                .expect("fixture must receive real event frames");
            let value: Value = serde_json::from_str(&frame).expect("actual event JSON");
            let topic = value["method"].as_str().unwrap_or_default();
            let payload = value.get("params");
            self.event_visibility.note_frame(topic, payload).await;
            if topic == "stream.run_error"
                && payload.and_then(|params| params["run_id"].as_str()) == Some(run_id)
            {
                let visible = self
                    .event_visibility
                    .event_admits_for(
                        topic,
                        payload,
                        Some(caller),
                        Some("member"),
                        &self.visibility_store,
                        None,
                    )
                    .await;
                assert!(
                    visible,
                    "terminal RunError must pass the real owner/scope event filter"
                );
                return value;
            }
        }
    }

    pub fn local_provider_effects(&self) -> usize {
        self.effects
            .provider_requests
            .lock()
            .expect("provider request observations")
            .iter()
            .filter(|observation| observation.name == LocalProvider::NAME)
            .count()
    }

    /// Both installed provider handles record requests here. Their local
    /// implementation returns a literal and has no HTTP client or endpoint.
    pub fn external_provider_effects(&self) -> usize {
        self.effects
            .provider_requests
            .lock()
            .expect("provider request observations")
            .iter()
            .filter(|observation| observation.name != LocalProvider::NAME)
            .count()
    }

    pub fn close_provider(&self) {
        self.effects.provider_gate.send_replace(false);
    }

    pub fn release_provider(&self) {
        self.effects.provider_gate.send_replace(true);
    }

    pub async fn wait_provider_entered(&self) {
        BarrierStore::wait_count(&self.effects.provider_entered, 1).await;
    }

    pub fn provider_observations(&self) -> Vec<ProviderObservation> {
        self.effects
            .provider_requests
            .lock()
            .expect("provider request observations")
            .clone()
    }
    pub fn active_run_id(&self, key: &str) -> Option<String> {
        self.context.engine.active_run_for_session(key)
    }

    pub async fn active_run_diagnostic(&self, run_id: &str) -> String {
        let status = self.context.engine.get_status(run_id).await;
        let running_sessions = self.context.engine.running_sessions();
        format!("status={status:?} running_sessions={running_sessions:?}")
    }

    pub fn pending_run_ids(&self, key: &str) -> Vec<String> {
        alephcore::gateway::busy_queue::pending_for(key)
            .into_iter()
            .map(|run| run.run_id)
            .collect()
    }

    pub fn tool_effects(&self) -> usize {
        self.effects.tools.load(Ordering::SeqCst)
    }

    pub fn queue_or_provider_effects(&self) -> usize {
        let mut frames = self.frames.lock().expect("event observations");
        loop {
            match frames.try_recv() {
                Ok(frame) => {
                    let value: Value = serde_json::from_str(&frame).expect("actual event JSON");
                    if value["method"]
                        .as_str()
                        .is_some_and(|m| m.starts_with("stream."))
                    {
                        self.stream_effects.fetch_add(1, Ordering::SeqCst);
                    }
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(error) => panic!("fixture lost its real event evidence: {error}"),
            }
        }
        let queued_or_active = self
            .keys
            .lock()
            .expect("fixture keys")
            .iter()
            .map(|key| {
                alephcore::gateway::busy_queue::pending_for(key).len()
                    + usize::from(self.context.engine.active_run_for_session(key).is_some())
            })
            .sum::<usize>();
        self.stream_effects.load(Ordering::SeqCst)
            + self.local_provider_effects()
            + self.effects.tools.load(Ordering::SeqCst)
            + queued_or_active
    }

    pub async fn close(mut self) -> anyhow::Result<()> {
        self.release_prepare();
        self.release_committed_prepare();
        self.shutdown.cancel();
        let worker = self.worker.take().expect("fixture runtime owner");
        let joined = tokio::task::spawn_blocking(move || worker.join())
            .await
            .context("join fixture runtime owner")?;
        joined.map_err(|_| anyhow!("fixture runtime panicked"))??;
        let active = self.context.store.active.load(Ordering::SeqCst);
        ensure!(
            active == 0,
            "store operation frames still alive after runtime join: {active}"
        );
        for key in self.keys.lock().expect("fixture keys").iter() {
            ensure!(
                alephcore::gateway::busy_queue::pending_for(key).is_empty(),
                "queued ticket remains after runtime join: {key}"
            );
        }
        eprintln!(
            "R1 close: runtime_thread_joined=true active_store_calls={active} local_provider_calls={} tool_calls={}",
            self.local_provider_effects(),
            self.effects.tools.load(Ordering::SeqCst)
        );
        Ok(())
    }
}

async fn serve_calls(
    context: Arc<HandlerContext>,
    mut incoming: mpsc::UnboundedReceiver<Call>,
    mut controls: mpsc::UnboundedReceiver<Control>,
    shutdown: CancellationToken,
    keys: Arc<Mutex<HashSet<String>>>,
) -> anyhow::Result<()> {
    let mut handlers = JoinSet::new();
    let mut handler_handles: HashMap<Id, (String, tokio::task::AbortHandle)> = HashMap::new();
    let mut settled_handlers: HashMap<Id, String> = HashMap::new();
    let run_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            Some(done) = handlers.join_next_with_id(), if !handlers.is_empty() => {
                match done {
                    Ok((id, (key, ()))) => {
                        handler_handles.remove(&id);
                        settled_handlers.insert(id, key);
                    }
                    Err(error) => {
                        let id = error.id();
                        if let Some((key, _)) = handler_handles.remove(&id) {
                            settled_handlers.insert(id, key);
                        }
                        ensure!(
                            error.is_cancelled(),
                            "real handler task panicked: {error}"
                        );
                    }
                }
            },
            Some(control) = controls.recv() => {
                let is_cancel = matches!(control, Control::Cancel { .. });
                let (key, target_id, abort, settled) = match control {
                    Control::Cancel { key, settled } | Control::Join { key, settled } => {
                        let matches = handler_handles
                            .iter()
                            .filter(|(_, (active_key, _))| active_key == &key)
                            .map(|(id, (_, abort))| (*id, abort.clone()))
                            .collect::<Vec<_>>();
                        if matches.len() > 1 {
                            let _ = settled.send(Err(anyhow!("ambiguous active handlers for session key: {key}")));
                            continue;
                        }
                        let Some((id, abort)) = matches.into_iter().next() else {
                            let already_settled = settled_handlers
                                .iter()
                                .find(|(_, active_key)| active_key == &&key)
                                .map(|(id, _)| *id);
                            if let Some(id) = already_settled {
                                settled_handlers.remove(&id);
                                let _ = settled.send(Ok(()));
                            } else {
                                let _ = settled.send(Err(anyhow!("no active handler for session key: {key}")));
                            }
                            continue;
                        };
                        let abort = if is_cancel { Some(abort) } else { None };
                        (key, Some(id), abort, settled)
                    }
                };
                if let Some(abort) = abort {
                    abort.abort();
                }
                let operation_result = join_handler_until(
                    &mut handlers,
                    &mut handler_handles,
                    &mut settled_handlers,
                    &key,
                    target_id,
                )
                .await;
                let _ = settled.send(operation_result);
            },
            call = incoming.recv() => {
                let Some(call) = call else { break; };
                let context = context.clone();
                let run_ids = run_ids.clone();
                let key = call.key.clone();
                let map_key = key.clone();
                let handle = handlers.spawn_local(async move {
                    let request = JsonRpcRequest::new(
                        call.method,
                        Some(json!({
                            "input": call.input,
                            "session_key": call.key
                        })),
                        Some(json!("readiness-rpc")),
                    );
                    let result = trusted_caller(&call.user, RPC_HANDLER.scope((), async {
                        if call.method == "chat.send" {
                            crate::server_init::handle_chat_send_with_engine(request, context.engine.clone(), context.events.clone(), context.router.clone(), context.agents.clone(), context.config.clone(), None, context.providers.clone(), context.store.clone(), None).await
                        } else {
                            crate::server_init::handle_run_with_engine(request, context.engine.clone(), context.events.clone(), context.router.clone(), context.agents.clone(), context.config.clone(), None, context.store.clone(), None).await
                        }
                    })).await;
                    if let Some(id) = result.result.as_ref().and_then(|v| v["run_id"].as_str()) {
                        run_ids.lock().expect("submitted run ids").push(id.to_string());
                    }
                    let _ = call.response.send(result);
                    (key, ())
                });
                handler_handles.insert(handle.id(), (map_key, handle));
            }
        }
    }
    // Releasing gates happens before the shutdown signal. Cancel real engine
    // tokens and queue tickets, then abort and join any still-pending RPCs.
    let submitted = run_ids.lock().expect("submitted run ids").clone();
    for run_id in submitted {
        alephcore::gateway::busy_queue::cancel_queued_run(&run_id);
        let _ = context.engine.cancel(&run_id).await;
    }
    for key in keys.lock().expect("fixture keys").iter() {
        alephcore::gateway::busy_queue::purge(key);
    }
    handlers.abort_all();
    while let Some(done) = handlers.join_next_with_id().await {
        match done {
            Ok((id, (key, ()))) => {
                handler_handles.remove(&id);
                settled_handlers.insert(id, key);
            }
            Err(error) => {
                let id = error.id();
                if let Some((key, _)) = handler_handles.remove(&id) {
                    settled_handlers.insert(id, key);
                }
                ensure!(
                    error.is_cancelled(),
                    "real handler panicked during shutdown: {error}"
                );
            }
        }
    }
    Ok(())
}

async fn join_handler_until(
    handlers: &mut JoinSet<(String, ())>,
    handler_handles: &mut HashMap<Id, (String, tokio::task::AbortHandle)>,
    settled_handlers: &mut HashMap<Id, String>,
    target: &str,
    target_id: Option<Id>,
) -> anyhow::Result<()> {
    loop {
        let Some(done) = handlers.join_next_with_id().await else {
            return Err(anyhow!(
                "handler task disappeared before settlement: {target}"
            ));
        };
        match done {
            Ok((id, (key, ()))) => {
                handler_handles.remove(&id);
                settled_handlers.insert(id, key);
                if target_id == Some(id) {
                    settled_handlers.remove(&id);
                    return Ok(());
                }
            }
            Err(error) if error.is_cancelled() => {
                let id = error.id();
                if let Some((key, _)) = handler_handles.remove(&id) {
                    settled_handlers.insert(id, key);
                }
                if target_id == Some(id) {
                    settled_handlers.remove(&id);
                    return Ok(());
                }
            }
            Err(error) => return Err(anyhow!("real handler task panicked: {error}")),
        }
    }
}

async fn trusted_caller<F: Future>(user: &str, future: F) -> F::Output {
    use alephcore::gateway::caller_identity::{
        CALLER_CONN_ID, CALLER_IS_LOOPBACK, CALLER_ROLE, CALLER_USER, CALLER_VERIFIED_USER,
    };
    // These public task-locals are the real handler inputs, set from trusted
    // fixture data rather than caller-controlled request parameters.
    with_scope(
        Some(ScopeAttribution::personal(user)),
        CALLER_USER.scope(
            Some(user.into()),
            CALLER_ROLE.scope(
                Some("member".into()),
                CALLER_IS_LOOPBACK.scope(
                    false,
                    CALLER_CONN_ID.scope(
                        Some("readiness-connection".into()),
                        CALLER_VERIFIED_USER.scope(Some(user.into()), future),
                    ),
                ),
            ),
        ),
    )
    .await
}

// Every non-gated trait operation forwards to the same real backend, including
// default methods whose implementation carries backend-specific behavior.
#[async_trait]
impl SessionStore for BarrierStore {
    async fn get_or_create(&self, key: &SessionKey) -> Result<SessionMetadata, SessionStoreError> {
        let rpc = RPC_HANDLER.try_with(|()| &self.active_rpc).ok();
        let _call = StoreCall::new(&self.active, rpc);
        self.entries.send_modify(|count| *count += 1);
        if let Some(message) = self.fail_next.lock().expect("failure injection").take() {
            return Err(SessionStoreError::DatabaseError(message));
        }
        self.prepare
            .subscribe()
            .wait_for(|open| *open)
            .await
            .expect("fixture owns pre-delegate gate");
        let metadata = self.inner.get_or_create(key).await?;
        self.metadata
            .lock()
            .expect("committed metadata observations")
            .push(metadata.clone());
        self.commits.send_modify(|count| *count += 1);
        self.committed_prepare
            .subscribe()
            .wait_for(|open| *open)
            .await
            .expect("fixture owns post-commit gate");
        Ok(metadata)
    }

    async fn get_metadata(
        &self,
        key: &SessionKey,
    ) -> Result<Option<SessionMetadata>, SessionStoreError> {
        self.inner.get_metadata(key).await
    }

    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMetadata>, SessionStoreError> {
        self.inner.list_sessions(filter).await
    }

    async fn delete_session(&self, key: &SessionKey) -> Result<DeleteResult, SessionStoreError> {
        self.inner.delete_session(key).await
    }

    async fn reset_session(&self, key: &SessionKey) -> Result<bool, SessionStoreError> {
        self.inner.reset_session(key).await
    }

    async fn append_message(
        &self,
        key: &SessionKey,
        msg: MessageRecord,
    ) -> Result<(), SessionStoreError> {
        self.inner.append_message(key, msg).await
    }

    async fn get_history(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
    ) -> Result<Vec<MessageRecord>, SessionStoreError> {
        self.inner.get_history(key, limit).await
    }

    async fn history_page(
        &self,
        key: &SessionKey,
        read: HistoryRead,
    ) -> Result<HistoryPage, SessionStoreError> {
        self.inner.history_page(key, read).await
    }

    async fn search_messages(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchHit>, SessionStoreError> {
        self.inner.search_messages(query, max_results).await
    }

    async fn truncate_messages(
        &self,
        key: &SessionKey,
        keep_count: usize,
    ) -> Result<TruncateResult, SessionStoreError> {
        self.inner.truncate_messages(key, keep_count).await
    }

    async fn delete_messages_from_seq(
        &self,
        key: &SessionKey,
        from_seq: u64,
    ) -> Result<usize, SessionStoreError> {
        self.inner.delete_messages_from_seq(key, from_seq).await
    }

    async fn backfill_attribution(
        &self,
        key: &SessionKey,
        owner_user_id: &str,
        scope_id: &str,
    ) -> Result<bool, SessionStoreError> {
        self.inner
            .backfill_attribution(key, owner_user_id, scope_id)
            .await
    }

    async fn rescope_attribution(
        &self,
        key: &SessionKey,
        project_id: &str,
    ) -> Result<bool, SessionStoreError> {
        self.inner.rescope_attribution(key, project_id).await
    }

    async fn list_checkpoints(
        &self,
        key: &SessionKey,
    ) -> Result<Vec<CheckpointSummary>, SessionStoreError> {
        self.inner.list_checkpoints(key).await
    }

    async fn branch_from_checkpoint(
        &self,
        key: &SessionKey,
        checkpoint_id: &str,
        new_key: &SessionKey,
    ) -> Result<SessionMetadata, SessionStoreError> {
        self.inner
            .branch_from_checkpoint(key, checkpoint_id, new_key)
            .await
    }

    async fn restore_checkpoint(
        &self,
        key: &SessionKey,
        checkpoint_id: &str,
    ) -> Result<SessionMetadata, SessionStoreError> {
        self.inner.restore_checkpoint(key, checkpoint_id).await
    }

    async fn close_session(
        &self,
        key: &SessionKey,
        topic: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        self.inner.close_session(key, topic).await
    }

    async fn set_topic(&self, key: &SessionKey, topic: &str) -> Result<(), SessionStoreError> {
        self.inner.set_topic(key, topic).await
    }

    async fn set_source_channel(
        &self,
        key: &SessionKey,
        channel: &str,
        conversation: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        self.inner
            .set_source_channel(key, channel, conversation)
            .await
    }

    async fn set_project_root(
        &self,
        key: &SessionKey,
        project_root: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        self.inner.set_project_root(key, project_root).await
    }

    async fn set_state(
        &self,
        key: &SessionKey,
        state: alephcore::gateway::session_manager::SessionState,
    ) -> Result<(), SessionStoreError> {
        self.inner.set_state(key, state).await
    }

    async fn get_state(
        &self,
        key: &SessionKey,
    ) -> Result<alephcore::gateway::session_manager::SessionState, SessionStoreError> {
        self.inner.get_state(key).await
    }

    async fn get_total_tokens(&self, key: &SessionKey) -> Result<Option<u64>, SessionStoreError> {
        self.inner.get_total_tokens(key).await
    }

    fn transcript_file(&self, key: &SessionKey) -> Option<std::path::PathBuf> {
        self.inner.transcript_file(key)
    }

    async fn get_identity_context(
        &self,
        session_key: &str,
        source_channel: &str,
    ) -> Result<aleph_protocol::IdentityContext, SessionStoreError> {
        self.inner
            .get_identity_context(session_key, source_channel)
            .await
    }

    async fn get_current_epoch(&self, base_key_pattern: &str) -> Result<u32, SessionStoreError> {
        self.inner.get_current_epoch(base_key_pattern).await
    }

    async fn reserve_next_epoch(&self, _base_key_pattern: &str) -> Result<u32, SessionStoreError> {
        self.inner.reserve_next_epoch(_base_key_pattern).await
    }

    async fn get_session_topic(
        &self,
        key: &SessionKey,
    ) -> Result<Option<String>, SessionStoreError> {
        self.inner.get_session_topic(key).await
    }

    async fn cleanup_expired(&self) -> Result<usize, SessionStoreError> {
        self.inner.cleanup_expired().await
    }

    async fn reap_task_sessions(
        &self,
        task_type: &str,
        cutoff_secs: i64,
    ) -> Result<usize, SessionStoreError> {
        self.inner.reap_task_sessions(task_type, cutoff_secs).await
    }

    async fn stamp_and_bill_in_range(
        &self,
        key: &SessionKey,
        after_seq: u64,
        before_seq: u64,
        metadata: &serde_json::Value,
        bill: Option<&RunBill>,
    ) -> Result<StampOutcome, SessionStoreError> {
        self.inner
            .stamp_and_bill_in_range(key, after_seq, before_seq, metadata, bill)
            .await
    }

    async fn patch_session(
        &self,
        key: &SessionKey,
        patch: &SessionPatch,
    ) -> Result<bool, SessionStoreError> {
        self.inner.patch_session(key, patch).await
    }

    async fn patch_session_if_model_pin_generation_newer(
        &self,
        key: &SessionKey,
        patch: &SessionPatch,
        _generation: u64,
    ) -> Result<bool, SessionStoreError> {
        self.inner
            .patch_session_if_model_pin_generation_newer(key, patch, _generation)
            .await
    }

    async fn get_session_preview(
        &self,
        key: &SessionKey,
        message_limit: usize,
    ) -> Result<SessionPreview, SessionStoreError> {
        self.inner.get_session_preview(key, message_limit).await
    }

    async fn set_idle(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.inner.set_idle(key).await
    }

    async fn load_window(
        &self,
        agent_id: &str,
        session_id: &str,
        max_turns: usize,
        max_chars: usize,
    ) -> Result<Vec<(String, String)>, SessionStoreError> {
        self.inner
            .load_window(agent_id, session_id, max_turns, max_chars)
            .await
    }
}

#[async_trait]
impl alephcore::tools::service::ToolService for EmptyTools {
    async fn execute(
        &self,
        name: &str,
        _input: Value,
    ) -> Result<alephcore::tools::service::ToolOutput, alephcore::tools::service::ToolError> {
        self.0.tools.fetch_add(1, Ordering::SeqCst);
        Err(alephcore::tools::service::ToolError::NotFound { name: name.into() })
    }

    async fn list(&self) -> Vec<alephcore::tools::service::ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<alephcore::tools::service::ToolDefinition> {
        None
    }
    fn metadata_schema(&self) -> Arc<[alephcore::tool_metadata::ToolDefinition]> {
        Arc::from(Vec::new())
    }
}

fn install_orchestrator(engine: &Engine, effects: Arc<Effects>) -> anyhow::Result<()> {
    use alephcore::orchestrator::{
        build_sandbox_factory, AgentHarnessRunner, BrainRef, FlowOverrides, FlowRegistry, FlowSet,
        FlowSpec, Orchestrator, SessionStrategy,
    };
    use alephcore::session::in_process::InProcessActorSessionService;
    use alephcore::session::service::SessionService;
    use alephcore::session::store::{
        migrate_add_session_events, SessionEventStore, SqliteEventStore,
    };

    let connection = rusqlite::Connection::open_in_memory()?;
    migrate_add_session_events(&connection)?;
    let events: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(connection));
    let session_service: Arc<dyn SessionService> =
        Arc::new(InProcessActorSessionService::new(events));
    let agent_registry = Arc::new(alephcore::agents::AgentRegistry::with_builtins());
    let runner = Arc::new(AgentHarnessRunner {
        agent_registry: agent_registry.clone(),
        session_service: session_service.clone(),
        tool_service: Arc::new(EmptyTools(effects.clone())),
        tool_descriptor_lookup: None,
        default_provider: Arc::new(alephcore::providers::StaticDefault::new(Arc::new(
            LocalProvider(effects),
        ))),
        named_providers: HashMap::new(),
        verifier_chain: None,
        context_budget_config: None,
        context_budget_refiner: None,
        skill_system: None,
        guardrails: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        default_max_iterations: 2,
        power: None,
        memory_context_provider: None,
        raw_memory_store: None,
        memory_project_scoped: false,
        tool_catalog: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        cheap_provider: None,
        default_prompt_mode: Default::default(),
        prompt_extra_files: None,
        mcp_handle: None,
        parallel_tool_concurrency: None,
        primary_context_window: None,
        estimate_overhead_cache: Arc::new(
            alephcore::orchestrator::harness_bridge::context_estimate::OverheadCache::default(),
        ),
        response_language: None,
    });
    let mut flows = FlowSet::new();
    flows.insert(
        "readiness".into(),
        Arc::new(FlowSpec {
            id: "readiness".into(),
            description: "local readiness fixture".into(),
            agent: "main".into(),
            brain: BrainRef::Default,
            session_strategy: SessionStrategy::Fresh,
            overrides: FlowOverrides::default(),
        }),
    );
    let defaults = HashMap::from([("main".into(), "readiness".into())]);
    let sandbox_factory = build_sandbox_factory(Arc::new(|_| {
        Ok(Arc::new(alephcore::sandbox::NoopSandbox) as Arc<dyn alephcore::sandbox::Sandbox>)
    }));
    let orchestrator = Orchestrator::new(
        Arc::new(FlowRegistry::new(flows)),
        Arc::new(defaults),
        session_service,
        sandbox_factory,
        runner,
    )
    .with_agent_registry(agent_registry);
    engine
        .orchestrator_cell()
        .set(Arc::new(orchestrator))
        .map_err(|_| anyhow!("fixture orchestrator already installed"))
}
