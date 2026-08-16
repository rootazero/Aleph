//! Sandbox rate limiting — session + tool-category based sliding window.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;

use crate::sandbox::hooks::{
    SandboxBeforeHook, SandboxHookContext, SandboxHookResult, SandboxHookResult::Deny,
};
use crate::session::service::SessionId;

/// Tool danger category for rate limiting.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum ToolCategory {
    Read,
    Write,
    Dangerous,
    Admin,
}

/// Per-category sliding window config.
#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub max_requests: u32,
    pub window_secs: u64,
    pub burst_allow: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            max_requests: 60,
            window_secs: 60,
            burst_allow: 20,
        }
    }
}

/// Sandbox rate limit configuration.
#[derive(Debug, Clone)]
pub struct SandboxRateLimitConfig {
    pub enabled: bool,
    pub per_category: HashMap<ToolCategory, WindowConfig>,
}

impl Default for SandboxRateLimitConfig {
    fn default() -> Self {
        let mut per_category = HashMap::new();
        per_category.insert(
            ToolCategory::Read,
            WindowConfig {
                max_requests: 60,
                window_secs: 60,
                burst_allow: 20,
            },
        );
        per_category.insert(
            ToolCategory::Write,
            WindowConfig {
                max_requests: 30,
                window_secs: 60,
                burst_allow: 10,
            },
        );
        per_category.insert(
            ToolCategory::Dangerous,
            WindowConfig {
                max_requests: 10,
                window_secs: 60,
                burst_allow: 5,
            },
        );
        per_category.insert(
            ToolCategory::Admin,
            WindowConfig {
                max_requests: 5,
                window_secs: 60,
                burst_allow: 2,
            },
        );
        Self {
            enabled: true,
            per_category,
        }
    }
}

/// Categorize a tool name into a `ToolCategory`.
#[must_use]
pub fn categorize_tool(tool_name: &str) -> ToolCategory {
    // `ALEPH_TOOL_NAME` carries "code_exec:python" / "code_exec:javascript" /
    // "bash" (see `code_exec.rs`); the rate-limit bucket only cares about the
    // tool family, so drop any `:language` suffix before matching.
    let base = tool_name.split(':').next().unwrap_or(tool_name);
    match base {
        "self_config" | "skill_install" => ToolCategory::Admin,
        "code_exec" | "bash" => ToolCategory::Dangerous,
        "file_ops" | "apply_patch" => ToolCategory::Write,
        _ => ToolCategory::Read,
    }
}

/// Internal per-key sliding window.
struct SlidingWindow {
    timestamps: VecDeque<Instant>,
}

/// Rate limit key: (`session_id`, `tool_category`).
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct RateLimitKey {
    session_id: SessionId,
    category: ToolCategory,
}

/// Sliding-window rate limiter for sandbox hooks.
pub struct SandboxRateLimiter {
    config: SandboxRateLimitConfig,
    windows: DashMap<RateLimitKey, SlidingWindow>,
}

impl SandboxRateLimiter {
    #[must_use]
    pub fn new(config: SandboxRateLimitConfig) -> Self {
        Self {
            config,
            windows: DashMap::new(),
        }
    }

    /// Check if execution is allowed. Returns Ok(()) or Err(reason).
    pub fn check_and_record(
        &self,
        session_id: &SessionId,
        category: &ToolCategory,
    ) -> Result<(), String> {
        if !self.config.enabled {
            return Ok(());
        }

        let wc = match self.config.per_category.get(category) {
            Some(wc) => wc,
            None => return Ok(()),
        };

        let now = Instant::now();
        let window_dur = Duration::from_secs(wc.window_secs);
        let key = RateLimitKey {
            session_id: session_id.clone(),
            category: category.clone(),
        };

        let mut entry = self.windows.entry(key).or_insert_with(|| SlidingWindow {
            timestamps: VecDeque::new(),
        });

        let cutoff = now - window_dur;
        while let Some(&front) = entry.timestamps.front() {
            if front < cutoff {
                entry.timestamps.pop_front();
            } else {
                break;
            }
        }

        let count: u32 = entry.timestamps.len().try_into().unwrap_or(u32::MAX);
        let max = wc.max_requests + wc.burst_allow;

        if count >= max {
            let oldest = match entry.timestamps.front() {
                Some(t) => t,
                None => return Ok(()),
            };
            let retry_after = (*oldest + window_dur).duration_since(now);
            return Err(format!(
                "rate limit exceeded for {:?}: {}/{} in {}s window (retry after {:?})",
                category, count, max, wc.window_secs, retry_after
            ));
        }

        entry.timestamps.push_back(now);
        Ok(())
    }
}

/// `SandboxBeforeHook` that rate-limits sandbox execution.
pub struct RateLimitHook {
    limiter: Arc<SandboxRateLimiter>,
}

impl RateLimitHook {
    #[must_use]
    pub const fn new(limiter: Arc<SandboxRateLimiter>) -> Self {
        Self { limiter }
    }
}

#[async_trait]
impl SandboxBeforeHook for RateLimitHook {
    async fn before(&self, ctx: SandboxHookContext<'_>) -> SandboxHookResult {
        let session_id = &ctx.command.session_id;
        // `SandboxHookContext::tool_name` is `command.program` ("bash"/
        // "python3"/"node"), which would send Python/JS `code_exec` into the
        // loose Read bucket (6x the Dangerous allowance). The tools stamp the
        // real identity into `ALEPH_TOOL_NAME`, so prefer that when present.
        let tool_name = ctx
            .command
            .env
            .get("ALEPH_TOOL_NAME")
            .map(String::as_str)
            .unwrap_or(ctx.tool_name);
        let category = categorize_tool(tool_name);

        if let Err(reason) = self.limiter.check_and_record(session_id, &category) {
            tracing::warn!(
                target: "sandbox_rate_limit",
                session_id = ?session_id,
                tool_name = tool_name,
                category = ?category,
                reason = %reason,
                "sandbox rate limit exceeded"
            );
            return Deny { reason };
        }

        SandboxHookResult::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;

    fn session_id() -> SessionId {
        SessionKey::ephemeral("test")
    }

    #[test]
    fn rate_limit_allows_under_limit() {
        let limiter = Arc::new(SandboxRateLimiter::new(SandboxRateLimitConfig::default()));
        let sid = session_id();
        let category = ToolCategory::Write;

        for _ in 0..40 {
            assert!(limiter.check_and_record(&sid, &category).is_ok());
        }
    }

    #[test]
    fn rate_limit_denies_over_limit() {
        let limiter = Arc::new(SandboxRateLimiter::new(SandboxRateLimitConfig::default()));
        let sid = session_id();
        let category = ToolCategory::Write;

        for _ in 0..40 {
            limiter.check_and_record(&sid, &category).unwrap();
        }

        let result = limiter.check_and_record(&sid, &category);
        assert!(result.is_err());
    }

    #[test]
    fn categorize_tool_dangerous() {
        assert_eq!(categorize_tool("code_exec"), ToolCategory::Dangerous);
        assert_eq!(categorize_tool("bash"), ToolCategory::Dangerous);
        // ALEPH_TOOL_NAME strips the language suffix but keeps the family.
        assert_eq!(categorize_tool("code_exec:python"), ToolCategory::Dangerous);
        assert_eq!(
            categorize_tool("code_exec:javascript"),
            ToolCategory::Dangerous
        );
    }

    #[test]
    fn categorize_tool_admin() {
        assert_eq!(categorize_tool("self_config"), ToolCategory::Admin);
        assert_eq!(categorize_tool("skill_install"), ToolCategory::Admin);
    }

    #[test]
    fn categorize_tool_write() {
        assert_eq!(categorize_tool("file_ops"), ToolCategory::Write);
        assert_eq!(categorize_tool("apply_patch"), ToolCategory::Write);
    }

    #[test]
    fn categorize_tool_read_default() {
        assert_eq!(categorize_tool("search"), ToolCategory::Read);
        assert_eq!(categorize_tool("memory_retrieval"), ToolCategory::Read);
        assert_eq!(categorize_tool("unknown_tool"), ToolCategory::Read);
    }
}
