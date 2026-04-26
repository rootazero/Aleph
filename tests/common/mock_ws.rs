use futures_util::SinkExt;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message};

pub struct MockWebSocket {
    listener: TcpListener,
    addr: SocketAddr,
}

impl MockWebSocket {
    pub async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        Self { listener, addr }
    }

    pub fn uri(&self) -> String {
        format!("ws://{}", self.addr)
    }

    pub async fn accept(&self) -> MockWebSocketConnection {
        let (stream, _) = self.listener.accept().await.unwrap();
        let ws = accept_async(stream).await.unwrap();
        MockWebSocketConnection { ws }
    }
}

pub struct MockWebSocketConnection {
    ws: tokio_tungstenite::WebSocketStream<TcpStream>,
}

impl MockWebSocketConnection {
    pub async fn send_json(&mut self, value: serde_json::Value) {
        self.ws
            .send(Message::Text(value.to_string().into()))
            .await
            .unwrap();
    }

    pub async fn recv_json(&mut self) -> Option<serde_json::Value> {
        use futures_util::StreamExt;
        match self.ws.next().await {
            Some(Ok(Message::Text(text))) => serde_json::from_str(&text).ok(),
            Some(Ok(Message::Binary(bin))) => serde_json::from_slice(&bin).ok(),
            _ => None,
        }
    }
}
