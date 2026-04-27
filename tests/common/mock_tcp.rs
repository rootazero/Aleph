use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

pub struct MockTcpServer {
    listener: TcpListener,
}

impl MockTcpServer {
    pub async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        Self { listener }
    }

    pub fn addr(&self) -> String {
        self.listener.local_addr().unwrap().to_string()
    }

    pub async fn accept(&self) -> MockTcpConnection {
        let (stream, _) = self.listener.accept().await.unwrap();
        MockTcpConnection::new(stream)
    }
}

pub struct MockTcpConnection {
    stream: TcpStream,
}

impl MockTcpConnection {
    fn new(stream: TcpStream) -> Self {
        Self { stream }
    }

    pub async fn read_line(&mut self) -> Option<String> {
        let mut buf = [0u8; 4096];
        match self.stream.try_read(&mut buf) {
            Ok(n) if n > 0 => {
                let text = String::from_utf8_lossy(&buf[..n]);
                Some(text.trim_end().to_string())
            }
            _ => None,
        }
    }

    pub async fn send_line(&mut self, line: &str) {
        let _ = self
            .stream
            .write_all(format!("{}\r\n", line).as_bytes())
            .await;
    }
}
