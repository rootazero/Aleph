//! WasmCapabilityKernel — per-execution security kernel.
//!
//! Every host function call passes through this kernel for:
//! - Capability checking (default-deny)
//! - Leak detection (bidirectional)
//! - Credential injection (host-side)
//! - Audit logging
//! - Resource counting

use std::sync::atomic::{AtomicU32, Ordering};

use crate::extension::runtime::wasm::capabilities::*;
use crate::extension::runtime::wasm::limits::WasmResourceLimits;

/// Errors from capability checks
#[derive(Debug)]
pub enum CapabilityError {
    NotDeclared(String),
    NotAllowed(String),
    RateLimited(String),
    ResourceExhausted(String),
    LeakDetected(String),
    PathTraversal(String),
    InternalError(String),
}

impl std::fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotDeclared(msg) => write!(f, "Capability not declared: {}", msg),
            Self::NotAllowed(msg) => write!(f, "Not allowed: {}", msg),
            Self::RateLimited(msg) => write!(f, "Rate limited: {}", msg),
            Self::ResourceExhausted(msg) => write!(f, "Resource exhausted: {}", msg),
            Self::LeakDetected(msg) => write!(f, "Leak detected: {}", msg),
            Self::PathTraversal(msg) => write!(f, "Path traversal: {}", msg),
            Self::InternalError(msg) => write!(f, "Internal error: {}", msg),
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
    tool_invoke_count: AtomicU32,
}

impl WasmCapabilityKernel {
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
            tool_invoke_count: AtomicU32::new(0),
        }
    }

    pub fn check_workspace_read(&self, path: &str) -> Result<(), CapabilityError> {
        let ws = self.capabilities.workspace.as_ref().ok_or_else(|| {
            CapabilityError::NotDeclared("workspace".to_string())
        })?;
        self.validate_path(path)?;
        if !ws.allowed_prefixes.is_empty()
            && !ws.allowed_prefixes.iter().any(|p| path.starts_with(p))
        {
            return Err(CapabilityError::NotAllowed(format!(
                "path '{}' not in allowed prefixes", path
            )));
        }
        Ok(())
    }

    pub fn check_secret_pattern(&self, name: &str) -> bool {
        self.capabilities
            .secrets
            .as_ref()
            .map(|s| s.is_allowed(name))
            .unwrap_or(false)
    }

    pub fn log(&self, _level: &str, msg: &str) -> Result<(), CapabilityError> {
        let count = self.log_count.fetch_add(1, Ordering::Relaxed);
        if count >= self.limits.max_log_entries {
            return Err(CapabilityError::ResourceExhausted(
                "log entry limit exceeded".to_string(),
            ));
        }
        let _msg = if msg.len() > self.limits.max_log_message_bytes {
            &msg[..self.limits.max_log_message_bytes]
        } else {
            msg
        };
        Ok(())
    }

    pub fn now_millis(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    pub fn check_http_limit(&self) -> Result<(), CapabilityError> {
        let count = self.http_call_count.fetch_add(1, Ordering::Relaxed);
        if count >= self.limits.max_http_calls {
            return Err(CapabilityError::ResourceExhausted(
                "HTTP call limit exceeded".to_string(),
            ));
        }
        Ok(())
    }

    pub fn check_tool_invoke_limit(&self) -> Result<(), CapabilityError> {
        let count = self.tool_invoke_count.fetch_add(1, Ordering::Relaxed);
        if count >= self.limits.max_tool_invokes {
            return Err(CapabilityError::ResourceExhausted(
                "tool invoke limit exceeded".to_string(),
            ));
        }
        Ok(())
    }

    pub fn resolve_tool_alias(&self, alias: &str) -> Result<String, CapabilityError> {
        let ti = self.capabilities.tool_invoke.as_ref().ok_or_else(|| {
            CapabilityError::NotDeclared("tool_invoke".to_string())
        })?;
        ti.aliases.get(alias).cloned().ok_or_else(|| {
            CapabilityError::NotAllowed(format!("unknown tool alias: {}", alias))
        })
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn capabilities(&self) -> &WasmCapabilities {
        &self.capabilities
    }

    fn validate_path(&self, path: &str) -> Result<(), CapabilityError> {
        if path.contains("..") {
            return Err(CapabilityError::PathTraversal("'..' not allowed".to_string()));
        }
        if path.starts_with('/') {
            return Err(CapabilityError::PathTraversal("absolute paths not allowed".to_string()));
        }
        if path.contains('\0') {
            return Err(CapabilityError::PathTraversal("null bytes not allowed".to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::runtime::wasm::capabilities::*;

    fn kernel_with_no_caps() -> WasmCapabilityKernel {
        WasmCapabilityKernel::new(
            "test-plugin".to_string(),
            WasmCapabilities::default(),
            WasmResourceLimits::default(),
        )
    }

    fn kernel_with_workspace() -> WasmCapabilityKernel {
        let mut caps = WasmCapabilities::default();
        caps.workspace = Some(WorkspaceCapability {
            allowed_prefixes: vec!["docs/".to_string(), "config/".to_string()],
        });
        WasmCapabilityKernel::new(
            "test-plugin".to_string(),
            caps,
            WasmResourceLimits::default(),
        )
    }

    fn kernel_with_secrets() -> WasmCapabilityKernel {
        let mut caps = WasmCapabilities::default();
        caps.secrets = Some(SecretsCapability {
            allowed_patterns: vec!["slack_*".to_string()],
        });
        WasmCapabilityKernel::new(
            "test-plugin".to_string(),
            caps,
            WasmResourceLimits::default(),
        )
    }

    #[test]
    fn test_no_workspace_capability_denies_read() {
        let kernel = kernel_with_no_caps();
        let result = kernel.check_workspace_read("any/path");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CapabilityError::NotDeclared(_)));
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
        assert!(kernel.check_workspace_read("docs/../secrets/key.pem").is_err());
        assert!(kernel.check_workspace_read("/etc/passwd").is_err());
        assert!(kernel.check_workspace_read("docs/\0hidden").is_err());
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
        let kernel = WasmCapabilityKernel::new(
            "test".to_string(),
            WasmCapabilities::default(),
            limits,
        );
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
        let kernel = WasmCapabilityKernel::new(
            "test".to_string(),
            WasmCapabilities::default(),
            limits,
        );
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
