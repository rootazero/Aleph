//! Per-instance observations of actual operations, never production authority.
use super::super::channel::{
    Channel, ChannelError, ChannelInfo, ChannelResult, ChannelState, OutboundMessage, SendOperation,
};
use crate::gateway::interfaces::webhook::WebhookChannel;
use crate::sync_primitives::{Arc, Mutex};
use async_trait::async_trait;
use tokio::sync::oneshot;

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
        self.0.lock().unwrap().push(event);
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
