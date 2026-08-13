// Browser snapshot tool — captures an accessibility tree snapshot of the page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Clamp bounds for the model-supplied `max_chars`.
///
/// `max_chars` is the one knob in the browser tools that opts *out* of
/// [`DEFAULT_CONTENT_MAX_CHARS`](super::DEFAULT_CONTENT_MAX_CHARS) — without a
/// ceiling it opts out entirely, and a single `max_chars: usize::MAX` snapshot
/// of a heavy page can be the whole request. Above the ceiling the offload
/// below is strictly the better deal anyway: the full tree lands on disk and
/// `ctx_search` retrieves only the relevant subtree. The floor keeps
/// `max_chars: 0` from producing an empty "successful" snapshot.
///
/// Clamped at the system boundary, the same way
/// [`wait_for::clamp_timeout`](super::wait_for::clamp_timeout) clamps a
/// model-supplied timeout.
pub(crate) const MIN_SNAPSHOT_CHARS: usize = 1_000;
pub(crate) const MAX_SNAPSHOT_CHARS: usize = 120_000;

/// Resolve the model-supplied `max_chars` into the safe
/// `[MIN_SNAPSHOT_CHARS, MAX_SNAPSHOT_CHARS]` window, defaulting to the shared
/// content budget when unset.
pub(crate) fn resolve_max_chars(requested: Option<usize>) -> usize {
    requested
        .unwrap_or(super::DEFAULT_CONTENT_MAX_CHARS)
        .clamp(MIN_SNAPSHOT_CHARS, MAX_SNAPSHOT_CHARS)
}

/// Arguments for the `browser_snapshot` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSnapshotArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Maximum output characters (default: 30000, clamped to 1000..=120000).
    /// A snapshot cut by this budget is offloaded whole — recover the dropped
    /// tail with `ctx_search` rather than by raising this.
    pub max_chars: Option<usize>,
}

/// Output from the `browser_snapshot` tool.
#[derive(Debug, Serialize)]
pub struct BrowserSnapshotOutput {
    pub success: bool,
    pub snapshot: Option<String>,
    pub truncated: bool,
    pub ref_count: usize,
    pub message: Option<String>,
}

/// Captures an accessibility tree (ARIA) snapshot of the current page.
#[derive(Clone)]
pub struct BrowserSnapshotTool {
    manager: Arc<ProfileManager>,
}

impl BrowserSnapshotTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }

    /// Offload the FULL snapshot; see [`super::offload_full_content`], which
    /// both this tool and `browser_exec`'s `snapshot` step share.
    fn offload_full(&self, full: &str) -> Option<String> {
        super::offload_full_content(&self.manager, Self::NAME, full)
    }
}

#[async_trait]
impl AlephTool for BrowserSnapshotTool {
    const NAME: &'static str = "browser_snapshot";
    const DESCRIPTION: &'static str =
        "Get an accessibility tree snapshot of the current browser page for structured understanding";
    type Args = BrowserSnapshotArgs;
    type Output = BrowserSnapshotOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Text-first: backend.snapshot() returns raw YAML/indented-tree text already.
        let max_chars = resolve_max_chars(args.max_chars);

        match super::make_backend_and_tab_guarded(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.snapshot(&tab_id).await {
                Ok(snap) => {
                    // Bound first (line-boundary, never splitting a `[ref=]` token),
                    // then count refs on the EMITTED text so the reported count
                    // matches exactly what the model can see and act on.
                    let (text, truncated) = super::bound_content(&snap.snapshot_text, max_chars);
                    let ref_count = text.matches("[ref=").count();
                    // Page-derived DOM text is untrusted external content: scrub
                    // embedded credentials, then wrap with the injection boundary
                    // so chat-template markers injected by a hostile page cannot
                    // escape (see `redact_wrap`).
                    let wrapped = super::redact_wrap(&self.manager, &text);
                    let snapshot = if truncated {
                        match self.offload_full(&snap.snapshot_text) {
                            Some(footer) => format!("{wrapped}\n{footer}"),
                            None => format!(
                                "{wrapped}\n[snapshot truncated to {max_chars} chars and the \
                                 full tree could not be offloaded here; the dropped tail is not \
                                 recoverable — act on the refs above, or use browser_evaluate \
                                 with a targeted DOM query]"
                            ),
                        }
                    } else {
                        wrapped
                    };
                    Ok(BrowserSnapshotOutput {
                        success: true,
                        snapshot: Some(snapshot),
                        truncated,
                        ref_count,
                        message: Some(format!("Snapshot captured in profile '{}'", args.profile)),
                    })
                }
                Err(e) => Ok(BrowserSnapshotOutput {
                    success: false,
                    snapshot: None,
                    truncated: false,
                    ref_count: 0,
                    message: Some(format!(
                        "Snapshot failed: {}",
                        super::backend_error_text(&self.manager, &e)
                    )),
                }),
            },
            Err(e) => Ok(BrowserSnapshotOutput {
                success: false,
                snapshot: None,
                truncated: false,
                ref_count: 0,
                message: Some(super::backend_error_text(&self.manager, &e)),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;
    use crate::tools::result_store::ToolResultStore;

    #[tokio::test]
    async fn test_snapshot_returns_snapshot() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserSnapshotTool::new(manager);

        let result = tool
            .call(BrowserSnapshotArgs {
                profile: "default".into(),
                max_chars: None,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    /// `max_chars` is the one lever that opts out of the shared content budget;
    /// without a ceiling it opts out entirely.
    #[test]
    fn max_chars_is_clamped_at_both_ends() {
        assert_eq!(resolve_max_chars(Some(usize::MAX)), MAX_SNAPSHOT_CHARS);
        assert_eq!(resolve_max_chars(Some(0)), MIN_SNAPSHOT_CHARS);
        assert_eq!(resolve_max_chars(Some(50_000)), 50_000);
        assert_eq!(
            resolve_max_chars(None),
            super::super::DEFAULT_CONTENT_MAX_CHARS,
            "the default must sit inside the clamp window"
        );
    }

    /// Truncating inside the tool used to be irreversible: the tail never
    /// reached `tool_output` ingress, so nothing downstream could persist it.
    /// The offload must put the WHOLE tree on disk — redacted, because the blob
    /// is read back into model context — and hand back a footer the model can
    /// act on.
    #[test]
    fn offload_persists_the_whole_tree_redacted() {
        let base = std::env::temp_dir()
            .join("aleph_test_browser_snapshot")
            .join("offload");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let store = ToolResultStore::with_dir_for_tests(base.clone());
        let manager = ProfileManager::new(BrowserSystemConfig::default());

        // A tree whose tail — the part `bound_content` would drop — carries both
        // an actionable ref and a credential the page leaked into its DOM.
        let mut tree: String = (0..4_000)
            .map(|i| format!("- generic \"filler {i}\" [ref=e{i}]\n"))
            .collect();
        tree.push_str("- text \"token sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789\"\n");
        tree.push_str("- button \"Submit order\" [ref=eLAST]\n");

        let footer = super::super::offload_content_to(
            &store,
            "call-1",
            &manager,
            BrowserSnapshotTool::NAME,
            &tree,
        )
        .expect("an over-budget tree must be offloaded");
        assert!(
            footer.contains("[Full output persisted: "),
            "the model needs a recovery handle: {footer}"
        );
        assert!(
            footer.contains("ctx_search"),
            "the blob must be indexed, not merely written: {footer}"
        );

        let path = footer
            .split("[Full output persisted: ")
            .nth(1)
            .and_then(|rest| rest.split(" (").next())
            .expect("marker names a path");
        let blob = std::fs::read_to_string(path).expect("blob exists on disk");
        assert!(
            blob.contains("[ref=eLAST]"),
            "the dropped tail must be recoverable"
        );
        assert!(
            !blob.contains("sk-ant-api03"),
            "the persisted copy is read back into context, so it must be redacted"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
