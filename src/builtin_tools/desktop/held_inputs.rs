//! Ledger of desktop inputs the agent currently holds down.
//!
//! `key_button` / `mouse_button` with `press_action: "press"` hold a key or a
//! mouse button down *across* tool calls — a real, physical hold on the user's
//! machine. Nothing guarantees the matching release ever arrives: the model can
//! error out, be aborted with Escape mid-drag, or simply forget, and the
//! modifier stays stuck on the keyboard until the user notices. The tool that
//! took the resource is the one that must give it back, so it records what it
//! holds here and releases it at the run boundary / on abort (see
//! `DesktopTool::check_escape`).
//!
//! Shape and locking discipline mirror the session-lock ledger in
//! [`super::session_lock`]: one process-global `Mutex<HashMap<session_id, …>>`,
//! poison-safe `unwrap_or_else(|e| e.into_inner())`, no `unwrap`.

use std::collections::HashMap;
use std::sync::LazyLock;

use aleph_desktop::{MouseButton, PressAction, ScreenCapability};
use tracing::{debug, warn};

use crate::sync_primitives::Mutex;

/// Ledger key for calls made outside a scoped turn (direct calls, tests).
/// They still drive the one physical keyboard, so they get a stable key of
/// their own rather than being dropped from the ledger entirely.
const NO_TURN_SESSION: &str = "__no_turn__";

/// A mouse button held down, with the point it was pressed at — a release must
/// be issued *somewhere*, and the press point is the only position the ledger
/// can know without querying the cursor.
///
/// `pid` records which rail the press rode: `Some(pid)` means it was delivered
/// to that process via the targeted rail (the user's real cursor never moved),
/// so its release must ride the same targeted rail — a global release would
/// leave the targeted process's mouse-down unmatched *and* teleport the user's
/// physical cursor to the recorded point. `None` means the global HID rail.
#[derive(Debug, Clone, Copy, PartialEq)]
struct HeldButton {
    button: MouseButton,
    x: f64,
    y: f64,
    pid: Option<i32>,
}

/// A key chord held down, and the rail its press rode.
///
/// `pid` carries the same obligation it does for [`HeldButton`]: a chord pressed
/// into one process must be released into that same process. A global release of
/// a targeted press leaves the target's key permanently down (nothing else will
/// ever send it an up) *and* fires a stray release at whatever the user has in
/// front of them.
#[derive(Debug, Clone, PartialEq)]
struct HeldKeys {
    keys: Vec<String>,
    pid: Option<i32>,
}

/// Everything one session currently holds down.
#[derive(Debug, Default)]
struct SessionHeld {
    /// Key chords, exactly as `key_button` received them.
    keys: Vec<HeldKeys>,
    buttons: Vec<HeldButton>,
}

impl SessionHeld {
    fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.buttons.is_empty()
    }
}

static HELD_INPUTS: LazyLock<Mutex<HashMap<String, SessionHeld>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Ledger key of the session issuing the current desktop call.
///
/// Resolved from the live turn — the same source the session lock uses
/// (`DesktopTool::acquire_lock`) — so a press recorded from the action arms and
/// the cleanup driven from `check_escape` land on the same key.
#[must_use]
pub fn current_session_id() -> String {
    crate::tools::turn_context::current_turn_context().map_or_else(
        || NO_TURN_SESSION.to_string(),
        |turn| turn.session_key.to_key_string(),
    )
}

/// Record that `keys` are now held down (a `PressAction::Press` on
/// `key_button`). `Click` presses and releases atomically and must not be
/// recorded. `pid` is `Some` when the press rode the targeted rail, so its
/// release rides the same one.
pub fn record_key_press(session_id: &str, keys: &[String], pid: Option<i32>) {
    if keys.is_empty() {
        return;
    }
    let mut map = HELD_INPUTS.lock().unwrap_or_else(|e| e.into_inner());
    let held = map.entry(session_id.to_string()).or_default();
    let chord = HeldKeys {
        keys: keys.to_vec(),
        pid,
    };
    // Same chord on the *other* rail is a different physical obligation, so it
    // is recorded separately rather than deduplicated away.
    if !held.keys.contains(&chord) {
        held.keys.push(chord);
    }
}

/// Drop `keys` from the ledger — the model released them itself.
///
/// Matches the recorded chord exactly. A partial release (`press ctrl+shift`
/// then `release ctrl`) leaves the chord recorded, so [`release_all`] re-issues
/// its release later; releasing an already-released key is a no-op at the OS
/// level, so over-releasing is harmless while under-releasing is not.
pub fn clear_key_release(session_id: &str, keys: &[String]) {
    let mut map = HELD_INPUTS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(held) = map.get_mut(session_id) else {
        return;
    };
    // Matched on the chord alone, not the rail: the model released these keys
    // and the tool just routed that release on whichever rail it chose. Keeping
    // a same-chord entry from the other rail would make the boundary re-release
    // keys the OS already let up.
    held.keys.retain(|chord| chord.keys.as_slice() != keys);
    if held.is_empty() {
        map.remove(session_id);
    }
}

/// Record that `button` is now held down at `(x, y)` (a `PressAction::Press` on
/// `mouse_button`). `pid` is `Some` when the press rode the targeted rail (so
/// its release must too). A second press of the same button replaces the first —
/// the physical button can only be down once, at one place.
pub fn record_button_press(
    session_id: &str,
    button: MouseButton,
    x: f64,
    y: f64,
    pid: Option<i32>,
) {
    let mut map = HELD_INPUTS.lock().unwrap_or_else(|e| e.into_inner());
    let held = map.entry(session_id.to_string()).or_default();
    held.buttons.retain(|b| b.button != button);
    held.buttons.push(HeldButton { button, x, y, pid });
}

/// Drop `button` from the ledger — the model released it itself.
pub fn clear_button_release(session_id: &str, button: MouseButton) {
    let mut map = HELD_INPUTS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(held) = map.get_mut(session_id) else {
        return;
    };
    held.buttons.retain(|b| b.button != button);
    if held.is_empty() {
        map.remove(session_id);
    }
}

/// Release everything `session_id` still holds and clear its ledger entry.
///
/// Returns how many releases succeeded. Best-effort by design: the entry is
/// dropped whether or not the platform release succeeds (a permanently failing
/// release would otherwise be retried on every subsequent call), and a failure
/// is logged rather than surfaced — a stuck key is bad, failing the caller's
/// tool call because cleanup failed is worse. Idempotent: a second call finds
/// nothing held and returns 0. Never panics.
pub async fn release_all(session_id: &str, screen: &dyn ScreenCapability) -> usize {
    // The guard is dropped before the first `.await`: `HELD_INPUTS` is a std
    // Mutex and must not be held across a suspension point.
    let held = {
        let mut map = HELD_INPUTS.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(session_id)
    };
    match held {
        Some(held) => release_held(&held, screen).await,
        None => 0,
    }
}

/// Release everything held by *every* session and clear the whole ledger.
///
/// The Escape abort is the user's emergency stop on the one physical desktop,
/// and the abort flag it sets is process-wide — so it must un-stick every held
/// key/button, not only those of whichever session happens to be calling. Same
/// best-effort, idempotent, never-panics contract as [`release_all`].
pub async fn release_all_sessions(screen: &dyn ScreenCapability) -> usize {
    let all: Vec<SessionHeld> = {
        let mut map = HELD_INPUTS.lock().unwrap_or_else(|e| e.into_inner());
        map.drain().map(|(_, held)| held).collect()
    };
    let mut released = 0;
    for held in &all {
        released += release_held(held, screen).await;
    }
    released
}

async fn release_held(held: &SessionHeld, screen: &dyn ScreenCapability) -> usize {
    let mut released = 0;
    for chord in &held.keys {
        // Release on the rail the press rode, for the same reason the buttons
        // below do: a targeted press is only matched by a targeted release.
        let result = match chord.pid {
            Some(pid) => {
                screen
                    .key_button_targeted(pid, &chord.keys, PressAction::Release)
                    .await
            }
            None => screen.key_button(&chord.keys, PressAction::Release).await,
        };
        match result {
            Ok(()) => released += 1,
            Err(e) => {
                warn!(error = %e, keys = ?chord.keys, pid = ?chord.pid, "held_inputs: failed to release held key(s)")
            }
        }
    }
    for b in &held.buttons {
        // Release on the same rail the press rode: targeted presses go back
        // through the targeted rail (matches the process's mouse-down and does
        // not move the user's cursor); global presses use the global HID tap.
        let result = match b.pid {
            Some(pid) => {
                screen
                    .mouse_button_targeted(pid, b.x, b.y, b.button, PressAction::Release)
                    .await
            }
            None => {
                screen
                    .mouse_button(b.x, b.y, b.button, PressAction::Release)
                    .await
            }
        };
        match result {
            Ok(()) => released += 1,
            Err(e) => {
                warn!(error = %e, button = ?b.button, pid = ?b.pid, "held_inputs: failed to release held mouse button")
            }
        }
    }
    if released > 0 {
        debug!(released, "held_inputs: released held desktop inputs");
    }
    released
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use aleph_desktop::{
        DesktopError, OcrResult, Result as DResult, ScreenRegion, Screenshot, WindowInfo,
    };
    use async_trait::async_trait;
    use std::future::Future;
    use std::sync::Mutex as StdMutex;

    /// The ledger is process-global, and `release_all_sessions` drains *every*
    /// session — so it would steal the entries a parallel test just recorded.
    /// Every test in this module takes this guard, and none of them holds it
    /// across an `.await` (each drives its own runtime with `block_on`).
    static LEDGER_GUARD: StdMutex<()> = StdMutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        LEDGER_GUARD.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn block_on<T>(fut: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime")
            .block_on(fut)
    }

    fn held_count(session_id: &str) -> usize {
        let map = HELD_INPUTS.lock().unwrap_or_else(|e| e.into_inner());
        map.get(session_id)
            .map_or(0, |held| held.keys.len() + held.buttons.len())
    }

    /// Records the releases the ledger issues; every other screen method is an
    /// unused stub (the ledger only ever calls `key_button` / `mouse_button`).
    #[derive(Default)]
    struct RecordingScreen {
        key_releases: StdMutex<Vec<Vec<String>>>,
        button_releases: StdMutex<Vec<(MouseButton, f64, f64)>>,
        /// Targeted releases, tagged with the pid they were routed to.
        targeted_button_releases: StdMutex<Vec<(i32, MouseButton, f64, f64)>>,
        targeted_key_releases: StdMutex<Vec<(i32, Vec<String>)>>,
        fail: bool,
    }

    #[async_trait]
    impl ScreenCapability for RecordingScreen {
        async fn screenshot(&self, _r: Option<ScreenRegion>) -> DResult<Screenshot> {
            Err(DesktopError::NotImplemented("screenshot".into()))
        }
        async fn ocr(&self, _i: Option<&[u8]>) -> DResult<OcrResult> {
            Err(DesktopError::NotImplemented("ocr".into()))
        }
        async fn click(&self, _x: f64, _y: f64, _b: MouseButton) -> DResult<()> {
            Ok(())
        }
        async fn type_text(&self, _t: &str) -> DResult<()> {
            Ok(())
        }
        async fn key_combo(&self, _m: &[String], _k: &str) -> DResult<()> {
            Ok(())
        }
        async fn scroll(&self, _d: &str, _a: i32) -> DResult<()> {
            Ok(())
        }
        async fn window_list(&self) -> DResult<Vec<WindowInfo>> {
            Ok(vec![])
        }
        async fn focus_window(&self, _id: u64) -> DResult<()> {
            Ok(())
        }
        async fn launch_app(&self, _n: &str) -> DResult<()> {
            Ok(())
        }
        async fn key_button(&self, keys: &[String], action: PressAction) -> DResult<()> {
            assert_eq!(action, PressAction::Release, "ledger only issues releases");
            if self.fail {
                return Err(DesktopError::InputFailed("boom".into()));
            }
            self.key_releases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(keys.to_vec());
            Ok(())
        }
        async fn mouse_button(
            &self,
            x: f64,
            y: f64,
            button: MouseButton,
            action: PressAction,
        ) -> DResult<()> {
            assert_eq!(action, PressAction::Release, "ledger only issues releases");
            if self.fail {
                return Err(DesktopError::InputFailed("boom".into()));
            }
            self.button_releases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((button, x, y));
            Ok(())
        }
        async fn key_button_targeted(
            &self,
            pid: i32,
            keys: &[String],
            action: PressAction,
        ) -> DResult<()> {
            assert_eq!(action, PressAction::Release, "ledger only issues releases");
            if self.fail {
                return Err(DesktopError::InputFailed("boom".into()));
            }
            self.targeted_key_releases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((pid, keys.to_vec()));
            Ok(())
        }
        async fn mouse_button_targeted(
            &self,
            pid: i32,
            x: f64,
            y: f64,
            button: MouseButton,
            action: PressAction,
        ) -> DResult<()> {
            assert_eq!(action, PressAction::Release, "ledger only issues releases");
            if self.fail {
                return Err(DesktopError::InputFailed("boom".into()));
            }
            self.targeted_button_releases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((pid, button, x, y));
            Ok(())
        }
    }

    fn keys(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn record_then_clear_leaves_nothing_held() {
        let _g = guard();
        let sid = "held-record-clear";
        record_key_press(sid, &keys(&["cmd"]), None);
        record_button_press(sid, MouseButton::Left, 10.0, 20.0, None);
        assert_eq!(held_count(sid), 2);

        clear_key_release(sid, &keys(&["cmd"]));
        clear_button_release(sid, MouseButton::Left);
        assert_eq!(held_count(sid), 0);
    }

    #[test]
    fn repeated_press_records_once() {
        let _g = guard();
        let sid = "held-dedupe";
        record_key_press(sid, &keys(&["ctrl", "shift"]), None);
        record_key_press(sid, &keys(&["ctrl", "shift"]), None);
        record_button_press(sid, MouseButton::Left, 1.0, 1.0, None);
        record_button_press(sid, MouseButton::Left, 9.0, 9.0, None);
        assert_eq!(held_count(sid), 2, "one chord + one button");

        // The physical button is down at the latest press point.
        let screen = RecordingScreen::default();
        block_on(release_all(sid, &screen));
        let buttons = screen
            .button_releases
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert_eq!(buttons.as_slice(), &[(MouseButton::Left, 9.0, 9.0)]);
    }

    #[test]
    fn empty_key_press_is_not_recorded() {
        let _g = guard();
        let sid = "held-empty-keys";
        record_key_press(sid, &[], None);
        assert_eq!(held_count(sid), 0);
    }

    #[test]
    fn release_all_releases_and_is_idempotent() {
        let _g = guard();
        let sid = "held-release-all";
        record_key_press(sid, &keys(&["cmd"]), None);
        record_button_press(sid, MouseButton::Left, 100.0, 200.0, None);

        let screen = RecordingScreen::default();
        assert_eq!(block_on(release_all(sid, &screen)), 2);
        assert_eq!(held_count(sid), 0, "ledger is cleared");
        assert_eq!(
            screen
                .key_releases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_slice(),
            &[keys(&["cmd"])]
        );
        assert_eq!(
            screen
                .button_releases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_slice(),
            &[(MouseButton::Left, 100.0, 200.0)]
        );

        // Second call: nothing held, nothing issued.
        assert_eq!(block_on(release_all(sid, &screen)), 0);
        assert_eq!(
            screen
                .key_releases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len(),
            1,
            "no redundant release issued"
        );
    }

    #[test]
    fn release_all_on_unknown_session_is_a_noop() {
        let _g = guard();
        let screen = RecordingScreen::default();
        assert_eq!(block_on(release_all("held-never-pressed", &screen)), 0);
    }

    #[test]
    fn targeted_press_is_released_on_the_targeted_rail() {
        let _g = guard();
        let sid = "held-targeted";
        // A button pressed into pid 4242 via the targeted rail…
        record_button_press(sid, MouseButton::Left, 12.0, 34.0, Some(4242));

        let screen = RecordingScreen::default();
        assert_eq!(block_on(release_all(sid, &screen)), 1);

        // …is released via mouse_button_targeted to the SAME pid, not the global
        // HID tap (which would strand the process's mouse-down and jump the
        // user's cursor).
        assert_eq!(
            screen
                .targeted_button_releases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_slice(),
            &[(4242, MouseButton::Left, 12.0, 34.0)]
        );
        assert!(
            screen
                .button_releases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "targeted press must not release on the global rail"
        );
    }

    /// The key rail's half of the same rule. A chord held inside one process is
    /// only let up inside that process: releasing it on the global tap would
    /// leave the target's key down forever *and* fire a stray release into
    /// whatever the user is typing in.
    #[test]
    fn a_targeted_key_press_is_released_on_the_targeted_rail() {
        let _g = guard();
        let sid = "held-targeted-keys";
        record_key_press(sid, &keys(&["cmd", "shift"]), Some(4242));

        let screen = RecordingScreen::default();
        assert_eq!(block_on(release_all(sid, &screen)), 1);

        assert_eq!(
            screen
                .targeted_key_releases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_slice(),
            &[(4242, keys(&["cmd", "shift"]))]
        );
        assert!(
            screen
                .key_releases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "targeted key press must not release on the global rail"
        );
    }

    /// The same chord held on both rails is two obligations, not one — the
    /// dedup must not collapse them, or one process keeps a key down forever.
    #[test]
    fn the_same_chord_on_two_rails_is_two_holds() {
        let _g = guard();
        let sid = "held-two-rails";
        record_key_press(sid, &keys(&["cmd"]), Some(7));
        record_key_press(sid, &keys(&["cmd"]), None);
        assert_eq!(held_count(sid), 2);

        let screen = RecordingScreen::default();
        assert_eq!(block_on(release_all(sid, &screen)), 2);
        assert_eq!(
            screen
                .targeted_key_releases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len(),
            1
        );
        assert_eq!(
            screen
                .key_releases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len(),
            1
        );
    }

    #[test]
    fn failed_release_is_swallowed_and_still_clears() {
        let _g = guard();
        let sid = "held-release-fails";
        record_key_press(sid, &keys(&["cmd"]), None);
        let screen = RecordingScreen {
            fail: true,
            ..Default::default()
        };
        assert_eq!(block_on(release_all(sid, &screen)), 0, "release failed");
        assert_eq!(
            held_count(sid),
            0,
            "entry is dropped anyway — a failing release must not be retried forever"
        );
    }

    #[test]
    fn release_all_sessions_drains_every_session() {
        let _g = guard();
        record_key_press("held-multi-a", &keys(&["cmd"]), None);
        record_button_press("held-multi-b", MouseButton::Right, 5.0, 6.0, None);

        let screen = RecordingScreen::default();
        assert_eq!(block_on(release_all_sessions(&screen)), 2);
        assert_eq!(held_count("held-multi-a"), 0);
        assert_eq!(held_count("held-multi-b"), 0);
    }
}
