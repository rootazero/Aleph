//! Stateless accessibility-element ranking, shared by every AX limb.
//!
//! `ax.set_value` and `ax.perform_action` carry an [`AxLocator`], not a handle:
//! the limb re-walks the accessibility tree on every call and picks the best
//! match. That keeps element handles from crossing an IPC boundary (where they
//! go stale) — but only works if every platform ranks candidates the *same*
//! way, or the model learns one platform's habits and is wrong on the next.
//!
//! macOS ranks in the Swift helper; this is the Rust half, shared by the
//! Windows UI Automation limb and the Linux AT-SPI one. Both flatten their own
//! native elements into [`RankCandidate`] — scalars only, so the decision stays
//! a pure function that needs neither COM nor a D-Bus connection to test.

use aleph_protocol::desktop_bridge::methods::ax::AxLocator;

/// A flattened element summary used purely for locator ranking.
#[derive(Clone, Debug)]
pub struct RankCandidate {
    /// Mapped `"AX*"` role string.
    pub role: String,
    /// Element name / title, if any.
    pub title: Option<String>,
    /// Bounding-rect center, in the same space the locator's `center` uses.
    pub center: (f64, f64),
}

/// Pick the best candidate for an [`AxLocator`], mirroring the macOS Swift
/// locator: role is a hard filter; title ranks exact (0) < contains (1) <
/// no-match (2), case-insensitive; `center` euclidean distance breaks ties.
/// Returns `None` when the role filter leaves no candidate.
#[must_use]
pub fn rank_candidates(cands: &[RankCandidate], loc: &AxLocator) -> Option<usize> {
    let mut best: Option<(usize, (u8, f64))> = None;
    for (i, c) in cands.iter().enumerate() {
        if let Some(role) = &loc.role {
            if &c.role != role {
                continue;
            }
        }
        let title_rank = match (&loc.title, &c.title) {
            (Some(want), Some(have)) => {
                let (want, have) = (want.to_lowercase(), have.to_lowercase());
                if have == want {
                    0
                } else if have.contains(&want) {
                    1
                } else {
                    2
                }
            }
            (Some(_), None) => 2,
            (None, _) => 0,
        };
        let dist = match loc.center {
            Some([x, y]) => {
                let (dx, dy) = (c.center.0 - x, c.center.1 - y);
                dx.hypot(dy)
            }
            None => 0.0,
        };
        let key = (title_rank, dist);
        if best.as_ref().is_none_or(|(_, bk)| key < *bk) {
            best = Some((i, key));
        }
    }
    best.map(|(i, _)| i)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(role: &str, title: Option<&str>, cx: f64, cy: f64) -> RankCandidate {
        RankCandidate {
            role: role.into(),
            title: title.map(Into::into),
            center: (cx, cy),
        }
    }

    fn loc(role: Option<&str>, title: Option<&str>, center: Option<[f64; 2]>) -> AxLocator {
        AxLocator {
            pid: None,
            role: role.map(Into::into),
            title: title.map(Into::into),
            center,
        }
    }

    #[test]
    fn role_filter_excludes_non_matching() {
        let cands = [
            cand("AXButton", Some("OK"), 0.0, 0.0),
            cand("AXTextField", Some("OK"), 0.0, 0.0),
        ];
        assert_eq!(
            rank_candidates(&cands, &loc(Some("AXTextField"), None, None)),
            Some(1)
        );
    }

    #[test]
    fn no_match_returns_none() {
        let cands = [cand("AXButton", None, 0.0, 0.0)];
        assert_eq!(
            rank_candidates(&cands, &loc(Some("AXTextField"), None, None)),
            None
        );
    }

    #[test]
    fn exact_title_beats_contains_case_insensitive() {
        let cands = [
            cand("AXTextField", Some("Email address"), 0.0, 0.0),
            cand("AXTextField", Some("email"), 0.0, 0.0),
        ];
        assert_eq!(
            rank_candidates(&cands, &loc(Some("AXTextField"), Some("Email"), None)),
            Some(1)
        );
    }

    #[test]
    fn center_breaks_ties_when_titles_equal_rank() {
        let cands = [
            cand("AXButton", None, 100.0, 100.0),
            cand("AXButton", None, 10.0, 10.0),
        ];
        assert_eq!(
            rank_candidates(&cands, &loc(Some("AXButton"), None, Some([0.0, 0.0]))),
            Some(1)
        );
    }

    #[test]
    fn no_role_filter_considers_all() {
        let cands = [
            cand("AXButton", Some("Save"), 0.0, 0.0),
            cand("AXMenuItem", Some("Save"), 0.0, 0.0),
        ];
        assert_eq!(
            rank_candidates(&cands, &loc(None, Some("Save"), None)),
            Some(0)
        );
    }

    #[test]
    fn an_untitled_candidate_loses_to_a_titled_one_when_a_title_is_asked_for() {
        let cands = [
            cand("AXButton", None, 0.0, 0.0),
            cand("AXButton", Some("Save"), 500.0, 500.0),
        ];
        // Distance would favour candidate 0; the title rank must dominate.
        assert_eq!(
            rank_candidates(&cands, &loc(None, Some("Save"), Some([0.0, 0.0]))),
            Some(1)
        );
    }

    #[test]
    fn an_empty_candidate_set_is_none_not_a_panic() {
        assert_eq!(rank_candidates(&[], &loc(None, None, None)), None);
    }
}
