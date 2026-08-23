//! Single source for "window_id → live window" resolution.
//!
//! Three call sites used to each hand-roll the same `window_list` + find +
//! "no bounds on this platform" ladder, with three different error phrasings
//! for the same failure (`native::resolve_target_pid`,
//! `set_of_marks::resolve_window`, `coord_resolve::window_frame`). The lookup
//! itself is one function here; callers that need the frame attach their own
//! purpose clause to [`WindowLookup::frame_or`] so the model still learns *why*
//! this caller needed bounds.

use aleph_desktop::{BoundingBox, ScreenCapability, WindowInfo};

/// A window that exists *and* reports the two things every targeting path
/// needs: an owning pid in the addressable range and a frame in the global
/// point space.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedWindow {
    pub id: u64,
    pub pid: i32,
    pub frame: BoundingBox,
}

/// Fetch the live window list and find `window_id` in it.
///
/// `Err` distinguishes "the limb could not list windows" from "the id is
/// stale" — a model can retry the first and must re-observe for the second.
pub async fn lookup_window(
    screen: &dyn ScreenCapability,
    window_id: u64,
) -> std::result::Result<WindowInfo, String> {
    let windows = screen
        .window_list()
        .await
        .map_err(|e| format!("window_list failed: {e}"))?;
    windows
        .into_iter()
        .find(|w| w.id == window_id)
        .ok_or_else(|| format!("no window with id {window_id} is open"))
}

/// The owning pid of `info`, or why it cannot be addressed.
pub fn pid_of(info: &WindowInfo) -> std::result::Result<i32, String> {
    i32::try_from(info.pid).map_err(|_| {
        format!(
            "window {} has a pid ({}) outside the addressable range",
            info.id, info.pid
        )
    })
}

impl ResolvedWindow {
    /// Resolve `window_id` to id + pid + frame. The frame is required:
    /// `purpose` completes the sentence "window N reports no bounds on this
    /// platform, so …" with *this caller's* reason (e.g. "its marks cannot be
    /// placed"), keeping the no-bounds refusal actionable instead of generic.
    pub async fn lookup(
        screen: &dyn ScreenCapability,
        window_id: u64,
        purpose: &str,
    ) -> std::result::Result<Self, String> {
        let info = lookup_window(screen, window_id).await?;
        let pid = pid_of(&info)?;
        let frame = info.bounds.ok_or_else(|| {
            format!("window {window_id} reports no bounds on this platform, so {purpose}")
        })?;
        Ok(Self {
            id: window_id,
            pid,
            frame,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: u64, pid: u64, bounds: Option<BoundingBox>) -> WindowInfo {
        WindowInfo {
            id,
            pid,
            bounds,
            ..Default::default()
        }
    }

    #[test]
    fn pid_out_of_range_names_both_the_window_and_the_pid() {
        let err = pid_of(&info(7, u64::MAX, None)).unwrap_err();
        assert!(err.contains('7'), "window id missing: {err}");
        assert!(err.contains(&u64::MAX.to_string()), "pid missing: {err}");
    }

    #[test]
    fn in_range_pid_round_trips() {
        assert_eq!(pid_of(&info(1, 4242, None)).unwrap(), 4242);
    }
}
