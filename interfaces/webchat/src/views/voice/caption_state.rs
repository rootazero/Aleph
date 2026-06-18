//! Pure two-stage caption reducer. No web_sys → host-testable (project test redline).

#[derive(Default, Clone, PartialEq)]
pub(crate) struct CaptionState {
    pub committed: String, // locked text (solid/white)
    pub interim: String,   // floating hypothesis (gray)
    pub locked: bool,      // utterance ended → wave fired
    pub formatted: bool,   // AI-polished text swapped in
}

pub(crate) struct Delta {
    pub committed: String,
    pub interim: String,
}

pub(crate) fn apply_delta(s: &mut CaptionState, d: Delta) {
    s.committed = d.committed;
    s.interim = d.interim;
}

/// Utterance end: drop the floating interim, mark locked (Panel fires the wave).
pub(crate) fn lock(s: &mut CaptionState) {
    s.interim.clear();
    s.locked = true;
}

/// AI-formatted text arrives → replace committed (quiet fade swap).
pub(crate) fn apply_formatted(s: &mut CaptionState, polished: &str) {
    s.committed = polished.to_string();
    s.formatted = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_updates_committed_and_interim() {
        let mut s = CaptionState::default();
        apply_delta(
            &mut s,
            Delta {
                committed: "你好".into(),
                interim: "世".into(),
            },
        );
        assert_eq!(s.committed, "你好");
        assert_eq!(s.interim, "世");
        assert!(!s.locked);
    }

    #[test]
    fn lock_drops_interim_and_marks_locked() {
        let mut s = CaptionState::default();
        apply_delta(
            &mut s,
            Delta {
                committed: "你好世界".into(),
                interim: "吗".into(),
            },
        );
        lock(&mut s);
        assert_eq!(s.committed, "你好世界");
        assert_eq!(s.interim, "");
        assert!(s.locked);
    }

    #[test]
    fn formatted_replaces_committed_after_lock() {
        let mut s = CaptionState::default();
        apply_delta(
            &mut s,
            Delta {
                committed: "额我想问下本地语音释放".into(),
                interim: String::new(),
            },
        );
        lock(&mut s);
        apply_formatted(&mut s, "请问如何实现本地语音模型的内存释放？");
        assert_eq!(s.committed, "请问如何实现本地语音模型的内存释放？");
        assert!(s.formatted);
    }
}
