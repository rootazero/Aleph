//! `wait_visual` action — block until the screen stabilizes.
//!
//! Polls [`ScreenCapability::screenshot`] at a fixed cadence and returns once
//! two consecutive captures are byte-identical (the screen has "settled"), or
//! when the timeout elapses. Useful after navigation, animations, or any
//! action that triggers gradual visual change.
//!
//! A capture that carries no pixels is refused rather than compared: two
//! identical dead frames are byte-identical, and would otherwise be reported as
//! a settled screen.
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

    let desktop_region = region.map(|r| {
        // `NaN.max(0.0)` is NaN; `NaN as u32` is 0 — so a NaN coordinate
        // would shrink the capture to the origin. Substitute 0 explicitly
        // for any non-finite component so the dispatch path keeps a sane
        // (zero-area) region instead of UB. The caller should be validating
        // upstream, but defense in depth costs one branch.
        let pick = |v: f64| if v.is_finite() { v.max(0.0) as u32 } else { 0 };
        DesktopRegion {
            x: pick(r.x),
            y: pick(r.y),
            width: pick(r.width),
            height: pick(r.height),
        }
    });

    let start = Instant::now();
    let mut polls: u32 = 0;
    let mut consecutive_hits: u32 = 0;
    let mut last_b64: Option<String> = None;

    // A park owes an arm to a mid-loop steer as well as to the cancel token:
    // the user's message is written to the session log and answered at the
    // running loop's next turn boundary, and for up to `MAX_TIMEOUT_MS` this
    // loop IS that turn. See `crate::session::steer_signal`.
    //
    // Hooked to the inter-poll sleep rather than wrapped around the whole
    // function: we own this loop, so there is no need to abandon an in-flight
    // capture to get out of it. Worst-case latency is therefore one screenshot
    // round-trip, not the remaining window. (`browser_wait_for` hands its wait
    // to a backend as one opaque call and has no loop to hook, so it races and
    // drops instead — different seam, same rule.)
    let mut steer = crate::session::steer_signal::watch_current_turn();

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

        // A frame carrying no pixels (locked screen, revoked recording grant,
        // wedged helper) is byte-identical to the next one just like it, so the
        // stability test would answer "the UI has settled" about a screen it
        // cannot see — the single most misleading verdict this tool can return.
        // Refusing costs a retry; a frame of genuinely uniform content is
        // indistinguishable from a dead capture from here, and that tradeoff is
        // the right way round.
        if aleph_desktop::perception::is_degenerate(&shot) {
            return DesktopOutput {
                success: false,
                data: Some(serde_json::json!({
                    "stable": false,
                    "polls": polls,
                    "elapsed_ms": start.elapsed().as_millis() as u64,
                    "reason": "blank_frame",
                })),
                message: Some(super::recovery::with_hint(
                    "wait_visual: screen capture returned a blank frame — stability is \
                     unknowable from it."
                        .to_string(),
                )),
            };
        }

        // Byte equality on the encoded image is enough when the encoder is
        // deterministic (PNG, BMP). JPEG and WebP encoders are typically
        // non-deterministic due to IDCT/Huffman optimizations — identical
        // pixels produce different bytes on each poll and `matched` stays
        // false even for a fully static UI. The caller's `timeout_ms` is
        // the safety net in that case. Subpixel jitter (animated cursor,
        // blinking caret) shows up as a diff and correctly prevents
        // false-positive stability on deterministic formats.
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

        tokio::select! {
            biased;
            // Polled first, so a steer that landed during the capture above is
            // acted on before we sleep another interval on top of it. Note this
            // is the opposite order from `SteerWatch::race`, and deliberately:
            // there the other arm is the WAIT ITSELF, and a result that already
            // landed must not be thrown away for an interruption. Here the other
            // arm is an idle delay that produces nothing, so there is no result
            // to lose. The rule is "a finished result beats the interruption",
            // not "the other arm always goes first".
            () = steer.steered() => {
                return DesktopOutput {
                    success: true,
                    data: Some(serde_json::json!({
                        // NOT a verdict on the screen — we stopped looking. The
                        // `reason` is what separates this from the timeout arm,
                        // which reports the same `stable: false` having actually
                        // watched for the whole window.
                        "stable": false,
                        "polls": polls,
                        "elapsed_ms": start.elapsed().as_millis() as u64,
                        "reason": "user_input",
                    })),
                    message: Some(
                        "wait_visual: the user sent new input, so this wait returned early \
                         instead of watching for the full window — their message is in your \
                         context, read it before deciding what to do next. The screen was \
                         NOT confirmed stable and NOT confirmed still changing; nothing was \
                         observed after this point."
                            .into(),
                    ),
                };
            }
            () = tokio::time::sleep(poll_interval) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Mutex;
    use aleph_desktop::{
        DisplayInfo, MouseButton, OcrResult, PressAction, Result, ScreenRegion as DesktopRegion,
        Screenshot, WindowInfo,
    };
    use async_trait::async_trait;

    /// 2×2 PNGs with real structure — the stability loop now decodes every
    /// frame to reject dead captures, so the fixtures must be decodable images
    /// rather than arbitrary strings.
    const FRAME_A: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAE0lEQVR4nGNgYGD4//8/GDMwAAAp5AX71ZPZmwAAAABJRU5ErkJggg==";
    const FRAME_B: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4//8/AwQAWQAp5AX7XiD3SwAAAABJRU5ErkJggg==";
    /// All-black 2×2 PNG — what a locked screen / revoked grant hands back.
    const BLANK_FRAME: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAC0lEQVR4nGNgQAYAAA4AAamRc7EAAAAASUVORK5CYII=";

    /// A scripted screen that returns a fixed sequence of base64 frames.
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
                width: 2,
                height: 2,
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
        let screen = ScriptedScreen::new(vec![FRAME_A, FRAME_B, FRAME_B, FRAME_B]);
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
        let screen =
            ScriptedScreen::new(vec![FRAME_A, FRAME_B, FRAME_A, FRAME_B, FRAME_A, FRAME_B]);
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
        let screen = ScriptedScreen::new(vec![FRAME_A, FRAME_A, FRAME_A]);
        let out = run_wait_visual(&screen, Some(u64::MAX), None).await;
        assert!(out.success);
        assert_eq!(out.data.unwrap()["stable"], serde_json::Value::Bool(true));
    }

    /// Round-10 — a mid-loop steer must cut the watch short.
    ///
    /// Without the arm this call watches a never-settling screen for the full
    /// `MAX_TIMEOUT_MS` (60 s) while the user's correction — already durably in
    /// the session log, already reported to the client as delivered — goes
    /// unread, because for that whole window this loop *is* the running turn.
    ///
    /// Bounded rather than merely measured: with the arm removed this would
    /// wedge for a minute, and a test that hangs costs the suite its signal.
    ///
    /// The **inert** half of the same arm needs no test of its own here:
    /// `returns_not_stable_on_timeout_when_screen_keeps_changing` runs the very
    /// same `select!` with no `TURN_CONTEXT` and asserts `reason == "timeout"`,
    /// so an inert arm that resolved immediately would turn it red.
    #[tokio::test]
    async fn a_steer_cuts_the_watch_short() {
        use crate::routing::session_key::SessionKey;
        use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

        // Cycles forever, so only the timeout or the steer can end this.
        let screen = ScriptedScreen::new(vec![FRAME_A, FRAME_B, FRAME_A, FRAME_B]);
        let session = SessionKey::peer("main", "wait-visual-steer");
        let turn = TurnContext {
            session_key: session.clone(),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
        };

        let steered = session.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            crate::session::steer_signal::note_steer(&steered);
        });

        let out = tokio::time::timeout(
            Duration::from_secs(10),
            TURN_CONTEXT.scope(turn, run_wait_visual(&screen, Some(60_000), None)),
        )
        .await
        .expect("a steered watch must not run out its window");

        assert!(out.success, "nothing failed; we stopped looking");
        let data = out.data.expect("data present");
        assert_eq!(
            data["reason"], "user_input",
            "a steer and a timeout are different answers and must not share a reason"
        );
        assert_eq!(data["stable"], serde_json::Value::Bool(false));
        let message = out.message.expect("message present");
        assert!(
            message.contains("NOT confirmed stable"),
            "the report must refuse to pass `stable: false` off as a verdict: {message}"
        );
    }

    /// A steer on somebody else's session must leave this watch alone — the
    /// station registry is process-global, which is exactly where per-session
    /// keying quietly degrades into "wake everyone".
    #[tokio::test]
    async fn a_steer_on_another_session_does_not_cut_the_watch_short() {
        use crate::routing::session_key::SessionKey;
        use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

        let screen = ScriptedScreen::new(vec![FRAME_A, FRAME_B, FRAME_A, FRAME_B]);
        let mine = SessionKey::peer("main", "wait-visual-scope-mine");
        let theirs = SessionKey::peer("main", "wait-visual-scope-theirs");
        let turn = TurnContext {
            session_key: mine,
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
        };

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            crate::session::steer_signal::note_steer(&theirs);
        });

        // Short window so the assertion is "we reached the TIMEOUT arm".
        let out = TURN_CONTEXT
            .scope(turn, run_wait_visual(&screen, Some(300), None))
            .await;
        assert_eq!(
            out.data.expect("data present")["reason"],
            "timeout",
            "another session's steer must not end this watch"
        );
    }

    #[tokio::test]
    async fn blank_frames_are_never_reported_as_stable() {
        // Two identical dead frames used to satisfy the byte-equality test and
        // come back as `stable: true` — the tool telling a model the screen had
        // settled while it could not see the screen at all.
        let screen = ScriptedScreen::new(vec![BLANK_FRAME, BLANK_FRAME, BLANK_FRAME]);
        let out = run_wait_visual(&screen, Some(5_000), None).await;
        assert!(!out.success, "a dead capture is a failure, not a verdict");
        let data = out.data.expect("data present");
        assert_eq!(data["stable"], serde_json::Value::Bool(false));
        assert_eq!(data["reason"], "blank_frame");
        let message = out.message.expect("message present");
        assert!(message.contains("blank frame"), "message: {message}");
        assert!(
            message.contains("screen-recording"),
            "the recovery hint must name the actionable causes: {message}"
        );
    }
}
