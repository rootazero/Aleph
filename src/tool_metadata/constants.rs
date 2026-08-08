//! Hardcoded configuration constants
//!
//! Security-enforced constants that are not user-configurable.
//! Previously in engine/constants.rs, retained here for backward compatibility.

// `REQUIRE_CONFIRMATION` / `MAX_PARALLELISM` used to live here. Their only
// readers were the `default_*` fns behind `[agent] require_confirmation` /
// `max_parallelism`, two keys that self-declared "legacy, ignored" and were
// retired in the same pass. Execution approval is decided by the exec tier in
// `src/tools/scoped/` (which reads each tool's declared metadata, not a global
// bool) and sub-agent fan-out is bounded by the spawn semaphore — neither ever
// consulted these.

// Security boundary constants for file operations and code execution
/// Maximum file size for file operations (100MB)
pub const DEFAULT_MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;
/// Whether sandbox is enabled by default for code execution
pub const DEFAULT_SANDBOX_ENABLED: bool = true;
/// Whether network access is allowed by default in sandbox
pub const DEFAULT_ALLOW_NETWORK: bool = false;
/// Default timeout for code execution in seconds
pub const DEFAULT_CODE_EXEC_TIMEOUT: u64 = 60;
/// Whether to require confirmation for write operations
pub const DEFAULT_REQUIRE_CONFIRMATION_FOR_WRITE: bool = true;
/// Whether to require confirmation for delete operations
pub const DEFAULT_REQUIRE_CONFIRMATION_FOR_DELETE: bool = true;
/// Whether file operations are enabled by default
pub const DEFAULT_FILE_OPS_ENABLED: bool = true;
/// Whether code execution is enabled by default (false for security)
pub const DEFAULT_CODE_EXEC_ENABLED: bool = false;
/// Default runtime for code execution
pub const DEFAULT_CODE_EXEC_RUNTIME: &str = "shell";
/// Default environment variables to pass to executed code
pub const DEFAULT_PASS_ENV: &[&str] = &["PATH", "HOME", "USER"];

// AI model defaults
/// Default max tokens for AI model responses.
pub const DEFAULT_MAX_TOKENS: u32 = 16384;
