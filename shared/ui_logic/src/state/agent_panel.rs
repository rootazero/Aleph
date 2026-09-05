//! The agent panel's model — the single source both sidebars render from.
//!
//! R2: sorting, grouping and the state glyph table live HERE and nowhere
//! else. Neither `interfaces/tui` nor `interfaces/webchat` may sort again. A
//! source-level guard in alephcore (`src/gateway/runtime/` tests, added by
//! Task 10) fails if either frontend's `agent_panel.rs` contains `.sort_by`
//! or `.sort()`.
//!
//! No grouping this phase: herdr groups by worktree parent/child, and the
//! worktree model is phase 2. Grouping now would build UI for a hierarchy
//! that does not exist yet.
//!
//! No manifest version here either: `agent_detect::manifest_version(agent)`
//! is per-agent and looked up at render time from the entry's `agent` label.
//! This model must not depend on `agent-detect` — the Panel is WASM, where
//! that crate's `regex` + `toml` dependencies may or may not build, and
//! that question belongs to whichever frontend renders the version, not to
//! this shared crate. Do not add a `manifest_version` field here "for
//! convenience".

use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState};

/// Lower rank sorts first — the state most likely to need a human's
/// attention right now outranks one that does not.
#[must_use]
pub fn attention_rank(state: RuntimeAgentState) -> u8 {
    match state {
        RuntimeAgentState::Blocked => 0,
        RuntimeAgentState::Working => 1,
        RuntimeAgentState::Idle => 2,
        RuntimeAgentState::Unknown => 3,
    }
}

/// The four-state glyph, for every surface that draws this panel.
///
/// One table, not one per frontend. The TUI and the Panel each carried their
/// own copy of this `match` with no test spanning both (判据 §1): the two
/// would have drifted the first time a state was added or a symbol changed,
/// and the copy nobody edited would have kept rendering the old answer while
/// looking correct in review. Colour is deliberately NOT here — ratatui
/// `Color`s and Tailwind class names have nothing in common, and forcing a
/// shared type on them would put a rendering vocabulary into a crate that
/// has no renderer.
///
/// `Unknown` gets its own glyph and never `Idle`'s: "I cannot tell what this
/// agent is doing" must not render as "nothing is happening here" (判据 §8).
/// [`super::agent_panel_parity::glyphs_are_distinct_and_unknown_is_not_idle`]
/// pins that, plus distinctness across all four.
#[must_use]
pub const fn state_glyph(state: RuntimeAgentState) -> &'static str {
    match state {
        RuntimeAgentState::Blocked => "\u{25cf}", // ●
        RuntimeAgentState::Working => "\u{25d0}", // ◐
        RuntimeAgentState::Idle => "\u{25cb}",    // ○
        RuntimeAgentState::Unknown => "?",
    }
}

/// What to call this row: the FOREGROUND program if the probe could see one,
/// else the recognised agent, else the spawn label.
///
/// In that order because that is falling order of how current the fact is.
/// `program` is what is running right now; `agent` is what the manifest
/// recognised, which can outlive the process by a few samples; `label` is what
/// the session was STARTED as and is the only one of the three that is
/// guaranteed to exist. `program: None` means the probe could not answer, not
/// that nothing is running (spec §5), so it falls through rather than
/// rendering an absence.
///
/// One derivation, not one per frontend. Both faces held a byte-identical copy
/// of this chain, and the Panel's said "same order and same reasoning as the
/// TUI's `entry_name`" — a claim about another crate's file with nothing
/// enforcing it (判据 §1 / §9). The ordering parity machinery could not see the
/// duplication: it scopes itself to sorting, in its own header. The source-level
/// half of this fix is `no_frontend_derives_its_own_agent_row_name` in
/// alephcore (`src/gateway/runtime/tests.rs`), which fails if either frontend's
/// `agent_panel.rs` reads `program` for itself again instead of calling this.
///
/// Words stay per-face (see [`QuietUnit`] for why this crate may name units and
/// never words); a session's NAME is not a word this crate composes — every
/// candidate is a value the server sent.
#[must_use]
pub fn entry_name(entry: &RuntimeAgentEntry) -> String {
    entry
        .program
        .as_deref()
        .or(entry.agent.as_deref())
        .unwrap_or(&entry.label)
        .to_string()
}

/// The unit [`quiet_age`] picked: the coarsest one that does not round to zero.
///
/// A unit, not a word. `shared_ui_logic` has no i18n and must not acquire an
/// opinion about English — the previous version of this composed `"quiet 3m"`
/// here, which shipped an untranslated string onto a localized surface where
/// the Panel's own `hardcoded_english_line_ratchet` could not see it (it only
/// scans that crate's sources — 判据 §18, a guard that covers what it
/// enumerates). Each frontend now names the unit in its own language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuietUnit {
    Seconds,
    Minutes,
    Hours,
    Days,
}

/// How long a session has been silent: a number and the unit it is counted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietAge {
    pub value: u64,
    pub unit: QuietUnit,
}

/// How long a session has been silent, derived once for every surface.
///
/// `None` while it is still producing output — `quiet_since` is `None` then,
/// and there is nothing to say.
///
/// # What this is NOT
///
/// It is not a state. Silence is not idle: an agent thinking for five minutes
/// emits nothing, and `RuntimeAgentEntry::state` is unaffected by this value
/// (spec R2-3). Rendering the age is how "is it stuck?" becomes answerable
/// without any code turning time into evidence.
///
/// # Rounding, and which way it is wrong
///
/// Always DOWN, and to the coarsest unit that does not round to zero: 89
/// seconds is 1 minute, not 1.5 and not 2. `now` and `quiet_since` are both
/// Unix epoch MILLISECONDS, and they come from different clocks (the server
/// stamps `quiet_since`; the caller passes its own `now`), so a skew is
/// possible. A future `quiet_since` clamps to zero rather than producing a
/// negative age — meaning a disagreeing clock can only ever make a session
/// look FRESHER than it is. That is the direction to be wrong in: this value
/// exists to raise an eyebrow, and one that overstates would raise it at
/// nothing.
#[must_use]
pub fn quiet_age(quiet_since: Option<i64>, now: i64) -> Option<QuietAge> {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    let since = quiet_since?;
    // `max(0)` before the cast: a future `quiet_since` must clamp, not wrap.
    let seconds = now.saturating_sub(since).max(0).unsigned_abs() / 1_000;
    Some(if seconds < MINUTE {
        QuietAge {
            value: seconds,
            unit: QuietUnit::Seconds,
        }
    } else if seconds < HOUR {
        QuietAge {
            value: seconds / MINUTE,
            unit: QuietUnit::Minutes,
        }
    } else if seconds < DAY {
        QuietAge {
            value: seconds / HOUR,
            unit: QuietUnit::Hours,
        }
    } else {
        QuietAge {
            value: seconds / DAY,
            unit: QuietUnit::Days,
        }
    })
}

/// Order entries by attention rank, then by recency, then by `session_id`.
///
/// `session_id` is the key of the server-side agent table, so it is unique
/// across entries: no two distinct entries can ever compare `Equal` on all
/// three keys together, which makes this a total order. That is deliberate
/// — the resulting order does not depend on the order the server sent
/// entries in, and does not depend on whether the underlying sort is
/// stable, so two rows that tie on state and `updated_at` still resolve to
/// the same position on every call instead of shuffling between refreshes.
pub fn sort_entries(entries: &mut [RuntimeAgentEntry]) {
    entries.sort_by(|a, b| {
        attention_rank(a.state)
            .cmp(&attention_rank(b.state))
            .then(b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
}

/// The panel's local layout state — the divider position, and nothing else.
///
/// A `collapsed: bool` lived here and was CUT (S4): nothing ever read it and
/// nothing ever set it to `true`. Both faces express "collapsed" through the
/// ratio the divider already owns — the Panel drags `split_ratio` and the
/// TUI toggles the whole column — so the flag was a second answer to a
/// question `split_ratio` had already answered, kept alive only by a default
/// value and a test asserting that default (判据 §2: a predicate with no
/// input that can change it). Adding it back needs a reader first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentPanelState {
    /// The clamp-and-NaN-fallback invariant lives in `with_split_ratio`,
    /// not here — this field is `pub` so the read side can use it directly,
    /// but a direct struct literal (or `..` update syntax) bypasses that
    /// guard entirely. Any caller computing a ratio from input (a drag
    /// delta, a stored preference) must construct through
    /// `with_split_ratio`, not by writing this field.
    pub split_ratio: f32,
}

/// A ratio at or below this collapses the agent pane to nothing with no way
/// to grab the divider back (判据 §14: 被闸住的人接下来会干什么).
pub const MIN_SPLIT_RATIO: f32 = 0.1;
/// A ratio at or above this collapses the *other* pane the same way.
pub const MAX_SPLIT_RATIO: f32 = 0.9;

impl Default for AgentPanelState {
    fn default() -> Self {
        Self { split_ratio: 0.4 }
    }
}

impl AgentPanelState {
    /// Returns a copy with `split_ratio` clamped to
    /// `[MIN_SPLIT_RATIO, MAX_SPLIT_RATIO]`.
    ///
    /// `f32::clamp` propagates a NaN input as NaN, which would reach the
    /// divider and lay out nothing — "I cannot compute a ratio" is not a
    /// ratio (判据 §8), so NaN falls back to the default split instead.
    #[must_use]
    pub fn with_split_ratio(self, ratio: f32) -> Self {
        let split_ratio = if ratio.is_nan() {
            Self::default().split_ratio
        } else {
            ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO)
        };
        Self { split_ratio }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState as S};

    fn e(session_id: &str, state: S, updated_at: i64) -> RuntimeAgentEntry {
        RuntimeAgentEntry {
            session_id: session_id.to_string(),
            label: "claude".to_string(),
            cwd: String::new(),
            agent: None,
            program: None,
            state,
            updated_at,
            quiet_since: None,
        }
    }

    /// 单键排序：blocked 恒在 working 前。
    #[test]
    fn blocked_always_outranks_working() {
        assert!(attention_rank(S::Blocked) < attention_rank(S::Working));
        assert!(attention_rank(S::Working) < attention_rank(S::Idle));
        assert!(attention_rank(S::Idle) < attention_rank(S::Unknown));
    }

    /// 同状态内按 updated_at 降序排列（无相同 updated_at，未测 tie-break——
    /// tie-break 见 `ties_on_state_and_updated_at_resolve_by_session_id`）。
    #[test]
    fn same_state_orders_by_recency_descending() {
        let mut v = vec![
            e("a", S::Working, 1),
            e("b", S::Blocked, 5),
            e("c", S::Working, 9),
        ];
        sort_entries(&mut v);
        assert_eq!(v[0].state, S::Blocked);
        assert_eq!(v[1].updated_at, 9);
        assert_eq!(v[2].updated_at, 1);
    }

    /// A state tie AND an `updated_at` tie resolve by `session_id` — not by
    /// input order. The input here is deliberately given in the OPPOSITE of
    /// `session_id` order ("second" before "first"), so this reddens if the
    /// `session_id` key is dropped (falling back to input order), reordered
    /// ahead of `state`/`updated_at`, or reversed (`b.cmp(&a)`).
    #[test]
    fn ties_on_state_and_updated_at_resolve_by_session_id() {
        let mut v = vec![e("second", S::Idle, 100), e("first", S::Idle, 100)];
        sort_entries(&mut v);
        assert_eq!(v[0].session_id, "first");
        assert_eq!(v[1].session_id, "second");
    }

    /// The fallback chain, in falling order of how current the fact is:
    /// probed program, then recognised agent, then the spawn label. Each step
    /// is reached only when the one above is absent — `program: None` means the
    /// probe could not answer, which is not "nothing is running".
    ///
    /// Moved here from the Panel's own test module when `entry_name` became one
    /// derivation instead of two: a per-face copy of this test asserted only
    /// that face's copy of the chain, which is exactly how the two were free to
    /// drift while both looked tested. Reddens if the order is permuted, if any
    /// step is dropped, or if an absent `program` stops falling through.
    #[test]
    fn a_row_prefers_the_probed_program_then_the_agent_then_the_label() {
        let mut entry = e("s", S::Working, 0);
        entry.label = "spawn-label".to_string();

        assert_eq!(entry_name(&entry), "spawn-label", "only the label exists");

        entry.agent = Some("codex".to_string());
        assert_eq!(
            entry_name(&entry),
            "codex",
            "a recognised agent beats the label"
        );

        entry.program = Some("claude".to_string());
        assert_eq!(
            entry_name(&entry),
            "claude",
            "what is running right now beats what was recognised"
        );

        entry.program = None;
        assert_eq!(
            entry_name(&entry),
            "codex",
            "an unanswerable probe falls through; it does not render an absence"
        );
    }

    /// `None` in, `None` out: a session that is still producing output has
    /// no quiet age, and inventing "0 seconds" for it would put a label on
    /// every healthy row.
    ///
    /// Rounding is DOWN at every boundary, and a clock that disagrees can
    /// only ever make a session look fresher (a future `quiet_since` clamps
    /// to zero rather than going negative).
    ///
    /// Reddens if: an active session gains an age; if any boundary rounds up
    /// (59s must not become 1 minute, 119s must not become 2); or if a
    /// negative age escapes as a wrapped-around number.
    #[test]
    fn quiet_age_is_none_when_active_and_rounds_down() {
        const NOW: i64 = 1_000_000_000;
        fn age(ms_ago: i64) -> QuietAge {
            quiet_age(Some(NOW - ms_ago), NOW).expect("a quiet session has an age")
        }

        assert_eq!(quiet_age(None, NOW), None, "an active session has no age");

        assert_eq!(
            age(0),
            QuietAge {
                value: 0,
                unit: QuietUnit::Seconds
            }
        );
        assert_eq!(
            age(59_999),
            QuietAge {
                value: 59,
                unit: QuietUnit::Seconds
            },
            "one millisecond short of a minute is still seconds"
        );
        assert_eq!(
            age(60_000),
            QuietAge {
                value: 1,
                unit: QuietUnit::Minutes
            }
        );
        assert_eq!(
            age(119_000),
            QuietAge {
                value: 1,
                unit: QuietUnit::Minutes
            },
            "1m59s rounds DOWN to 1 minute"
        );
        assert_eq!(
            age(180_000),
            QuietAge {
                value: 3,
                unit: QuietUnit::Minutes
            }
        );
        assert_eq!(
            age(3_599_000),
            QuietAge {
                value: 59,
                unit: QuietUnit::Minutes
            }
        );
        assert_eq!(
            age(3_600_000),
            QuietAge {
                value: 1,
                unit: QuietUnit::Hours
            }
        );
        assert_eq!(
            age(86_399_000),
            QuietAge {
                value: 23,
                unit: QuietUnit::Hours
            },
            "one second short of a day is still hours"
        );
        assert_eq!(
            age(86_400_000),
            QuietAge {
                value: 1,
                unit: QuietUnit::Days
            }
        );
        assert_eq!(
            age(200_000_000),
            QuietAge {
                value: 2,
                unit: QuietUnit::Days
            }
        );

        // Clock skew: a `quiet_since` in the future clamps, never wraps.
        assert_eq!(
            quiet_age(Some(NOW + 500_000), NOW),
            Some(QuietAge {
                value: 0,
                unit: QuietUnit::Seconds
            }),
            "a disagreeing clock may only make a session look fresher"
        );
    }

    /// `shared_ui_logic` must not acquire an opinion about English. This is
    /// the structural half of I2: the module may name units, never words, and
    /// only a scan of its own source can say so — a type cannot.
    ///
    /// # It reads LITERALS, not prose
    ///
    /// The first version of this searched the raw text for `"quiet "` and went
    /// red on its own doc comments, which discuss the string this fix removed.
    /// A guard that fires on the explanation of the rule is a guard that will
    /// be silenced rather than obeyed (判据 §3: a false positive costs more
    /// than a missing one, because it gets quoted as evidence). So it looks at
    /// string literals only, with comments stripped first, and asks whether
    /// any of them contains a WORD — three consecutive ASCII letters. The four
    /// glyphs (`"\u{25cf}"`, `"?"`) survive that; `format!("quiet {n}m")`
    /// does not.
    ///
    /// # Why the test-module cut is checked rather than assumed
    ///
    /// Cutting production code at the first `#[cfg(test)]` marker under-scans
    /// the moment a second one appears above the test module, and it does so
    /// silently — the webchat crate has a dedicated guard against exactly that
    /// hand-rolled cut. This crate has no equivalent helper, so the cut asserts
    /// its own precondition instead: exactly one marker in the file. A second
    /// one reddens here rather than quietly shrinking what is checked.
    #[test]
    fn this_module_composes_no_user_facing_words() {
        let src = include_str!("agent_panel.rs").replace('\r', "");
        let marker = src
            .find("#[cfg(test)]")
            .expect("this file has a test module");
        // Counting markers would be circular — this very function mentions the
        // attribute, in prose and as a literal. Pinning WHERE the first one
        // lands is not: if anything gated ever appears above the test module,
        // the cut stops being the test-module boundary and this reddens
        // instead of quietly scanning less.
        assert!(
            src[marker..]
                .trim_start_matches("#[cfg(test)]")
                .trim_start()
                .starts_with("mod tests {"),
            "the first `#[cfg(test)]` in this file must be the test module's, \
             or the cut below is no longer the production/test boundary"
        );
        let production = &src[..marker];

        // Comments first: they discuss the copy this rule forbids.
        let code: String = production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut in_string = false;
        let mut escaped = false;
        let mut run = 0_usize;
        let mut literal = String::new();
        for c in code.chars() {
            if !in_string {
                if c == '"' {
                    in_string = true;
                    literal.clear();
                    run = 0;
                }
                continue;
            }
            if escaped {
                escaped = false;
                run = 0;
                continue;
            }
            match c {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {
                    literal.push(c);
                    run = if c.is_ascii_alphabetic() { run + 1 } else { 0 };
                    assert!(
                        run < 3,
                        "string literal {literal:?} in a crate with no i18n reads as \
                         user-facing copy; the words belong to whichever frontend \
                         renders them, and only the number and the unit belong here"
                    );
                }
            }
        }
    }

    #[test]
    fn the_default_split_is_forty_percent() {
        let state = AgentPanelState::default();
        assert_eq!(state.split_ratio, 0.4);
    }

    #[test]
    fn with_split_ratio_clamps_below_min() {
        let state = AgentPanelState::default().with_split_ratio(0.0);
        assert_eq!(state.split_ratio, MIN_SPLIT_RATIO);
    }

    #[test]
    fn with_split_ratio_clamps_above_max() {
        let state = AgentPanelState::default().with_split_ratio(1.0);
        assert_eq!(state.split_ratio, MAX_SPLIT_RATIO);
    }

    #[test]
    fn with_split_ratio_nan_falls_back_to_default() {
        let state = AgentPanelState::default().with_split_ratio(f32::NAN);
        assert_eq!(state.split_ratio, AgentPanelState::default().split_ratio);
    }
}
