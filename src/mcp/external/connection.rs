//! MCP Server Connection
//!
//! Manages the lifecycle and communication with an external MCP server.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::error::{AlephError, Result};
use crate::mcp::jsonrpc::{mcp as mcp_types, IdGenerator, JsonRpcNotification, JsonRpcRequest};
use crate::mcp::modern::cache::{is_stale, CacheDirective};
use crate::mcp::modern::discover::{
    select_version, DiscoverResult, VersionChoice, DISCOVER_METHOD,
};
use crate::mcp::modern::headers::{collect_param_headers, extract_param_headers, ParamHeader};
use crate::mcp::modern::mrtr::{self, InputRequired};
use crate::mcp::modern::{
    aleph_client_capabilities, aleph_client_info, is_modern_error, McpDialect, RequestMeta,
    UnsupportedVersion, MCP_MODERN_PROTOCOL_VERSION,
};
use crate::mcp::sampling::SamplingHandler;
use crate::mcp::transport::{McpTransport, StdioTransport};
use crate::mcp::types::McpTool;
use crate::sync_primitives::Arc;

fn strip_server_prefix<'a>(s: &'a str, server_name: &str) -> &'a str {
    s.strip_prefix(&format!("{server_name}:")).unwrap_or(s)
}

/// Default timeout for the entire MCP server connection process
/// This includes: process spawn + initialize handshake + tools/list
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(300);

/// Which cached lists came back different after a TTL-driven re-fetch.
///
/// Deliberately mirrors the granularity of the server-sent list-changed
/// notifications, so a caller can feed both into the same publish path rather
/// than inventing a second one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChangedLists {
    /// `tools/list` differs from what was cached.
    pub tools: bool,
    /// `resources/list` or `resources/templates/list` differs.
    pub resources: bool,
    /// `prompts/list` differs.
    pub prompts: bool,
}

/// Marker that says "the server answered, and its answer was a failure".
///
/// A tool-level failure (`isError: true`) and a transport failure both leave
/// this layer as `AlephError::IoError`, which erases a distinction some callers
/// depend on: `browser::chrome_mcp` documents that a tool's own verdict may be
/// folded into a value (`wait_for` → `Ok(false)`) while a dead pipe never may,
/// and it was classifying every tool error as a dead pipe — so "the text never
/// appeared" reached the model as a transport failure instead of an answer.
///
/// Exposed as a constant, and used to *build* the message as well as to
/// recognise it, so the two ends cannot drift.
pub(crate) const TOOL_ERROR_MARKER: &str = "' returned error: ";

/// Whether an error from [`McpConnection::call_tool`] is the tool's own verdict
/// rather than a failure to reach it.
///
/// Substring match on the marker alone is too permissive: an
/// `AlephError::IoError` whose formatted message happens to contain
/// `' returned error: ` (e.g. a wrapped inner error quoting a tool name
/// verbatim) would be misclassified as a tool verdict, suppressing
/// retries downstream. Require the literal `Tool ` prefix that the call
/// site always emits, so only messages produced by the canonical
/// formatter match.
#[must_use]
pub(crate) fn is_tool_error(message: &str) -> bool {
    message.contains(TOOL_ERROR_MARKER)
        && message
            .split_once(TOOL_ERROR_MARKER)
            .is_some_and(|(prefix, _)| prefix.trim_start().starts_with("Tool "))
}

impl ChangedLists {
    /// Combine two reports, as when one client fronts several connections.
    #[must_use]
    pub const fn merged(self, other: Self) -> Self {
        Self {
            tools: self.tools || other.tools,
            resources: self.resources || other.resources,
            prompts: self.prompts || other.prompts,
        }
    }
}

/// A cheap content signature for change detection.
///
/// The cached list types carry no `PartialEq`, and serializing catches a
/// changed description or input schema that comparing names alone would miss.
/// A serialization failure yields the same value on both sides, which reports
/// "unchanged" — the quiet outcome, not a spurious re-sync.
fn fingerprint<T: serde::Serialize>(items: &T) -> String {
    serde_json::to_string(items).unwrap_or_default()
}

/// What the `server/discover` probe concluded about a server.
#[derive(Debug)]
enum EraProbe {
    /// Speak the stateless shape at this revision.
    Modern {
        /// The revision both sides implement.
        version: String,
        /// What discovery reported, or `None` when the era was inferred from an
        /// error rather than from a successful `DiscoverResult` — in which case
        /// capabilities are still unknown and have to be asked for again.
        discovered: Option<DiscoverResult>,
    },
    /// The server predates `2026-07-28`; open with the `initialize` handshake.
    Legacy,
    /// A modern server with no revision in common.
    Incompatible(Vec<String>),
}

/// External MCP server connection
///
/// This struct manages the lifecycle and communication with an MCP server.
/// It uses a trait object (`Box<dyn McpTransport>`) to support different
/// transport implementations (stdio, HTTP, SSE).
pub struct McpServerConnection {
    /// Server name
    name: String,
    /// Transport layer (trait object for flexibility)
    transport: Arc<dyn McpTransport>,
    /// Request ID generator
    id_gen: IdGenerator,
    /// Server capabilities (after initialize)
    capabilities: RwLock<Option<mcp_types::ServerCapabilities>>,
    /// Cached tools list
    cached_tools: RwLock<Vec<McpTool>>,
    /// Cached resources list
    cached_resources: RwLock<Vec<crate::mcp::types::McpResource>>,
    /// Cached resource-templates list (parameterized URIs)
    cached_resource_templates: RwLock<Vec<crate::mcp::types::McpResourceTemplate>>,
    /// Cached prompts list
    cached_prompts: RwLock<Vec<crate::mcp::prompts::McpPrompt>>,
    /// Cached server instructions (from the handshake or `server/discover`)
    cached_instructions: RwLock<Option<String>>,
    /// Which protocol era this server speaks.
    ///
    /// Settled exactly once, by [`Self::handshake`], before any other request —
    /// the spec makes the era a property of the server, not of a request, so
    /// re-deciding per call would be both wasteful and a source of drift. A
    /// `OnceLock` rather than a lock keeps the read on the request path free.
    dialect: OnceLock<McpDialect>,
    /// The `_meta` block attached to every modern request. Absent on the legacy
    /// path, where the same information was exchanged once during `initialize`.
    request_meta: OnceLock<RequestMeta>,
    /// `x-mcp-header` annotations per server-local tool name, harvested when
    /// the tool list is refreshed and consumed when the tool is called.
    param_headers: RwLock<HashMap<String, Vec<ParamHeader>>>,
    /// When each cached list goes stale, per the server's `ttlMs` hints.
    cache_expiry: RwLock<CacheExpiry>,
    /// Services MRTR input requests. Installed by [`crate::mcp::McpClient`]
    /// after construction, mirroring how the notification handler is wired.
    sampling: RwLock<Option<Arc<SamplingHandler>>>,
}

/// When each cached list stops being fresh.
///
/// `None` means the server gave no `ttlMs` hint for that list, which is every
/// pre-`2026-07-28` server — those entries stay valid until a `listChanged`
/// notification or a reconnect replaces them, exactly as before.
#[derive(Debug, Default)]
struct CacheExpiry {
    tools: Option<Instant>,
    resources: Option<Instant>,
    resource_templates: Option<Instant>,
    prompts: Option<Instant>,
}

impl McpServerConnection {
    /// Connect to an external MCP server with timeout protection
    ///
    /// # Arguments
    /// * `name` - Server name for identification
    /// * `command` - Command to execute
    /// * `args` - Command arguments
    /// * `env` - Environment variables
    /// * `cwd` - Working directory
    /// * `timeout` - Per-request timeout (defaults to 30s for individual RPCs).
    ///   The total connection timeout is always `DEFAULT_CONNECT_TIMEOUT` (300s)
    ///   to allow for slow server startup.
    /// * `sampling` - Handler for server-requested LLM completions, or `None`.
    ///   Supplied here rather than installed afterwards because the connection
    ///   declares its capabilities during the handshake: a connection that
    ///   cannot sample must not claim it can.
    pub async fn connect(
        name: impl Into<String>,
        command: impl AsRef<str>,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: Option<&PathBuf>,
        timeout: Option<Duration>,
        sampling: Option<Arc<SamplingHandler>>,
    ) -> Result<Self> {
        let name = name.into();
        // Total connection timeout is always the larger default (300s) to allow
        // for slow server startup. Per-request timeout is set separately on the transport.
        let connect_timeout = DEFAULT_CONNECT_TIMEOUT;

        // Wrap entire connection process with timeout
        tokio::time::timeout(
            connect_timeout,
            Self::connect_internal(&name, command, args, env, cwd, timeout, sampling),
        )
        .await
        .map_err(|_| {
            AlephError::Timeout {
                suggestion: Some(format!(
                    "MCP server '{}' connection timed out after {}s. Check if the server is installed and responding.",
                    name,
                    connect_timeout.as_secs()
                )),
            }
        })?
    }

    /// Internal connection logic (without timeout wrapper)
    async fn connect_internal(
        name: &str,
        command: impl AsRef<str>,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: Option<&PathBuf>,
        timeout: Option<Duration>,
        sampling: Option<Arc<SamplingHandler>>,
    ) -> Result<Self> {
        // Spawn the server process
        let mut transport = StdioTransport::spawn(name, command, args, env, cwd).await?;

        // Set per-request timeout if provided
        if let Some(t) = timeout {
            transport = transport.with_timeout(t);
        }

        Self::with_transport(name, Arc::new(transport), sampling).await
    }

    /// Create a connection with a custom transport
    ///
    /// This constructor allows creating connections with any transport implementation,
    /// enabling support for HTTP, SSE, or mock transports for testing.
    ///
    /// # Arguments
    /// * `name` - Server name for identification
    /// * `transport` - An `Arc`-wrapped transport implementing `McpTransport`
    /// * `sampling` - Handler for server-requested LLM completions, or `None`
    ///
    /// # Example
    /// ```ignore
    /// let http_transport = HttpTransport::new("https://mcp.example.com").await?;
    /// let conn = McpServerConnection::with_transport("remote-server", Arc::new(http_transport), None).await?;
    /// ```
    pub async fn with_transport(
        name: impl Into<String>,
        transport: Arc<dyn McpTransport>,
        sampling: Option<Arc<SamplingHandler>>,
    ) -> Result<Self> {
        let name = name.into();
        let conn = Self {
            name,
            transport,
            id_gen: IdGenerator::new(),
            capabilities: RwLock::new(None),
            cached_tools: RwLock::new(Vec::new()),
            cached_resources: RwLock::new(Vec::new()),
            cached_resource_templates: RwLock::new(Vec::new()),
            cached_prompts: RwLock::new(Vec::new()),
            cached_instructions: RwLock::new(None),
            dialect: OnceLock::new(),
            request_meta: OnceLock::new(),
            param_headers: RwLock::new(HashMap::new()),
            cache_expiry: RwLock::new(CacheExpiry::default()),
            sampling: RwLock::new(sampling),
        };

        conn.handshake().await?;

        Ok(conn)
    }

    /// Whether this connection can service a server's request for an LLM
    /// completion — and therefore whether it may declare the capability.
    ///
    /// The predicate is "is a callback registered", not "was a handler struct
    /// passed in": every connection is constructed with a handler, so the
    /// latter is structurally true and made the declaration unconditional. A
    /// server that believed it answered `sampling/createMessage` and got back
    /// "No sampling callback registered" — a declared capability the host could
    /// not honour. The callback is installed on the client before any transport
    /// starts, so this reads the settled answer at handshake time.
    async fn can_sample(&self) -> bool {
        match self.sampling.read().await.as_ref() {
            Some(handler) => handler.has_callback().await,
            None => false,
        }
    }

    /// Whether this connection speaks the stateless `2026-07-28` shape.
    fn is_modern(&self) -> bool {
        self.dialect.get().is_some_and(McpDialect::is_modern)
    }

    /// Build an outbound request.
    ///
    /// The single point at which a request on this connection comes into
    /// existence. A modern server rejects any request that lacks the required
    /// `_meta`, and there is no handshake left to carry that information once —
    /// so it has to ride on every request, and the only way to keep that true is
    /// for no call site to be able to build a request without passing through
    /// here. Legacy connections have no `request_meta`, so their params travel
    /// exactly as they did before.
    fn request(&self, method: &str, params: Option<Value>) -> JsonRpcRequest {
        let id = self.id_gen.next();
        match self.request_meta.get() {
            Some(meta) => JsonRpcRequest::with_params(id, method, meta.attach(params)),
            None => match params {
                Some(params) => JsonRpcRequest::with_params(id, method, params),
                None => JsonRpcRequest::new(id, method),
            },
        }
    }

    /// Decide which era this server speaks, then open the connection that way.
    async fn handshake(&self) -> Result<()> {
        let can_sample = self.can_sample().await;
        let probe_meta = RequestMeta::new(
            MCP_MODERN_PROTOCOL_VERSION,
            &aleph_client_info(),
            &aleph_client_capabilities(can_sample),
        );

        // Per-step bound inside the overall 300 s connect timeout. Without
        // it, a server that answers `server/discover` in 100 ms but then
        // hangs on `tools/list` would spend the rest of the 300 s
        // blocking the handshake — and the manager's health probe, which
        // calls `refresh_tools` again, would hit the same hang.
        const HANDSHAKE_STEP_TIMEOUT: Duration = Duration::from_secs(60);

        match tokio::time::timeout(HANDSHAKE_STEP_TIMEOUT, self.probe_era(&probe_meta)).await {
            Ok(EraProbe::Modern {
                version,
                discovered,
            }) => {
                self.adopt_modern(version, discovered, can_sample).await?;
            }
            Ok(EraProbe::Legacy) => {
                self.initialize_legacy(can_sample).await?;
            }
            Ok(EraProbe::Incompatible(supported)) => {
                return Err(AlephError::IoError(format!(
                    "MCP server '{}' supports only protocol {:?}; Aleph speaks {} \
                     and the handshake-based revisions up to {}",
                    self.name,
                    supported,
                    MCP_MODERN_PROTOCOL_VERSION,
                    mcp_types::MCP_LEGACY_PROTOCOL_VERSION
                )))
            }
            Err(_) => {
                return Err(AlephError::Timeout {
                    suggestion: Some(format!(
                        "MCP server '{}' server/discover probe did not complete in {}s",
                        self.name,
                        HANDSHAKE_STEP_TIMEOUT.as_secs()
                    )),
                });
            }
        }

        // Per-step timeout on the post-handshake list-method drains. A
        // hung server should not consume the remainder of the global
        // connect timeout silently.
        let drain = async {
            self.refresh_tools().await?;
            if let Err(e) = self.refresh_resources().await {
                tracing::debug!(server = %self.name, error = %e, "Resources refresh failed (may not be supported)");
            }
            if let Err(e) = self.refresh_resource_templates().await {
                tracing::debug!(server = %self.name, error = %e, "Resource templates refresh failed (may not be supported)");
            }
            if let Err(e) = self.refresh_prompts().await {
                tracing::debug!(server = %self.name, error = %e, "Prompts refresh failed (may not be supported)");
            }
            Ok::<(), AlephError>(())
        };
        tokio::time::timeout(HANDSHAKE_STEP_TIMEOUT, drain)
            .await
            .map_err(|_| AlephError::Timeout {
                suggestion: Some(format!(
                    "MCP server '{}' post-handshake list refresh did not complete in {}s",
                    self.name,
                    HANDSHAKE_STEP_TIMEOUT.as_secs()
                )),
            })??;

        Ok(())
    }

    /// Probe the server with `server/discover`.
    ///
    /// Modern servers must implement it, so its answer settles the era in one
    /// round-trip. The spec's rule is "fall back on any error that is not a
    /// recognized modern error" — a spec-reserved code proves the peer is
    /// modern and means *correct the request*, while everything else (a
    /// `method not found`, an HTTP error page, a transport failure) means the
    /// server predates this revision and wants the `initialize` handshake.
    async fn probe_era(&self, probe_meta: &RequestMeta) -> EraProbe {
        let request = JsonRpcRequest::with_params(
            self.id_gen.next(),
            DISCOVER_METHOD,
            probe_meta.attach(None),
        );

        let response = match self.transport.send_request(&request).await {
            Ok(response) => response,
            Err(e) => {
                // Not an answer at all. If the server is simply dead the
                // legacy handshake fails next and reports it properly; there is
                // nothing here worth failing the connection over.
                tracing::debug!(
                    server = %self.name,
                    error = %e,
                    "server/discover did not answer; treating server as legacy"
                );
                return EraProbe::Legacy;
            }
        };

        let result = match response.into_result() {
            Ok(result) => result,
            Err(error) => {
                if !is_modern_error(error.code) {
                    tracing::debug!(
                        server = %self.name,
                        code = error.code,
                        "server/discover rejected with a non-modern error; using the handshake"
                    );
                    return EraProbe::Legacy;
                }
                return match UnsupportedVersion::from_error(&error) {
                    Some(unsupported) => {
                        match select_version(&unsupported.supported, MCP_MODERN_PROTOCOL_VERSION) {
                            VersionChoice::Modern(version) => EraProbe::Modern {
                                version,
                                discovered: None,
                            },
                            VersionChoice::Legacy => EraProbe::Legacy,
                            VersionChoice::Incompatible(supported) => {
                                EraProbe::Incompatible(supported)
                            }
                        }
                    }
                    None => {
                        // A spec-reserved code identifies a modern peer even
                        // when it carries no version list to negotiate with.
                        tracing::warn!(
                            server = %self.name,
                            code = error.code,
                            message = %error.message,
                            "server/discover returned a modern protocol error; \
                             continuing as a modern server"
                        );
                        EraProbe::Modern {
                            version: MCP_MODERN_PROTOCOL_VERSION.to_string(),
                            discovered: None,
                        }
                    }
                };
            }
        };

        let discovered: DiscoverResult = match serde_json::from_value(result) {
            Ok(discovered) => discovered,
            Err(e) => {
                tracing::warn!(
                    server = %self.name,
                    error = %e,
                    "server/discover answered with an unparsable result; using the handshake"
                );
                return EraProbe::Legacy;
            }
        };

        match select_version(&discovered.supported_versions, MCP_MODERN_PROTOCOL_VERSION) {
            VersionChoice::Modern(version) => EraProbe::Modern {
                version,
                discovered: Some(discovered),
            },
            VersionChoice::Legacy => EraProbe::Legacy,
            VersionChoice::Incompatible(supported) => EraProbe::Incompatible(supported),
        }
    }

    /// Commit this connection to the stateless shape.
    async fn adopt_modern(
        &self,
        version: String,
        discovered: Option<DiscoverResult>,
        can_sample: bool,
    ) -> Result<()> {
        let dialect = McpDialect::Modern {
            version: version.clone(),
        };
        self.transport.set_dialect(&dialect);
        let _ = self.dialect.set(dialect);
        let _ = self.request_meta.set(RequestMeta::new(
            &version,
            &aleph_client_info(),
            &aleph_client_capabilities(can_sample),
        ));

        // The era can be settled by an *error* that merely proves the peer is
        // modern (a version rejection, a missing-capability complaint). That
        // tells us nothing about what the server serves, and leaving
        // capabilities empty would silently skip resources and prompts for the
        // rest of the connection's life. Now that the dialect is committed, ask
        // again — properly this time.
        let discovered = match discovered {
            Some(discovered) => discovered,
            None => self.rediscover().await,
        };

        let server_info = discovered.server_info();
        tracing::info!(
            server = %self.name,
            protocol = %version,
            server_name = ?server_info.as_ref().map(|i| &i.name),
            "MCP server discovered (stateless protocol)"
        );

        {
            let mut caps = self.capabilities.write().await;
            *caps = Some(discovered.capabilities);
        }
        {
            let mut inst = self.cached_instructions.write().await;
            *inst = discovered.instructions;
        }

        Ok(())
    }

    /// Re-run discovery on a connection whose dialect is already settled.
    ///
    /// Non-fatal: a server that still will not answer leaves capabilities
    /// empty, which degrades to "tools only" rather than failing a connection
    /// that has already proved it speaks the protocol.
    async fn rediscover(&self) -> DiscoverResult {
        let request = self.request(DISCOVER_METHOD, None);
        let parsed = match self.transport.send_request(&request).await {
            Ok(response) => response
                .into_result()
                .ok()
                .and_then(|result| serde_json::from_value(result).ok()),
            Err(_) => None,
        };

        parsed.unwrap_or_else(|| {
            tracing::warn!(
                server = %self.name,
                "Modern MCP server did not answer server/discover; \
                 continuing without its capability list"
            );
            DiscoverResult::default()
        })
    }

    /// Open the connection the way every revision before `2026-07-28` expects.
    async fn initialize_legacy(&self, can_sample: bool) -> Result<()> {
        let params = mcp_types::InitializeParams::aleph_default(can_sample);
        let request = JsonRpcRequest::with_params(
            self.id_gen.next(),
            "initialize",
            serde_json::to_value(&params).map_err(|e| {
                AlephError::IoError(format!("Failed to serialize initialize params: {e}"))
            })?,
        );

        let response = self.transport.send_request(&request).await?;
        let result = response.into_result().map_err(|e| {
            AlephError::IoError(format!(
                "MCP server '{}' initialize failed: {}",
                self.name, e
            ))
        })?;

        // Parse initialize result
        let init_result: mcp_types::InitializeResult =
            serde_json::from_value(result).map_err(|e| {
                AlephError::IoError(format!(
                    "Failed to parse initialize result from '{}': {}",
                    self.name, e
                ))
            })?;

        tracing::info!(
            server = %self.name,
            protocol = %init_result.protocol_version,
            server_name = ?init_result.server_info.as_ref().map(|i| &i.name),
            "MCP server initialized"
        );

        // Honor the negotiated revision on every subsequent request. The
        // Streamable HTTP transport echoes it on the `MCP-Protocol-Version`
        // header; other transports treat this as a no-op.
        let dialect = McpDialect::Legacy {
            version: init_result.protocol_version,
        };
        self.transport.set_dialect(&dialect);
        let _ = self.dialect.set(dialect);

        // Store capabilities
        {
            let mut caps = self.capabilities.write().await;
            *caps = Some(init_result.capabilities);
        }

        // Store instructions (if provided by server)
        {
            let mut inst = self.cached_instructions.write().await;
            *inst = init_result.instructions;
        }

        // Send initialized notification (per JSON-RPC spec, notifications have no id)
        let notification = JsonRpcNotification::new("notifications/initialized");
        if let Err(e) = self.transport.send_notification(&notification).await {
            tracing::warn!(
                server = %self.name,
                error = %e,
                "Failed to send initialized notification (non-fatal)"
            );
        }

        Ok(())
    }

    /// Drain a paginated MCP list method, following the spec `nextCursor`
    /// chain until the server stops returning one.
    ///
    /// The first page omits `params` (the cursor is optional per spec); each
    /// subsequent page echoes the previous page's `nextCursor` back as
    /// `params.cursor`. `extract` parses one page's raw JSON-RPC result into
    /// `(items, next_cursor)`. `MAX_PAGES` bounds a server whose cursor never
    /// terminates (or fails to advance) so a buggy or hostile server cannot
    /// pin the connection in an unbounded fetch loop.
    /// Returns the drained items alongside the freshness hint the server
    /// attached. The hint is read from the **first** page: it describes the
    /// listing as a whole, and honoring a later page's shorter TTL would expire
    /// a list that is already complete.
    async fn drain_paginated<T, F>(
        &self,
        method: &str,
        mut extract: F,
    ) -> Result<(Vec<T>, CacheDirective)>
    where
        F: FnMut(Value) -> Result<(Vec<T>, Option<String>)>,
    {
        const MAX_PAGES: usize = 100;
        let mut items: Vec<T> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut directive = CacheDirective::default();

        for page_index in 0..MAX_PAGES {
            let params = cursor.as_ref().map(|c| json!({ "cursor": c }));
            let request = self.request(method, params);
            let response = self.transport.send_request(&request).await?;
            let result = response.into_result().map_err(|e| {
                AlephError::IoError(format!(
                    "MCP server '{}' {} failed: {}",
                    self.name, method, e
                ))
            })?;

            if page_index == 0 {
                directive = CacheDirective::from_result(&result);
            }

            let (page, next) = extract(result)?;
            items.extend(page);
            match next {
                Some(c) if !c.is_empty() => cursor = Some(c),
                _ => return Ok((items, directive)),
            }
        }

        tracing::warn!(
            server = %self.name,
            method,
            max_pages = MAX_PAGES,
            collected = items.len(),
            "MCP list pagination hit the page cap; truncating (server cursor did not terminate)"
        );
        Ok((items, directive))
    }

    /// Refresh the cached tools list
    pub async fn refresh_tools(&self) -> Result<()> {
        let (raw_tools, directive) = self
            .drain_paginated("tools/list", |result| {
                let page: mcp_types::ToolsListResult =
                    serde_json::from_value(result).map_err(|e| {
                        AlephError::IoError(format!(
                            "Failed to parse tools list from '{}': {}",
                            self.name, e
                        ))
                    })?;
                Ok((page.tools, page.next_cursor))
            })
            .await?;

        // Convert to our McpTool format. External tool metadata is untrusted:
        // normalize the input schema so strict providers do not reject malformed
        // schemas, and flag descriptions that look like prompt injection.
        let mirrors_headers = self.transport.mirrors_param_headers();
        let mut param_headers: HashMap<String, Vec<ParamHeader>> = HashMap::new();
        let tools: Vec<McpTool> = raw_tools
            .into_iter()
            .filter_map(|t| {
                // `x-mcp-header` annotations are read from the raw schema, before
                // normalization, and a malformed one takes only its own tool
                // out — the spec requires excluding it from `tools/list`, since
                // the client could not build headers the server would accept,
                // but one bad definition must not cost the server its others.
                if mirrors_headers {
                    let raw_schema = t.input_schema.as_ref().unwrap_or(&Value::Null);
                    match collect_param_headers(raw_schema) {
                        Ok(annotations) if !annotations.is_empty() => {
                            param_headers.insert(t.name.clone(), annotations);
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(
                                server = %self.name,
                                tool = %t.name,
                                reason = %e,
                                "MCP tool definition has an invalid x-mcp-header annotation; \
                                 excluding the tool"
                            );
                            return None;
                        }
                    }
                }

                let description = t.description.unwrap_or_default();
                let description = crate::mcp::tool_sanitize::truncate_description(&description);
                if let Some(marker) = crate::mcp::scan_description_for_injection(&description) {
                    tracing::warn!(
                        server = %self.name,
                        tool = %t.name,
                        marker,
                        "MCP tool description matches a prompt-injection heuristic; \
                         tool is still surfaced but flagged for review"
                    );
                }
                // Behavioral hints (MCP `ToolAnnotations`) are consumed
                // conservatively: read-only/idempotent only relax scheduling
                // when explicitly true; an explicit destructive hint routes
                // the tool through the user-confirmation gate. Absent
                // annotations behave exactly like the pre-annotation default
                // (exclusive, no confirmation).
                let annotations = t.annotations.unwrap_or_default();
                Some(McpTool {
                    name: format!("{}:{}", self.name, t.name), // Namespace with server name
                    description,
                    input_schema: crate::mcp::normalize_tool_schema(
                        t.input_schema.unwrap_or_else(|| json!({"type": "object"})),
                    ),
                    requires_confirmation: annotations.is_destructive(),
                    read_only: annotations.is_read_only(),
                    idempotent: annotations.is_idempotent(),
                })
            })
            .collect();

        tracing::debug!(
            server = %self.name,
            tool_count = tools.len(),
            "Cached tools list"
        );

        // Update cache
        {
            let mut cached = self.cached_tools.write().await;
            *cached = tools;
        }
        {
            let mut headers = self.param_headers.write().await;
            *headers = param_headers;
        }
        self.cache_expiry.write().await.tools = directive.expires_at(Instant::now());

        Ok(())
    }

    /// Refresh the cached resources list
    pub async fn refresh_resources(&self) -> Result<()> {
        // Check if server supports resources
        let caps = self.capabilities.read().await;
        if caps.as_ref().and_then(|c| c.resources.as_ref()).is_none() {
            tracing::debug!(server = %self.name, "Server does not support resources");
            return Ok(());
        }
        drop(caps);

        let (raw_resources, directive) = self
            .drain_paginated("resources/list", |result| {
                let page: mcp_types::ResourcesListResult =
                    serde_json::from_value(result).map_err(|e| {
                        AlephError::IoError(format!(
                            "Failed to parse resources list from '{}': {}",
                            self.name, e
                        ))
                    })?;
                Ok((page.resources, page.next_cursor))
            })
            .await?;

        // Convert to our McpResource format
        let resources: Vec<crate::mcp::types::McpResource> = raw_resources
            .into_iter()
            .map(|r| crate::mcp::types::McpResource {
                uri: format!("{}:{}", self.name, r.uri), // Namespace with server
                name: r.name,
                description: r.description,
                mime_type: r.mime_type,
            })
            .collect();

        tracing::debug!(
            server = %self.name,
            resource_count = resources.len(),
            "Cached resources list"
        );

        {
            let mut cached = self.cached_resources.write().await;
            *cached = resources;
        }
        self.cache_expiry.write().await.resources = directive.expires_at(Instant::now());

        Ok(())
    }

    /// Refresh the cached resource-templates list (`resources/templates/list`).
    ///
    /// Templates live under the same `resources` capability as concrete
    /// resources. Many servers that support resources do not implement the
    /// templates method; a drain error is non-fatal (the caller logs it at
    /// debug) and simply leaves the cache empty. The stored `uri_template`
    /// keeps its raw RFC-6570 form (NO `server:` prefix) — it is a pattern the
    /// model fills in, then reads as `mcp_read_resource(uri = "<server>:<filled>")`.
    pub async fn refresh_resource_templates(&self) -> Result<()> {
        // Templates are advertised under the resources capability.
        let caps = self.capabilities.read().await;
        if caps.as_ref().and_then(|c| c.resources.as_ref()).is_none() {
            tracing::debug!(server = %self.name, "Server does not support resources (skip templates)");
            return Ok(());
        }
        drop(caps);

        let (raw_templates, directive) = self
            .drain_paginated("resources/templates/list", |result| {
                let page: mcp_types::ResourceTemplatesListResult = serde_json::from_value(result)
                    .map_err(|e| {
                    AlephError::IoError(format!(
                        "Failed to parse resource templates list from '{}': {}",
                        self.name, e
                    ))
                })?;
                Ok((page.resource_templates, page.next_cursor))
            })
            .await?;

        // Convert to our McpResourceTemplate format (raw pattern, no prefix).
        let templates: Vec<crate::mcp::types::McpResourceTemplate> = raw_templates
            .into_iter()
            .map(|t| crate::mcp::types::McpResourceTemplate {
                uri_template: t.uri_template,
                name: t.name,
                description: t.description,
                mime_type: t.mime_type,
            })
            .collect();

        tracing::debug!(
            server = %self.name,
            resource_template_count = templates.len(),
            "Cached resource templates list"
        );

        {
            let mut cached = self.cached_resource_templates.write().await;
            *cached = templates;
        }
        self.cache_expiry.write().await.resource_templates = directive.expires_at(Instant::now());

        Ok(())
    }

    /// Refresh the cached prompts list
    pub async fn refresh_prompts(&self) -> Result<()> {
        // Check if server supports prompts
        let caps = self.capabilities.read().await;
        if caps.as_ref().and_then(|c| c.prompts.as_ref()).is_none() {
            tracing::debug!(server = %self.name, "Server does not support prompts");
            return Ok(());
        }
        drop(caps);

        let (raw_prompts, directive) = self
            .drain_paginated("prompts/list", |result| {
                let page: mcp_types::PromptsListResult =
                    serde_json::from_value(result).map_err(|e| {
                        AlephError::IoError(format!(
                            "Failed to parse prompts list from '{}': {}",
                            self.name, e
                        ))
                    })?;
                Ok((page.prompts, page.next_cursor))
            })
            .await?;

        // Convert to our McpPrompt format
        let prompts: Vec<crate::mcp::prompts::McpPrompt> = raw_prompts
            .into_iter()
            .map(|p| crate::mcp::prompts::McpPrompt {
                name: format!("{}:{}", self.name, p.name), // Namespace with server
                description: p.description,
                arguments: p
                    .arguments
                    .into_iter()
                    .map(|a| crate::mcp::prompts::McpPromptArgument {
                        name: a.name,
                        description: a.description,
                        required: a.required,
                    })
                    .collect(),
            })
            .collect();

        tracing::debug!(
            server = %self.name,
            prompt_count = prompts.len(),
            "Cached prompts list"
        );

        {
            let mut cached = self.cached_prompts.write().await;
            *cached = prompts;
        }
        self.cache_expiry.write().await.prompts = directive.expires_at(Instant::now());

        Ok(())
    }

    /// Re-fetch any cached list whose server-supplied `ttlMs` has lapsed, and
    /// report which of them actually came back different.
    ///
    /// `2026-07-28` removed the always-on server-to-client stream that carried
    /// `listChanged`; a client that has not opened a `subscriptions/listen`
    /// stream has the TTL hint as its only freshness signal. A server that
    /// supplies no TTL — every pre-`2026-07-28` one — never becomes stale here,
    /// so this costs those connections nothing.
    ///
    /// The *changed* half matters as much as the refresh. A lapsed TTL means
    /// "you may no longer assume this is fresh", not "this changed", so
    /// announcing a change on every expiry would make the tool registry
    /// re-sync on a timer forever. Only a list whose content actually differs
    /// is reported, and the caller turns that into the same list-changed signal
    /// a server notification would have produced.
    pub async fn refresh_expired_lists(&self) -> ChangedLists {
        let now = Instant::now();
        let expiry = {
            let guard = self.cache_expiry.read().await;
            (
                guard.tools,
                guard.resources,
                guard.resource_templates,
                guard.prompts,
            )
        };
        let mut changed = ChangedLists::default();

        if is_stale(expiry.0, now) {
            let before = fingerprint(&*self.cached_tools.read().await);
            match self.refresh_tools().await {
                Ok(()) => {
                    changed.tools = fingerprint(&*self.cached_tools.read().await) != before;
                }
                Err(e) => {
                    tracing::debug!(server = %self.name, error = %e, "Expired tools list refresh failed");
                }
            }
        }

        // Templates live under the resources capability and share its
        // list-changed signal, so either going stale refreshes both.
        if is_stale(expiry.1, now) || is_stale(expiry.2, now) {
            let before = (
                fingerprint(&*self.cached_resources.read().await),
                fingerprint(&*self.cached_resource_templates.read().await),
            );
            if let Err(e) = self.refresh_resources().await {
                tracing::debug!(server = %self.name, error = %e, "Expired resources list refresh failed");
            }
            if let Err(e) = self.refresh_resource_templates().await {
                tracing::debug!(server = %self.name, error = %e, "Expired resource templates refresh failed");
            }
            let after = (
                fingerprint(&*self.cached_resources.read().await),
                fingerprint(&*self.cached_resource_templates.read().await),
            );
            changed.resources = after != before;
        }

        if is_stale(expiry.3, now) {
            let before = fingerprint(&*self.cached_prompts.read().await);
            match self.refresh_prompts().await {
                Ok(()) => {
                    changed.prompts = fingerprint(&*self.cached_prompts.read().await) != before;
                }
                Err(e) => {
                    tracing::debug!(server = %self.name, error = %e, "Expired prompts list refresh failed");
                }
            }
        }

        changed
    }

    /// Get cached tools list
    pub async fn list_tools(&self) -> Vec<McpTool> {
        // rust-doctor-disable-next-line excessive-clone
        self.cached_tools.read().await.clone()
    }

    /// Get cached resources list
    pub async fn list_resources(&self) -> Vec<crate::mcp::types::McpResource> {
        // rust-doctor-disable-next-line excessive-clone
        self.cached_resources.read().await.clone()
    }

    /// Get cached resource-templates list
    pub async fn list_resource_templates(&self) -> Vec<crate::mcp::types::McpResourceTemplate> {
        // rust-doctor-disable-next-line excessive-clone
        self.cached_resource_templates.read().await.clone()
    }

    /// Get cached prompts list
    pub async fn list_prompts(&self) -> Vec<crate::mcp::prompts::McpPrompt> {
        // rust-doctor-disable-next-line excessive-clone
        self.cached_prompts.read().await.clone()
    }

    /// Get server-provided instructions (if any).
    pub async fn instructions(&self) -> Option<String> {
        // rust-doctor-disable-next-line excessive-clone
        self.cached_instructions.read().await.clone()
    }

    /// Check if this connection provides a specific tool
    pub async fn has_tool(&self, name: &str) -> bool {
        // Check with and without namespace prefix
        let full_name = if name.starts_with(&format!("{}:", self.name)) {
            name.to_string()
        } else {
            format!("{}:{}", self.name, name)
        };

        self.cached_tools
            .read()
            .await
            .iter()
            .any(|t| t.name == full_name || t.name == name)
    }

    /// Answer the input requests a server attached to an interim result.
    ///
    /// Only capabilities Aleph declared can appear here (a conformant server
    /// must not ask for others), so in practice this dispatches sampling.
    async fn fulfill_input_requests(
        &self,
        interim: &InputRequired,
    ) -> Result<serde_json::Map<String, Value>> {
        let mut responses = serde_json::Map::new();
        if !interim.needs_input() {
            return Ok(responses);
        }

        // rust-doctor-disable-next-line excessive-clone
        let handler = self.sampling.read().await.clone();
        let Some(handler) = handler else {
            return Err(AlephError::IoError(format!(
                "MCP server '{}' asked for additional input, but no sampling handler \
                 is installed on this connection",
                self.name
            )));
        };

        for (key, request) in &interim.requests {
            let answer = mrtr::fulfill(key, request, &handler, &self.name).await?;
            responses.insert(key.clone(), answer);
        }
        Ok(responses)
    }

    /// Send a request that may come back asking for more input, and drive it to
    /// a final result.
    ///
    /// This is the Multi Round-Trip Requests loop. Each leg is an independent
    /// request — a fresh JSON-RPC id, the *original* params plus this round's
    /// answers, and the server's opaque `requestState` echoed back untouched.
    /// Servers that carry no such state simply never take the retry branch, so
    /// legacy connections walk straight through.
    async fn send_with_mrtr(
        &self,
        method: &str,
        original_params: Value,
        extra_headers: &[(String, String)],
        context: &str,
    ) -> Result<Value> {
        debug_assert!(
            mrtr::supports_input_required(method),
            "MRTR is defined only for tools/call, resources/read, and prompts/get"
        );

        // rust-doctor-disable-next-line excessive-clone
        let mut attempt = original_params.clone();

        for _ in 0..mrtr::MAX_ROUNDS {
            let request = self.request(method, Some(attempt));
            let response = match self
                .transport
                .send_request_with_headers(&request, extra_headers)
                .await
            {
                Ok(response) => response,
                Err(e) => {
                    // Classify for actionable diagnostics, then surface the
                    // original error unchanged (variant-preserving).
                    let kind = crate::mcp::classify_mcp_error(&e.to_string());
                    tracing::warn!(
                        server = %self.name,
                        method,
                        error_kind = kind.as_str(),
                        error = %e,
                        "MCP request failed at transport"
                    );
                    return Err(e);
                }
            };

            let result = response.into_result().map_err(|e| {
                let kind = crate::mcp::classify_mcp_error(&e.to_string());
                AlephError::IoError(format!("{context} failed: {e}{}", kind.guidance_suffix()))
            })?;

            let Some(interim) = InputRequired::from_result(&result) else {
                return Ok(result);
            };

            tracing::debug!(
                server = %self.name,
                method,
                requested = interim.requests.len(),
                "MCP server requested additional input; retrying"
            );

            let responses = self.fulfill_input_requests(&interim).await?;
            attempt = mrtr::retry_params(
                &original_params,
                responses,
                interim.request_state.as_deref(),
            );
        }

        Err(AlephError::IoError(format!(
            "{context} did not complete: MCP server '{}' asked for more input \
             {} times without producing a result",
            self.name,
            mrtr::MAX_ROUNDS
        )))
    }

    /// Call a tool on this server
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        // Strip server namespace prefix if present
        let tool_name = strip_server_prefix(name, &self.name);

        // Parameters the server asked to have mirrored into HTTP headers. Read
        // from the schema captured when the tool list was refreshed, so the
        // headers describe the arguments actually being sent.
        let extra_headers = {
            let annotations = self.param_headers.read().await;
            annotations
                .get(tool_name)
                .map(|a| extract_param_headers(a, &arguments))
                .unwrap_or_default()
        };

        let params = mcp_types::ToolCallParams {
            name: tool_name.to_string(),
            arguments: Some(arguments),
        };
        let params = serde_json::to_value(&params).map_err(|e| {
            AlephError::IoError(format!("Failed to serialize tool call params: {e}"))
        })?;

        tracing::debug!(
            server = %self.name,
            tool = %tool_name,
            "Calling tool"
        );

        let result = self
            .send_with_mrtr(
                "tools/call",
                params,
                &extra_headers,
                &format!("Tool call '{}' on '{}'", tool_name, self.name),
            )
            .await?;

        // Parse tool call result
        let call_result: mcp_types::ToolCallResult =
            serde_json::from_value(result).map_err(|e| {
                AlephError::IoError(format!(
                    "Tool '{}' returned malformed result from '{}': {}",
                    tool_name, self.name, e
                ))
            })?;

        // Convert result to Value
        if call_result.is_error == Some(true) {
            let error_text = call_result
                .content
                .into_iter()
                .filter_map(|c| match c {
                    mcp_types::ToolResultContent::Text { text } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");

            // A tool's `isError: true` verdict is the tool's answer to the
            // call. The classifier (`classify_mcp_error`) is tuned for
            // transport-level messages ("connection reset", "401",
            // "session expired"); applying it to a tool verdict produces
            // misleading recovery hints ("the MCP session expired — it
            // will reconnect on the next health probe" for a tool that
            // merely returned the substring "session expired"). Surface
            // the tool's text verbatim and let the transport classifier
            // do its job on the transport-error path further down.
            return Err(AlephError::IoError(format!(
                "Tool '{}{}{}",
                tool_name, TOOL_ERROR_MARKER, error_text
            )));
        }

        // Extract content from result. Unknown blocks become an explicit
        // marker so the model knows something was elided rather than silently
        // seeing a shorter result.
        let content: Vec<Value> = call_result
            .content
            .into_iter()
            .map(|c| match c {
                mcp_types::ToolResultContent::Text { text } => {
                    json!({"type": "text", "text": text})
                }
                mcp_types::ToolResultContent::Image { data, mime_type } => {
                    json!({"type": "image", "data": data, "mimeType": mime_type})
                }
                mcp_types::ToolResultContent::Audio { data, mime_type } => {
                    json!({"type": "audio", "data": data, "mimeType": mime_type})
                }
                mcp_types::ToolResultContent::ResourceLink {
                    uri,
                    name,
                    description,
                } => {
                    json!({"type": "resource_link", "uri": uri, "name": name, "description": description})
                }
                mcp_types::ToolResultContent::Resource { resource } => {
                    json!({
                        "type": "resource",
                        "uri": resource.uri,
                        "mimeType": resource.mime_type,
                        "text": resource.text,
                        "blob": resource.blob,
                    })
                }
                mcp_types::ToolResultContent::Unknown => {
                    json!({"type": "text", "text": "[unsupported MCP content type omitted]"})
                }
            })
            .collect();

        Ok(json!({
            "content": content,
        }))
    }

    /// Read a resource by URI
    pub async fn read_resource(&self, uri: &str) -> Result<crate::mcp::resources::ResourceContent> {
        // Strip server namespace prefix if present
        let resource_uri = strip_server_prefix(uri, &self.name);

        let params = mcp_types::ResourceReadParams {
            uri: resource_uri.to_string(),
        };

        let params = serde_json::to_value(&params).map_err(|e| {
            AlephError::IoError(format!("Failed to serialize resource read params: {e}"))
        })?;

        tracing::debug!(
            server = %self.name,
            uri = %resource_uri,
            "Reading resource"
        );

        let result = self
            .send_with_mrtr(
                "resources/read",
                params,
                &[],
                &format!("Resource read '{}' on '{}'", resource_uri, self.name),
            )
            .await?;

        let read_result: mcp_types::ResourceReadResult =
            serde_json::from_value(result).map_err(|e| {
                AlephError::IoError(format!(
                    "Failed to parse resource read result from '{}': {}",
                    self.name, e
                ))
            })?;

        // Convert first content item to ResourceContent
        if let Some(content) = read_result.contents.into_iter().next() {
            match content {
                mcp_types::ResourceContentItem::Text { text, .. } => {
                    Ok(crate::mcp::resources::ResourceContent::Text(text))
                }
                mcp_types::ResourceContentItem::Blob {
                    blob, mime_type, ..
                } => {
                    // Decode base64
                    use base64::Engine;
                    let data = base64::engine::general_purpose::STANDARD
                        .decode(&blob)
                        .map_err(|e| AlephError::IoError(format!("Failed to decode blob: {e}")))?;
                    Ok(crate::mcp::resources::ResourceContent::Binary {
                        data,
                        mime_type: mime_type
                            .unwrap_or_else(|| "application/octet-stream".to_string()),
                    })
                }
            }
        } else {
            Ok(crate::mcp::resources::ResourceContent::Text(String::new()))
        }
    }

    /// Get a prompt by name with optional arguments
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<std::collections::HashMap<String, serde_json::Value>>,
    ) -> Result<crate::mcp::prompts::PromptResult> {
        // Strip server namespace prefix if present
        let prompt_name = strip_server_prefix(name, &self.name);

        let params = mcp_types::PromptGetParams {
            name: prompt_name.to_string(),
            arguments,
        };

        let params = serde_json::to_value(&params).map_err(|e| {
            AlephError::IoError(format!("Failed to serialize prompt get params: {e}"))
        })?;

        tracing::debug!(
            server = %self.name,
            prompt = %prompt_name,
            "Getting prompt"
        );

        let result = self
            .send_with_mrtr(
                "prompts/get",
                params,
                &[],
                &format!("Prompt get '{}' on '{}'", prompt_name, self.name),
            )
            .await?;

        let get_result: mcp_types::PromptGetResult =
            serde_json::from_value(result).map_err(|e| {
                AlephError::IoError(format!(
                    "Failed to parse prompt get result from '{}': {}",
                    self.name, e
                ))
            })?;

        // Convert to our PromptResult format
        let messages = get_result
            .messages
            .into_iter()
            .map(|m| {
                let content = match m.content {
                    mcp_types::PromptContentItem::Text { text } => {
                        crate::mcp::prompts::PromptContent::Text { text }
                    }
                    mcp_types::PromptContentItem::Image { data, mime_type } => {
                        crate::mcp::prompts::PromptContent::Image { data, mime_type }
                    }
                    // Audio has no internal counterpart; degrade to a text
                    // marker rather than dropping the message.
                    mcp_types::PromptContentItem::Audio { mime_type, .. } => {
                        crate::mcp::prompts::PromptContent::Text {
                            text: format!("[audio content: {mime_type}]"),
                        }
                    }
                    mcp_types::PromptContentItem::Resource { resource } => {
                        crate::mcp::prompts::PromptContent::Resource {
                            uri: resource.uri,
                            text: resource.text,
                        }
                    }
                    mcp_types::PromptContentItem::Unknown => {
                        crate::mcp::prompts::PromptContent::Text {
                            text: "[unsupported MCP content type omitted]".to_string(),
                        }
                    }
                };
                crate::mcp::prompts::PromptMessage {
                    role: m.role,
                    content,
                }
            })
            .collect();

        Ok(crate::mcp::prompts::PromptResult {
            description: get_result.description,
            messages,
        })
    }

    /// Get server name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Check if server is running
    pub async fn is_running(&self) -> bool {
        self.transport.is_alive().await
    }

    /// Active liveness probe: send an MCP `ping` and report whether the server
    /// is reachable over the wire.
    ///
    /// Any JSON-RPC reply — a success *or* a `method not found` error — proves
    /// the transport round-trips, so a server that does not implement `ping` is
    /// still counted alive. Only a transport-level failure (timeout, refused,
    /// broken pipe) reports unreachable. This is the only real signal for a
    /// stateless HTTP transport, whose [`is_running`](Self::is_running) can do
    /// no better than always return `true`; it doubles as a keepalive that
    /// holds long-lived connections open against idle disconnects.
    /// Revision `2026-07-28` removed `ping`, so modern connections probe with
    /// `server/discover` instead — the cheapest method every modern server is
    /// required to implement, and one whose answer is genuinely informative
    /// rather than merely non-empty.
    ///
    /// If the modern probe is rejected with `Method not found` (a server that
    /// returned `server/discover` once and then lost the method), fall back to
    /// the transport's passive liveness check rather than reporting unreachable.
    /// The bridge's health probe escalates consecutive failures to a restart;
    /// a single false negative here can take a working server down.
    pub async fn ping(&self) -> bool {
        let method = if self.is_modern() {
            DISCOVER_METHOD
        } else {
            "ping"
        };
        let request = self.request(method, None);
        match self.transport.send_request(&request).await {
            Ok(_) => true,
            Err(e) => {
                let kind = crate::mcp::classify_mcp_error(&e.to_string());
                if self.is_modern()
                    && matches!(
                        kind,
                        crate::mcp::McpErrorKind::Unknown | crate::mcp::McpErrorKind::Transient
                    )
                {
                    // Modern probe was rejected or timed out; the transport
                    // is still alive, so report as such to avoid an
                    // unnecessary restart.
                    tracing::debug!(
                        server = %self.name,
                        error = %e,
                        "modern ping probe inconclusive; falling back to transport liveness"
                    );
                    self.transport.is_alive().await
                } else {
                    false
                }
            }
        }
    }

    /// Install a handler for server-initiated notifications on this
    /// connection's transport (e.g. `notifications/tools/list_changed`).
    ///
    /// Transports that cannot receive notifications keep the default no-op.
    pub fn set_notification_handler(&self, handler: crate::mcp::transport::NotificationCallback) {
        self.transport.set_notification_handler(handler);
    }

    /// Close the connection
    pub async fn close(&self) -> Result<()> {
        tracing::info!(server = %self.name, "Closing MCP connection");

        self.transport.close().await
    }
}

#[cfg(test)]
mod tests {

    /// The marker must separate a tool's verdict from every other `IoError`
    /// this layer produces — that is its only job, and getting it wrong turns
    /// "the page never showed the text" into "the browser is unreachable".
    #[test]
    fn only_a_tool_verdict_is_recognised_as_one() {
        let verdict = format!(
            "Tool '{}{}{}",
            "wait_for", TOOL_ERROR_MARKER, "Error: Timed out after waiting 2000ms"
        );
        assert!(is_tool_error(&verdict));

        // A dead pipe is not a verdict.
        assert!(!is_tool_error("broken pipe (os error 32)"));
        // Neither is a protocol failure, even though it also names the tool.
        assert!(!is_tool_error(
            "Tool 'wait_for' returned malformed result from 'srv': EOF while parsing a value"
        ));
    }
    use super::*;

    use std::collections::VecDeque;

    use crate::mcp::jsonrpc::{JsonRpcError, JsonRpcResponse};
    use crate::mcp::modern::{META_FIELD, META_PROTOCOL_VERSION};
    use async_trait::async_trait;
    use tokio::sync::Mutex;

    /// A transport that answers from a per-method script and records what it
    /// was asked, so a test can assert on the *wire* rather than on internals.
    struct ScriptedTransport {
        name: String,
        replies: Mutex<HashMap<String, VecDeque<std::result::Result<Value, JsonRpcError>>>>,
        seen: Mutex<Vec<JsonRpcRequest>>,
        notifications: Mutex<Vec<String>>,
        headers_seen: Mutex<Vec<(String, Vec<(String, String)>)>>,
        dialect: std::sync::Mutex<Option<McpDialect>>,
        mirrors_headers: bool,
    }

    impl ScriptedTransport {
        fn new() -> Self {
            Self {
                name: "scripted".to_string(),
                replies: Mutex::new(HashMap::new()),
                seen: Mutex::new(Vec::new()),
                notifications: Mutex::new(Vec::new()),
                headers_seen: Mutex::new(Vec::new()),
                dialect: std::sync::Mutex::new(None),
                mirrors_headers: false,
            }
        }

        fn mirroring() -> Self {
            Self {
                mirrors_headers: true,
                ..Self::new()
            }
        }

        /// Queue one answer for `method`. Answers are consumed in order, so a
        /// method can behave differently on a retry.
        async fn push(
            self: &Arc<Self>,
            method: &str,
            reply: std::result::Result<Value, JsonRpcError>,
        ) {
            self.replies
                .lock()
                .await
                .entry(method.to_string())
                .or_default()
                .push_back(reply);
        }

        /// The default script for a modern server: discovery plus an empty tool
        /// list, which is what `handshake` fetches before returning.
        async fn script_modern(self: &Arc<Self>) {
            self.push(
                DISCOVER_METHOD,
                Ok(json!({
                    "resultType": "complete",
                    "supportedVersions": [MCP_MODERN_PROTOCOL_VERSION],
                    "capabilities": {"tools": {}},
                    "_meta": {"io.modelcontextprotocol/serverInfo": {"name": "s", "version": "1"}}
                })),
            )
            .await;
            self.push(
                "tools/list",
                Ok(json!({"resultType": "complete", "tools": []})),
            )
            .await;
        }

        /// The default script for a legacy server: `server/discover` is an
        /// unknown method, then the handshake proceeds.
        async fn script_legacy(self: &Arc<Self>) {
            self.push(
                DISCOVER_METHOD,
                Err(JsonRpcError {
                    code: -32601,
                    message: "Method not found".to_string(),
                    data: None,
                }),
            )
            .await;
            self.push(
                "initialize",
                Ok(json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "s", "version": "1"}
                })),
            )
            .await;
            self.push("tools/list", Ok(json!({"tools": []}))).await;
        }

        async fn methods_seen(&self) -> Vec<String> {
            self.seen
                .lock()
                .await
                .iter()
                .map(|r| r.method.clone())
                .collect()
        }

        async fn requests_for(&self, method: &str) -> Vec<JsonRpcRequest> {
            self.seen
                .lock()
                .await
                .iter()
                .filter(|r| r.method == method)
                .cloned()
                .collect()
        }
    }

    #[async_trait]
    impl McpTransport for ScriptedTransport {
        async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
            self.seen.lock().await.push(request.clone());

            let reply = self
                .replies
                .lock()
                .await
                .get_mut(&request.method)
                .and_then(VecDeque::pop_front);

            let (result, error) = match reply {
                Some(Ok(value)) => (Some(value), None),
                Some(Err(e)) => (None, Some(e)),
                None => (
                    None,
                    Some(JsonRpcError {
                        code: -32601,
                        message: format!("no scripted reply for {}", request.method),
                        data: None,
                    }),
                ),
            };

            Ok(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: Some(request.id),
                result,
                error,
            })
        }

        async fn send_request_with_headers(
            &self,
            request: &JsonRpcRequest,
            extra_headers: &[(String, String)],
        ) -> Result<JsonRpcResponse> {
            self.headers_seen
                .lock()
                .await
                .push((request.method.clone(), extra_headers.to_vec()));
            self.send_request(request).await
        }

        async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()> {
            self.notifications
                .lock()
                .await
                .push(notification.method.clone());
            Ok(())
        }

        async fn is_alive(&self) -> bool {
            true
        }

        async fn close(&self) -> Result<()> {
            Ok(())
        }

        fn server_name(&self) -> &str {
            &self.name
        }

        fn mirrors_param_headers(&self) -> bool {
            self.mirrors_headers
        }

        fn set_dialect(&self, dialect: &McpDialect) {
            *self.dialect.lock().unwrap_or_else(|e| e.into_inner()) = Some(dialect.clone());
        }
    }

    async fn connect_with(transport: &Arc<ScriptedTransport>) -> Result<McpServerConnection> {
        McpServerConnection::with_transport(
            "scripted",
            Arc::clone(transport) as Arc<dyn McpTransport>,
            None,
        )
        .await
    }

    #[tokio::test]
    async fn modern_server_skips_the_handshake() {
        let transport = Arc::new(ScriptedTransport::new());
        transport.script_modern().await;

        let conn = connect_with(&transport).await.unwrap();

        assert_eq!(
            conn.dialect.get(),
            Some(&McpDialect::Modern {
                version: MCP_MODERN_PROTOCOL_VERSION.to_string()
            })
        );
        let methods = transport.methods_seen().await;
        assert!(!methods.contains(&"initialize".to_string()), "{methods:?}");
        assert!(transport.notifications.lock().await.is_empty());
    }

    #[tokio::test]
    async fn modern_requests_all_carry_the_required_meta() {
        // There is no handshake left to state the protocol version once, so
        // every single request has to carry it or the server rejects it.
        let transport = Arc::new(ScriptedTransport::new());
        transport.script_modern().await;

        connect_with(&transport).await.unwrap();

        let seen = transport.seen.lock().await;
        assert!(!seen.is_empty());
        for request in seen.iter() {
            let meta = request
                .params
                .as_ref()
                .and_then(|p| p.get(META_FIELD))
                .unwrap_or_else(|| panic!("{} carried no _meta", request.method));
            assert_eq!(meta[META_PROTOCOL_VERSION], MCP_MODERN_PROTOCOL_VERSION);
        }
    }

    #[tokio::test]
    async fn legacy_server_falls_back_to_the_handshake() {
        let transport = Arc::new(ScriptedTransport::new());
        transport.script_legacy().await;

        let conn = connect_with(&transport).await.unwrap();

        assert_eq!(
            conn.dialect.get(),
            Some(&McpDialect::Legacy {
                version: "2025-03-26".to_string()
            })
        );
        let methods = transport.methods_seen().await;
        assert!(methods.contains(&"initialize".to_string()), "{methods:?}");
        assert_eq!(
            transport.notifications.lock().await.as_slice(),
            ["notifications/initialized"]
        );
    }

    #[tokio::test]
    async fn legacy_requests_carry_no_modern_meta() {
        // The legacy path must be byte-identical to what it was before the
        // modern one existed.
        let transport = Arc::new(ScriptedTransport::new());
        transport.script_legacy().await;

        connect_with(&transport).await.unwrap();

        for request in transport.requests_for("tools/list").await {
            let carries_meta = request
                .params
                .as_ref()
                .is_some_and(|p| p.get(META_FIELD).is_some());
            assert!(!carries_meta, "legacy tools/list carried _meta");
        }
    }

    #[tokio::test]
    async fn a_spec_reserved_error_identifies_a_modern_server() {
        // Even when discovery fails, a spec-reserved code proves the peer is
        // modern — falling back to `initialize` there would be wrong.
        let transport = Arc::new(ScriptedTransport::new());
        transport
            .push(
                DISCOVER_METHOD,
                Err(JsonRpcError {
                    code: crate::mcp::modern::error_codes::UNSUPPORTED_PROTOCOL_VERSION,
                    message: "Unsupported protocol version".to_string(),
                    data: Some(json!({
                        "supported": [MCP_MODERN_PROTOCOL_VERSION, "2025-11-25"],
                        "requested": "1900-01-01"
                    })),
                }),
            )
            .await;
        transport
            .push(
                "tools/list",
                Ok(json!({"resultType": "complete", "tools": []})),
            )
            .await;

        let conn = connect_with(&transport).await.unwrap();

        assert!(conn.dialect.get().is_some_and(McpDialect::is_modern));
        assert!(!transport
            .methods_seen()
            .await
            .contains(&"initialize".to_string()));
    }

    #[tokio::test]
    async fn an_era_settled_by_an_error_still_learns_the_capabilities() {
        // A spec-reserved error proves the peer is modern but says nothing
        // about what it serves. Leaving capabilities empty would silently skip
        // resources and prompts for the life of the connection.
        let transport = Arc::new(ScriptedTransport::new());
        transport
            .push(
                DISCOVER_METHOD,
                Err(JsonRpcError {
                    code: crate::mcp::modern::error_codes::MISSING_REQUIRED_CLIENT_CAPABILITY,
                    message: "need something".to_string(),
                    data: None,
                }),
            )
            .await;
        transport
            .push(
                DISCOVER_METHOD,
                Ok(json!({
                    "resultType": "complete",
                    "supportedVersions": [MCP_MODERN_PROTOCOL_VERSION],
                    "capabilities": {"tools": {}, "resources": {}},
                    "instructions": "use me well"
                })),
            )
            .await;
        transport
            .push(
                "tools/list",
                Ok(json!({"resultType": "complete", "tools": []})),
            )
            .await;
        transport
            .push(
                "resources/list",
                Ok(json!({
                    "resultType": "complete",
                    "resources": [{"uri": "file:///a", "name": "a"}]
                })),
            )
            .await;
        transport
            .push(
                "resources/templates/list",
                Ok(json!({"resultType": "complete", "resourceTemplates": []})),
            )
            .await;

        let conn = connect_with(&transport).await.unwrap();

        assert!(conn.dialect.get().is_some_and(McpDialect::is_modern));
        assert_eq!(conn.instructions().await.as_deref(), Some("use me well"));
        // The resources capability was learned on the second ask, so the
        // resource list was actually fetched.
        assert_eq!(conn.list_resources().await.len(), 1);
        assert!(!transport
            .methods_seen()
            .await
            .contains(&"initialize".to_string()));
    }

    #[tokio::test]
    async fn a_server_offering_only_older_revisions_uses_the_handshake() {
        let transport = Arc::new(ScriptedTransport::new());
        transport
            .push(
                DISCOVER_METHOD,
                Ok(json!({
                    "resultType": "complete",
                    "supportedVersions": ["2025-06-18", "2025-11-25"]
                })),
            )
            .await;
        transport
            .push(
                "initialize",
                Ok(json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {}
                })),
            )
            .await;
        transport.push("tools/list", Ok(json!({"tools": []}))).await;

        let conn = connect_with(&transport).await.unwrap();

        assert!(conn.dialect.get().is_some_and(|d| !d.is_modern()));
    }

    #[tokio::test]
    async fn no_shared_revision_fails_the_connection_with_the_reason() {
        let transport = Arc::new(ScriptedTransport::new());
        transport
            .push(
                DISCOVER_METHOD,
                Ok(json!({
                    "resultType": "complete",
                    "supportedVersions": ["2099-01-01"]
                })),
            )
            .await;

        let Err(err) = connect_with(&transport).await else {
            panic!("connecting to a server with no shared revision should fail");
        };
        let err = err.to_string();

        assert!(err.contains("2099-01-01"), "{err}");
    }

    #[tokio::test]
    async fn mrtr_retries_with_the_answers_and_a_fresh_id() {
        let transport = Arc::new(ScriptedTransport::new());
        transport.script_modern().await;
        // First attempt: the server needs an LLM completion. Second: done.
        transport
            .push(
                "tools/call",
                Ok(json!({
                    "resultType": "input_required",
                    "inputRequests": {
                        "capital": {
                            "method": "sampling/createMessage",
                            "params": {
                                "messages": [{
                                    "role": "user",
                                    "content": {"type": "text", "text": "capital of France?"}
                                }],
                                "maxTokens": 50
                            }
                        }
                    },
                    "requestState": "opaque-blob"
                })),
            )
            .await;
        transport
            .push(
                "tools/call",
                Ok(json!({
                    "resultType": "complete",
                    "content": [{"type": "text", "text": "done"}]
                })),
            )
            .await;

        let sampling = Arc::new(SamplingHandler::new());
        sampling
            .set_callback(|_req| async { Ok(SamplingHandler::text_response("Paris")) })
            .await;
        let conn = McpServerConnection::with_transport(
            "scripted",
            Arc::clone(&transport) as Arc<dyn McpTransport>,
            Some(sampling),
        )
        .await
        .unwrap();

        let result = conn.call_tool("t", json!({"x": 1})).await.unwrap();
        assert_eq!(result["content"][0]["text"], "done");

        let calls = transport.requests_for("tools/call").await;
        assert_eq!(calls.len(), 2, "expected an initial call and one retry");

        // Independent requests, so the ids must differ.
        assert_ne!(calls[0].id, calls[1].id);

        let first = calls[0].params.as_ref().unwrap();
        assert!(first.get("inputResponses").is_none());
        assert!(first.get("requestState").is_none());

        let retry = calls[1].params.as_ref().unwrap();
        assert_eq!(retry["requestState"], "opaque-blob");
        assert_eq!(retry["inputResponses"]["capital"]["role"], "assistant");
        assert_eq!(
            retry["inputResponses"]["capital"]["content"]["text"],
            "Paris"
        );
        // The original arguments must survive the retry.
        assert_eq!(retry["arguments"]["x"], 1);
    }

    #[tokio::test]
    async fn input_requests_without_a_sampler_fail_with_a_reason() {
        let transport = Arc::new(ScriptedTransport::new());
        transport.script_modern().await;
        transport
            .push(
                "tools/call",
                Ok(json!({
                    "resultType": "input_required",
                    "inputRequests": {
                        "k": {"method": "sampling/createMessage", "params": {}}
                    }
                })),
            )
            .await;

        let conn = connect_with(&transport).await.unwrap();
        let err = conn
            .call_tool("t", json!({}))
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("no sampling handler"), "{err}");
    }

    #[tokio::test]
    async fn a_server_that_never_finishes_is_bounded() {
        let transport = Arc::new(ScriptedTransport::new());
        transport.script_modern().await;
        // Always asks for more, never produces a result.
        for _ in 0..(mrtr::MAX_ROUNDS + 2) {
            transport
                .push(
                    "tools/call",
                    Ok(json!({
                        "resultType": "input_required",
                        "requestState": "again"
                    })),
                )
                .await;
        }

        let conn = connect_with(&transport).await.unwrap();
        let err = conn
            .call_tool("t", json!({}))
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("did not complete"), "{err}");
        assert_eq!(
            transport.requests_for("tools/call").await.len(),
            mrtr::MAX_ROUNDS
        );
    }

    #[tokio::test]
    async fn a_tool_with_a_malformed_header_annotation_is_excluded() {
        let transport = Arc::new(ScriptedTransport::mirroring());
        transport
            .push(
                DISCOVER_METHOD,
                Ok(json!({
                    "resultType": "complete",
                    "supportedVersions": [MCP_MODERN_PROTOCOL_VERSION],
                    "capabilities": {"tools": {}}
                })),
            )
            .await;
        transport
            .push(
                "tools/list",
                Ok(json!({
                    "resultType": "complete",
                    "tools": [
                        {
                            "name": "good",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "region": {"type": "string", "x-mcp-header": "Region"}
                                }
                            }
                        },
                        {
                            "name": "bad",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "ratio": {"type": "number", "x-mcp-header": "Ratio"}
                                }
                            }
                        }
                    ]
                })),
            )
            .await;

        let conn = connect_with(&transport).await.unwrap();
        let names: Vec<String> = conn
            .list_tools()
            .await
            .into_iter()
            .map(|t| t.name)
            .collect();

        // One bad definition must not cost the server its working tools.
        assert_eq!(names, vec!["scripted:good".to_string()]);
    }

    #[tokio::test]
    async fn annotated_parameters_are_mirrored_into_headers() {
        let transport = Arc::new(ScriptedTransport::mirroring());
        transport
            .push(
                DISCOVER_METHOD,
                Ok(json!({
                    "resultType": "complete",
                    "supportedVersions": [MCP_MODERN_PROTOCOL_VERSION],
                    "capabilities": {"tools": {}}
                })),
            )
            .await;
        transport
            .push(
                "tools/list",
                Ok(json!({
                    "resultType": "complete",
                    "tools": [{
                        "name": "execute_sql",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "region": {"type": "string", "x-mcp-header": "Region"},
                                "query": {"type": "string"}
                            }
                        }
                    }]
                })),
            )
            .await;
        transport
            .push(
                "tools/call",
                Ok(json!({"resultType": "complete", "content": []})),
            )
            .await;

        let conn = connect_with(&transport).await.unwrap();
        conn.call_tool(
            "execute_sql",
            json!({"region": "us-west1", "query": "SELECT 1"}),
        )
        .await
        .unwrap();

        let headers = transport.headers_seen.lock().await;
        let call = headers
            .iter()
            .find(|(method, _)| method == "tools/call")
            .expect("tools/call was not sent");
        assert_eq!(
            call.1,
            vec![("mcp-param-region".to_string(), "us-west1".to_string())]
        );
    }

    // Note: Most tests require an actual MCP server to be available
    // These are basic structure tests

    #[tokio::test]
    async fn test_connect_nonexistent() {
        let result = McpServerConnection::connect(
            "test-fail",
            "/nonexistent/mcp/server",
            &[],
            &HashMap::new(),
            None,
            None,
            None,
        )
        .await;

        assert!(result.is_err());
    }
}
