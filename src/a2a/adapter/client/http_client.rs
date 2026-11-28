use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;

use super::protocol::{decode_stream_result, decode_task, discover_urls, resolve_card};
use super::sse_stream::{parse_sse_frames, parse_sse_response};
use crate::a2a::domain::{A2AError, A2AMessage, A2ATask, AgentCard, UpdateEvent};
use crate::a2a::port::{
    append_path, http_url, legacy_endpoint, A2AResult, RegisteredAgent, ResolvedEndpoint,
    RpcProtocol,
};
use crate::security::ssrf::{validate_url_with_pinned, SsrfPolicy};

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const STREAM_OPEN_TIMEOUT: Duration = Duration::from_secs(15);

type EventStream = Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>>;

/// Reuses connections for a registration while pinning each validated DNS result.
pub struct A2AClient {
    http: reqwest::Client,
    pinned_http: Mutex<Option<(String, SocketAddr, reqwest::Client)>>,
    base_url: String,
    auth_token: Option<String>,
    agent_id: String,
    endpoint: Option<ResolvedEndpoint>,
    pinned_rpc_url: Option<String>,
    timeout: Duration,
    policy: SsrfPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    NotSent,
    Unknown,
    ResponseReceived,
}

#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct DispatchError {
    pub delivery: DeliveryStatus,
    #[source]
    pub error: A2AError,
}

impl DispatchError {
    fn unknown(error: A2AError) -> Self {
        Self {
            delivery: DeliveryStatus::Unknown,
            error,
        }
    }

    fn rejected(error: A2AError) -> Self {
        Self {
            delivery: DeliveryStatus::ResponseReceived,
            error,
        }
    }
}

pub(crate) struct PreparedDelegation {
    client: reqwest::Client,
    request: reqwest::Request,
    id: String,
    endpoint: ResolvedEndpoint,
    timeout: Duration,
}

pub(crate) enum DelegationResponse {
    Unary(A2ATask),
    Streaming(EventStream),
}

#[derive(Deserialize)]
struct JsonRpcErrorData {
    code: i64,
    message: String,
}

fn http_builder(policy: &SsrfPolicy) -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never());
    if policy.use_system_proxy {
        builder
    } else {
        builder.no_proxy()
    }
}

impl A2AClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::with_ssrf_policy(base_url, SsrfPolicy::default())
    }

    pub fn with_ssrf_policy(base_url: impl Into<String>, policy: SsrfPolicy) -> Self {
        let http = http_builder(&policy)
            .build()
            .expect("reqwest ClientBuilder configuration is static and must succeed");
        let base_url = base_url.into();
        Self {
            http,
            pinned_http: Mutex::new(None),
            endpoint: legacy_endpoint(&base_url, false).ok(),
            base_url,
            auth_token: None,
            agent_id: String::new(),
            pinned_rpc_url: None,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            policy,
        }
    }

    pub fn with_auth(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        let mut client = Self::new(base_url);
        client.auth_token = Some(token.into());
        client
    }

    pub(crate) fn for_agent(agent: &RegisteredAgent, policy: SsrfPolicy) -> Self {
        let mut client = Self::with_ssrf_policy(&agent.base_url, policy);
        client.agent_id = agent.card.id.clone();
        client.auth_token = agent.auth_token().map(str::to_owned);
        client.endpoint = agent.endpoint().cloned();
        client.pinned_rpc_url = agent.pinned_rpc_url().map(str::to_owned);
        client
    }

    pub(crate) fn matches(&self, agent: &RegisteredAgent) -> bool {
        self.base_url == agent.base_url
            && self.auth_token.as_deref() == agent.auth_token()
            && self.endpoint.as_ref() == agent.endpoint()
            && self.pinned_rpc_url.as_deref() == agent.pinned_rpc_url()
    }

    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }

    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    async fn client_for_url(&self, url: &Url) -> A2AResult<reqwest::Client> {
        let (_, pinned) = validate_url_with_pinned(url.as_str(), &self.policy)
            .await
            .map_err(|e| {
                A2AError::AgentUnreachable(format!("A2A URL blocked by network policy: {e}"))
            })?;
        let Some(address) = pinned.filter(|_| !self.policy.use_system_proxy) else {
            return Ok(self.http.clone());
        };
        let host = url.host_str().expect("validated A2A URL has a host");
        let mut cache = self
            .pinned_http
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((cached_host, cached_address, client)) = cache.as_ref() {
            if cached_host == host && *cached_address == address {
                return Ok(client.clone());
            }
        }
        let client = http_builder(&self.policy)
            .resolve(host, address)
            .build()
            .map_err(|e| A2AError::InternalError(e.without_url().to_string()))?;
        *cache = Some((host.to_owned(), address, client.clone()));
        Ok(client)
    }

    fn request_auth(
        &self,
        request: reqwest::RequestBuilder,
        url: &Url,
    ) -> A2AResult<reqwest::RequestBuilder> {
        let origin = http_url(&self.base_url)?.origin();
        let pin = self.pinned_rpc_url.as_deref().map(http_url).transpose()?;
        if origin != url.origin() && pin.as_ref() != Some(url) {
            return Err(A2AError::InvalidParams(
                "A2A endpoint is outside the configured origin".into(),
            ));
        }
        Ok(match &self.auth_token {
            Some(token) => request.bearer_auth(token),
            None => request,
        })
    }

    /// Fetch and select the card's endpoint, then validate the selected destination.
    /// A configured root with no discovery route can still use the legacy unary API.
    pub(crate) async fn discover(
        &self,
        fallback: Option<&AgentCard>,
    ) -> A2AResult<(AgentCard, ResolvedEndpoint)> {
        let base = http_url(&self.base_url)?;
        let pin = self.pinned_rpc_url.as_deref().map(http_url).transpose()?;
        let mut legacy_root = None;
        for url in discover_urls(&base) {
            if url == append_path(&base, ".well-known/agent-card.json") {
                legacy_root = Some(&base);
            }
            let client = self.client_for_url(&url).await?;
            let response = self
                .request_auth(client.get(url.clone()), &url)?
                .timeout(self.timeout)
                .send()
                .await
                .map_err(transport_error)?;
            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                continue;
            }
            if !status.is_success() {
                return Err(A2AError::AgentUnreachable(format!(
                    "A2A discovery returned HTTP {status}"
                )));
            }
            let value = response
                .json()
                .await
                .map_err(|e| A2AError::ParseError(e.without_url().to_string()))?;
            let (card, endpoint) =
                resolve_card(value, &self.agent_id, &url, legacy_root, pin.as_ref())?;
            self.client_for_url(endpoint.url()).await?;
            return Ok((card, endpoint));
        }
        if let (Some(card), Some(root)) = (fallback, legacy_root) {
            let url = pin.unwrap_or_else(|| append_path(root, "a2a"));
            self.client_for_url(&url).await?;
            return Ok((
                card.clone(),
                ResolvedEndpoint::new(url, RpcProtocol::Legacy, false),
            ));
        }
        Err(A2AError::AgentUnreachable(
            "A2A discovery returned HTTP 404".into(),
        ))
    }

    pub async fn fetch_agent_card(&self) -> A2AResult<AgentCard> {
        self.discover(None).await.map(|(card, _)| card)
    }

    pub(crate) async fn probe(&self, agent: &RegisteredAgent) -> A2AResult<()> {
        if let Some(endpoint) = self.endpoint.as_ref() {
            self.client_for_url(endpoint.url()).await?;
        }
        let fallback = (agent.card.version == "unknown").then_some(&agent.card);
        self.discover(fallback).await.map(|_| ())
    }

    pub(crate) async fn prepare_delegation(
        &self,
        task_id: &str,
        message: &A2AMessage,
    ) -> A2AResult<PreparedDelegation> {
        let endpoint = self.endpoint.clone().ok_or_else(|| {
            A2AError::AgentUnreachable("A2A registration has no resolved endpoint".into())
        })?;
        self.prepare(endpoint, task_id, message, None).await
    }

    async fn prepare(
        &self,
        endpoint: ResolvedEndpoint,
        task_id: &str,
        message: &A2AMessage,
        session_id: Option<&str>,
    ) -> A2AResult<PreparedDelegation> {
        let url = endpoint.url();
        let client = self.client_for_url(url).await?;
        let id = uuid::Uuid::new_v4().to_string();
        let body = json!({"jsonrpc": "2.0", "id": id, "method": endpoint.method(), "params": endpoint.params(task_id, message, session_id)});
        let request = self
            .request_auth(client.post(url.clone()), url)?
            .json(&body);
        let request = if endpoint.streaming() {
            request.header(reqwest::header::ACCEPT, "text/event-stream")
        } else {
            request.timeout(self.timeout)
        };
        let request = if endpoint.protocol() == RpcProtocol::V1 {
            request.header("A2A-Version", "1.0")
        } else {
            request
        };
        let request = request
            .build()
            .map_err(|e| A2AError::InvalidRequest(e.without_url().to_string()))?;
        Ok(PreparedDelegation {
            client,
            request,
            id,
            endpoint,
            timeout: self.timeout,
        })
    }

    /// Send once using the explicitly chosen legacy unary API.
    pub async fn send_message(
        &self,
        task_id: &str,
        message: &A2AMessage,
        session_id: Option<&str>,
    ) -> A2AResult<A2ATask> {
        let endpoint = legacy_endpoint(&self.base_url, false)?;
        let response = self
            .prepare(endpoint, task_id, message, session_id)
            .await?
            .send()
            .await
            .map_err(|err| err.error)?;
        match response {
            DelegationResponse::Unary(task) => Ok(task),
            DelegationResponse::Streaming(_) => unreachable!("prepared unary request"),
        }
    }

    /// Open one stream using the explicitly chosen legacy streaming API.
    pub async fn send_message_stream(
        &self,
        task_id: &str,
        message: &A2AMessage,
        session_id: Option<&str>,
    ) -> A2AResult<EventStream> {
        let endpoint = legacy_endpoint(&self.base_url, true)?;
        let response = self
            .prepare(endpoint, task_id, message, session_id)
            .await?
            .send()
            .await
            .map_err(|err| err.error)?;
        match response {
            DelegationResponse::Streaming(stream) => Ok(stream),
            DelegationResponse::Unary(_) => unreachable!("prepared stream request"),
        }
    }
}

impl PreparedDelegation {
    pub(crate) async fn send(self) -> Result<DelegationResponse, DispatchError> {
        let streaming = self.endpoint.streaming();
        let execute = self.client.execute(self.request);
        let response = if streaming {
            tokio::time::timeout(STREAM_OPEN_TIMEOUT, execute)
                .await
                .map_err(|_| DispatchError::unknown(A2AError::Timeout(STREAM_OPEN_TIMEOUT)))?
        } else {
            execute.await
        }
        .map_err(|e| DispatchError::unknown(transport_error(e)))?;

        let status = response.status();
        if !status.is_success() {
            return Err(match status.as_u16() {
                401 => DispatchError::rejected(A2AError::Unauthorized),
                403 => DispatchError::rejected(A2AError::Forbidden),
                _ => DispatchError::unknown(A2AError::AgentUnreachable(format!(
                    "A2A RPC returned HTTP {status}"
                ))),
            });
        }

        let protocol = self.endpoint.protocol();
        if streaming {
            let is_sse = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| {
                    h.split(';')
                        .next()
                        .is_some_and(|main| main.trim().eq_ignore_ascii_case("text/event-stream"))
                });
            if !is_sse {
                return Err(DispatchError::unknown(A2AError::InvalidRequest(
                    "A2A streaming endpoint did not return an event stream".into(),
                )));
            }
            let stream = if protocol == RpcProtocol::Legacy {
                parse_sse_response(response, STREAM_IDLE_TIMEOUT)
            } else {
                parse_sse_frames(
                    response.bytes_stream(),
                    STREAM_IDLE_TIMEOUT,
                    move |_, data| {
                        let result = rpc_result(data, Some(&self.id), self.timeout)
                            .map_err(|err| err.error)?;
                        decode_stream_result(protocol, result)
                    },
                )
            };
            Ok(DelegationResponse::Streaming(stream))
        } else {
            let body = response
                .bytes()
                .await
                .map_err(|err| DispatchError::unknown(transport_error(err)))?;
            let result = rpc_result(
                &body,
                (protocol != RpcProtocol::Legacy).then_some(self.id.as_str()),
                self.timeout,
            )?;
            let task = decode_task(protocol, result).map_err(DispatchError::unknown)?;
            Ok(DelegationResponse::Unary(task))
        }
    }
}

fn transport_error(err: reqwest::Error) -> A2AError {
    A2AError::AgentUnreachable(err.without_url().to_string())
}

fn rpc_result(
    body: &[u8],
    request_id: Option<&str>,
    timeout: Duration,
) -> Result<Value, DispatchError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| {
        DispatchError::unknown(A2AError::ParseError(
            "Malformed A2A JSON-RPC response".into(),
        ))
    })?;
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(DispatchError::unknown(A2AError::InvalidRequest(
            "response jsonrpc field must be \"2.0\"".into(),
        )));
    }
    if request_id.is_some_and(|id| value.get("id").and_then(Value::as_str) != Some(id)) {
        return Err(DispatchError::unknown(A2AError::InvalidRequest(
            "A2A response ID does not match the request".into(),
        )));
    }
    match (
        value.get("error").filter(|e| !e.is_null()),
        value.get("result"),
    ) {
        (None, Some(result)) => Ok(result.clone()),
        (Some(error), None) => {
            let error: JsonRpcErrorData = serde_json::from_value(error.clone()).map_err(|_| {
                DispatchError::unknown(A2AError::ParseError("Malformed A2A JSON-RPC error".into()))
            })?;
            let remote_error = match error.code {
                -32700 => A2AError::ParseError(error.message),
                -32600 => A2AError::InvalidRequest(error.message),
                -32601 => A2AError::MethodNotFound(error.message),
                -32602 => A2AError::InvalidParams(error.message),
                -32000 => A2AError::Unauthorized,
                -32001 => A2AError::TaskNotFound(error.message),
                -32002 => A2AError::TaskNotCancelable(crate::a2a::domain::TaskState::Failed),
                -32003 => A2AError::PushNotSupported,
                -32004 => A2AError::UnsupportedContentType,
                -32005 => A2AError::Forbidden,
                -32010 => A2AError::AgentUnreachable(error.message),
                -32011 => A2AError::NoMatchingAgent,
                -32012 => A2AError::Timeout(timeout),
                _ => A2AError::InternalError(error.message),
            };
            Err(DispatchError::rejected(remote_error))
        }
        _ => Err(DispatchError::unknown(A2AError::InvalidRequest(
            "A2A response requires exactly one result or error".into(),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::A2ARole;
    use futures::StreamExt;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn local_policy() -> SsrfPolicy {
        SsrfPolicy {
            enabled: false,
            ..SsrfPolicy::default()
        }
    }

    fn v1_card(url: &str) -> Value {
        json!({
            "name": "Peer", "version": "1", "capabilities": {"streaming": false},
            "supportedInterfaces": [{
                "url": url, "protocolBinding": "JSONRPC", "protocolVersion": "1.0"
            }]
        })
    }

    fn assert_message_request(request: &Request, expected_method: &str) -> Value {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["method"], expected_method);
        assert_eq!(body["params"]["message"]["parts"][0]["text"], "hello");
        assert!(body["params"]["message"].get("taskId").is_none());
        body
    }

    #[test]
    fn legacy_routes_preserve_reverse_proxy_prefix() {
        let client = A2AClient::with_auth("https://example.com/api/v1/", "token")
            .with_timeout(Duration::from_secs(30));
        assert_eq!(client.base_url(), "https://example.com/api/v1");
        assert_eq!(client.timeout(), Duration::from_secs(30));
        assert_eq!(
            legacy_endpoint(client.base_url(), false)
                .unwrap()
                .url()
                .path(),
            "/api/v1/a2a"
        );
        assert_eq!(
            legacy_endpoint(client.base_url(), true)
                .unwrap()
                .url()
                .path(),
            "/api/v1/a2a/stream"
        );
    }

    #[tokio::test]
    async fn direct_card_selects_v1_unary_and_keeps_credentials_on_origin() {
        let server = MockServer::start().await;
        let rpc_url = format!("{}/specific/rpc", server.uri());
        Mock::given(method("GET"))
            .and(path("/cards/peer.json"))
            .and(header("authorization", "Bearer peer-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(v1_card(&rpc_url)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/specific/rpc"))
            .and(header("authorization", "Bearer peer-token"))
            .and(header("a2a-version", "1.0"))
            .respond_with(|request: &Request| {
                let body = assert_message_request(request, "SendMessage");
                assert_eq!(body["params"]["message"]["role"], "ROLE_USER");
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0", "id": body["id"], "result": {"message": {
                        "messageId": "m1", "role": "ROLE_AGENT", "parts": [{"text": "answer"}]
                    }}
                }))
            })
            .expect(1)
            .mount(&server)
            .await;
        let mut client = A2AClient::with_ssrf_policy(
            format!("{}/cards/peer.json", server.uri()),
            local_policy(),
        );
        client.auth_token = Some("peer-token".into());
        let (_, endpoint) = client.discover(None).await.unwrap();
        assert_eq!(endpoint.url().as_str(), rpc_url);
        client.endpoint = Some(endpoint);
        let response = client
            .prepare_delegation("local-id", &A2AMessage::text(A2ARole::User, "hello"))
            .await
            .unwrap()
            .send()
            .await
            .unwrap();
        let DelegationResponse::Unary(task) = response else {
            panic!("expected a unary response")
        };
        assert_eq!(task.status.message.unwrap().text_content(), "answer");
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn root_card_selects_v03_stream_on_advertised_route() {
        let server = MockServer::start().await;
        let rpc_url = format!("{}/shared-rpc", server.uri());
        Mock::given(method("GET"))
            .and(path("/prefix/.well-known/agent-card.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "Peer", "version": "1", "protocolVersion": "0.3.0",
                "url": rpc_url, "preferredTransport": "JSONRPC", "capabilities": {"streaming": true}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/shared-rpc"))
            .and(header("accept", "text/event-stream"))
            .respond_with(|request: &Request| {
                let body = assert_message_request(request, "message/stream");
                assert_eq!(body["params"]["message"]["role"], "user");
                let event = json!({"jsonrpc": "2.0", "id": body["id"], "result": {
                    "kind": "status-update", "taskId": "t", "contextId": "c", "final": true,
                    "status": {"state": "completed", "message": {"parts": [{"kind": "text", "text": "已完成"}]}}
                }});
                ResponseTemplate::new(200).set_body_raw(format!("data: {event}\n\n"), "text/event-stream")
            })
            .expect(1)
            .mount(&server)
            .await;
        let mut client =
            A2AClient::with_ssrf_policy(format!("{}/prefix", server.uri()), local_policy());
        let (_, endpoint) = client.discover(None).await.unwrap();
        client.endpoint = Some(endpoint);
        let response = client
            .prepare_delegation("local-id", &A2AMessage::text(A2ARole::User, "hello"))
            .await
            .unwrap()
            .send()
            .await
            .unwrap();
        let DelegationResponse::Streaming(mut stream) = response else {
            panic!("expected an SSE response")
        };
        let UpdateEvent::StatusUpdate(event) = stream.next().await.unwrap().unwrap() else {
            panic!("expected a status update")
        };
        assert_eq!(event.status.message.unwrap().text_content(), "已完成");
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn delegates_once_to_selected_rpc_and_refuses_unconfigured_origin() {
        let discovery = MockServer::start().await;
        let peer = MockServer::start().await;
        let advertised = format!("{}/rpc", peer.uri());
        Mock::given(method("GET"))
            .and(path("/.well-known/agent-card.json"))
            .and(header("authorization", "Bearer peer-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(v1_card(&advertised)))
            .expect(2)
            .mount(&discovery)
            .await;
        Mock::given(method("POST"))
            .and(path("/rpc"))
            .and(header("authorization", "Bearer peer-token"))
            .respond_with(ResponseTemplate::new(307).insert_header("location", "/other"))
            .expect(1)
            .mount(&peer)
            .await;
        let mut client = A2AClient::with_ssrf_policy(discovery.uri(), local_policy());
        client.auth_token = Some("peer-token".into());
        assert!(client.discover(None).await.is_err());
        assert!(peer.received_requests().await.unwrap().is_empty());

        client.pinned_rpc_url = Some(advertised);
        let (_, endpoint) = client.discover(None).await.unwrap();
        client.endpoint = Some(endpoint);
        let result = client
            .prepare_delegation("local-id", &A2AMessage::text(A2ARole::User, "hello"))
            .await
            .unwrap()
            .send()
            .await;
        let Err(error) = result else {
            panic!("expected the redirect to fail")
        };
        assert_eq!(error.delivery, DeliveryStatus::Unknown);
        assert_eq!(peer.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn dispatch_rechecks_network_policy_for_a_resolved_endpoint() {
        let server = MockServer::start().await;
        let rpc_url = format!("{}/rpc", server.uri());
        Mock::given(method("GET"))
            .and(path("/.well-known/agent-card.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(v1_card(&rpc_url)))
            .expect(1)
            .mount(&server)
            .await;
        let discovery = A2AClient::with_ssrf_policy(server.uri(), local_policy());
        let (_, endpoint) = discovery.discover(None).await.unwrap();
        let mut dispatch = A2AClient::new(server.uri());
        dispatch.endpoint = Some(endpoint);
        let error = dispatch
            .prepare_delegation("local-id", &A2AMessage::text(A2ARole::User, "hello"))
            .await
            .err()
            .expect("default policy rejects loopback even for a resolved endpoint");
        assert!(error.to_string().contains("blocked by network policy"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[test]
    fn rpc_envelope_validates_version_and_response_id() {
        let timeout = Duration::from_secs(1);
        for response in [
            br#"{"jsonrpc":"1.0","id":"one","result":{}}"#.as_slice(),
            br#"{"jsonrpc":"2.0","id":"two","result":{}}"#.as_slice(),
        ] {
            let error = rpc_result(response, Some("one"), timeout).unwrap_err();
            assert_eq!(error.delivery, DeliveryStatus::Unknown);
        }
    }
}
