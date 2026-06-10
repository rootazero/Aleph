//! `wait_visual` action — block until the screen stabilizes.
//!
//! Polls [`ScreenCapability::screenshot`] at a fixed cadence and returns once
//! two consecutive captures are byte-identical (the screen has "settled"), or
//! when the timeout elapses. Useful after navigation, animations, or any
//! action that triggers gradual visual change.
//!
//! Pure orchestration over the existing `ScreenCapability` — no new trait
//! methods, no platform impls touched. R10-aligned (thin harness).

use std::time::{Duration, Instant};

use aleph_desktop::{ScreenCapability, ScreenRegion as DesktopRegion};

use super::types::{DesktopOutput, ScreenRegion};

/// Default poll interval — short enough for quick UI transitions, long enough
/// not to thrash the bridge for a JPEG re-encode every iteration.
const DEFAULT_POLL_MS: u64 = 250;

/// Number of consecutive byte-identical captures required to declare the
/// screen stable. Two suffices to filter single-frame jitter without
/// over-waiting on truly static content.
const REQUIRED_STABLE_HITS: u32 = 2;

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_TIMEOUT_MS: u64 = 60_000;
const MIN_POLL_MS: u64 = 50;

/// Wait until the screen (or a region of it) stabilizes.
///
/// Returns a successful `DesktopOutput` regardless of whether the screen
/// stabilized — callers inspect `data.stable` (bool) to differentiate. A
/// screenshot failure mid-poll returns `success=false`.
pub async fn run_wait_visual(
    screen: &dyn ScreenCapability,
    timeout_ms: Option<u64>,
    region: Option<ScreenRegion>,
) -> DesktopOutput {
    let timeout =
        Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).min(MAX_TIMEOUT_MS));
    let poll_interval = Duration::from_millis(DEFAULT_POLL_MS.max(MIN_POLL_MS));

    let desktop_region = region.map(|r| DesktopRegion {
        x: r.x.max(0.0) as u32,
        y: r.y.max(0.0) as u32,
        width: r.width.max(0.0) as u32,
        height: r.height.max(0.0) as u32,
    });

    let start = Instant::now();
    let mut polls: u32 = 0;
    let mut consecutive_hits: u32 = 0;
    let mut last_b64: Option<String> = None;

    loop {
        polls += 1;
        let shot = match screen.screenshot(desktop_region).await {
            Ok(s) => s,
            Err(e) => {
                return DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(format!("wait_visual: screenshot failed: {e}")),
                };
            }
        };

        // Byte equality on the encoded image is enough: identical pixels
        // produce identical PNG output (the encoder is deterministic), and
        // for JPEG, identical input yields identical output too. Subpixel
        // jitter (animated cursor, blinking caret) will show up as a diff
        // and prevent false-positive stability — the caller's timeout is
        // the safety net for genuinely animated UIs.
        let matched = match last_b64.as_ref() {
            Some(prev) => *prev == shot.image_base64,
            None => false,
        };

        if matched {
            consecutive_hits += 1;
            if consecutive_hits >= REQUIRED_STABLE_HITS {
                return DesktopOutput {
                    success: true,
                    data: Some(serde_json::json!({
                        "stable": true,
                        "polls": polls,
                        "elapsed_ms": start.elapsed().as_millis() as u64,
                    })),
                    message: None,
                };
            }
        } else {
            consecutive_hits = 0;
        }

        last_b64 = Some(shot.image_base64);

        if start.elapsed() >= timeout {
            return DesktopOutput {
                success: true,
                data: Some(serde_json::json!({
                    "stable": false,
                    "polls": polls,
                    "elapsed_ms": start.elapsed().as_millis() as u64,
                    "reason": "timeout",
                })),
                message: Some(
                    "wait_visual: timed out without stabilization (screen still changing)".into(),
                ),
            };
        }

        tokio::time::sleep(poll_interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_desktop::{
        DisplayInfo, MouseButton, OcrResult, PressAction, Result, ScreenRegion as DesktopRegion,
        Screenshot, WindowInfo,
    };
    use async_trait::async_trait;
    use crate::sync_primitives::Mutex;

    /// A scripted screen that returns a fixed sequence of base64 strings.
    /// Other capability methods are unimplemented — the polling loop only
    /// exercises `screenshot`.
    struct ScriptedScreen {
        frames: Mutex<Vec<&'static str>>,
        idx: Mutex<usize>,
    }

    impl ScriptedScreen {
        fn new(frames: Vec<&'static str>) -> Self {
            Self {
                frames: Mutex::new(frames),
                idx: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl ScreenCapability for ScriptedScreen {
        async fn screenshot(&self, _region: Option<DesktopRegion>) -> Result<Screenshot> {
            let mut idx = self.idx.lock().unwrap_or_else(|e| e.into_inner());
            let frames = self.frames.lock().unwrap_or_else(|e| e.into_inner());
            let i = (*idx).min(frames.len() - 1);
            *idx = (*idx + 1).min(frames.len() - 1);
            Ok(Screenshot {
                image_base64: frames[i].to_string(),
                width: 1,
                height: 1,
                format: "png".to_string(),
                scale_factor: None,
            })
        }

        async fn ocr(&self, _image_png: Option<&[u8]>) -> Result<OcrResult> {
            unimplemented!()
        }

        async fn click(&self, _x: f64, _y: f64, _button: MouseButton) -> Result<()> {
            unimplemented!()
        }

        async fn type_text(&self, _text: &str) -> Result<()> {
            unimplemented!()
        }

        async fn key_combo(&self, _modifiers: &[String], _key: &str) -> Result<()> {
            unimplemented!()
        }

        async fn scroll(&self, _direction: &str, _amount: i32) -> Result<()> {
            unimplemented!()
        }

        async fn window_list(&self) -> Result<Vec<WindowInfo>> {
            unimplemented!()
        }

        async fn focus_window(&self, _window_id: u64) -> Result<()> {
            unimplemented!()
        }

        async fn launch_app(&self, _app_name: &str) -> Result<()> {
            unimplemented!()
        }

        async fn cursor_position(&self) -> Result<(f64, f64)> {
            unimplemented!()
        }

        async fn mouse_button(
            &self,
            _x: f64,
            _y: f64,
            _button: MouseButton,
            _action: PressAction,
        ) -> Result<()> {
            unimplemented!()
        }

        async fn display_list(&self) -> Result<Vec<DisplayInfo>> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn returns_stable_when_two_consecutive_frames_match() {
        // Sequence: A, B, B, B — polls 1,2,3 should hit stable on poll 3
        // (2 consecutive matches after the first frame).
        let screen = ScriptedScreen::new(vec!["A", "B", "B", "B"]);
        let out = run_wait_visual(&screen, Some(5_000), None).await;
        assert!(out.success);
        let data = out.data.expect("data present");
        assert_eq!(data["stable"], serde_json::Value::Bool(true));
        // We require REQUIRED_STABLE_HITS=2 consecutive identical, so at
        // least 3 polls. The exact count depends on the scripted sequence.
        assert!(data["polls"].as_u64().unwrap() >= 3);
    }

    #[tokio::test]
    async fn returns_not_stable_on_timeout_when_screen_keeps_changing() {
        // Sequence cycles A,B,A,B,... — never stabilizes.
        let screen = ScriptedScreen::new(vec!["A", "B", "A", "B", "A", "B"]);
        let out = run_wait_visual(&screen, Some(250), None).await;
        assert!(out.success); // timeout is a valid outcome, not an error
        let data = out.data.expect("data present");
        assert_eq!(data["stable"], serde_json::Value::Bool(false));
        assert_eq!(data["reason"], "timeout");
    }

    #[tokio::test]
    async fn caps_timeout_to_maximum() {
        // Pass a deliberately huge timeout — implementation should clamp to
        // MAX_TIMEOUT_MS, but we just verify the call returns promptly when
        // the screen is already stable.
        let screen = ScriptedScreen::new(vec!["X", "X", "X"]);
        let out = run_wait_visual(&screen, Some(u64::MAX), None).await;
        assert!(out.success);
        assert_eq!(out.data.unwrap()["stable"], serde_json::Value::Bool(true));
    }
}
