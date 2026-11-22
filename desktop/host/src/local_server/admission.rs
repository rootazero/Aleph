//! Compatibility admission only; HTTP documents do not authenticate the peer.

use aleph_protocol::gateway_probe::{Health, Readiness, ReadinessPhase};
use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;

use super::{port_open, DAEMON_HOST, POLL_INTERVAL, PROBE_TIMEOUT, READY_TIMEOUT};

const MAX_DOCUMENT_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admission {
    Free,
    Ready,
    Booting,
    Incompatible,
}

fn loopback_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(PROBE_TIMEOUT)
        .build()
}

async fn probe(client: &Client, port: u16) -> Admission {
    match port_open(port).await {
        Some(false) => Admission::Free,
        Some(true) => read_admission(client, port)
            .await
            .unwrap_or(Admission::Incompatible),
        None => Admission::Incompatible,
    }
}

const INCOMPATIBLE: &str = "cannot establish compatible local Gateway readiness";

pub(super) async fn ensure(
    port: u16,
    start: impl AsyncFnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let client = loopback_client().map_err(|_| "could not initialize local Gateway probe")?;
    match probe(&client, port).await {
        Admission::Ready => return Ok(()),
        Admission::Incompatible => return Err(INCOMPATIBLE.to_owned()),
        Admission::Free => start().await?,
        Admission::Booting => {}
    }
    tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            match probe(&client, port).await {
                Admission::Ready => return Ok(()),
                Admission::Incompatible => return Err(INCOMPATIBLE.to_owned()),
                Admission::Free | Admission::Booting => tokio::time::sleep(POLL_INTERVAL).await,
            }
        }
    })
    .await
    .map_err(|_| "compatible local Gateway did not become ready in time".to_owned())?
}

async fn read_admission(client: &Client, port: u16) -> Option<Admission> {
    let (health_status, health) = document::<Health>(client, port, "/health").await?;
    if health_status != StatusCode::OK
        || health.version.trim().is_empty()
        || health.instance_id.trim().is_empty()
        || health.uptime_secs < 0
    {
        return None;
    }
    let (status, readiness) = document::<Readiness>(client, port, "/ready").await?;
    if readiness.version != health.version {
        return None;
    }
    match (status, readiness.ready, readiness.phase) {
        (StatusCode::OK, true, ReadinessPhase::Complete) => Some(Admission::Ready),
        (StatusCode::SERVICE_UNAVAILABLE, false, ReadinessPhase::Booting) => {
            Some(Admission::Booting)
        }
        _ => None,
    }
}

async fn document<T: DeserializeOwned>(
    client: &Client,
    port: u16,
    path: &str,
) -> Option<(StatusCode, T)> {
    let mut response = client
        .get(format!("http://{DAEMON_HOST}:{port}{path}"))
        .send()
        .await
        .ok()?;
    let status = response.status();
    if !matches!(status, StatusCode::OK | StatusCode::SERVICE_UNAVAILABLE)
        || response
            .content_length()
            .is_some_and(|length| length > MAX_DOCUMENT_BYTES as u64)
    {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if chunk.len() > MAX_DOCUMENT_BYTES - body.len() {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    Some((status, serde_json::from_slice(&body).ok()?))
}

#[cfg(test)]
mod tests {
    use super::{ensure, loopback_client, probe, Admission};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    const HEALTH: &str = r#"{"status":"ok","version":"1.2.3","instance_id":"test-id","started_at_unix":123,"uptime_secs":3}"#;
    const READY: &str = r#"{"ready":true,"phase":"complete","version":"1.2.3"}"#;
    const BOOTING: &str = r#"{"ready":false,"phase":"booting","version":"1.2.3"}"#;

    async fn observe(health: String, readiness: String) -> Admission {
        with_listener(health, [readiness], async |port| {
            probe(&loopback_client().unwrap(), port).await
        })
        .await
    }

    async fn with_listener<T, const N: usize>(
        health: String,
        readiness: [String; N],
        use_listener: impl AsyncFnOnce(u16) -> T,
    ) -> T {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let mut ready_reads = 0;
            loop {
                let (peer, _) = listener.accept().await.unwrap();
                let mut peer = BufReader::new(peer);
                let mut request = String::new();
                if peer.read_line(&mut request).await.unwrap() == 0 {
                    continue;
                }
                let response = match request.split_whitespace().nth(1) {
                    Some("/health") => &health,
                    Some("/ready") => {
                        let response = &readiness[ready_reads.min(N - 1)];
                        ready_reads += 1;
                        response
                    }
                    _ => panic!("unexpected probe request: {request:?}"),
                };
                loop {
                    let mut header = String::new();
                    let n = peer.read_line(&mut header).await.unwrap();
                    assert!(n > 0);
                    if header == "\r\n" {
                        break;
                    }
                }
                let _ = peer.get_mut().write_all(response.as_bytes()).await;
            }
        });
        let result = use_listener(port).await;
        server.abort();
        let _ = server.await;
        result
    }

    fn reply(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn compatible_documents_distinguish_ready_from_booting() {
        for (status, body, expected) in [
            ("200 OK", READY, Admission::Ready),
            ("503 Service Unavailable", BOOTING, Admission::Booting),
        ] {
            assert_eq!(
                observe(reply("200 OK", HEALTH), reply(status, body)).await,
                expected
            );
        }
    }

    #[tokio::test]
    async fn booting_listener_becomes_ready_without_launch() {
        with_listener(
            reply("200 OK", HEALTH),
            [
                reply("503 Service Unavailable", BOOTING),
                reply("200 OK", READY),
            ],
            async |port| {
                ensure(port, async || panic!("existing listener was replaced"))
                    .await
                    .unwrap();
            },
        )
        .await;
    }

    #[tokio::test]
    #[ignore = "requires an already running QA-owned production Gateway on a non-default loopback port"]
    async fn accepts_owned_production_gateway_without_launch() {
        let port = std::env::var("ALEPH_GATEWAY_ADMISSION_QA_PORT")
            .unwrap()
            .parse::<u16>()
            .unwrap();
        assert!(port != 0 && port != super::super::DAEMON_PORT);
        ensure(port, async || panic!("the QA Gateway was replaced"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn unknown_listener_is_refused_without_launch() {
        with_listener(
            reply("200 OK", "{}"),
            [reply("200 OK", READY)],
            async |port| {
                assert!(
                    ensure(port, async || panic!("unknown listener was replaced"))
                        .await
                        .is_err()
                );
            },
        )
        .await;
    }

    #[tokio::test]
    async fn same_status_is_not_enough_and_errors_fail_closed() {
        for (health, ready) in [
            (reply("200 OK", "{}"), reply("200 OK", READY)),
            (reply("200 OK", HEALTH), reply("200 OK", "{}")),
            (reply("200 OK", HEALTH), reply("200 OK", BOOTING)),
            (
                reply("200 OK", HEALTH),
                reply("503 Service Unavailable", READY),
            ),
            (
                reply("200 OK", HEALTH),
                reply("200 OK", &READY.replace("complete", "booting")),
            ),
            (
                reply("200 OK", HEALTH),
                reply("200 OK", &READY.replace("1.2.3", "2.3.4")),
            ),
            (
                reply("200 OK", &HEALTH.replace("test-id", " ")),
                reply("200 OK", READY),
            ),
            (
                reply("200 OK", HEALTH),
                reply("200 OK", &format!("{READY}{}", " ".repeat(4096))),
            ),
            (
                reply("200 OK", HEALTH),
                "HTTP/1.1 200 OK\r\nContent-Length: 300\r\nConnection: close\r\n\r\n{}".to_owned(),
            ),
        ] {
            assert_eq!(observe(health, ready).await, Admission::Incompatible);
        }
    }

    #[tokio::test]
    async fn chunked_body_is_bounded_without_a_content_length() {
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        let valid = format!("{head}{:x}\r\n{READY}\r\n0\r\n\r\n", READY.len());
        assert_eq!(
            observe(reply("200 OK", HEALTH), valid).await,
            Admission::Ready
        );
        let oversized = format!(
            "{head}2000\r\n{READY}{}\r\n0\r\n\r\n",
            " ".repeat(8192 - READY.len())
        );
        assert_eq!(
            observe(reply("200 OK", HEALTH), oversized).await,
            Admission::Incompatible
        );
    }
}
