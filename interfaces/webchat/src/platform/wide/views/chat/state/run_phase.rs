//! The chat surface's top-level phase.

/// Top-level Chat UI phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatPhase {
    #[default]
    Idle,
    /// Waiting in the session's server-side lane — the run exists and has an
    /// id, but the engine has not admitted it yet.
    ///
    /// Before this variant there was no way to say that, so every client
    /// painted `Thinking` over a run the engine had never heard of, for as
    /// long as `max_wait_secs`.
    ///
    /// `ahead` is how many messages ahead of this one may still run. `0` means
    /// "nobody ahead, but not started" — it reaches the wire both from a front
    /// ticket refused for steering backpressure and from the lane's fail-open
    /// read, and both are true.
    Queued {
        ahead: u16,
    },
    Thinking,
    Streaming,
    Error,
}

impl ChatPhase {
    /// Whether a turn is in flight from the user's point of view.
    ///
    /// The single predicate every surface gates on, so a new phase cannot be
    /// classified one way in the composer and the other way in the message
    /// list. Waiting counts: the composer must not offer a fresh send, and
    /// Stop must stay reachable.
    #[must_use]
    pub const fn is_busy(self) -> bool {
        matches!(self, Self::Queued { .. } | Self::Thinking | Self::Streaming)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Queued is a busy phase. Every surface that gates on "is a turn in
    /// flight" must treat a waiting run as in flight — the composer must not
    /// offer a fresh send, and Stop must stay reachable.
    #[test]
    fn queued_counts_as_busy() {
        assert!(ChatPhase::Queued { ahead: 0 }.is_busy());
        assert!(ChatPhase::Queued { ahead: 3 }.is_busy());
        assert!(ChatPhase::Thinking.is_busy());
        assert!(ChatPhase::Streaming.is_busy());
        assert!(!ChatPhase::Idle.is_busy());
        assert!(!ChatPhase::Error.is_busy());
    }

    /// `ahead = 0` means "nobody ahead of me, but not started" — it reaches
    /// the wire from a front ticket refused for steering backpressure and from
    /// the lane's fail-open read. Both are true and both render the same.
    #[test]
    fn zero_ahead_is_still_a_queued_phase() {
        assert_eq!(
            ChatPhase::Queued { ahead: 0 },
            ChatPhase::Queued { ahead: 0 }
        );
        assert_ne!(ChatPhase::Queued { ahead: 0 }, ChatPhase::Thinking);
    }

    /// No surface may spell the busy predicate by hand.
    ///
    /// `ChatPhase` has no exhaustive `match` anywhere in this crate — every
    /// reader is an `==`, a `matches!`, or a discarded read — so adding a
    /// variant is silently compatible and the compiler names nobody. That is
    /// how `platform/phone/chat/composer.rs` came to enumerate
    /// `Thinking | Streaming` inline: correct on the day it was written, and
    /// wrong the moment a third busy phase existed, with the phone composer
    /// offering a fresh send into a queue the user cannot see.
    ///
    /// The rule is derived, not a list: a `matches!` naming TWO OR MORE
    /// `ChatPhase` variants on one line is an inline phase set, and the only
    /// place allowed to hold one is [`ChatPhase::is_busy`]. A single-variant
    /// `matches!` is fine — that is asking about one specific phase, which
    /// `==` cannot express for `Queued { .. }`.
    #[test]
    fn no_surface_enumerates_the_busy_phases_by_hand() {
        let mut offenders = Vec::new();
        let mut inspected = 0usize;
        for path in crate::disposed_reads::rust_sources(&crate::disposed_reads::src_dir()) {
            // This file's RED fixture below is by construction the shape the
            // rule forbids; scanning it would make the guard report itself and
            // never go green. Same carve-out, same reason, as
            // `disposed_reads::rust_sources` makes for its own file.
            if path.file_name().is_some_and(|n| n == "run_phase.rs") {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (i, line) in src.lines().enumerate() {
                // The scanner judges code; a comment is documentation.
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if !line.contains("matches!(") || !line.contains("ChatPhase::") {
                    continue;
                }
                inspected += 1;
                if line.matches("ChatPhase::").count() >= 2 {
                    offenders.push(format!("{}:{}", path.display(), i + 1));
                }
            }
        }
        assert!(
            inspected > 0,
            "the scanner matched no `matches!(… ChatPhase::…)` anywhere — it has \
             gone vacuous and would agree with any code at all"
        );
        assert!(
            offenders.is_empty(),
            "{offenders:?} enumerate ChatPhase variants inline instead of asking \
             `ChatPhase::is_busy()`. Adding a phase cannot reach them: this crate \
             has no exhaustive match on ChatPhase, so the compiler names nobody"
        );
    }

    /// The scanner itself, on hand-built input — a guard whose scanner is wrong
    /// fails open, matching nothing and agreeing with everything.
    #[test]
    fn the_scanner_separates_a_phase_set_from_a_single_phase_test() {
        let two = "matches!(p, ChatPhase::Thinking | ChatPhase::Streaming)";
        let one = "matches!(p, ChatPhase::Queued { .. })";
        assert_eq!(two.matches("ChatPhase::").count(), 2);
        assert_eq!(one.matches("ChatPhase::").count(), 1);
    }
}
