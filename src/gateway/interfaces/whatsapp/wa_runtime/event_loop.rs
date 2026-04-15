use tokio::sync::mpsc;

pub async fn run_event_loop(
    _event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => break,
            else => break,
        }
    }
}
