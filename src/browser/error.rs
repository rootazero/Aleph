use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrowserError {
    /// A launch that did not reach a usable browser, tagged with **which
    /// step** failed. The stage is not decoration: "the binary would not
    /// spawn", "the process died before it published a port" and "the port
    /// file never appeared" are three different operator problems, and a
    /// single opaque string made the tool answer identical for all three.
    /// `stage` values in use: `"spawn"`, `"chromium-exit"`, `"devtools-port"`
    /// (this module's Chromium launch) and `"chrome-mcp"` (the existing-session
    /// driver's Chrome launch).
    #[error("Failed to launch browser at stage '{stage}': {detail}")]
    LaunchFailed { stage: &'static str, detail: String },

    #[error("Tab not found: {0}")]
    TabNotFound(String),

    #[error("Navigation failed: {0}")]
    NavigationFailed(String),

    #[error("Browser action failed: {0}")]
    ActionFailed(String),

    #[error("Browser operation timed out after {0}ms")]
    Timeout(u64),

    #[error("Chromium binary not found. Install Chrome/Chromium or specify a binary path.")]
    ChromiumNotFound,

    /// The **managed** driver has no browser to launch: the pin (if any) is
    /// gone, no system Chromium-family browser was found, and Playwright's own
    /// Chromium is not installed either. Distinct from [`Self::ChromiumNotFound`]
    /// on purpose — that one answers "is there a system browser?", which the
    /// doctor and the existing-session driver ask and this driver does not.
    /// The message names the command that fixes it, because a fail-closed
    /// answer that does not say how to open the gate is fail-dead (判据 §14).
    ///
    /// The FIRST door named must be one the reader can actually open (Final
    /// Review M8): `runtime_manage` is in `method_authz::OPERATOR_TOOLS`
    /// (checked against the array itself, not this comment — the array holds
    /// `"runtime_manage"` and does NOT hold `"bash"`, and `tool_requires_operator`
    /// is what `tools/scoped/dispatch.rs`'s channel gate actually calls), so a
    /// chat-tier caller who reads "ask me to run `runtime_manage{...}`" as its
    /// next step tries it and is refused. `bash` is absent from that array, so
    /// it stays open to chat tier — the plain `playwright-cli` command goes
    /// first and is labelled as self-runnable; the operator-only remedies
    /// follow, labelled as such, so a chat-tier reader is told which doors are
    /// not its own rather than discovering that by being refused.
    ///
    /// The command text is `format!`-built from `CHROMIUM_INSTALL_ARGS`
    /// (`runtime_manage.rs`), not typed out a second time: a literal here
    /// compared against a literal in the test only proves the two agree with
    /// each other, never that either still names a command that exists.
    #[error(
        "No Chromium for the managed browser driver ({tried}). \
             Run `playwright-cli {install_args}` yourself — a plain local \
             command, not an operator-gated tool. Operators can instead run \
             `runtime_manage{{action:\"install\", capability:\"chromium\"}}` or pin \
             one with [general.browser.runtime] binary_path.",
        install_args = crate::builtin_tools::runtime_manage::CHROMIUM_INSTALL_ARGS.join(" ")
    )]
    ChromiumUnavailable { tried: String },

    #[error("Screenshot failed: {0}")]
    ScreenshotFailed(String),

    #[error("Failed to attach to browser: {0}")]
    AttachFailed(String),

    /// The MCP server answered, and its answer was a failure — the tool's own
    /// verdict about the page (element missing, wait elapsed, …).
    #[error("Chrome DevTools MCP error: {0}")]
    ChromeMcpError(String),

    /// The MCP call never got an answer (broken pipe, dead server, client-side
    /// request timeout). Distinct from [`Self::ChromeMcpError`] because "the
    /// tool said no" and "nothing ever looked" are different facts, and a
    /// caller that folds a negative verdict into a value (`wait_for` →
    /// `Ok(false)`) must never fold this one.
    #[error("Chrome DevTools MCP transport failure: {0}")]
    ChromeMcpTransport(String),

    #[error("Playwright CLI error: {0}")]
    PlaywrightCliError(String),

    #[error("Playwright CLI not installed. Open Settings → Browser → Install All.")]
    PlaywrightCliNotInstalled,

    #[error("No active browser session for '{0}'. Call open/goto first.")]
    NoSession(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Browser profile not found: {0}")]
    ProfileNotFound(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M1 (task-8 fix round, promoted from Minor): this message names
    /// `runtime_manage` as the fix for a missing Chromium — a bare literal,
    /// unpinned against a rename, while `chromium_missing.rs`'s doctor fix
    /// hint (the other producer of the same fact) was already pinned via
    /// `missing_finding_for_test`. Derived from the tool's own name constant,
    /// not a fresh literal: a fresh literal here would only prove the two
    /// strings agree with each other, not that either still names a real tool
    /// (判据 §10).
    #[test]
    fn the_chromium_unavailable_message_names_a_tool_that_actually_exists() {
        use crate::builtin_tools::runtime_manage::RuntimeManageTool;
        use crate::tools::AlephTool;

        let err = BrowserError::ChromiumUnavailable {
            tried: "no system browser".to_string(),
        };
        assert!(
            err.to_string().contains(RuntimeManageTool::NAME),
            "the fix hint must name a tool that still exists: {err}"
        );
    }

    /// M8: the first remedy named must be one a chat-tier caller can act on
    /// itself. `runtime_manage` is in `method_authz::OPERATOR_TOOLS` — naming
    /// it first (as "ask me to run ...") reads as the model's own next step
    /// and gets it refused. The plain `playwright-cli` command (which `bash`,
    /// absent from that array, can run for any caller) must come first, and
    /// the operator-gated remedy must be labelled as such rather than left to
    /// look equally available.
    ///
    /// The CLI command is matched against `CHROMIUM_INSTALL_ARGS` itself, not
    /// a second typed-out copy of "install-browser chromium" — a
    /// literal-vs-literal match would only prove the message text agrees with
    /// itself, and would stay green the day the real command changed and the
    /// hint quietly started naming one that no longer exists (判据 §10, §17).
    #[test]
    fn the_first_remedy_named_is_one_a_chat_tier_caller_can_actually_run() {
        let err = BrowserError::ChromiumUnavailable {
            tried: "no system browser".to_string(),
        };
        let text = err.to_string();
        let install_args = crate::builtin_tools::runtime_manage::CHROMIUM_INSTALL_ARGS.join(" ");
        let cli_marker = format!("playwright-cli {install_args}");
        let cli_at = text
            .find(&cli_marker)
            .unwrap_or_else(|| panic!("names the plain CLI command ({cli_marker:?}): {text}"));
        let runtime_manage_at = text
            .find("runtime_manage")
            .expect("still names runtime_manage as an alternative");
        assert!(
            cli_at < runtime_manage_at,
            "the self-runnable remedy must be named before the operator-gated \
             one, not after: {text}"
        );
        assert!(
            text.contains("Operators can instead"),
            "the operator-gated remedy must be labelled as such, not left \
             looking equally available to every caller: {text}"
        );
    }
}
