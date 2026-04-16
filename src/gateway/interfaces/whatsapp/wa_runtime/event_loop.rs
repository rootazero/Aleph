use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::gateway::interfaces::whatsapp::wa_runtime::state::{AtomicConnectionState, ConnectionState};
use whatsapp_rust::types::events::{Event, EventHandler};

struct ChannelEventHandler {
    tx: mpsc::Sender<Event>,
}

impl ChannelEventHandler {
    fn new(tx: mpsc::Sender<Event>) -> Self {
        Self { tx }
    }
}

impl EventHandler for ChannelEventHandler {
    fn handle_event(&self, event: &Event) {
        let tx = self.tx.clone();
        let event = event.clone();
        tokio::spawn(async move {
            if tx.send(event).await.is_err() {
                debug!("event receiver dropped in ChannelEventHandler");
            }
        });
    }
}

pub async fn run_event_loop(
    mut bot: whatsapp_rust::bot::Bot,
    event_tx: mpsc::Sender<Event>,
    state: Arc<AtomicConnectionState>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    info!("WhatsApp event loop started");

    let (handler_tx, mut handler_rx) = mpsc::channel::<Event>(100);

    let client = bot.client();
    client.register_handler(Arc::new(ChannelEventHandler::new(handler_tx)));

    let bot_handle = match bot.run().await {
        Ok(handle) => handle,
        Err(e) => {
            error!(error = %e, "Failed to start WhatsApp bot");
            state.set(ConnectionState::Error);
            return;
        }
    };

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Received shutdown signal, disconnecting...");
                bot_handle.abort();
                state.set(ConnectionState::Disconnected);
                break;
            }

            event = handler_rx.recv() => {
                match event {
                    Some(event) => {
                        match &event {
                            Event::Connected(_) => {
                                info!("WhatsApp connection opened");
                                state.set(ConnectionState::Connected);
                            }
                            Event::Disconnected(_) => {
                                warn!("WhatsApp connection closed");
                                state.set(ConnectionState::Disconnected);
                            }
                            Event::PairingQrCode { .. } => {
                                debug!("WhatsApp QR code event received");
                            }
                            _ => {
                                debug!(event = ?event, "WhatsApp event received");
                            }
                        }

                        if event_tx.send(event).await.is_err() {
                            error!("Event receiver dropped, stopping event loop");
                            break;
                        }
                    }
                    None => {
                        warn!("Bot event stream ended");
                        state.set(ConnectionState::Disconnected);
                        break;
                    }
                }
            }
        }
    }

    state.set(ConnectionState::Disconnected);
    info!("WhatsApp event loop stopped");
}
