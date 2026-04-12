//! PlaywrightCliBackend — implements `BrowserBackend` by shelling out to `playwright-cli`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::network_policy::BrowserSsrfGuard;
use super::playwright_cli::{CliOutput, PlaywrightCliDriver};
use super::types::{
    ActionTarget, ScreenshotOpts, ScreenshotOutput, ScrollDirection, SnapshotOutput, TabId,
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

#[async_trait]
impl BrowserBackend for PlaywrightCliBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        self.ssrf_guard
            .check_url(url)
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
        let mut args: Vec<&str> = Vec::new();
        if !self.headless {
            args.push("--headed");
        }
        args.push("tab-new");
        args.push(url);
        let _ = self.run(&args, self.nav_timeout()).await?;
        Ok("last".into())
    }

    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        let _ = self.run(&["tab-close", tab_id], self.action_timeout()).await?;
        Ok(())
    }

    async fn list_tabs(&self) -> Result<String, BrowserError> {
        Ok(self.run(&["tab-list"], self.action_timeout()).await?.stdout)
    }

    async fn navigate(&self, _tab_id: &str, url: &str) -> Result<(), BrowserError> {
        self.ssrf_guard
            .check_url(url)
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
        let _ = self.run(&["goto", url], self.nav_timeout()).await?;
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
                self.run(&["mousemove", &xs, &ys], self.action_timeout()).await?;
                self.run(&["mousedown"], self.action_timeout()).await?;
                self.run(&["mouseup"], self.action_timeout()).await?;
                Ok(())
            }
        }
    }

    async fn type_text(
        &self,
        _tab_id: &str,
        _target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError> {
        let _ = self.run(&["type", text], self.action_timeout()).await?;
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
        let (dx, dy) = match direction {
            ScrollDirection::Up => ("0", "-400"),
            ScrollDirection::Down => ("0", "400"),
            ScrollDirection::Left => ("-400", "0"),
            ScrollDirection::Right => ("400", "0"),
        };
        let _ = self
            .run(&["mousewheel", dx, dy], self.action_timeout())
            .await?;
        Ok(())
    }

    async fn screenshot(
        &self,
        _tab_id: &str,
        _opts: ScreenshotOpts,
    ) -> Result<ScreenshotOutput, BrowserError> {
        let mut path = std::env::temp_dir();
        let fname = format!("aleph-ss-{}.png", uuid::Uuid::new_v4());
        path.push(fname);
        let path_str = path.to_string_lossy().to_string();
        let _ = self
            .run(
                &["screenshot", "--filename", &path_str],
                Duration::from_secs(15),
            )
            .await?;
        let png_bytes = tokio::fs::read(&path).await.map_err(BrowserError::Io)?;
        let _ = tokio::fs::remove_file(&path).await;
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
        let result = backend.navigate("last", "http://127.0.0.1:8080/secret").await;
        assert!(matches!(result, Err(BrowserError::NavigationFailed(_))));
    }
}
