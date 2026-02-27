use serde::{Deserialize, Serialize};

/// Resource limits for WASM plugin execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmResourceLimits {
    pub memory_mb: u32,
    pub fuel: u64,
    pub timeout_secs: u64,
    pub max_http_calls: u32,
    pub max_tool_invokes: u32,
    pub max_log_entries: u32,
    pub max_log_message_bytes: usize,
}

impl Default for WasmResourceLimits {
    fn default() -> Self {
        Self {
            memory_mb: 10,
            fuel: 10_000_000,
            timeout_secs: 60,
            max_http_calls: 50,
            max_tool_invokes: 20,
            max_log_entries: 1000,
            max_log_message_bytes: 4096,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limits() {
        let limits = WasmResourceLimits::default();
        assert_eq!(limits.memory_mb, 10);
        assert_eq!(limits.fuel, 10_000_000);
        assert_eq!(limits.timeout_secs, 60);
        assert_eq!(limits.max_http_calls, 50);
        assert_eq!(limits.max_tool_invokes, 20);
        assert_eq!(limits.max_log_entries, 1000);
        assert_eq!(limits.max_log_message_bytes, 4096);
    }

    #[test]
    fn test_custom_limits() {
        let limits = WasmResourceLimits {
            memory_mb: 64,
            fuel: 50_000_000,
            ..Default::default()
        };
        assert_eq!(limits.memory_mb, 64);
        assert_eq!(limits.fuel, 50_000_000);
        // Non-overridden fields keep defaults
        assert_eq!(limits.timeout_secs, 60);
        assert_eq!(limits.max_http_calls, 50);
        assert_eq!(limits.max_tool_invokes, 20);
        assert_eq!(limits.max_log_entries, 1000);
        assert_eq!(limits.max_log_message_bytes, 4096);
    }
}
