//! `WasmCapabilityKernel` — per-execution security kernel.
//!
//! Every host function call passes through this kernel for:
//! - Capability checking (default-deny)
//! - Leak detection (bidirectional)
//! - Audit logging
//! - Resource counting
//!
//! The kernel owns a [`SecretResolver`](super::secret_resolver::SecretResolver)
//! that `host_functions::try_http_fetch` consults when injecting declared
//! credentials host-side — the resolver supplies the secret value, but the
//! plugin guest only sees the final URL/headers after the injector has
//! applied the binding, so "plugins never see secret values" is now a live
//! property (not a goal).

use crate::sync_primitives::{Arc, AtomicU32, Mutex, Ordering};

use crate::extension::runtime::wasm::allowlist::AllowlistValidator;
use crate::extension::runtime::wasm::capabilities::{HttpCapability, WasmCapabilities};
use crate::extension::runtime::wasm::limits::WasmResourceLimits;
use crate::extension::runtime::wasm::secret_resolver::{DenyAllSecretResolver, SecretResolver};

/// Errors from capability checks
#[derive(Debug)]
pub enum CapabilityError {
    NotDeclared(String),
    NotAllowed(String),
    RateLimited(String),
    ResourceExhausted(String),
    PathTraversal(String),
}

impl std::fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotDeclared(msg) => write!(f, "Capability not declared: {msg}"),
            Self::NotAllowed(msg) => write!(f, "Not allowed: {msg}"),
            Self::RateLimited(msg) => write!(f, "Rate limited: {msg}"),
            Self::ResourceExhausted(msg) => write!(f, "Resource exhausted: {msg}"),
            Self::PathTraversal(msg) => write!(f, "Path traversal: {msg}"),
        }
    }
}

impl std::error::Error for CapabilityError {}

/// Per-execution security kernel for WASM plugins
pub struct WasmCapabilityKernel {
    plugin_id: String,
    capabilities: WasmCapabilities,
    limits: WasmResourceLimits,
    log_count: AtomicU32,
    http_call_count: AtomicU32,
    /// Monotonic millis timestamps of recent HTTP calls, for sliding-window
    /// rate limiting. Pruned to the last hour on each `check_rate_limit`.
    http_timestamps: Mutex<Vec<u64>>,
    /// Host-side secret store consulted by `host_functions::try_http_fetch`
    /// when resolving declared `CredentialBinding`s. Defaults to
    /// [`DenyAllSecretResolver`] (every lookup returns `None`) — call
    /// [`Self::with_secret_resolver`] before exposing the kernel to a plugin
    /// that declares `http.credentials`. The resolver is `Arc`-shared with
    /// the WASM host-function closures, so `Send + Sync` is required.
    secret_resolver: Arc<dyn SecretResolver>,
}

impl WasmCapabilityKernel {
    #[must_use]
    pub fn new(
        plugin_id: String,
        capabilities: WasmCapabilities,
        limits: WasmResourceLimits,
    ) -> Self {
        Self {
            plugin_id,
            capabilities,
            limits,
            log_count: AtomicU32::new(0),
            http_call_count: AtomicU32::new(0),
            http_timestamps: Mutex::new(Vec::new()),
            secret_resolver: Arc::new(DenyAllSecretResolver),
        }
    }

    /// Install a custom secret resolver. The kernel owns an `Arc` to it; the
    /// resolver's lifetime must outlive the kernel (and any in-flight host
    /// function calls captured by the WASM runtime). Defaults to
    /// [`DenyAllSecretResolver`] until this is called.
    #[must_use]
    pub fn with_secret_resolver(mut self, resolver: Arc<dyn SecretResolver>) -> Self {
        self.secret_resolver = resolver;
        self
    }

    /// Borrow the active secret resolver. Used by `host_functions::try_http_fetch`
    /// to look up declared `CredentialBinding` values before egress.
    #[must_use]
    pub fn secret_resolver(&self) -> &dyn SecretResolver {
        self.secret_resolver.as_ref()
    }

    /// Resolve a single secret name to its plaintext value, or `None` if the
    /// resolver doesn't recognise it.
    ///
    /// With the deny-all resolver every plugin carries today, this always
    /// returns `None` — and a *matching* credential binding then fails the
    /// request closed rather than letting it "proceed unchanged", which is
    /// what this comment claimed until 2026-08-19.
    ///
    /// NOTE for whoever installs a real resolver: this applies no
    /// `check_secret_pattern` gate, and `try_http_fetch` passes
    /// `binding.secret_name` straight from the manifest. Add the gate in the
    /// same change, or `[capabilities.http.credentials]` becomes a way to name
    /// any vault key and bypass `[capabilities.secrets] allowed_patterns`.
    #[must_use]
    pub fn resolve_secret(&self, name: &str) -> Option<String> {
        self.secret_resolver.resolve(name)
    }

    pub fn check_workspace_read(&self, path: &str) -> Result<(), CapabilityError> {
        let ws = self
            .capabilities
            .workspace
            .as_ref()
            .ok_or_else(|| CapabilityError::NotDeclared("workspace".to_string()))?;
        self.validate_path(path)?;
        // Default-deny: an empty prefix list grants nothing. A declared
        // workspace capability must enumerate the prefixes it needs, otherwise
        // an omitted/typo'd prefix list would silently allow reading any
        // workspace path.
        if ws.allowed_prefixes.is_empty()
            || !ws.allowed_prefixes.iter().any(|prefix| {
                let prefix = prefix.strip_suffix('/').unwrap_or(prefix);
                !prefix.is_empty()
                    && path
                        .strip_prefix(prefix)
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            })
        {
            return Err(CapabilityError::NotAllowed(format!(
                "path '{path}' not in allowed prefixes"
            )));
        }
        Ok(())
    }

    pub fn check_secret_pattern(&self, name: &str) -> bool {
        self.capabilities
            .secrets
            .as_ref()
            .is_some_and(|s| s.is_allowed(name))
    }

    pub fn log(&self, level: &str, msg: &str) -> Result<(), CapabilityError> {
        let prev = self.log_count.fetch_add(1, Ordering::SeqCst);
        if prev >= self.limits.max_log_entries {
            self.log_count.fetch_sub(1, Ordering::SeqCst);
            return Err(CapabilityError::ResourceExhausted(
                "log entry limit exceeded".to_string(),
            ));
        }
        let msg = if msg.len() > self.limits.max_log_message_bytes {
            // Find a valid char boundary at or before the byte limit
            let mut end = self.limits.max_log_message_bytes;
            while end > 0 && !msg.is_char_boundary(end) {
                end -= 1;
            }
            &msg[..end]
        } else {
            msg
        };
        // Actually emit the (rate-limited, truncated) plugin log line. Without
        // this the entire host log function was a silent no-op.
        let plugin_id = &self.plugin_id;
        match level {
            "error" => tracing::error!(target: "wasm_plugin", plugin_id, "{}", msg),
            "warn" => tracing::warn!(target: "wasm_plugin", plugin_id, "{}", msg),
            "debug" | "trace" => tracing::debug!(target: "wasm_plugin", plugin_id, "{}", msg),
            _ => tracing::info!(target: "wasm_plugin", plugin_id, "{}", msg),
        }
        Ok(())
    }

    pub fn now_millis(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    pub fn check_http_limit(&self) -> Result<(), CapabilityError> {
        let prev = self.http_call_count.fetch_add(1, Ordering::SeqCst);
        if prev >= self.limits.max_http_calls {
            self.http_call_count.fetch_sub(1, Ordering::SeqCst);
            return Err(CapabilityError::ResourceExhausted(
                "HTTP call limit exceeded".to_string(),
            ));
        }
        Ok(())
    }

    /// Validate an outbound HTTP request against the declared `http` capability.
    ///
    /// Returns `NotDeclared` if the plugin never requested the `http`
    /// capability (default-deny), or `NotAllowed` if the request fails the
    /// allowlist (HTTPS-only, anti host-confusion, path-traversal safe).
    pub fn check_http_request(&self, method: &str, url: &str) -> Result<(), CapabilityError> {
        let http = self
            .capabilities
            .http
            .as_ref()
            .ok_or_else(|| CapabilityError::NotDeclared("http".to_string()))?;
        AllowlistValidator::new(http.allowlist.clone())
            .check(method, url)
            .map_err(|e| CapabilityError::NotAllowed(e.to_string()))
    }

    /// Enforce the declared per-minute / per-hour HTTP rate limit using a
    /// sliding window. A zero threshold (or no `rate_limit`) means unlimited.
    /// Records the current call's timestamp when the request is admitted.
    pub fn check_rate_limit(&self) -> Result<(), CapabilityError> {
        let Some(rl) = self
            .capabilities
            .http
            .as_ref()
            .and_then(|h| h.rate_limit.as_ref())
        else {
            return Ok(());
        };
        let now = self.now_millis();
        let mut stamps = self
            .http_timestamps
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Drop anything older than the longest (hour) window.
        stamps.retain(|t| now.saturating_sub(*t) < 3_600_000);
        let last_minute = u32::try_from(
            stamps
                .iter()
                .filter(|t| now.saturating_sub(**t) < 60_000)
                .count(),
        )
        .unwrap_or(u32::MAX);
        if rl.requests_per_minute > 0 && last_minute >= rl.requests_per_minute {
            return Err(CapabilityError::RateLimited(format!(
                "{} requests/minute exceeded",
                rl.requests_per_minute
            )));
        }
        let last_hour = u32::try_from(stamps.len()).unwrap_or(u32::MAX);
        if rl.requests_per_hour > 0 && last_hour >= rl.requests_per_hour {
            return Err(CapabilityError::RateLimited(format!(
                "{} requests/hour exceeded",
                rl.requests_per_hour
            )));
        }
        stamps.push(now);
        Ok(())
    }

    /// Borrow the declared HTTP capability (timeout / size caps live here),
    /// or `None` if the plugin did not request `http` access.
    #[must_use]
    pub fn http_config(&self) -> Option<&HttpCapability> {
        self.capabilities.http.as_ref()
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub const fn capabilities(&self) -> &WasmCapabilities {
        &self.capabilities
    }

    fn validate_path(&self, path: &str) -> Result<(), CapabilityError> {
        // Check raw path first
        if path.contains("..") {
            return Err(CapabilityError::PathTraversal(
                "'..' not allowed".to_string(),
            ));
        }
        if path.starts_with('/') {
            return Err(CapabilityError::PathTraversal(
                "absolute paths not allowed".to_string(),
            ));
        }
        if path.contains('\0') {
            return Err(CapabilityError::PathTraversal(
                "null bytes not allowed".to_string(),
            ));
        }

        // Also check percent-decoded form to prevent encoded traversal (%2e%2e)
        let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy();
        if decoded.contains("..") {
            return Err(CapabilityError::PathTraversal(
                "encoded '..' not allowed".to_string(),
            ));
        }
        if decoded.starts_with('/') {
            return Err(CapabilityError::PathTraversal(
                "encoded absolute path not allowed".to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::runtime::wasm::capabilities::{
        EndpointPattern, HttpCapability, RateLimit, SecretsCapability, WorkspaceCapability,
    };

    fn kernel_with_no_caps() -> WasmCapabilityKernel {
        WasmCapabilityKernel::new(
            "test-plugin".to_string(),
            WasmCapabilities::default(),
            WasmResourceLimits::default(),
        )
    }

    fn kernel_with_workspace() -> WasmCapabilityKernel {
        let caps = WasmCapabilities {
            workspace: Some(WorkspaceCapability {
                allowed_prefixes: vec!["docs/".to_string(), "config/".to_string()],
            }),
            ..Default::default()
        };
        WasmCapabilityKernel::new(
            "test-plugin".to_string(),
            caps,
            WasmResourceLimits::default(),
        )
    }

    fn kernel_with_secrets() -> WasmCapabilityKernel {
        let caps = WasmCapabilities {
            secrets: Some(SecretsCapability {
                allowed_patterns: vec!["slack_*".to_string()],
            }),
            ..Default::default()
        };
        WasmCapabilityKernel::new(
            "test-plugin".to_string(),
            caps,
            WasmResourceLimits::default(),
        )
    }

    fn kernel_with_http(rate_limit: Option<RateLimit>) -> WasmCapabilityKernel {
        let caps = WasmCapabilities {
            http: Some(HttpCapability {
                allowlist: vec![EndpointPattern {
                    host: "api.example.com".to_string(),
                    path_prefix: "/v1/".to_string(),
                    methods: vec!["GET".to_string()],
                }],
                credentials: vec![],
                rate_limit,
                timeout_secs: 30,
                max_request_bytes: 1024,
                max_response_bytes: 2048,
            }),
            ..Default::default()
        };
        WasmCapabilityKernel::new(
            "test-plugin".to_string(),
            caps,
            WasmResourceLimits::default(),
        )
    }

    #[test]
    fn test_http_request_denied_without_capability() {
        let kernel = kernel_with_no_caps();
        let result = kernel.check_http_request("GET", "https://api.example.com/v1/x");
        assert!(matches!(result, Err(CapabilityError::NotDeclared(_))));
    }

    #[test]
    fn test_http_request_allowlist_enforced() {
        let kernel = kernel_with_http(None);
        // Matches host + path + method
        assert!(kernel
            .check_http_request("GET", "https://api.example.com/v1/users")
            .is_ok());
        // Wrong host
        assert!(matches!(
            kernel.check_http_request("GET", "https://evil.com/v1/users"),
            Err(CapabilityError::NotAllowed(_))
        ));
        // Non-HTTPS is rejected by the validator
        assert!(matches!(
            kernel.check_http_request("GET", "http://api.example.com/v1/users"),
            Err(CapabilityError::NotAllowed(_))
        ));
        // http_config exposes the size caps
        assert_eq!(kernel.http_config().unwrap().max_response_bytes, 2048);
    }

    #[test]
    fn test_rate_limit_sliding_window() {
        let kernel = kernel_with_http(Some(RateLimit {
            requests_per_minute: 2,
            requests_per_hour: 100,
        }));
        assert!(kernel.check_rate_limit().is_ok());
        assert!(kernel.check_rate_limit().is_ok());
        // Third call within the same minute is rate-limited.
        assert!(matches!(
            kernel.check_rate_limit(),
            Err(CapabilityError::RateLimited(_))
        ));
    }

    #[test]
    fn test_rate_limit_noop_when_undeclared() {
        let kernel = kernel_with_http(None);
        for _ in 0..10 {
            assert!(kernel.check_rate_limit().is_ok());
        }
    }

    #[test]
    fn test_no_workspace_capability_denies_read() {
        let kernel = kernel_with_no_caps();
        let result = kernel.check_workspace_read("any/path");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CapabilityError::NotDeclared(_)
        ));
    }

    #[test]
    fn test_workspace_allowed_prefix() {
        let kernel = kernel_with_workspace();
        assert!(kernel.check_workspace_read("docs/readme.md").is_ok());
        assert!(kernel.check_workspace_read("config/app.toml").is_ok());
    }

    #[test]
    fn test_workspace_rejects_outside_prefix() {
        let kernel = kernel_with_workspace();
        let result = kernel.check_workspace_read("secrets/key.pem");
        assert!(result.is_err());
    }

    #[test]
    fn test_workspace_rejects_path_traversal() {
        let kernel = kernel_with_workspace();
        assert!(kernel
            .check_workspace_read("docs/../secrets/key.pem")
            .is_err());
        assert!(kernel.check_workspace_read("/etc/passwd").is_err());
        assert!(kernel.check_workspace_read("docs/\0hidden").is_err());
    }

    #[test]
    fn test_workspace_rejects_percent_encoded_traversal() {
        let kernel = kernel_with_workspace();
        // %2e = '.', so %2e%2e = '..'
        assert!(kernel
            .check_workspace_read("docs/%2e%2e/secrets/key.pem")
            .is_err());
        assert!(kernel
            .check_workspace_read("docs/%2E%2E/secrets/key.pem")
            .is_err());
        // Encoded absolute path: %2f = '/'
        assert!(kernel.check_workspace_read("%2fetc/passwd").is_err());
    }

    #[test]
    fn test_secret_exists_with_capability() {
        let kernel = kernel_with_secrets();
        assert!(kernel.check_secret_pattern("slack_bot_token"));
        assert!(!kernel.check_secret_pattern("aws_key"));
    }

    #[test]
    fn test_secret_exists_without_capability_denies_all() {
        let kernel = kernel_with_no_caps();
        assert!(!kernel.check_secret_pattern("anything"));
    }

    #[test]
    fn test_log_respects_limits() {
        let limits = WasmResourceLimits {
            max_log_entries: 2,
            ..Default::default()
        };
        let kernel =
            WasmCapabilityKernel::new("test".to_string(), WasmCapabilities::default(), limits);
        assert!(kernel.log("info", "first").is_ok());
        assert!(kernel.log("info", "second").is_ok());
        assert!(kernel.log("info", "third").is_err()); // limit exceeded
    }

    #[test]
    fn test_log_truncates_long_messages() {
        let limits = WasmResourceLimits {
            max_log_message_bytes: 10,
            ..Default::default()
        };
        let kernel =
            WasmCapabilityKernel::new("test".to_string(), WasmCapabilities::default(), limits);
        assert!(kernel.log("info", "this is a very long message").is_ok());
    }

    #[test]
    fn test_now_millis_returns_reasonable_value() {
        let kernel = kernel_with_no_caps();
        let ts = kernel.now_millis();
        assert!(ts > 1_767_225_600_000);
        assert!(ts < 1_893_456_000_000);
    }
}
