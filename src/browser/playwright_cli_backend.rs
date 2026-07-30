//! `PlaywrightCliBackend` — implements `BrowserBackend` by shelling out to `playwright-cli`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::network_policy::BrowserSsrfGuard;
use super::playwright_cli::{CliOutput, PlaywrightCliDriver};
use super::types::{
    ActionTarget, CookieOp, EmulateOptions, HistoryNav, ScreenshotOpts, ScreenshotOutput,
    ScrollDirection, SnapshotOutput, TabId,
};

pub struct PlaywrightCliBackend {
    driver: Arc<PlaywrightCliDriver>,
    session_key: String,
    ssrf_guard: Arc<BrowserSsrfGuard>,
    headless: bool,
}

impl PlaywrightCliBackend {
    pub fn new(
        driver: Arc<PlaywrightCliDriver>,
        session_key: impl Into<String>,
        ssrf_guard: Arc<BrowserSsrfGuard>,
        headless: bool,
    ) -> Self {
        Self {
            driver,
            session_key: session_key.into(),
            ssrf_guard,
            headless,
        }
    }

    fn nav_timeout(&self) -> Duration {
        Duration::from_secs(self.driver.config().nav_timeout_secs)
    }

    fn action_timeout(&self) -> Duration {
        Duration::from_secs(self.driver.config().action_timeout_secs)
    }

    async fn run(&self, args: &[&str], timeout: Duration) -> Result<CliOutput, BrowserError> {
        self.driver.run(&self.session_key, args, timeout).await
    }
}

fn target_ref(target: &ActionTarget) -> Result<&str, BrowserError> {
    match target {
        ActionTarget::Ref { ref_id } => Ok(ref_id.as_str()),
        ActionTarget::Coordinates { .. } => Err(BrowserError::ActionFailed(
            "this action requires a snapshot ref; coordinates unsupported for this op".into(),
        )),
    }
}

/// Translate a [`CookieOp`] into the `playwright-cli cookie-*` argv.
///
/// Pure so the presence-flag (`--httpOnly`/`--secure`) vs value-flag
/// (`--domain`/`--path`/`--expires`/`--sameSite`) wiring is unit-testable
/// without spawning a browser.
fn cookie_argv(op: &CookieOp) -> Vec<String> {
    match op {
        CookieOp::List { domain, path } => {
            let mut a = vec!["cookie-list".to_string()];
            if let Some(d) = domain {
                a.push("--domain".into());
                a.push(d.clone());
            }
            if let Some(p) = path {
                a.push("--path".into());
                a.push(p.clone());
            }
            a
        }
        CookieOp::Get { name } => vec!["cookie-get".into(), name.clone()],
        CookieOp::Set {
            name,
            value,
            domain,
            path,
            expires,
            http_only,
            secure,
            same_site,
        } => {
            let mut a = vec!["cookie-set".into(), name.clone(), value.clone()];
            if let Some(d) = domain {
                a.push("--domain".into());
                a.push(d.clone());
            }
            if let Some(p) = path {
                a.push("--path".into());
                a.push(p.clone());
            }
            if let Some(e) = expires {
                a.push("--expires".into());
                a.push(e.to_string());
            }
            // httpOnly / secure are presence flags: pass when true, omit otherwise.
            if *http_only == Some(true) {
                a.push("--httpOnly".into());
            }
            if *secure == Some(true) {
                a.push("--secure".into());
            }
            if let Some(ss) = same_site {
                a.push("--sameSite".into());
                a.push(ss.as_cli().to_string());
            }
            a
        }
        CookieOp::Delete { name } => vec!["cookie-delete".into(), name.clone()],
        CookieOp::Clear => vec!["cookie-clear".into()],
    }
}

#[async_trait]
impl BrowserBackend for PlaywrightCliBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        self.ssrf_guard
            .check_navigation(url)
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
        let mut args: Vec<&str> = Vec::new();
        if !self.headless {
            args.push("--headed");
        }
        args.push("tab-new");
        args.push(url);
        let _ = self.run(&args, self.nav_timeout()).await?;
        // Re-list once: the CLI's `tab-new` returns no id, so both the real
        // tab id AND the post-navigation audit come from this single snapshot
        // (the newly opened tab is the last-listed one). A failed listing
        // degrades to an empty snapshot — the audit skips and the id falls
        // back to the "last" sentinel rather than failing a successful open.
        let tabs_text = self.list_tabs().await.unwrap_or_default();
        let tab_id = super::tab_registry::parse_tab_ids(&tabs_text)
            .last()
            .cloned();
        // Post-navigation audit: a redirect may have landed the new tab on a
        // blocked origin the navigation-time guard never saw. On a violation,
        // quarantine — best-effort close the fresh tab — then surface the
        // policy error.
        if let Err(e) =
            super::post_nav::verify_landed_url(&self.ssrf_guard, &tabs_text, tab_id.as_deref())
                .await
        {
            if let Some(ref id) = tab_id {
                if let Err(close_err) = self.close_tab(id).await {
                    tracing::warn!(tab = %id, error = %close_err, "post-navigation quarantine: failed to close blocked tab");
                }
            }
            return Err(e);
        }
        Ok(tab_id.unwrap_or_else(|| "last".into()))
    }

    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        let _ = self
            .run(&["tab-close", tab_id], self.action_timeout())
            .await?;
        Ok(())
    }

    async fn list_tabs(&self) -> Result<String, BrowserError> {
        Ok(self.run(&["tab-list"], self.action_timeout()).await?.stdout)
    }

    async fn navigate(&self, _tab_id: &str, url: &str) -> Result<(), BrowserError> {
        self.ssrf_guard
            .check_navigation(url)
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
        let _ = self.run(&["goto", url], self.nav_timeout()).await?;
        // Post-navigation audit: a redirect may have landed the tab on a
        // blocked origin. `goto` drives the CLI session's current tab (the
        // last-listed one by the tool layer's convention), so the audit
        // resolves the active tab rather than the ignored `_tab_id`.
        super::post_nav::audit_landed_tab(self, &self.ssrf_guard, None).await?;
        Ok(())
    }

    async fn click(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        match target {
            ActionTarget::Ref { ref_id } => {
                let _ = self.run(&["click", &ref_id], self.action_timeout()).await?;
                Ok(())
            }
            ActionTarget::Coordinates { x, y } => {
                let xs = x.to_string();
                let ys = y.to_string();
                self.run(&["mousemove", &xs, &ys], self.action_timeout())
                    .await?;
                self.run(&["mousedown"], self.action_timeout()).await?;
                self.run(&["mouseup"], self.action_timeout()).await?;
                Ok(())
            }
        }
    }

    async fn type_text(
        &self,
        _tab_id: &str,
        target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError> {
        match target {
            // Ref target — type into the specific element (positional, mirrors `fill`).
            ActionTarget::Ref { ref_id } => {
                let _ = self
                    .run(&["type", &ref_id, text], self.action_timeout())
                    .await?;
            }
            // No element ref — type into whatever currently holds focus.
            ActionTarget::Coordinates { .. } => {
                let _ = self.run(&["type", text], self.action_timeout()).await?;
            }
        }
        Ok(())
    }

    async fn fill(
        &self,
        _tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        let ref_id = target_ref(&target)?;
        let _ = self
            .run(&["fill", ref_id, value], self.action_timeout())
            .await?;
        Ok(())
    }

    async fn hover(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let ref_id = target_ref(&target)?;
        let _ = self.run(&["hover", ref_id], self.action_timeout()).await?;
        Ok(())
    }

    async fn scroll(
        &self,
        _tab_id: &str,
        _target: ActionTarget,
        direction: ScrollDirection,
    ) -> Result<(), BrowserError> {
        let (dx, dy) = direction.wheel_delta();
        let (dx, dy) = (dx.to_string(), dy.to_string());
        let _ = self
            .run(&["mousewheel", &dx, &dy], self.action_timeout())
            .await?;
        Ok(())
    }

    async fn screenshot(
        &self,
        _tab_id: &str,
        opts: ScreenshotOpts,
    ) -> Result<ScreenshotOutput, BrowserError> {
        // playwright-cli infers the image format from the file extension, so
        // honor `opts.format` by choosing the matching extension.
        let ext = match opts.format.to_ascii_lowercase().as_str() {
            "jpeg" | "jpg" => "jpg",
            _ => "png",
        };
        let mut path = std::env::temp_dir();
        let fname = format!("aleph-ss-{}.{ext}", uuid::Uuid::new_v4());
        path.push(fname);
        let path_str = path.to_string_lossy().to_string();

        let mut args: Vec<String> =
            vec!["screenshot".to_string(), "--filename".to_string(), path_str];
        if opts.full_page {
            args.push("--full-page".to_string());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let result = self.run(&arg_refs, Duration::from_secs(15)).await;
        let png_bytes = match result {
            Ok(_) => {
                let bytes = tokio::fs::read(&path).await;
                let _ = tokio::fs::remove_file(&path).await;
                bytes.map_err(BrowserError::Io)?
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&path).await;
                return Err(e);
            }
        };
        Ok(ScreenshotOutput { png_bytes })
    }

    async fn snapshot(&self, _tab_id: &str) -> Result<SnapshotOutput, BrowserError> {
        let output = self.run(&["snapshot"], Duration::from_secs(15)).await?;
        let meta = output.page_meta.unwrap_or_default();
        let snapshot_text = if let Some(p) = meta.snapshot_file.as_ref() {
            tokio::fs::read_to_string(p)
                .await
                .unwrap_or_else(|_| output.stdout.clone())
        } else {
            output.stdout.clone()
        };
        Ok(SnapshotOutput {
            snapshot_text,
            page_url: meta.url,
            page_title: meta.title,
        })
    }

    async fn evaluate(&self, _tab_id: &str, js: &str) -> Result<String, BrowserError> {
        let output = self.run(&["eval", js], self.action_timeout()).await?;
        Ok(output.stdout)
    }

    async fn select(
        &self,
        _tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        let ref_id = target_ref(&target)?;
        let _ = self
            .run(&["select", ref_id, value], self.action_timeout())
            .await?;
        Ok(())
    }

    async fn press_key(&self, _tab_id: &str, key: &str) -> Result<(), BrowserError> {
        let _ = self.run(&["press", key], self.action_timeout()).await?;
        Ok(())
    }

    async fn history(&self, _tab_id: &str, nav: HistoryNav) -> Result<(), BrowserError> {
        // Native commands wait for the resulting navigation, unlike the JS-eval
        // default (history.back() returns before the load starts).
        let cmd = match nav {
            HistoryNav::Back => "go-back",
            HistoryNav::Forward => "go-forward",
            HistoryNav::Refresh => "reload",
        };
        let _ = self.run(&[cmd], self.nav_timeout()).await?;
        Ok(())
    }

    async fn dblclick(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let ref_id = target_ref(&target)?;
        let _ = self
            .run(&["dblclick", ref_id], self.action_timeout())
            .await?;
        Ok(())
    }

    // `wait_for` is NOT overridden: playwright-cli has no wait command, so the
    // trait default's evaluate-polling (see `wait_probe`) is exactly right —
    // this backend's `evaluate` runs the probe via `eval`.

    async fn console_messages(&self, _tab_id: &str) -> Result<String, BrowserError> {
        Ok(self.run(&["console"], self.action_timeout()).await?.stdout)
    }

    async fn network_log(&self, _tab_id: &str) -> Result<String, BrowserError> {
        Ok(self.run(&["network"], self.action_timeout()).await?.stdout)
    }

    async fn pdf(&self, _tab_id: &str, output_path: &Path) -> Result<(), BrowserError> {
        let path_str = output_path.to_string_lossy().to_string();
        let _ = self
            .run(&["pdf", "--filename", &path_str], Duration::from_secs(30))
            .await?;
        Ok(())
    }

    async fn drag(
        &self,
        _tab_id: &str,
        from: ActionTarget,
        to: ActionTarget,
    ) -> Result<(), BrowserError> {
        let from_ref = target_ref(&from)?;
        let to_ref = target_ref(&to)?;
        let _ = self
            .run(&["drag", from_ref, to_ref], self.action_timeout())
            .await?;
        Ok(())
    }

    async fn upload(
        &self,
        _tab_id: &str,
        _target: Option<ActionTarget>,
        paths: &[String],
    ) -> Result<(), BrowserError> {
        if paths.is_empty() {
            return Err(BrowserError::ActionFailed(
                "upload requires at least one file path".into(),
            ));
        }
        // `playwright-cli upload <file...>` self-targets the page's file chooser;
        // the element ref is only meaningful to the existing-session backend.
        let mut args: Vec<&str> = Vec::with_capacity(paths.len() + 1);
        args.push("upload");
        args.extend(paths.iter().map(String::as_str));
        let _ = self.run(&args, self.action_timeout()).await?;
        Ok(())
    }

    async fn resize(&self, _tab_id: &str, width: u32, height: u32) -> Result<(), BrowserError> {
        let w = width.to_string();
        let h = height.to_string();
        let _ = self.run(&["resize", &w, &h], self.action_timeout()).await?;
        Ok(())
    }

    async fn switch_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        let _ = self
            .run(&["tab-select", tab_id], self.action_timeout())
            .await?;
        Ok(())
    }

    async fn handle_dialog(
        &self,
        _tab_id: &str,
        action: &str,
        prompt_text: Option<&str>,
    ) -> Result<(), BrowserError> {
        match action.to_ascii_lowercase().as_str() {
            "accept" | "ok" | "confirm" => {
                let mut args = vec!["dialog-accept"];
                if let Some(text) = prompt_text {
                    args.push(text);
                }
                let _ = self.run(&args, self.action_timeout()).await?;
                Ok(())
            }
            "dismiss" | "cancel" | "reject" => {
                let _ = self.run(&["dialog-dismiss"], self.action_timeout()).await?;
                Ok(())
            }
            other => Err(BrowserError::ActionFailed(format!(
                "unknown dialog action '{other}' — expected 'accept' or 'dismiss'"
            ))),
        }
    }

    async fn emulate(&self, _tab_id: &str, opts: &EmulateOptions) -> Result<(), BrowserError> {
        opts.validate().map_err(BrowserError::ActionFailed)?;
        // The managed Playwright CLI can only toggle online/offline at runtime;
        // color scheme, geolocation, CPU throttling, HTTP headers and user-agent
        // are context-construction options it does not expose as live commands.
        if opts.color_scheme.is_some()
            || opts.geolocation.is_some()
            || opts.cpu_throttle.is_some()
            || opts.extra_http_headers.is_some()
            || opts.user_agent.is_some()
        {
            return Err(BrowserError::ActionFailed(
                "managed profile only supports network_condition emulation; for color scheme, \
                 geolocation, CPU throttle, HTTP headers or user-agent use an existing-session \
                 profile (e.g. 'user')"
                    .into(),
            ));
        }
        match opts.network_condition {
            Some(cond) => match cond.as_playwright_state() {
                Some(state) => {
                    let _ = self
                        .run(&["network-state-set", state], self.action_timeout())
                        .await?;
                    Ok(())
                }
                None => Err(BrowserError::ActionFailed(format!(
                    "managed profile supports only offline/online network emulation, not {cond:?}; \
                     use an existing-session profile for throttled tiers"
                ))),
            },
            // validate() already guaranteed at least one field is set, and the
            // block above rejected every non-network field, so this is unreachable.
            None => Err(BrowserError::ActionFailed(
                "emulate requires at least one option".into(),
            )),
        }
    }

    async fn save_state(&self, path: &Path) -> Result<(), BrowserError> {
        let p = path.to_string_lossy().to_string();
        self.run(&["state-save", &p], self.action_timeout()).await?;
        Ok(())
    }

    async fn load_state(&self, path: &Path) -> Result<(), BrowserError> {
        let p = path.to_string_lossy().to_string();
        self.run(&["state-load", &p], self.action_timeout()).await?;
        Ok(())
    }

    async fn cookies(&self, op: &CookieOp) -> Result<String, BrowserError> {
        let argv = cookie_argv(op);
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        Ok(self.run(&refs, self.action_timeout()).await?.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::network_policy::{BrowserSsrfGuard, SsrfConfig};
    use crate::browser::profile::PlaywrightCliConfig;

    fn test_backend() -> PlaywrightCliBackend {
        let driver = Arc::new(PlaywrightCliDriver::new(PlaywrightCliConfig::default()));
        let guard = Arc::new(BrowserSsrfGuard::new(SsrfConfig::default()));
        PlaywrightCliBackend::new(driver, "test", guard, true)
    }

    #[test]
    fn test_target_ref_rejects_coordinates() {
        let result = target_ref(&ActionTarget::Coordinates { x: 0.0, y: 0.0 });
        assert!(matches!(result, Err(BrowserError::ActionFailed(_))));
    }

    #[test]
    fn test_target_ref_accepts_ref() {
        let target = ActionTarget::Ref {
            ref_id: "e42".into(),
        };
        let result = target_ref(&target);
        assert_eq!(result.unwrap(), "e42");
    }

    #[tokio::test]
    async fn test_navigate_rejects_ssrf_blocked_url() {
        let backend = test_backend();
        let result = backend
            .navigate("last", "http://127.0.0.1:8080/secret")
            .await;
        assert!(matches!(result, Err(BrowserError::NavigationFailed(_))));
    }

    #[tokio::test]
    async fn test_emulate_rejects_mcp_only_fields_before_spawn() {
        use crate::browser::types::{ColorScheme, EmulateOptions};
        // color_scheme is an existing-session-only override; the managed backend
        // must reject it up-front (without spawning the CLI) and point to 'user'.
        let backend = test_backend();
        let opts = EmulateOptions {
            color_scheme: Some(ColorScheme::Dark),
            ..Default::default()
        };
        let err = backend.emulate("last", &opts).await.unwrap_err();
        match err {
            BrowserError::ActionFailed(msg) => {
                assert!(msg.contains("existing-session"), "got: {msg}");
            }
            other => panic!("expected ActionFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_emulate_rejects_empty_options() {
        use crate::browser::types::EmulateOptions;
        let backend = test_backend();
        let err = backend
            .emulate("last", &EmulateOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, BrowserError::ActionFailed(_)));
    }

    #[test]
    fn test_cookie_argv_list_and_mutations() {
        assert_eq!(
            cookie_argv(&CookieOp::List {
                domain: Some("example.com".into()),
                path: None,
            }),
            vec!["cookie-list", "--domain", "example.com"]
        );
        assert_eq!(
            cookie_argv(&CookieOp::Get { name: "sid".into() }),
            vec!["cookie-get", "sid"]
        );
        assert_eq!(
            cookie_argv(&CookieOp::Delete { name: "sid".into() }),
            vec!["cookie-delete", "sid"]
        );
        assert_eq!(cookie_argv(&CookieOp::Clear), vec!["cookie-clear"]);
    }

    #[test]
    fn test_cookie_argv_set_presence_and_value_flags() {
        use crate::browser::types::SameSite;
        // httpOnly true → flag present; secure false → flag omitted; expires &
        // sameSite are value flags.
        let argv = cookie_argv(&CookieOp::Set {
            name: "token".into(),
            value: "abc".into(),
            domain: Some("example.com".into()),
            path: Some("/".into()),
            expires: Some(1_900_000_000),
            http_only: Some(true),
            secure: Some(false),
            same_site: Some(SameSite::Lax),
        });
        assert_eq!(
            argv,
            vec![
                "cookie-set",
                "token",
                "abc",
                "--domain",
                "example.com",
                "--path",
                "/",
                "--expires",
                "1900000000",
                "--httpOnly",
                "--sameSite",
                "Lax",
            ]
        );
        // Minimal set: no optional attributes, secure omitted when None.
        assert_eq!(
            cookie_argv(&CookieOp::Set {
                name: "k".into(),
                value: "v".into(),
                domain: None,
                path: None,
                expires: None,
                http_only: None,
                secure: None,
                same_site: None,
            }),
            vec!["cookie-set", "k", "v"]
        );
    }
}
