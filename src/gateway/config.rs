//! Gateway Configuration
//!
//! Parses and manages the Gateway configuration from TOML files.
//! Supports multi-agent setup, channel bindings, and extended features.

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::info;

/// Deserialize agents field accepting either:
/// - Legacy format: `[agents.main]` → `HashMap`<String, `AgentConfig`>
/// - New format: `[agents.defaults]` + `[[agents.list]]` → falls back to empty `HashMap`
fn deserialize_agents_compat<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, AgentConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    // Try to deserialize as the expected HashMap<String, AgentConfig>.
    // If the TOML has the new AgentsConfig format (with "defaults" and "list" keys),
    // this will fail — in that case, return an empty HashMap with a default "main" agent.
    let value = serde_json::Value::deserialize(deserializer)?;
    match serde_json::from_value::<HashMap<String, AgentConfig>>(value) {
        Ok(map) => Ok(map),
        Err(_) => {
            let mut map = HashMap::new();
            map.insert("main".to_string(), AgentConfig::default());
            Ok(map)
        }
    }
}

use super::agent_instance::AgentInstanceConfig;
use super::lane::LaneConfig;
use crate::config::PrivacyConfig;

/// Root Gateway configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayConfig {
    /// Gateway server settings
    pub gateway: GatewayServerConfig,

    /// Agent configurations (keyed by `agent_id`)
    ///
    /// Accepts either the legacy `[agents.<id>]` table format (`HashMap`)
    /// or silently ignores the newer `AgentsConfig` format (`[[agents.list]]`)
    /// which is handled by `Config.agents` instead.
    #[serde(default, deserialize_with = "deserialize_agents_compat")]
    pub agents: HashMap<String, AgentConfig>,

    /// Channel bindings (pattern -> `agent_id`)
    #[serde(default)]
    pub bindings: HashMap<String, String>,

    /// Channel connector configurations (parsed by app config, ignored here)
    #[serde(default)]
    pub channels: serde_json::Value,

    /// Sandbox configuration
    #[serde(default)]
    pub sandbox: SandboxConfig,

    /// Tool configurations
    #[serde(default)]
    pub tools: ToolsConfig,

    /// Privacy and PII filtering configuration
    #[serde(default)]
    pub privacy: PrivacyConfig,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        let mut agents = HashMap::new();
        agents.insert("main".to_string(), AgentConfig::default());

        Self {
            gateway: GatewayServerConfig::default(),
            agents,
            bindings: HashMap::new(),
            channels: serde_json::Value::Object(serde_json::Map::new()),
            sandbox: SandboxConfig::default(),
            tools: ToolsConfig::default(),
            privacy: PrivacyConfig::default(),
        }
    }
}

/// Native in-process TLS for the gateway listener. Default off → plaintext,
/// unchanged. When `enabled` with empty paths, a self-signed cert is
/// auto-generated and persisted (see [`crate::gateway::tls`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayTlsConfig {
    /// Terminate TLS in-process. Default false.
    pub enabled: bool,
    /// PEM certificate chain path. Empty + `enabled` ⇒ auto self-signed.
    pub cert_path: String,
    /// PEM private-key path. Empty + `enabled` ⇒ auto self-signed.
    pub key_path: String,
    /// Extra SAN entries (hostnames / IPs) added to the auto self-signed cert,
    /// on top of loopback + auto-discovered interface IPs. Ignored for a
    /// provided cert. Default empty.
    pub san: Vec<String>,
}

/// Trusted reverse-proxy forwarding. When `enabled`, `X-Forwarded-For` /
/// `X-Forwarded-Proto` from an immediate peer in `trusted_ips` are believed,
/// restoring the real client IP and TLS status behind a proxy. Default off.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TrustedProxyConfig {
    /// Honor forwarding headers from trusted peers. Default false.
    pub enabled: bool,
    /// Immediate-peer IPs whose `X-Forwarded-*` are trusted. Default loopback.
    pub trusted_ips: Vec<String>,
}

impl Default for TrustedProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            trusted_ips: vec!["127.0.0.1".to_string(), "::1".to_string()],
        }
    }
}

/// Gateway server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayServerConfig {
    /// Bind address
    pub host: String,
    /// Port number
    pub port: u16,
    /// Maximum concurrent connections
    pub max_connections: usize,
    /// Maximum concurrent connections from a single non-loopback remote IP.
    /// Bounds slot-exhaustion (a remote peer opening many idle sockets).
    /// `0` disables the cap; loopback is always exempt. Default 64. The
    /// struct-level `#[serde(default)]` keeps old TOML loading.
    pub max_connections_per_ip: usize,
    /// Protocol version
    pub protocol_version: u32,
    /// Extra browser origins allowed on the `/ws` upgrade — additional to the
    /// built-in same-origin / loopback / `tauri:` rules. Lives under
    /// `[gateway]`; the legacy `[gateway.auth]` table that once held it is
    /// ignored on load (see `from_toml` legacy-config test).
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// Trust every Origin on the `/ws` upgrade. Escape hatch for reverse
    /// proxy deployments. SECURITY: leaves the agent drivable by any web
    /// page the user's browser visits — keep false unless you know why.
    #[serde(default)]
    pub allow_any_origin: bool,
    /// Native in-process TLS. See [`GatewayTlsConfig`].
    #[serde(default)]
    pub tls: GatewayTlsConfig,
    /// Trusted reverse-proxy forwarding. See [`TrustedProxyConfig`].
    #[serde(default)]
    pub trusted_proxy: TrustedProxyConfig,
    /// Allow plaintext to a remote (non-loopback) client. Default `false` ⇒
    /// remote connections MUST be TLS (native or trusted-proxy https); an
    /// insecure remote is refused and the server refuses to bind a plaintext
    /// non-loopback listener. Set `true` only to knowingly restore
    /// LAN-plaintext trust.
    #[serde(default)]
    pub allow_insecure_remote: bool,
    /// Lane concurrency & channel-class priority configuration. Missing
    /// keys fall back to [`LaneConfig::default`], so old TOML files
    /// without a `[gateway.lane]` block keep loading.
    #[serde(default)]
    pub lane: LaneConfig,
    /// How often the server sends a WS-level Ping frame per connection.
    /// Detects half-open TCP sockets that the OS hasn't reaped (e.g. after
    /// a laptop sleeps). Default 30s.
    #[serde(default = "default_ping_interval_secs")]
    pub ping_interval_secs: u64,
    /// Close the connection if no inbound frame (including the auto-Pong
    /// reply) arrives within this many seconds. Must be ≥ `ping_interval_secs`.
    /// Default 90s.
    #[serde(default = "default_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    /// When true, mutating RPCs (Execute / Mutate / System lanes) MUST
    /// carry an `idempotency_key` in their params or the gateway rejects
    /// them with `IDEMPOTENCY_KEY_REQUIRED` (-32030) before lane dispatch.
    /// Read-only Query-lane calls are exempt. Default `false` so existing
    /// clients keep working; ops can flip this on to harden against
    /// double-send bugs (e.g. retried mutations after a network blip).
    #[serde(default)]
    pub require_idempotency_key: bool,
    /// Periodic `[MEMORY]` log cadence in seconds for the gateway process.
    /// `0` disables the monitor. Default 300s (5 min) — see
    /// [`crate::gateway::memory_monitor`] for the log format.
    #[serde(default = "default_memory_monitor_secs")]
    pub memory_monitor_secs: u64,
    /// Optional runtime-metadata footer (`model · tokens · cwd`) appended
    /// to the final agent reply. Disabled by default — see
    /// [`crate::gateway::runtime_footer`].
    #[serde(default)]
    pub runtime_footer: crate::gateway::runtime_footer::RuntimeFooterConfig,
    /// Channel health monitor — periodically auto-restarts wedged channels
    /// (status=Error + stale past threshold). `check_secs = 0` disables. See
    /// [`crate::gateway::channel_health_monitor`].
    #[serde(default)]
    pub channel_health: crate::gateway::channel_health_monitor::ChannelHealthConfig,
    /// Durable outbound delivery queue tuning (attempts, backoff, drain
    /// cadence, bounded length). Missing keys fall back to the historic
    /// hardcoded defaults, so old TOML files keep loading. See
    /// [`crate::gateway::delivery_queue`].
    #[serde(default)]
    pub delivery_queue: crate::gateway::delivery_queue::DeliveryQueueTomlConfig,
    /// Outbound rate-limit retry policy (how many `retry_after` waits to honor
    /// and their per-wait cap). `max_rate_limit_retries = 0` restores the
    /// legacy fire-once behavior. See
    /// [`crate::gateway::channel_registry::SendRetryPolicy`].
    #[serde(default)]
    pub send_retry: crate::gateway::channel_registry::SendRetryTomlConfig,
}

const fn default_memory_monitor_secs() -> u64 {
    crate::gateway::memory_monitor::DEFAULT_INTERVAL_SECS
}

const fn default_ping_interval_secs() -> u64 {
    30
}

const fn default_idle_timeout_secs() -> u64 {
    90
}

impl Default for GatewayServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 18790,
            max_connections: 100,
            max_connections_per_ip: 64,
            protocol_version: 1,
            allowed_origins: Vec::new(),
            allow_any_origin: false,
            tls: GatewayTlsConfig::default(),
            trusted_proxy: TrustedProxyConfig::default(),
            allow_insecure_remote: false,
            lane: LaneConfig::default(),
            ping_interval_secs: default_ping_interval_secs(),
            idle_timeout_secs: default_idle_timeout_secs(),
            require_idempotency_key: false,
            memory_monitor_secs: default_memory_monitor_secs(),
            runtime_footer: crate::gateway::runtime_footer::RuntimeFooterConfig::default(),
            channel_health: crate::gateway::channel_health_monitor::ChannelHealthConfig::default(),
            delivery_queue: crate::gateway::delivery_queue::DeliveryQueueTomlConfig::default(),
            send_retry: crate::gateway::channel_registry::SendRetryTomlConfig::default(),
        }
    }
}

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Workspace directory (supports ~ expansion)
    pub workspace: String,
    /// Primary model
    pub model: String,
    /// Fallback models
    #[serde(default)]
    pub fallback_models: Vec<String>,
    /// Maximum loop iterations
    pub max_loops: u32,
    /// Maximum total token usage per request (loop guard)
    #[serde(default)]
    pub max_tokens: Option<usize>,
    /// Custom system prompt
    pub system_prompt: Option<String>,
    /// Tool whitelist (empty = all allowed)
    #[serde(default)]
    pub tool_whitelist: Vec<String>,
    /// Tool blacklist
    #[serde(default)]
    pub tool_blacklist: Vec<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            workspace: "~/.aleph/agents/main".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            fallback_models: vec![],
            max_loops: 100,
            max_tokens: None,
            system_prompt: None,
            tool_whitelist: vec![],
            tool_blacklist: vec![],
        }
    }
}

impl AgentConfig {
    /// Convert to `AgentInstanceConfig`
    #[must_use]
    pub fn to_instance_config(&self, agent_id: &str) -> AgentInstanceConfig {
        AgentInstanceConfig {
            agent_id: agent_id.to_string(),
            display_name: None,
            workspace: expand_path(&self.workspace),
            model: self.model.clone(),
            fallback_models: self.fallback_models.clone(),
            max_loops: self.max_loops,
            max_tokens: self.max_tokens,
            system_prompt: self.system_prompt.clone(),
            tool_whitelist: self.tool_whitelist.clone(),
            tool_blacklist: self.tool_blacklist.clone(),
            agent_dir: dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                .join(format!(".aleph/agents/{agent_id}")),
            allowed_links: None,
            tool_permissions: None,
            timeout_secs: None,
        }
    }
}

// Channel connector configurations have been unified into the app Config system
// (Config.channels: HashMap<String, Value>). GatewayConfig.channels is kept as
// a raw Value to avoid parse errors — the actual parsing happens in
// Config::resolved_channels().

/// Sandbox configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    /// Enable Docker sandbox
    pub enabled: bool,
    /// Docker image for sandbox
    pub docker_image: String,
    /// Memory limit in MB
    pub memory_limit_mb: u64,
    /// CPU quota percentage
    pub cpu_quota_percent: u32,
    /// Network mode
    pub network_mode: NetworkMode,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            docker_image: "aleph-sandbox:latest".to_string(),
            memory_limit_mb: 512,
            cpu_quota_percent: 50,
            network_mode: NetworkMode::Restricted,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    None,
    #[default]
    Restricted,
    Full,
}

/// Tools configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// Chrome CDP configuration
    pub chrome: Option<ChromeConfig>,
    /// Cron scheduler configuration
    pub cron: Option<CronConfig>,
    /// Webhook listener configuration
    pub webhook: Option<WebhookConfig>,
}

/// Chrome CDP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChromeConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub executable_path: Option<String>,
    #[serde(default = "default_false")]
    pub headless: bool,
}

/// Cron scheduler configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CronConfig {
    pub enabled: bool,
    pub max_jobs: usize,
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_jobs: 100,
        }
    }
}

/// Webhook listener configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhookConfig {
    pub enabled: bool,
    pub port: u16,
    pub max_endpoints: usize,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 18791,
            max_endpoints: 50,
        }
    }
}

// Helper functions for serde defaults
const fn default_true() -> bool {
    true
}

const fn default_false() -> bool {
    false
}

impl GatewayConfig {
    /// Load configuration from a TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|e| {
            ConfigError::LoadFailed(format!("Failed to read {}: {}", path.display(), e))
        })?;

        Self::from_toml(&content)
    }

    /// Parse configuration from TOML string
    pub fn from_toml(content: &str) -> Result<Self, ConfigError> {
        let config: Self =
            toml::from_str(content).map_err(|e| ConfigError::ParseFailed(e.to_string()))?;

        // Validate configuration
        config.validate()?;

        Ok(config)
    }

    /// Load from this process's effective config file (~/.aleph/config.toml
    /// unless `--config` pinned another one).
    ///
    /// Resolving by hand here used to make this the one loader that honoured
    /// neither `ALEPH_HOME` (`dirs::home_dir()` ignores `$HOME` on macOS, the
    /// same divergence the instance lock was bitten by) nor the `--config`
    /// pin — so an isolated server could read the real user's config.
    pub fn load_default() -> Result<Self, ConfigError> {
        let config_path = crate::config::Config::effective_path();

        if config_path.exists() {
            Self::load(&config_path)
        } else {
            info!("No config file found, using defaults");
            Ok(Self::default())
        }
    }

    /// Validate the configuration
    fn validate(&self) -> Result<(), ConfigError> {
        // Validate port numbers
        if self.gateway.port == 0 {
            return Err(ConfigError::Invalid("Gateway port cannot be 0".to_string()));
        }

        // Validate at least one agent exists
        if self.agents.is_empty() {
            return Err(ConfigError::Invalid(
                "At least one agent must be configured".to_string(),
            ));
        }

        // Validate bindings reference existing agents
        for (pattern, agent_id) in &self.bindings {
            if !self.agents.contains_key(agent_id) {
                return Err(ConfigError::Invalid(format!(
                    "Binding '{pattern}' references unknown agent '{agent_id}'"
                )));
            }
        }

        Ok(())
    }

    /// Get agent configs as instance configs
    #[must_use]
    pub fn get_agent_instance_configs(&self) -> Vec<AgentInstanceConfig> {
        self.agents
            .iter()
            .map(|(id, cfg)| cfg.to_instance_config(id))
            .collect()
    }

    /// Get the default agent ID (first one, or "main" if exists)
    #[must_use]
    pub fn default_agent_id(&self) -> Option<&str> {
        if self.agents.contains_key("main") {
            Some("main")
        } else {
            self.agents.keys().next().map(|s| s.as_str())
        }
    }
}

/// Configuration errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Failed to load config: {0}")]
    LoadFailed(String),

    #[error("Failed to parse config: {0}")]
    ParseFailed(String),

    #[error("Invalid config: {0}")]
    Invalid(String),
}

/// Expand ~ in paths
fn expand_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

/// Expand ${ENV_VAR} in strings
#[cfg(test)]
fn expand_env_var(s: &str) -> String {
    let mut result = s.to_string();
    let mut search_from = 0;

    // Find ${...} patterns, advancing past substituted values to prevent infinite loops
    while let Some(rel_start) = result[search_from..].find("${") {
        let start = search_from + rel_start;
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let value = std::env::var(var_name).unwrap_or_default();
            let value_len = value.len();
            result = format!(
                "{}{}{}",
                &result[..start],
                value,
                &result[start + end + 1..]
            );
            search_from = start + value_len;
        } else {
            break;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GatewayConfig::default();
        assert_eq!(config.gateway.port, 18790);
        assert!(config.agents.contains_key("main"));
    }

    #[test]
    fn test_parse_minimal_config() {
        let toml = r#"
[gateway]
port = 9000

[agents.main]
model = "claude-opus-4-5"
"#;
        let config = GatewayConfig::from_toml(toml).unwrap();
        assert_eq!(config.gateway.port, 9000);
        assert_eq!(config.agents["main"].model, "claude-opus-4-5");
    }

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
[gateway]
host = "0.0.0.0"
port = 18790
max_connections = 200

[agents.main]
workspace = "~/aleph-main"
model = "claude-sonnet-4-5"
max_loops = 30

[agents.work]
workspace = "~/aleph-work"
model = "claude-opus-4-5"

[bindings]
"gui:window1" = "main"
"cli:*" = "work"

[channels.telegram]
enabled = true
token = "${TELEGRAM_BOT_TOKEN}"

[sandbox]
enabled = true
docker_image = "aleph-sandbox:latest"
memory_limit_mb = 1024

[tools.chrome]
enabled = true
headless = true
"#;
        let config = GatewayConfig::from_toml(toml).unwrap();

        assert_eq!(config.agents.len(), 2);
        assert!(config.agents.contains_key("work"));
        assert_eq!(config.bindings["cli:*"], "work");
        assert!(config.channels.get("telegram").is_some());
        assert!(config.sandbox.enabled);
    }

    #[test]
    fn test_require_idempotency_key_default_false_and_parse() {
        // Default stays false for existing TOML files (additive knob).
        let defaults = GatewayServerConfig::default();
        assert!(!defaults.require_idempotency_key);

        // The knob round-trips through TOML when opted in.
        let toml = r#"
[agents.main]
model = "test"

[gateway]
require_idempotency_key = true
"#;
        let parsed = GatewayConfig::from_toml(toml).expect("parse opt-in flag");
        assert!(parsed.gateway.require_idempotency_key);

        // Older TOML files (missing the key) keep loading.
        let legacy_toml = r#"
[agents.main]
model = "test"

[gateway]
host = "0.0.0.0"
"#;
        let legacy = GatewayConfig::from_toml(legacy_toml).expect("legacy still loads");
        assert!(!legacy.gateway.require_idempotency_key);
    }

    #[test]
    fn test_invalid_binding() {
        let toml = r#"
[agents.main]
model = "test"

[bindings]
"test" = "nonexistent"
"#;
        let result = GatewayConfig::from_toml(toml);
        assert!(result.is_err());
    }

    #[test]
    fn test_expand_path() {
        let expanded = expand_path("~/test/path");
        assert!(!expanded.to_string_lossy().starts_with("~"));
    }

    #[test]
    fn test_expand_env_var() {
        std::env::set_var("TEST_VAR", "hello");
        let result = expand_env_var("prefix_${TEST_VAR}_suffix");
        assert_eq!(result, "prefix_hello_suffix");
    }

    #[test]
    fn legacy_auth_tables_are_silently_ignored() {
        // Old user configs still carry `[gateway.auth]` tables and removed
        // auth knobs. The LAN-trust revert must load them without error
        // (the root struct has no `deny_unknown_fields`), simply ignoring
        // the dead keys.
        let toml = r#"
[gateway]
port = 18790
require_auth = true
enable_pairing = false
allow_guest = false
require_challenge = true
trusted_proxies = ["10.0.0.0/8"]

[gateway.auth]
mode = "token"
session_expiry_hours = 48
token_expiry_hours = 12
allowed_origins = ["https://legacy.example.com"]

[gateway.bootstrap]
nonce_ttl_secs = 30

[agents.main]
model = "test"
"#;
        let config = GatewayConfig::from_toml(toml).expect("legacy auth config still loads");
        assert_eq!(config.gateway.port, 18790);
        // The legacy nested allowed_origins is NOT migrated — operators move
        // it to the gateway root themselves (documented in the release note).
        assert!(config.gateway.allowed_origins.is_empty());
        assert!(!config.gateway.allow_any_origin);
    }

    #[test]
    fn delivery_queue_and_send_retry_parse_from_gateway() {
        let toml = r#"
[agents.main]
model = "test"

[gateway.delivery_queue]
max_attempts = 5
initial_backoff_secs = 10
max_backoff_secs = 600
tick_secs = 15

[gateway.send_retry]
max_rate_limit_retries = 4
max_retry_after_secs = 45
"#;
        let config = GatewayConfig::from_toml(toml).expect("parse resilience knobs");
        let dq = config.gateway.delivery_queue.to_runtime();
        assert_eq!(dq.max_attempts, 5);
        assert_eq!(dq.initial_backoff.as_secs(), 10);
        assert_eq!(dq.max_backoff.as_secs(), 600);
        assert_eq!(dq.tick.as_secs(), 15);
        // Unspecified keys fall back to the runtime defaults.
        assert_eq!(dq.batch, 32);

        let sr = config.gateway.send_retry.to_policy();
        assert_eq!(sr.max_rate_limit_retries, 4);
        assert_eq!(sr.max_retry_after.as_secs(), 45);
    }

    #[test]
    fn resilience_knobs_default_when_absent() {
        // Old TOML files with no delivery/retry blocks keep the historic
        // hardcoded behavior byte-for-byte.
        let toml = r#"
[agents.main]
model = "test"

[gateway]
port = 18790
"#;
        let config = GatewayConfig::from_toml(toml).expect("legacy still loads");
        let dq = config.gateway.delivery_queue.to_runtime();
        assert_eq!(dq.max_attempts, 10);
        assert_eq!(dq.max_queue_len, 10_000);
        let sr = config.gateway.send_retry.to_policy();
        assert_eq!(sr.max_rate_limit_retries, 2);
        assert_eq!(sr.max_retry_after.as_secs(), 30);
    }

    #[test]
    fn origin_knobs_parse_from_gateway_root() {
        let toml = r#"
[gateway]
port = 18790
allowed_origins = ["https://panel.example.com"]
allow_any_origin = true

[agents.main]
model = "test"
"#;
        let config = GatewayConfig::from_toml(toml).expect("parse origin knobs");
        assert_eq!(
            config.gateway.allowed_origins,
            vec!["https://panel.example.com".to_string()]
        );
        assert!(config.gateway.allow_any_origin);

        // Defaults: empty allow-list, escape hatch off.
        let defaults = GatewayServerConfig::default();
        assert!(defaults.allowed_origins.is_empty());
        assert!(!defaults.allow_any_origin);
    }

    #[test]
    fn tls_and_trusted_proxy_default_off_and_parse() {
        // Defaults: everything off, loopback trusted.
        let d = GatewayServerConfig::default();
        assert!(!d.tls.enabled);
        assert!(!d.trusted_proxy.enabled);
        assert!(!d.allow_insecure_remote);
        assert_eq!(d.trusted_proxy.trusted_ips, vec!["127.0.0.1", "::1"]);

        // Round-trips from TOML.
        let toml = r#"
host = "0.0.0.0"
allow_insecure_remote = false
[tls]
enabled = true
cert_path = "/x/cert.pem"
[trusted_proxy]
enabled = true
"#;
        let c: GatewayServerConfig = toml::from_str(toml).unwrap();
        assert!(c.tls.enabled);
        assert_eq!(c.tls.cert_path, "/x/cert.pem");
        assert!(c.trusted_proxy.enabled);
        assert_eq!(c.trusted_proxy.trusted_ips, vec!["127.0.0.1", "::1"]); // still defaulted
    }
}
