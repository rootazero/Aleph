//! Per-instance observations of actual operations, never production authority.
use super::super::channel::{
    Channel, ChannelError, ChannelInfo, ChannelResult, ChannelState, OutboundMessage, SendOperation,
};
use crate::gateway::interfaces::webhook::WebhookChannel;
use crate::sync_primitives::{Arc, Mutex};
use async_trait::async_trait;
use tokio::sync::oneshot;

pub(super) fn lifecycle_contract() -> super::binding::ChannelLifecycleContract {
    super::binding::ChannelLifecycleContract::new(
        crate::capability::ownership::OwnerRef::Runtime,
        crate::capability::ownership::LifetimeScope::Runtime,
        crate::capability::ownership::VisibilityScope::default(),
        super::binding::ChannelLifecycleDeclarationV1::schema(),
        crate::capability::descriptor::ProjectionMetadata::default(),
    )
    .unwrap()
}

/// Mirror the source claim a Bound registration needs, using the existing
/// registry and ownership tree as the only authorities.
pub(super) async fn register_bound(
    registry: &Arc<super::ChannelRegistry>,
    tree: &Arc<crate::capability::ownership::OwnershipTree>,
    channel: Box<dyn Channel>,
    initial: super::binding::ChannelInitialState,
) -> super::ChannelId {
    let id = registry
        .register_bound_channel(channel, lifecycle_contract(), tree.clone(), initial)
        .await
        .unwrap();
    let capture = registry.capture_bound_channel(&id).await.unwrap();
    tree.register(
        capture.descriptor.id.clone(),
        capture.descriptor.owner.clone(),
        capture.descriptor.lifetime,
        capture.descriptor.visibility.clone(),
    )
    .unwrap();
    id
}

pub(super) struct LifecycleBlock {
    pub(super) entered: oneshot::Sender<()>,
    pub(super) release: oneshot::Receiver<()>,
}

pub(super) struct HeldWebhook {
    pub(super) channel: WebhookChannel,
    pub(super) start: Option<LifecycleBlock>,
    pub(super) stop: Option<LifecycleBlock>,
}

#[async_trait]
impl Channel for HeldWebhook {
    fn info(&self) -> &ChannelInfo {
        self.channel.info()
    }
    fn state(&self) -> &ChannelState {
        self.channel.state()
    }
    async fn start(&mut self) -> ChannelResult<()> {
        if let Some(block) = self.start.take() {
            let _ = block.entered.send(());
            let _ = block.release.await;
        }
        self.channel.start().await
    }
    async fn stop(&mut self) -> ChannelResult<()> {
        if let Some(block) = self.stop.take() {
            let _ = block.entered.send(());
            let _ = block.release.await;
        }
        self.channel.stop().await
    }
    fn send(&self, message: OutboundMessage) -> SendOperation {
        self.channel.send(message)
    }
}

pub(super) struct FailingStopWebhook {
    channel: WebhookChannel,
}

impl FailingStopWebhook {
    pub(super) fn new(id: &str, callback_url: String, probe: ResourceProbe) -> Self {
        Self {
            channel: WebhookChannel::observed(
                id,
                crate::gateway::interfaces::webhook::WebhookChannelConfig {
                    secret: "test-secret".into(),
                    callback_url,
                    ..Default::default()
                },
                probe,
            ),
        }
    }
}

#[async_trait]
impl Channel for FailingStopWebhook {
    fn info(&self) -> &ChannelInfo {
        self.channel.info()
    }
    fn state(&self) -> &ChannelState {
        self.channel.state()
    }
    async fn start(&mut self) -> ChannelResult<()> {
        self.channel.start().await
    }
    async fn stop(&mut self) -> ChannelResult<()> {
        Err(ChannelError::Internal("injected stop failure".into()))
    }
    fn send(&self, message: OutboundMessage) -> SendOperation {
        self.channel.send(message)
    }
}

pub(super) struct HeldFactory(pub(super) Mutex<Option<LifecycleBlock>>);

#[async_trait]
impl super::super::channel::ChannelFactory for HeldFactory {
    fn channel_type(&self) -> &str {
        "held-webhook"
    }
    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        self.create_with_id("held-legacy", config).await
    }
    async fn create_with_id(
        &self,
        id: &str,
        config: serde_json::Value,
    ) -> ChannelResult<Box<dyn Channel>> {
        let block = self.0.lock().unwrap().take();
        if let Some(block) = block {
            let _ = block.entered.send(());
            let _ = block.release.await;
        }
        Ok(Box::new(WebhookChannel::new(
            id,
            serde_json::from_value(config).unwrap(),
        )))
    }
}

/// A real HTTP peer with a held response and an awaited connection close.
pub(super) struct LoopbackPeer {
    pub(super) url: String,
    pub(super) arrived: oneshot::Receiver<Vec<u8>>,
    pub(super) release: oneshot::Sender<()>,
    pub(super) task: tokio::task::JoinHandle<()>,
}

impl LoopbackPeer {
    pub(super) async fn new(status: u16) -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/callback", listener.local_addr().unwrap());
        let (arrived, request) = oneshot::channel();
        let (release, blocked) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            drop(listener);
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let count = socket.read(&mut chunk).await.unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&bytes[..end]);
                    let length = header
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|n| n.trim().parse::<usize>().ok())
                        })
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let _ = arrived.send(bytes);
            let _ = blocked.await;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.shutdown().await.unwrap();
        });
        Self {
            url,
            arrived: request,
            release,
            task,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct ResourceProbe(Arc<Mutex<Vec<&'static str>>>);

impl ResourceProbe {
    pub(crate) fn record(&self, event: &'static str) {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(event);
    }

    pub(crate) fn count(&self, event: &str) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|item| **item == event)
            .count()
    }
}

// --- Polling/socket resource fixture: event vocabulary ---
//
// One name per real boundary, recorded by the fixture and asserted by
// `resource_matrix`. These are test-only observations, never production
// authority; no count here is derived from `ChannelStatus` or a return value.
pub(super) const TCP_CONNECT_ATTEMPT: &str = "tcp_connect_attempt";
pub(super) const TCP_CONNECTED: &str = "tcp_connected";
pub(super) const TCP_DROP: &str = "tcp_drop";
pub(super) const WORKER_SPAWNED: &str = "worker_spawned";
pub(super) const WORKER_EXITED: &str = "worker_exited";
pub(super) const WORKER_JOINED: &str = "worker_joined";
pub(super) const TIMER_CREATED: &str = "timer_created";
pub(super) const TIMER_TICK: &str = "timer_tick";
pub(super) const TIMER_DROP: &str = "timer_drop";
pub(super) const CACHE_CREATED: &str = "cache_created";
pub(super) const CACHE_DROP: &str = "cache_drop";
pub(super) const DATABASE_OPEN_ATTEMPT: &str = "database_open_attempt";
pub(super) const DATABASE_OPENED: &str = "database_opened";
pub(super) const DATABASE_CLOSED: &str = "database_closed";
pub(super) const DATABASE_DROP: &str = "database_drop";
pub(super) const SEND_ATTEMPT: &str = "send_attempt";
pub(super) const SEND_COMPLETED: &str = "send_completed";

/// Receipts for one `PollingSocketChannel` instance.
///
/// `probe` records boundary events, `payloads` keeps the exact attempted
/// payload, and `ticks` publishes the worker's tick count so a test can await a
/// real tick instead of sleeping.
#[derive(Clone)]
pub(super) struct PollingReceipts {
    probe: ResourceProbe,
    payloads: Arc<Mutex<Vec<String>>>,
    ticks: Arc<tokio::sync::watch::Sender<u64>>,
}

impl PollingReceipts {
    pub(super) fn new() -> (Self, tokio::sync::watch::Receiver<u64>) {
        let (ticks, tick_receiver) = tokio::sync::watch::channel(0u64);
        (
            Self {
                probe: ResourceProbe::default(),
                payloads: Arc::new(Mutex::new(Vec::new())),
                ticks: Arc::new(ticks),
            },
            tick_receiver,
        )
    }

    pub(super) fn count(&self, event: &str) -> usize {
        self.probe.count(event)
    }

    pub(super) fn payloads(&self) -> Vec<String> {
        self.payloads.lock().unwrap().clone()
    }
}

/// A real loopback TCP peer that holds one accepted connection open and reports
/// the peer-observed EOF once the channel drops its stream.
pub(super) struct LoopbackSocket {
    pub(super) address: std::net::SocketAddr,
    pub(super) closed: oneshot::Receiver<()>,
    pub(super) task: tokio::task::JoinHandle<()>,
}

impl LoopbackSocket {
    pub(super) async fn new() -> Self {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (closed, eof) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 64];
            while !matches!(socket.read(&mut buffer).await, Ok(0) | Err(_)) {}
            let _ = closed.send(());
        });
        Self {
            address,
            closed: eof,
            task,
        }
    }
}

// --- Real resource guards: one receipt per construction and destruction ---
//
// Each guard takes `Option<T>` so the drop receipt is recorded *after* the
// resource itself is gone, never before. The channel has no implicit release
// path that bypasses the observed provider lifecycle; each guard's own Drop
// records destruction when an owner releases it.

struct OwnedStream {
    stream: Option<tokio::net::TcpStream>,
    probe: ResourceProbe,
}

impl OwnedStream {
    fn new(stream: tokio::net::TcpStream, probe: ResourceProbe) -> Self {
        Self {
            stream: Some(stream),
            probe,
        }
    }

    fn release(&mut self) {
        if let Some(stream) = self.stream.take() {
            drop(stream);
            self.probe.record(TCP_DROP);
        }
    }
}

impl Drop for OwnedStream {
    fn drop(&mut self) {
        self.release();
    }
}

struct OwnedTimer {
    interval: Option<tokio::time::Interval>,
    probe: ResourceProbe,
}

impl OwnedTimer {
    fn new(probe: ResourceProbe) -> Self {
        let interval = tokio::time::interval(std::time::Duration::from_millis(10));
        probe.record(TIMER_CREATED);
        Self {
            interval: Some(interval),
            probe,
        }
    }

    async fn tick(&mut self) {
        if let Some(interval) = self.interval.as_mut() {
            interval.tick().await;
        }
    }

    fn release(&mut self) {
        if let Some(interval) = self.interval.take() {
            drop(interval);
            self.probe.record(TIMER_DROP);
        }
    }
}

impl Drop for OwnedTimer {
    fn drop(&mut self) {
        self.release();
    }
}

/// Provider-implementation cache owned by the fixture, not by `ProviderHandle`.
struct OwnedCache {
    ticks: Option<Vec<u64>>,
    probe: ResourceProbe,
}

impl OwnedCache {
    fn new(probe: ResourceProbe) -> Self {
        let ticks = Vec::new();
        probe.record(CACHE_CREATED);
        Self {
            ticks: Some(ticks),
            probe,
        }
    }

    fn insert(&mut self, tick: u64) {
        if let Some(ticks) = self.ticks.as_mut() {
            ticks.push(tick);
        }
    }

    fn release(&mut self) {
        if let Some(ticks) = self.ticks.take() {
            drop(ticks);
            self.probe.record(CACHE_DROP);
        }
    }
}

impl Drop for OwnedCache {
    fn drop(&mut self) {
        self.release();
    }
}

struct OwnedDatabase {
    connection: Option<rusqlite::Connection>,
    probe: ResourceProbe,
}

impl OwnedDatabase {
    fn new(connection: rusqlite::Connection, probe: ResourceProbe) -> Self {
        probe.record(DATABASE_OPENED);
        Self {
            connection: Some(connection),
            probe,
        }
    }

    /// Records the *result* of `Connection::close`; a failed close keeps the
    /// connection owned so the drop receipt still fires.
    fn close(&mut self) -> rusqlite::Result<()> {
        let Some(connection) = self.connection.take() else {
            return Ok(());
        };
        match connection.close() {
            Ok(()) => {
                self.probe.record(DATABASE_CLOSED);
                Ok(())
            }
            Err((connection, error)) => {
                self.connection = Some(connection);
                Err(error)
            }
        }
    }

    fn release(&mut self) {
        if let Some(connection) = self.connection.take() {
            drop(connection);
            self.probe.record(DATABASE_DROP);
        }
    }
}

impl Drop for OwnedDatabase {
    fn drop(&mut self) {
        self.release();
    }
}

/// A second, independently implementable provider shape.
///
/// Cold construction is configuration-only: no socket, worker, timer, cache or
/// database exists until `start` allocates them and hands them to one worker.
pub(super) struct PollingSocketChannel {
    info: ChannelInfo,
    state: ChannelState,
    peer: std::net::SocketAddr,
    database_path: std::path::PathBuf,
    receipts: PollingReceipts,
    worker: Option<PollingWorker>,
}

struct PollingWorker {
    cancel: tokio_util::sync::CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

impl PollingSocketChannel {
    pub(super) fn new(
        id: &str,
        peer: std::net::SocketAddr,
        database_path: std::path::PathBuf,
        receipts: PollingReceipts,
    ) -> Self {
        Self {
            info: ChannelInfo {
                id: crate::gateway::channel::ChannelId::new(id),
                name: "Polling Socket".into(),
                channel_type: "polling-socket".into(),
                status: crate::gateway::channel::ChannelStatus::Disconnected,
                capabilities: Default::default(),
            },
            state: ChannelState::new(16),
            peer,
            database_path,
            receipts,
            worker: None,
        }
    }

    async fn connect(&self) -> ChannelResult<OwnedStream> {
        self.receipts.probe.record(TCP_CONNECT_ATTEMPT);
        let stream = tokio::net::TcpStream::connect(self.peer)
            .await
            .map_err(|error| {
                ChannelError::NotConnected(format!("polling loopback connect failed: {error}"))
            })?;
        self.receipts.probe.record(TCP_CONNECTED);
        Ok(OwnedStream::new(stream, self.receipts.probe.clone()))
    }

    fn open_database(&self) -> ChannelResult<OwnedDatabase> {
        self.receipts.probe.record(DATABASE_OPEN_ATTEMPT);
        let connection = rusqlite::Connection::open(&self.database_path).map_err(|error| {
            ChannelError::ConfigError(format!("polling sqlite open failed: {error}"))
        })?;
        let database = OwnedDatabase::new(connection, self.receipts.probe.clone());
        database
            .connection
            .as_ref()
            .expect("a newly opened polling database is still owned")
            .execute_batch("CREATE TABLE polling_ticks (tick INTEGER NOT NULL)")
            .map_err(|error| {
                ChannelError::ConfigError(format!("polling sqlite schema failed: {error}"))
            })?;
        Ok(database)
    }
}

#[async_trait]
impl Channel for PollingSocketChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        if self.worker.is_some() {
            return Err(ChannelError::Busy(
                "polling socket is already active".into(),
            ));
        }
        self.state
            .set_status(crate::gateway::channel::ChannelStatus::Connecting)
            .await;

        // Ordered so a later failure can only leave resources that are already
        // guarded and observed: the stream is dropped on the early return.
        let stream = self.connect().await?;
        let database = self.open_database()?;
        let cache = OwnedCache::new(self.receipts.probe.clone());
        let timer = OwnedTimer::new(self.receipts.probe.clone());

        let cancel = tokio_util::sync::CancellationToken::new();
        let handle = tokio::spawn(run_polling_worker(
            stream,
            database,
            cache,
            timer,
            cancel.clone(),
            self.receipts.clone(),
        ));
        self.receipts.probe.record(WORKER_SPAWNED);
        self.worker = Some(PollingWorker { cancel, handle });
        self.state
            .set_status(crate::gateway::channel::ChannelStatus::Connected)
            .await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        let Some(worker) = self.worker.as_mut() else {
            self.state
                .set_status(crate::gateway::channel::ChannelStatus::Disconnected)
                .await;
            return Ok(());
        };
        worker.cancel.cancel();
        // Keep the handle in `self` until the join succeeds. If this future is
        // cancelled, a later stop can await the same worker instead of
        // detaching it and falsely reporting completion.
        (&mut worker.handle).await.map_err(|error| {
            ChannelError::Internal(format!("polling worker join failed: {error}"))
        })?;
        self.worker.take();
        self.receipts.probe.record(WORKER_JOINED);
        self.state
            .set_status(crate::gateway::channel::ChannelStatus::Disconnected)
            .await;
        Ok(())
    }

    fn send(&self, message: OutboundMessage) -> SendOperation {
        let receipts = self.receipts.clone();
        let state = self.state.clone();
        let channel_id = self.info.id.0.clone();
        Box::pin(async move {
            if state.status() != crate::gateway::channel::ChannelStatus::Connected {
                return Err(ChannelError::NotConnected(
                    "polling socket is not active".into(),
                ));
            }
            receipts.probe.record(SEND_ATTEMPT);
            receipts.payloads.lock().unwrap().push(message.text.clone());
            receipts.probe.record(SEND_COMPLETED);
            Ok(crate::gateway::channel::SendResult {
                message_id: crate::gateway::channel::MessageId::new(format!(
                    "{channel_id}:{}",
                    message.conversation_id.0
                )),
                timestamp: chrono::Utc::now(),
            })
        })
    }
}

/// The one worker that owns the stream, database, cache, timer and cancellation
/// receiver. It ticks until cancelled, then closes SQLite explicitly, releases
/// every owned resource and records its own exit before returning to `stop`,
/// which owns the tracked join.
async fn run_polling_worker(
    mut stream: OwnedStream,
    mut database: OwnedDatabase,
    mut cache: OwnedCache,
    mut timer: OwnedTimer,
    cancel: tokio_util::sync::CancellationToken,
    receipts: PollingReceipts,
) {
    let mut tick = 0u64;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = timer.tick() => {
                tick += 1;
                cache.insert(tick);
                receipts.probe.record(TIMER_TICK);
                let _ = receipts.ticks.send(tick);
            }
        }
    }
    let _ = database.close();
    stream.release();
    cache.release();
    timer.release();
    receipts.probe.record(WORKER_EXITED);
}
