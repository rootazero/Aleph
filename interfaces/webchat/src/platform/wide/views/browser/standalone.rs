//! Standalone browser-viewer page (`/viewer/browser`).
//!
//! Mounted by `App()` *before* `DashboardContext`, so it needs no Panel
//! token, no `TokenWall` and no `browser.live.*` bus subscription: the launch
//! fragment is the only credential and the exchange snapshot (see
//! [`super::session`]) the only binding authority.
//!
//! A valid link starts the real transport ([`super::transport`]): consume,
//! exchange, then the one viewer WebSocket. The page shows one fixed localized
//! status line (connecting / failed / closed / invalid link) and, once the
//! session has accepted a frame, that frame as an `<img>`. It never shows a
//! static snapshot as if it were live and never echoes the fragment, a secret
//! or any server body: an absent or invalid fragment renders one fixed
//! localized sentence. Read-only: no input is wired.

use super::session::{
    is_standalone_viewer_path, CloseReason, SessionPhase, ViewerSession, VIEWER_PAGE_PATH,
};
use super::transport::{self, TransportUi};
use crate::i18n::{t_string, use_i18n, I18nCtx};
use leptos::prelude::*;

/// The parts of `window.location` the standalone route needs.
///
/// `hash` is the one-shot launch credential: `Debug` never prints it.
#[derive(Clone, PartialEq, Eq)]
pub struct LocationParts {
    pub pathname: String,
    pub origin: String,
    pub hash: String,
}

impl std::fmt::Debug for LocationParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocationParts")
            .field("pathname", &self.pathname)
            .field("origin", &self.origin)
            .field("hash", &"[REDACTED]")
            .finish()
    }
}

#[cfg(target_arch = "wasm32")]
fn read_location() -> Option<LocationParts> {
    let location = web_sys::window()?.location();
    Some(LocationParts {
        pathname: location.pathname().ok()?,
        origin: location.origin().ok()?,
        hash: location.hash().ok()?,
    })
}

/// Native builds have no browser location; the standalone route never matches.
#[cfg(not(target_arch = "wasm32"))]
fn read_location() -> Option<LocationParts> {
    None
}

/// Replace the CURRENT history entry's URL with `path` (no new entry, no
/// navigation, no reload). The path is always the fixed same-origin viewer
/// path, so nothing from the old URL is carried over. The error carries no
/// detail on purpose: a browser error could echo the URL.
#[cfg(target_arch = "wasm32")]
fn replace_current_entry(path: &str) -> Result<(), ()> {
    let history = web_sys::window().ok_or(())?.history().map_err(|_| ())?;
    history
        .replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(path))
        .map_err(|_| ())
}

/// Native builds have no history; `read_location` is `None` there, so this is
/// never reached. Fail closed anyway.
#[cfg(not(target_arch = "wasm32"))]
fn replace_current_entry(_path: &str) -> Result<(), ()> {
    Err(())
}

/// Hand-off scrub for the standalone route.
///
/// The credential lives in `location.hash`, so it must leave the address bar
/// (and the session history entry) before anything else can read it. The
/// caller has already copied `location` into memory; this function:
///
/// 1. refuses any page that is not exactly the standalone viewer path (so a
///    different same-origin page is never rewritten);
/// 2. replaces the current entry with the fixed `VIEWER_PAGE_PATH` (no
///    fragment, no query) via `scrub`, ALWAYS - an invalid or malformed
///    fragment is cleared too;
/// 3. only then parses the in-memory copy into a session.
///
/// If `scrub` fails the session is never built, so no consume/exchange/auth
/// request can start while the credential is still in the address bar.
fn begin_standalone_session(
    location: LocationParts,
    scrub: impl FnOnce(&str) -> Result<(), ()>,
) -> Option<(ViewerSession, String)> {
    if !is_standalone_viewer_path(&location.pathname) {
        return None;
    }
    scrub(VIEWER_PAGE_PATH).ok()?;
    let LocationParts { origin, hash, .. } = location;
    ViewerSession::from_location(&origin, &hash)
        .ok()
        .map(|session| (session, origin))
}

/// True only when the page path is exactly `/viewer/browser`.
#[must_use]
pub fn is_standalone_viewer_route() -> bool {
    read_location().is_some_and(|location| is_standalone_viewer_path(&location.pathname))
}

/// The one line the standalone page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandaloneNotice {
    /// Absent or invalid launch link. Fixed text; nothing from the URL.
    InvalidLink,
    /// A valid link, but live streaming is not available in this build.
    Unavailable,
    /// The transport is consuming / exchanging / opening the socket.
    Connecting,
    /// Authenticated: frames are shown as they arrive.
    Live,
    /// A transport-level failure (network, rejected auth, auth timeout, socket
    /// closed). Fixed text; no detail.
    Failed,
    /// The viewer session ended.
    Closed,
}

/// Map a session phase to its notice.
#[must_use]
pub fn notice_for_phase(phase: SessionPhase) -> StandaloneNotice {
    match phase {
        SessionPhase::Idle
        | SessionPhase::Consuming
        | SessionPhase::Exchanging
        | SessionPhase::Exchanged => StandaloneNotice::Connecting,
        SessionPhase::Authenticated => StandaloneNotice::Live,
        SessionPhase::Closed(CloseReason::InvalidResponse) => StandaloneNotice::InvalidLink,
        SessionPhase::Closed(
            CloseReason::AuthRejected | CloseReason::AuthTimeout | CloseReason::TransportClosed,
        ) => StandaloneNotice::Failed,
        SessionPhase::Closed(CloseReason::Local) => StandaloneNotice::Closed,
    }
}

/// Phase notice, except that a transport failure signal always wins.
#[must_use]
pub fn notice_for(phase: SessionPhase, transport_failed: bool) -> StandaloneNotice {
    if transport_failed {
        StandaloneNotice::Failed
    } else {
        notice_for_phase(phase)
    }
}

/// Notice once the session's history is known: a session that already went
/// live and then loses its transport (Gateway revoke, navigation fence, target
/// gone) has *ended*; reporting "could not connect" there would be false.
#[must_use]
pub fn notice_with_history(
    phase: SessionPhase,
    transport_failed: bool,
    was_live: bool,
) -> StandaloneNotice {
    match notice_for(phase, transport_failed) {
        StandaloneNotice::Failed if was_live => StandaloneNotice::Closed,
        notice => notice,
    }
}

fn notice_text(i18n: I18nCtx, notice: StandaloneNotice) -> String {
    match notice {
        StandaloneNotice::InvalidLink => {
            t_string!(i18n, browser_viewer.standalone_invalid).to_string()
        }
        StandaloneNotice::Unavailable => {
            t_string!(i18n, browser_viewer.standalone_unavailable).to_string()
        }
        // `Live` without a frame yet is still "connecting": nothing is shown
        // as live until the first accepted frame arrives.
        StandaloneNotice::Connecting | StandaloneNotice::Live => {
            t_string!(i18n, browser_viewer.standalone_connecting).to_string()
        }
        StandaloneNotice::Failed => t_string!(i18n, browser_viewer.standalone_failed).to_string(),
        StandaloneNotice::Closed => t_string!(i18n, browser_viewer.standalone_closed).to_string(),
    }
}

/// Route entry: builds a session from the page location, or shows the fixed
/// invalid-link notice when the origin or fragment is unusable.
#[component]
pub fn StandaloneBrowserViewerRoute() -> impl IntoView {
    // Capture the location into memory FIRST, scrub the address bar, and only
    // then build the session from the captured copy (see
    // `begin_standalone_session`). Reading the hash after the scrub would see
    // an empty fragment.
    let parsed = read_location()
        .and_then(|location| begin_standalone_session(location, replace_current_entry));
    match parsed {
        Some((session, origin)) => {
            view! { <StandaloneBrowserViewer session=session origin=origin /> }.into_any()
        }
        None => view! {
            <StandaloneShell
                notice=Signal::derive(|| StandaloneNotice::InvalidLink)
                frame_url=Signal::derive(|| None)
            />
        }
        .into_any(),
    }
}

/// The standalone viewer for a valid session. Independent of every Panel
/// context. `origin` is the page origin the transport derives its endpoints
/// from; the transport's lifetime is bound to this component.
#[component]
pub fn StandaloneBrowserViewer(session: ViewerSession, origin: String) -> impl IntoView {
    // The transport writes to this signal (`close` / `accept_*`) and the
    // notice follows the phase; its failure flag overrides the phase notice.
    let session = RwSignal::new(session);
    let ui = TransportUi::new();
    transport::start(session, ui, origin);
    // Sticky: true once the session has authenticated at least once.
    let was_live = Memo::new(move |prev: Option<&bool>| {
        prev.copied().unwrap_or(false) || session.with(|s| s.phase()) == SessionPhase::Authenticated
    });
    let notice = Signal::derive(move || {
        notice_with_history(session.with(|s| s.phase()), ui.failed.get(), was_live.get())
    });
    let frame_url = Signal::derive(move || ui.frame_url.get());
    view! { <StandaloneShell notice=notice frame_url=frame_url /> }
}

#[component]
fn StandaloneShell(
    notice: Signal<StandaloneNotice>,
    frame_url: Signal<Option<String>>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <section
            data-testid="browser-live-viewer"
            class="flex flex-col gap-3 p-4 min-h-screen"
            aria-label=t_string!(i18n, browser_viewer.region_label).to_string()
        >
            <h2 class="text-base font-semibold text-text-primary">
                {t_string!(i18n, browser_viewer.title)}
            </h2>
            {move || {
                // A frame is only ever shown while the session is live.
                match (notice.get(), frame_url.get()) {
                    (StandaloneNotice::Live, Some(url)) => view! {
                        <img
                            data-testid="browser-live-frame"
                            src=url
                            alt=t_string!(i18n, browser_viewer.title).to_string()
                            class="max-w-full h-auto"
                        />
                    }
                    .into_any(),
                    (notice, _) => view! {
                        <p data-testid="browser-live-status" class="text-sm text-text-secondary" role="status">
                            {notice_text(i18n, notice)}
                        </p>
                    }
                    .into_any(),
                }
            }}
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_map_to_fixed_notices() {
        for phase in [
            SessionPhase::Idle,
            SessionPhase::Consuming,
            SessionPhase::Exchanging,
            SessionPhase::Exchanged,
        ] {
            assert_eq!(notice_for_phase(phase), StandaloneNotice::Connecting);
        }
        assert_eq!(
            notice_for_phase(SessionPhase::Authenticated),
            StandaloneNotice::Live
        );
        assert_eq!(
            notice_for_phase(SessionPhase::Closed(CloseReason::InvalidResponse)),
            StandaloneNotice::InvalidLink
        );
        for reason in [
            CloseReason::AuthRejected,
            CloseReason::AuthTimeout,
            CloseReason::TransportClosed,
        ] {
            assert_eq!(
                notice_for_phase(SessionPhase::Closed(reason)),
                StandaloneNotice::Failed
            );
        }
        assert_eq!(
            notice_for_phase(SessionPhase::Closed(CloseReason::Local)),
            StandaloneNotice::Closed
        );
    }

    #[test]
    fn transport_failure_signal_wins_over_phase() {
        assert_eq!(
            notice_for(SessionPhase::Authenticated, true),
            StandaloneNotice::Failed
        );
        assert_eq!(
            notice_for(SessionPhase::Closed(CloseReason::Local), true),
            StandaloneNotice::Failed
        );
        assert_eq!(
            notice_for(SessionPhase::Idle, false),
            StandaloneNotice::Connecting
        );
    }

    #[test]
    fn a_session_that_was_live_reports_closed_not_failed() {
        for (phase, failed) in [
            (SessionPhase::Closed(CloseReason::TransportClosed), false),
            (SessionPhase::Authenticated, true),
            (SessionPhase::Closed(CloseReason::Local), true),
        ] {
            assert_eq!(
                notice_with_history(phase, failed, true),
                StandaloneNotice::Closed
            );
            assert_eq!(
                notice_with_history(phase, failed, false),
                StandaloneNotice::Failed
            );
        }
        // History never upgrades a link problem or a live stream.
        assert_eq!(
            notice_with_history(
                SessionPhase::Closed(CloseReason::InvalidResponse),
                false,
                true
            ),
            StandaloneNotice::InvalidLink
        );
        assert_eq!(
            notice_with_history(SessionPhase::Authenticated, false, true),
            StandaloneNotice::Live
        );
    }

    const ORIGIN: &str = "https://panel.example.com";
    const VALID_HASH: &str = "#grant_id=grant-abc_123&nonce=nonce-SECRET_value-0123456789abcdef";

    fn parts(pathname: &str, hash: &str) -> LocationParts {
        LocationParts {
            pathname: pathname.to_string(),
            origin: ORIGIN.to_string(),
            hash: hash.to_string(),
        }
    }

    #[test]
    fn valid_fragment_is_scrubbed_then_still_consumed_from_the_memory_copy() {
        let mut scrubbed = Vec::new();
        let started = begin_standalone_session(parts("/viewer/browser", VALID_HASH), |path| {
            scrubbed.push(path.to_string());
            Ok(())
        });
        // The address bar is rewritten to the fixed path: no fragment, no query.
        assert_eq!(scrubbed, vec!["/viewer/browser".to_string()]);
        let (mut session, origin) = started.expect("valid link still starts a session");
        assert_eq!(origin, ORIGIN);
        // The in-memory copy still carries the handoff credential for consume.
        let request = session.begin_consume().expect("consume request is built");
        assert_eq!(request.grant_id, "grant-abc_123");
        assert_eq!(session.phase(), SessionPhase::Consuming);
    }

    #[test]
    fn scrub_runs_before_the_hash_is_parsed() {
        // A scrub that observes the order: the session does not exist yet when
        // the address bar is rewritten, and the rewrite target never contains
        // the credential.
        let mut seen = None;
        let started = begin_standalone_session(parts("/viewer/browser", VALID_HASH), |path| {
            seen = Some(path.to_string());
            Ok(())
        });
        assert!(started.is_some());
        let target = seen.expect("scrub ran");
        assert!(!target.contains('#') && !target.contains('?'));
        assert!(!target.contains("SECRET") && !target.contains("grant-abc"));
        assert!(target.starts_with('/') && !target.starts_with("//"));
    }

    #[test]
    fn malformed_or_empty_fragment_is_scrubbed_and_then_invalid() {
        for hash in [
            "",
            "#",
            "#not-a-valid-handoff",
            "#grant_id=a",
            "#grant_id=a&nonce=b&extra=c",
            "#grant_id=a&nonce=b%20c",
        ] {
            let mut scrubbed = 0;
            let started = begin_standalone_session(parts("/viewer/browser", hash), |path| {
                assert_eq!(path, "/viewer/browser");
                scrubbed += 1;
                Ok(())
            });
            assert_eq!(scrubbed, 1, "{hash:?} must still be cleared");
            assert!(started.is_none(), "{hash:?} must render InvalidLink");
        }
    }

    #[test]
    fn insecure_remote_origin_is_scrubbed_and_then_invalid() {
        let mut scrubbed = 0;
        let location = LocationParts {
            pathname: "/viewer/browser".to_string(),
            origin: "http://panel.example.com".to_string(),
            hash: VALID_HASH.to_string(),
        };
        let started = begin_standalone_session(location, |_| {
            scrubbed += 1;
            Ok(())
        });
        assert_eq!(scrubbed, 1);
        assert!(started.is_none());
    }

    #[test]
    fn history_error_fails_closed_without_starting_a_session() {
        // If the address bar cannot be cleared the credential must not be
        // consumed: no session means no consume / exchange / auth request.
        let started = begin_standalone_session(parts("/viewer/browser", VALID_HASH), |_| Err(()));
        assert!(started.is_none());
        let started = begin_standalone_session(parts("/viewer/browser", ""), |_| Err(()));
        assert!(started.is_none());
    }

    #[test]
    fn other_same_origin_pages_are_never_rewritten() {
        for pathname in [
            "/",
            "/viewer/browser/",
            "/viewer/browser/ws",
            "/viewer",
            "/Viewer/browser",
            "/settings",
        ] {
            let started = begin_standalone_session(parts(pathname, VALID_HASH), |_| {
                panic!("{pathname} is not the standalone route and must not be scrubbed");
            });
            assert!(started.is_none());
        }
    }

    #[test]
    fn location_parts_debug_redacts_the_fragment() {
        let rendered = format!("{:?}", parts("/viewer/browser", VALID_HASH));
        assert!(!rendered.contains("SECRET") && !rendered.contains("grant-abc"));
        assert!(rendered.contains("REDACTED"));
    }

    #[test]
    fn native_builds_never_match_the_standalone_route() {
        #[cfg(not(target_arch = "wasm32"))]
        assert!(!is_standalone_viewer_route());
    }
}
