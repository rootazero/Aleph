#[cfg(target_os = "macos")]
fn start_gateway_probe(
    client: aleph_client::GatewayClient,
) -> tokio::sync::oneshot::Receiver<String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let version = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(async move {
                match client.call::<serde_json::Value>("system.info", None).await {
                    Ok(info) => info["version"].as_str().unwrap_or("unknown").to_owned(),
                    Err(error) => format!("error: {error}"),
                }
            }),
            Err(error) => format!("error: tokio runtime: {error}"),
        };
        // The receiver is gone if the app was closed before the probe finished.
        let _ = sender.send(version);
    });
    receiver
}

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "macos")]
    {
        use nsh_core::prelude::*;
        use nsh_platform_macos::{run_app, AppOptions, AppSource, Root};

        #[component]
        fn App() -> impl IntoElement {
            let version = signal("connecting...".to_string());
            let receiver = start_gateway_probe(aleph_client::GatewayClient::new());
            // Await on NSH's engine thread; only that thread writes its signal.
            spawn(async move {
                version.set(match receiver.await {
                    Ok(version) => version,
                    Err(error) => format!("error: gateway worker: {error}"),
                });
            });
            text(move || format!("aleph-server {}", version.get()))
        }

        let root = Root::of::<App>();
        run_app(
            AppSource::Rust(root),
            AppOptions {
                title: "Aleph NSH Spike".to_string(),
                size: (800, 600),
                app_id: "com.aleph.nsh-spike".to_string(),
                ..AppOptions::default()
            },
        )
    }
    #[cfg(not(target_os = "macos"))]
    std::process::ExitCode::SUCCESS
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{json, Value};
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    #[tokio::test]
    async fn worker_delivers_version_even_when_response_takes_longer_than_startup() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let connect = socket.next().await.unwrap().unwrap();
            let connect: Value = serde_json::from_str(connect.to_text().unwrap()).unwrap();
            assert_eq!(connect["method"], "connect");
            assert_eq!(connect["params"]["device_name"], "aleph-cli");
            assert_eq!(connect["params"]["channel_kind"], "cli");
            let request = socket.next().await.unwrap().unwrap();
            let request: Value = serde_json::from_str(request.to_text().unwrap()).unwrap();
            assert_eq!(request["method"], "system.info");
            tokio::time::sleep(Duration::from_millis(200)).await;
            let response = json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": { "version": "26.11.5-spike" }
            });
            socket
                .send(Message::Text(response.to_string().into()))
                .await
                .unwrap();
        });
        let client = aleph_client::GatewayClient::new().with_url(&format!("ws://{address}/ws"));

        let version = tokio::time::timeout(Duration::from_secs(5), start_gateway_probe(client))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(version, "26.11.5-spike");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn worker_delivers_connection_error_instead_of_remaining_connecting() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            // Close the TCP stream before the WebSocket handshake completes.
            drop(stream);
        });
        let client = aleph_client::GatewayClient::new().with_url(&format!("ws://{address}/ws"));

        let version = tokio::time::timeout(Duration::from_secs(5), start_gateway_probe(client))
            .await
            .unwrap()
            .unwrap();

        assert!(version.starts_with("error: "), "{version}");
        assert!(!version.contains("connecting"), "{version}");
        server.await.unwrap();
    }
}
