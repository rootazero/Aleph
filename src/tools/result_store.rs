//! ToolResultStore — disk persistence for large tool results.
//!
//! When a tool result exceeds the token threshold, it is written to a
//! session-scoped directory on disk. A compact reference marker is injected
//! into the context window so the LLM can identify that the full output
//! exists but was offloaded.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use crate::context::budget::pressure::estimate_tokens_smart;

/// Prefix used to identify persisted-result reference lines.
const PERSISTED_REF_PREFIX: &str = "[Full output persisted: ";

// =============================================================================
// Process-wide installer
// =============================================================================
//
// Layer 2 / Layer 3 of the tool-result budget both need access to a shared
// `Arc<ToolResultStore>` so the marker path the LLM sees matches a real file
// on disk. Rather than thread the `Arc` through every constructor that builds
// a `ScopedToolService` (gateway path) or an `AgentHarnessRunner` (orchestrator
// path), we install one at server boot via `set_global_tool_result_store` and
// read it back through `global_tool_result_store`. The same singleton is
// consumed by:
//   * `gateway::execution_engine::tool_service_builder::build_request_tool_service`
//   * `orchestrator::harness_bridge::AgentHarnessRunner` (HarnessDeps wiring)
// Tests or alternative bootstraps that prefer per-instance injection can still
// use `ScopedToolService::with_result_store` / `HarnessDeps.result_store`
// directly; the global slot is `Option`-shaped and a `None` value means
// "fall back to in-line truncation only".
static GLOBAL_STORE: OnceLock<Arc<ToolResultStore>> = OnceLock::new();

/// Install the process-wide `ToolResultStore`. Idempotent — subsequent calls
/// are silently ignored so multiple boot paths cannot stomp each other.
pub fn set_global_tool_result_store(store: Arc<ToolResultStore>) {
    let _ = GLOBAL_STORE.set(store);
}

/// Read the process-wide `ToolResultStore`, if installed.
pub fn global_tool_result_store() -> Option<Arc<ToolResultStore>> {
    GLOBAL_STORE.get().cloned()
}

// =============================================================================
// ToolResultStore
// =============================================================================

/// Session-scoped store that offloads large tool outputs to disk.
///
/// On drop the store removes its base directory, so tool result files are
/// automatically cleaned up when the session ends.
pub struct ToolResultStore {
    base_dir: PathBuf,
}

impl ToolResultStore {
    /// Create a new store for the given session.
    ///
    /// Creates `~/.aleph/data/tool_results/{session_id}/` on disk.
    pub fn new(session_id: &str) -> std::io::Result<Self> {
        let base_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".aleph")
            .join("data")
            .join("tool_results")
            .join(session_id);

        std::fs::create_dir_all(&base_dir)?;
        Ok(Self { base_dir })
    }

    /// Construct a store rooted at an arbitrary base directory. The
    /// caller must create the directory before this returns. Exposed
    /// for tests in adjacent modules (e.g. `result_processing`,
    /// `scoped`) that need a `ToolResultStore` without touching
    /// `~/.aleph/`.
    #[doc(hidden)]
    pub fn with_dir_for_tests(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Persist the content to disk if it exceeds `threshold_tokens`.
    ///
    /// Returns a reference marker string if the content was persisted, or
    /// `None` if the content is small enough to remain in the context window.
    pub fn persist_if_large(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        content: &str,
        threshold_tokens: usize,
    ) -> Option<String> {
        let tokens = estimate_tokens_smart(content);
        if tokens <= threshold_tokens {
            return None;
        }

        // Use a sanitized filename: {tool_call_id}_{tool_name}.txt
        let safe_name = format!(
            "{}_{}.txt",
            sanitize_for_filename(tool_call_id),
            sanitize_for_filename(tool_name)
        );
        let path = self.base_dir.join(&safe_name);

        if let Err(e) = std::fs::write(&path, content) {
            tracing::warn!(
                tool_call_id = tool_call_id,
                tool_name = tool_name,
                error = %e,
                "failed to persist tool result to disk"
            );
            return None;
        }

        let marker = format!(
            "{}{} ({} tokens, {})]",
            PERSISTED_REF_PREFIX,
            path.display(),
            tokens,
            tool_name,
        );
        Some(marker)
    }

    /// Remove the base directory and all its contents.
    pub fn cleanup(&self) {
        if self.base_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&self.base_dir) {
                tracing::warn!(
                    dir = %self.base_dir.display(),
                    error = %e,
                    "failed to clean up tool result store"
                );
            }
        }
    }
}

impl Drop for ToolResultStore {
    fn drop(&mut self) {
        self.cleanup();
    }
}

// =============================================================================
// Standalone helpers
// =============================================================================

/// Scan `text` for a `[Full output persisted: ...]` reference line and return
/// the first matching line if found.
pub fn extract_persisted_ref(text: &str) -> Option<&str> {
    text.lines()
        .find(|line| line.starts_with(PERSISTED_REF_PREFIX))
}

/// Replace characters unsafe for filenames with underscores.
fn sanitize_for_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Build a test store rooted in a temp directory instead of ~/.aleph/.
    fn test_store(name: &str) -> (ToolResultStore, PathBuf) {
        let base = std::env::temp_dir()
            .join("aleph_test_tool_result_store")
            .join(name);
        std::fs::create_dir_all(&base).unwrap();
        let store = ToolResultStore {
            base_dir: base.clone(),
        };
        (store, base)
    }

    #[test]
    fn small_result_not_persisted() {
        let (store, _base) = test_store("small_result_not_persisted");
        // threshold = 10_000 tokens; short content is well under
        let result = store.persist_if_large("call_1", "read_file", "hello world", 10_000);
        assert!(result.is_none(), "short content should not be persisted");
    }

    #[test]
    fn large_result_persisted_and_recoverable() {
        let (store, base) = test_store("large_result_persisted");
        // Generate content that is definitely > 1 token (threshold = 1)
        let content = "a".repeat(1000);
        let result = store.persist_if_large("call_abc", "bash", &content, 1);
        assert!(result.is_some(), "large content should be persisted");
        let marker = result.unwrap();
        assert!(
            marker.starts_with(PERSISTED_REF_PREFIX),
            "marker must start with prefix: {marker}"
        );
        // Verify a .txt file was created and its content matches
        let files: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1, "exactly one file should be written");
        let written = std::fs::read_to_string(files[0].path()).unwrap();
        assert_eq!(written, content, "written content must match original");
    }

    #[test]
    fn cleanup_removes_directory() {
        let base = std::env::temp_dir()
            .join("aleph_test_tool_result_store")
            .join("cleanup_test");
        std::fs::create_dir_all(&base).unwrap();
        let store = ToolResultStore {
            base_dir: base.clone(),
        };
        assert!(base.exists());
        store.cleanup();
        assert!(!base.exists(), "cleanup should remove the base directory");
    }

    #[test]
    fn extract_persisted_ref_finds_marker() {
        let text =
            "some output\n[Full output persisted: /tmp/foo.txt (1234 tokens, bash)]\nmore text";
        let found = extract_persisted_ref(text);
        assert!(found.is_some(), "should find marker line");
        assert!(found.unwrap().contains("Full output persisted"));
    }

    #[test]
    fn extract_persisted_ref_returns_none_when_absent() {
        let text = "no marker here\njust regular output";
        let found = extract_persisted_ref(text);
        assert!(found.is_none(), "should return None when no marker present");
    }
}
