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
    reader: BufReader<tokio::net::tcp::ReadHalf>,
    writer: tokio::net::tcp::WriteHalf,
}

impl MockTcpConnection {
    fn new(stream: TcpStream) -> Self {
        let (reader, writer) = stream.split();
        Self {
            reader: BufReader::new(reader),
            writer,
        }
    }

    pub async fn read_line(&mut self) -> Option<String> {
        let mut line = String::new();
        match self.reader.read_line(&mut line).await {
            Ok(0) => None,
            Ok(_) => Some(line.trim_end().to_string()),
            Err(_) => None,
        }
    }

    pub async fn send_line(&mut self, line: &str) {
        self.writer
            .write_all(format!("{}\r\n", line).as_bytes())
            .await
            .unwrap();
    }
}
