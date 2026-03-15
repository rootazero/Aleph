//! WASM capability TOML types and conversion to runtime types

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::types::CapabilitiesSection;
use crate::extension::runtime::wasm::{
    CredentialBinding, CredentialInject, EndpointPattern, HttpCapability, RateLimit,
    SecretsCapability, ToolInvokeCapability, WasmCapabilities, WorkspaceCapability,
};

// =============================================================================
// WASM Capability TOML Types
// =============================================================================

/// Workspace capability declaration in TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmWorkspaceToml {
    #[serde(default)]
    pub allowed_prefixes: Vec<String>,
}

/// HTTP capability declaration in TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmHttpToml {
    #[serde(default)]
    pub allowlist: Vec<WasmEndpointToml>,
    #[serde(default)]
    pub credentials: Vec<WasmCredentialToml>,
    #[serde(default)]
    pub rate_limit: Option<WasmRateLimitToml>,
    #[serde(default = "default_http_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_request_bytes")]
    pub max_request_bytes: usize,
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
}

fn default_http_timeout() -> u64 {
    30
}

fn default_max_request_bytes() -> usize {
    1_048_576
}

fn default_max_response_bytes() -> usize {
    10_485_760
}

/// Endpoint pattern in TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmEndpointToml {
    pub host: String,
    #[serde(default = "default_toml_path_prefix")]
    pub path_prefix: String,
    #[serde(default)]
    pub methods: Vec<String>,
}

fn default_toml_path_prefix() -> String {
    "/".to_string()
}

/// Credential binding in TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmCredentialToml {
    pub secret_name: String,
    pub inject: WasmCredentialInjectToml,
    #[serde(default)]
    pub host_patterns: Vec<String>,
}

/// Credential injection method in TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WasmCredentialInjectToml {
    Bearer,
    Basic {
        username: String,
    },
    Header {
        name: String,
        #[serde(default)]
        prefix: Option<String>,
    },
    Query {
        param_name: String,
    },
    UrlPath {
        placeholder: String,
    },
}

/// Rate limit in TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmRateLimitToml {
    #[serde(default)]
    pub requests_per_minute: u32,
    #[serde(default)]
    pub requests_per_hour: u32,
}

/// Tool invoke capability in TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmToolInvokeToml {
    #[serde(default)]
    pub aliases: HashMap<String, String>,
    #[serde(default = "default_toml_max_per_execution")]
    pub max_per_execution: u32,
}

fn default_toml_max_per_execution() -> u32 {
    20
}

/// Secrets capability in TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmSecretsToml {
    #[serde(default)]
    pub allowed_patterns: Vec<String>,
}

// =============================================================================
// WASM Capability Conversion
// =============================================================================

/// Convert TOML capabilities section to runtime WasmCapabilities
///
/// Returns `None` if no WASM capabilities were declared.
pub fn convert_wasm_capabilities(caps: &CapabilitiesSection) -> Option<WasmCapabilities> {
    // Check if any WASM capability is declared
    if caps.workspace.is_none()
        && caps.http.is_none()
        && caps.tool_invoke.is_none()
        && caps.secrets.is_none()
    {
        return None;
    }

    Some(WasmCapabilities {
        workspace: caps.workspace.as_ref().map(|w| WorkspaceCapability {
            allowed_prefixes: w.allowed_prefixes.clone(),
        }),
        http: caps.http.as_ref().map(|h| HttpCapability {
            allowlist: h
                .allowlist
                .iter()
                .map(|e| EndpointPattern {
                    host: e.host.clone(),
                    path_prefix: e.path_prefix.clone(),
                    methods: e.methods.clone(),
                })
                .collect(),
            credentials: h
                .credentials
                .iter()
                .map(|c| CredentialBinding {
                    secret_name: c.secret_name.clone(),
                    inject: convert_credential_inject(&c.inject),
                    host_patterns: c.host_patterns.clone(),
                })
                .collect(),
            rate_limit: h.rate_limit.as_ref().map(|r| RateLimit {
                requests_per_minute: r.requests_per_minute,
                requests_per_hour: r.requests_per_hour,
            }),
            timeout_secs: h.timeout_secs,
            max_request_bytes: h.max_request_bytes,
            max_response_bytes: h.max_response_bytes,
        }),
        tool_invoke: caps.tool_invoke.as_ref().map(|t| ToolInvokeCapability {
            aliases: t.aliases.clone(),
            max_per_execution: t.max_per_execution,
        }),
        secrets: caps.secrets.as_ref().map(|s| SecretsCapability {
            allowed_patterns: s.allowed_patterns.clone(),
        }),
    })
}

fn convert_credential_inject(inject: &WasmCredentialInjectToml) -> CredentialInject {
    match inject {
        WasmCredentialInjectToml::Bearer => CredentialInject::Bearer,
        WasmCredentialInjectToml::Basic { username } => CredentialInject::Basic {
            username: username.clone(),
        },
        WasmCredentialInjectToml::Header { name, prefix } => CredentialInject::Header {
            name: name.clone(),
            prefix: prefix.clone(),
        },
        WasmCredentialInjectToml::Query { param_name } => CredentialInject::Query {
            param_name: param_name.clone(),
        },
        WasmCredentialInjectToml::UrlPath { placeholder } => CredentialInject::UrlPath {
            placeholder: placeholder.clone(),
        },
    }
}
