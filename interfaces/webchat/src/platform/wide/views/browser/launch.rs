//! Pure rules for the "open viewer" launch workflow.
//!
//! The Panel never builds a viewer URL. It asks the Gateway for a `[Frames]`
//! grant on the exact Safe resource it shows, validates that the returned
//! `launch_url` is the standalone route carrying a well-formed grant fragment,
//! and hands that string — unchanged — to the browser's new-tab launcher.
//!
//! Everything here is I/O-free so the eligibility, URL vetting and state
//! transitions are unit-tested natively; `viewer.rs` only wires them to the
//! authenticated RPC and `window.open`.
//!
//! The launch URL is a secret (its fragment holds grant id + nonce): it lives
//! only in [`LaunchPhase::Ready`] / [`LaunchPhase::Blocked`], is dropped when
//! opened, dismissed or the view is rebuilt, and no `Debug` shows it.

use super::model::{BoundBrowser, BrowserViewerModel, UnboundReason, ViewerPhase};
use super::session::{parse_fragment, ViewerEndpoints, VIEWER_PAGE_PATH};
use aleph_protocol::browser_live::{IssueViewerGrantResponse, RedactionStatus};
use std::fmt;

/// The exact resource revision a launch is requested for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchTarget {
    pub resource_id: String,
    pub revision: u64,
}

/// Why no launch may be requested right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchBlock {
    /// Nothing bound.
    NoResource,
    /// The viewer reported closed (target closed, revoked, expired, …).
    Closed,
    /// The resource's absolute expiry has passed.
    Expired,
    /// Only an affirmative `Safe` verdict may be launched: Pending, Refused
    /// and merely `Redacted` are all refused (the Gateway agrees).
    NotSafe,
}

/// Derive the launch target from a bound resource, or say why not.
pub fn launch_target(bound: &BoundBrowser, now_ms: i64) -> Result<LaunchTarget, LaunchBlock> {
    let resource = bound.resource();
    if matches!(bound.phase(), ViewerPhase::Closed(_)) {
        return Err(LaunchBlock::Closed);
    }
    if resource.redaction != RedactionStatus::Safe {
        return Err(LaunchBlock::NotSafe);
    }
    if now_ms >= resource.expires_at_ms {
        return Err(LaunchBlock::Expired);
    }
    Ok(LaunchTarget {
        resource_id: resource.resource_id.clone(),
        revision: resource.revision,
    })
}

/// Same derivation over the workspace model. Used both to enable the control
/// and to re-check, at click time and when the Gateway answers, that the
/// target is still the one the workspace holds.
pub fn model_launch_target(
    model: &BrowserViewerModel,
    now_ms: i64,
) -> Result<LaunchTarget, LaunchBlock> {
    match model {
        BrowserViewerModel::Bound(bound) => launch_target(bound, now_ms),
        BrowserViewerModel::Unbound(UnboundReason::Closed(_)) => Err(LaunchBlock::Closed),
        BrowserViewerModel::Unbound(_) => Err(LaunchBlock::NoResource),
    }
}

/// Why a `launch_url` was refused. Carries no input text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchUrlError {
    NoFragment,
    BadOrigin,
    BadPath,
    BadFragment,
    GrantMismatch,
}

/// Check — without altering it — that `launch_url` is
/// `<origin>/viewer/browser#grant_id=…&nonce=…` for the grant just issued.
///
/// The origin must be one the standalone viewer would itself accept (https, or
/// loopback http); the path must be exactly the standalone route with no
/// query; the fragment must parse and name `expected_grant_id`.
pub fn validate_launch_url(
    launch_url: &str,
    expected_grant_id: &str,
) -> Result<(), LaunchUrlError> {
    let (head, fragment) = launch_url
        .split_once('#')
        .ok_or(LaunchUrlError::NoFragment)?;
    let scheme_end = head.find("://").ok_or(LaunchUrlError::BadOrigin)? + 3;
    let path_start = head[scheme_end..]
        .find('/')
        .map(|i| scheme_end + i)
        .ok_or(LaunchUrlError::BadPath)?;
    let (origin, path) = head.split_at(path_start);
    ViewerEndpoints::from_origin(origin).map_err(|_| LaunchUrlError::BadOrigin)?;
    if path != VIEWER_PAGE_PATH {
        return Err(LaunchUrlError::BadPath);
    }
    let parsed = parse_fragment(fragment).map_err(|_| LaunchUrlError::BadFragment)?;
    if parsed.grant_id() != expected_grant_id {
        return Err(LaunchUrlError::GrantMismatch);
    }
    Ok(())
}

/// A grant issued for this view and not yet opened.
#[derive(Clone, PartialEq, Eq)]
pub struct IssuedLaunch {
    launch_url: String,
    grant_id: String,
    expires_at_ms: i64,
}

impl IssuedLaunch {
    #[must_use]
    pub fn grant_id(&self) -> &str {
        &self.grant_id
    }
}

impl fmt::Debug for IssuedLaunch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedLaunch")
            .field("launch_url", &"[REDACTED]")
            .field("grant_id", &self.grant_id)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

/// Single launch state: one value, so "issuing" and "ready" can never both
/// hold and one check guards re-entry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LaunchPhase {
    #[default]
    Idle,
    /// `grant.issue` sent; waiting for the Gateway.
    Issuing,
    /// A validated link is held; the user's next click opens it.
    Ready(IssuedLaunch),
    /// The browser refused to open the tab; the same link may be retried.
    Blocked(IssuedLaunch),
    /// The tab was opened. The one-shot link is dropped.
    Opened,
    /// Denied, invalid, expired or transport failure. Retry allowed.
    Failed,
}

/// Enter `Issuing` unless a request is in flight or a link is already held.
pub fn begin_issue(phase: &mut LaunchPhase) -> bool {
    match phase {
        LaunchPhase::Idle | LaunchPhase::Opened | LaunchPhase::Failed => {
            *phase = LaunchPhase::Issuing;
            true
        }
        LaunchPhase::Issuing | LaunchPhase::Ready(_) | LaunchPhase::Blocked(_) => false,
    }
}

/// Apply the Gateway's answer. Returns a grant id the caller must revoke
/// (best effort) because it was issued but cannot be used: the view moved on
/// (`still_current` false), the phase is no longer `Issuing`, or the response
/// failed validation. The link is kept only if every check passes.
#[must_use]
pub fn finish_issue(
    phase: &mut LaunchPhase,
    still_current: bool,
    result: Result<IssueViewerGrantResponse, String>,
    now_ms: i64,
) -> Option<String> {
    let issuing = *phase == LaunchPhase::Issuing;
    match result {
        Err(_) => {
            if issuing {
                *phase = LaunchPhase::Failed;
            }
            None
        }
        Ok(response) => {
            let usable = issuing
                && still_current
                && response.expires_at_ms > now_ms
                && validate_launch_url(&response.launch_url, &response.grant_id).is_ok();
            if usable {
                *phase = LaunchPhase::Ready(IssuedLaunch {
                    launch_url: response.launch_url,
                    grant_id: response.grant_id,
                    expires_at_ms: response.expires_at_ms,
                });
                None
            } else {
                if issuing {
                    *phase = LaunchPhase::Failed;
                }
                Some(response.grant_id)
            }
        }
    }
}

/// Open a held link through `opener` (the browser's new-tab launcher). The
/// opener sees the issuer's string exactly as returned. `true` → `Opened`
/// (link dropped); `false` → `Blocked` (link kept). An expired link is dropped
/// into `Failed` without calling the opener. Returns whether the opener ran
/// and succeeded.
pub fn open_ready(phase: &mut LaunchPhase, now_ms: i64, opener: impl FnOnce(&str) -> bool) -> bool {
    let issued = match std::mem::take(phase) {
        LaunchPhase::Ready(issued) | LaunchPhase::Blocked(issued) => issued,
        other => {
            *phase = other;
            return false;
        }
    };
    if issued.expires_at_ms <= now_ms {
        *phase = LaunchPhase::Failed;
        return false;
    }
    if opener(&issued.launch_url) {
        *phase = LaunchPhase::Opened;
        true
    } else {
        *phase = LaunchPhase::Blocked(issued);
        false
    }
}

/// Drop an unopened link (dismiss, or the view is being torn down) and return
/// its grant id so the caller can revoke it. `None` when nothing unopened.
#[must_use]
pub fn take_unopened(phase: &mut LaunchPhase) -> Option<String> {
    match std::mem::take(phase) {
        LaunchPhase::Ready(issued) | LaunchPhase::Blocked(issued) => Some(issued.grant_id),
        other => {
            *phase = other;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::browser_live::{
        BrowserTargetBinding, ClosedReason, ConversationBinding, DisplayPolicy, IssuerIdentity,
        PreviewKind, PreviewProvenance, PreviewResource, PreviewSource, ResourceBinding,
        ViewerBinding,
    };

    const NOW: i64 = 1_000;

    fn resource(redaction: RedactionStatus, revision: u64) -> PreviewResource {
        PreviewResource {
            resource_id: "resource-5".to_string(),
            revision,
            binding: ResourceBinding::Browser {
                viewer: ViewerBinding {
                    principal: IssuerIdentity::User {
                        user_id: "user-42".to_string(),
                    },
                    task: ConversationBinding {
                        session_key: "agent:operator:browser".to_string(),
                        run_id: "run-7".to_string(),
                    },
                    target: BrowserTargetBinding {
                        profile: "default".to_string(),
                        browser_session_id: "browser-session-3".to_string(),
                        target_id: "target-9".to_string(),
                    },
                },
            },
            kind: PreviewKind::Page,
            display: DisplayPolicy::AutoRevealCurrentRun,
            source: PreviewSource::BrowserTool,
            provenance: PreviewProvenance::NotManaged,
            expires_at_ms: 2_000,
            redaction,
            companion_candidates: Vec::new(),
        }
    }

    fn model(redaction: RedactionStatus, revision: u64) -> BrowserViewerModel {
        BrowserViewerModel::from_resource(resource(redaction, revision)).unwrap()
    }

    const URL: &str = "https://gw.example/viewer/browser#grant_id=g-1&nonce=n_2";

    fn issued(url: &str, grant: &str, expires: i64) -> IssueViewerGrantResponse {
        IssueViewerGrantResponse {
            launch_url: url.to_string(),
            grant_id: grant.to_string(),
            expires_at_ms: expires,
        }
    }

    fn issuing() -> LaunchPhase {
        let mut p = LaunchPhase::Idle;
        assert!(begin_issue(&mut p));
        p
    }

    fn ready() -> LaunchPhase {
        let mut p = issuing();
        assert_eq!(
            finish_issue(&mut p, true, Ok(issued(URL, "g-1", 5_000)), NOW),
            None
        );
        p
    }

    #[test]
    fn only_a_safe_live_unexpired_resource_is_launchable_for_its_exact_revision() {
        assert_eq!(
            model_launch_target(&model(RedactionStatus::Safe, 3), NOW),
            Ok(LaunchTarget {
                resource_id: "resource-5".into(),
                revision: 3
            })
        );
        assert_eq!(
            model_launch_target(&model(RedactionStatus::Pending, 1), NOW),
            Err(LaunchBlock::NotSafe)
        );
        assert_eq!(
            model_launch_target(&model(RedactionStatus::Redacted, 1), NOW),
            Err(LaunchBlock::NotSafe)
        );
        // Refused never binds: the same answer as nothing bound.
        assert_eq!(
            model_launch_target(&model(RedactionStatus::Refused, 1), NOW),
            Err(LaunchBlock::NoResource)
        );
        assert_eq!(
            model_launch_target(&BrowserViewerModel::default(), NOW),
            Err(LaunchBlock::NoResource)
        );
        // Expiry is absolute and exclusive of the boundary.
        assert_eq!(
            model_launch_target(&model(RedactionStatus::Safe, 1), 2_000),
            Err(LaunchBlock::Expired)
        );
    }

    #[test]
    fn a_closed_viewer_is_not_launchable_and_a_new_revision_changes_the_target() {
        let mut m = model(RedactionStatus::Safe, 1);
        let before = model_launch_target(&m, NOW).unwrap();
        m.bind_resource(resource(RedactionStatus::Safe, 2)).unwrap();
        let after = model_launch_target(&m, NOW).unwrap();
        assert_ne!(
            before, after,
            "stale revision must not equal the new target"
        );
        assert_eq!(after.revision, 2);

        m.apply_closed(&aleph_protocol::browser_live::BrowserLiveClosed {
            resource_id: "resource-5".into(),
            binding: ResourceBinding::Browser {
                viewer: ViewerBinding {
                    principal: IssuerIdentity::User {
                        user_id: "user-42".into(),
                    },
                    task: ConversationBinding {
                        session_key: "agent:operator:browser".into(),
                        run_id: "run-7".into(),
                    },
                    target: BrowserTargetBinding {
                        profile: "default".into(),
                        browser_session_id: "browser-session-3".into(),
                        target_id: "target-9".into(),
                    },
                },
            },
            reason: ClosedReason::Revoked,
            at_ms: 2_000,
        })
        .unwrap();
        assert_eq!(model_launch_target(&m, NOW), Err(LaunchBlock::Closed));
    }

    #[test]
    fn launch_url_must_be_the_standalone_route_for_the_issued_grant() {
        assert_eq!(validate_launch_url(URL, "g-1"), Ok(()));
        assert_eq!(
            validate_launch_url(
                "http://127.0.0.1:8080/viewer/browser#nonce=n&grant_id=g-1",
                "g-1"
            ),
            Ok(())
        );
        for (url, want) in [
            (
                "https://gw.example/viewer/browser",
                LaunchUrlError::NoFragment,
            ),
            (
                "http://gw.example/viewer/browser#grant_id=g-1&nonce=n",
                LaunchUrlError::BadOrigin,
            ),
            (
                "javascript:alert(1)#grant_id=g-1&nonce=n",
                LaunchUrlError::BadOrigin,
            ),
            (
                "https://user@gw.example/viewer/browser#grant_id=g-1&nonce=n",
                LaunchUrlError::BadOrigin,
            ),
            (
                "https://gw.example/other#grant_id=g-1&nonce=n",
                LaunchUrlError::BadPath,
            ),
            (
                "https://gw.example/viewer/browser?x=1#grant_id=g-1&nonce=n",
                LaunchUrlError::BadPath,
            ),
            (
                "https://gw.example#grant_id=g-1&nonce=n",
                LaunchUrlError::BadPath,
            ),
            (
                "https://gw.example/viewer/browser#grant_id=g-1",
                LaunchUrlError::BadFragment,
            ),
            (
                "https://gw.example/viewer/browser#grant_id=g-1&nonce=n&x=1",
                LaunchUrlError::BadFragment,
            ),
            (
                "https://gw.example/viewer/browser#grant_id=other&nonce=n",
                LaunchUrlError::GrantMismatch,
            ),
        ] {
            assert_eq!(validate_launch_url(url, "g-1"), Err(want), "{url}");
        }
    }

    #[test]
    fn issue_is_single_flight_and_a_held_link_blocks_a_second_issue() {
        let mut p = LaunchPhase::Idle;
        assert!(begin_issue(&mut p));
        assert!(!begin_issue(&mut p));
        let mut p = ready();
        assert!(!begin_issue(&mut p));
        let mut p = LaunchPhase::Failed;
        assert!(begin_issue(&mut p));
        let mut p = LaunchPhase::Opened;
        assert!(begin_issue(&mut p));
    }

    #[test]
    fn a_valid_response_is_held_unchanged_and_never_debug_printed() {
        let p = ready();
        let LaunchPhase::Ready(held) = &p else {
            panic!("not ready")
        };
        assert_eq!(held.launch_url, URL);
        assert!(!format!("{p:?}").contains("n_2"));
    }

    #[test]
    fn unusable_responses_fail_and_ask_for_revocation() {
        // Stale view: grant is issued but must be revoked, not held.
        let mut p = issuing();
        assert_eq!(
            finish_issue(&mut p, false, Ok(issued(URL, "g-1", 5_000)), NOW),
            Some("g-1".into())
        );
        assert_eq!(p, LaunchPhase::Failed);
        // Foreign / malformed URL, wrong grant, already expired.
        for resp in [
            issued("https://evil.example/x#grant_id=g-1&nonce=n", "g-1", 5_000),
            issued(URL, "g-2", 5_000),
            issued(URL, "g-1", NOW),
        ] {
            let mut p = issuing();
            let gid = resp.grant_id.clone();
            assert_eq!(finish_issue(&mut p, true, Ok(resp), NOW), Some(gid));
            assert_eq!(p, LaunchPhase::Failed);
        }
        // Gateway denial: nothing to revoke.
        let mut p = issuing();
        assert_eq!(finish_issue(&mut p, true, Err("denied".into()), NOW), None);
        assert_eq!(p, LaunchPhase::Failed);
    }

    #[test]
    fn a_late_response_after_the_phase_moved_on_is_revoked_not_adopted() {
        // e.g. view was dismissed/reset to Idle while the call was in flight.
        let mut p = LaunchPhase::Idle;
        assert_eq!(
            finish_issue(&mut p, true, Ok(issued(URL, "g-1", 5_000)), NOW),
            Some("g-1".into())
        );
        assert_eq!(p, LaunchPhase::Idle);
        assert_eq!(finish_issue(&mut p, true, Err("x".into()), NOW), None);
        assert_eq!(p, LaunchPhase::Idle);
    }

    #[test]
    fn open_passes_the_issuer_string_verbatim_then_drops_it() {
        let mut p = ready();
        let mut seen = String::new();
        assert!(open_ready(&mut p, NOW, |u| {
            seen = u.to_string();
            true
        }));
        assert_eq!(seen, URL);
        assert_eq!(p, LaunchPhase::Opened);
        // Single-use: a second open has no link and never calls the opener.
        assert!(!open_ready(&mut p, NOW, |_| panic!("must not open")));
        assert_eq!(p, LaunchPhase::Opened);
    }

    #[test]
    fn a_blocked_popup_keeps_the_link_for_retry_and_expiry_drops_it() {
        let mut p = ready();
        assert!(!open_ready(&mut p, NOW, |_| false));
        assert!(matches!(p, LaunchPhase::Blocked(_)));
        assert!(open_ready(&mut p, NOW, |_| true));
        assert_eq!(p, LaunchPhase::Opened);

        let mut p = ready();
        assert!(!open_ready(&mut p, 5_000, |_| panic!(
            "expired link must not open"
        )));
        assert_eq!(p, LaunchPhase::Failed);
        // Not-ready phases are untouched.
        let mut p = issuing();
        assert!(!open_ready(&mut p, NOW, |_| panic!("not ready")));
        assert_eq!(p, LaunchPhase::Issuing);
    }

    #[test]
    fn dismiss_or_teardown_returns_only_unopened_grants_for_revocation() {
        let mut p = ready();
        assert_eq!(take_unopened(&mut p), Some("g-1".into()));
        assert_eq!(p, LaunchPhase::Idle);
        let mut p = ready();
        assert!(!open_ready(&mut p, NOW, |_| false));
        assert_eq!(take_unopened(&mut p), Some("g-1".into()));
        for mut p in [
            LaunchPhase::Idle,
            LaunchPhase::Issuing,
            LaunchPhase::Opened,
            LaunchPhase::Failed,
        ] {
            let before = p.clone();
            assert_eq!(take_unopened(&mut p), None);
            assert_eq!(p, before);
        }
    }
}
