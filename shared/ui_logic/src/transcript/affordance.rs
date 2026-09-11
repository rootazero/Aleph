//! Small presentation decisions both surfaces share verbatim.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modality {
    Mouse,
    Key(&'static str),
}

/// `… +N lines (ctrl+o to expand)` or `… +N lines · click to expand`.
#[must_use]
pub fn expand_hint(modality: Modality, hidden_rows: usize) -> String {
    let unit = if hidden_rows == 1 { "line" } else { "lines" };
    match modality {
        Modality::Mouse => format!("… +{hidden_rows} {unit} · click to expand"),
        Modality::Key(k) => format!("… +{hidden_rows} {unit} ({k} to expand)"),
    }
}

/// `… (ctrl+o to collapse)` or `… click to collapse`.
///
/// # Why an unfolded row still carries a line about folding
///
/// Unfolding removes the hint the gesture was made on. If the only other
/// target were the row's header, a body taller than the viewport would push
/// it off the top the moment it unfolded — and the gesture that undoes the
/// last one would be unreachable from where the last one left the reader.
/// So the affordance moves to the far end of the body, which is exactly
/// where a viewport following the bottom lands.
///
/// No row count: there is nothing hidden to count, and a number here would
/// be inventing one to fill the shape of the other hint.
#[must_use]
pub fn collapse_hint(modality: Modality) -> String {
    match modality {
        Modality::Mouse => "… click to collapse".to_string(),
        Modality::Key(k) => format!("… ({k} to collapse)"),
    }
}

pub const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
pub const SPINNER_PERIOD_MS: u64 = 80;

/// Pure function of wall-clock time, so every row repaints in step without
/// a shared timer (pi-cc-extensions `tool-loading-icon.ts`).
#[must_use]
pub fn spinner_frame(now_ms: u64) -> char {
    SPINNER_FRAMES[((now_ms / SPINNER_PERIOD_MS) % SPINNER_FRAMES.len() as u64) as usize]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    En,
    Zh,
}

pub const VERB_REROLL_MS: u64 = 7_000;

pub const VERBS_EN: &[&str] = &[
    "Thinking",
    "Pondering",
    "Reading",
    "Searching",
    "Tracing",
    "Weaving",
    "Mapping",
    "Sifting",
    "Assembling",
    "Composing",
    "Checking",
    "Untangling",
    "Refining",
    "Sketching",
    "Digging",
    "Connecting",
    "Comparing",
    "Testing",
    "Measuring",
    "Planning",
    "Drafting",
    "Reviewing",
    "Aligning",
    "Stitching",
    "Polishing",
    "Verifying",
    "Scanning",
    "Gathering",
    "Shaping",
    "Working",
];
pub const VERBS_ZH: &[&str] = &[
    "思考中",
    "琢磨中",
    "阅读中",
    "搜索中",
    "追踪中",
    "梳理中",
    "拼装中",
    "整理中",
    "核对中",
    "推演中",
    "勾勒中",
    "挖掘中",
    "串联中",
    "比对中",
    "测试中",
    "度量中",
    "规划中",
    "起草中",
    "复核中",
    "对齐中",
    "缝合中",
    "打磨中",
    "验证中",
    "扫描中",
    "收集中",
    "成形中",
    "构思中",
    "校准中",
    "编排中",
    "工作中",
];

#[must_use]
pub fn verb(locale: Locale, seed: u64) -> &'static str {
    let list = match locale {
        Locale::En => VERBS_EN,
        Locale::Zh => VERBS_ZH,
    };
    list[(seed % list.len() as u64) as usize]
}

/// `0.8s`, `12s`, `2m 05s`, `1h 03m`.
#[must_use]
pub fn fmt_duration_ms(ms: u64) -> String {
    if ms < 10_000 {
        return format!("{:.1}s", ms as f64 / 1000.0);
    }
    let s = ms / 1000;
    if s < 60 {
        return format!("{s}s");
    }
    let (m, s) = (s / 60, s % 60);
    if m < 60 {
        return format!("{m}m {s:02}s");
    }
    format!("{}h {:02}m", m / 60, m % 60)
}

/// The turn trailer.
#[must_use]
pub fn worked_for(duration_ms: u64) -> String {
    format!("✻ Worked for {}", fmt_duration_ms(duration_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hint_is_a_function_of_modality() {
        assert_eq!(
            expand_hint(Modality::Mouse, 41),
            "… +41 lines · click to expand"
        );
        assert_eq!(
            expand_hint(Modality::Key("ctrl+o"), 1),
            "… +1 line (ctrl+o to expand)"
        );
    }

    /// The undo of the expand hint, worded by the same modality. It must not
    /// claim a row count: nothing is hidden.
    #[test]
    fn the_collapse_hint_follows_the_same_modality_and_counts_nothing() {
        assert_eq!(collapse_hint(Modality::Mouse), "… click to collapse");
        assert_eq!(
            collapse_hint(Modality::Key("ctrl+o")),
            "… (ctrl+o to collapse)"
        );
        // No `+N lines`: nothing is hidden, so a count here would be invented
        // to fill the shape of the other hint. Checked as "no digits" rather
        // than "no `+`" — the key name itself carries one (`ctrl+o`), which
        // is what the first version of this assertion tripped over.
        for m in [Modality::Mouse, Modality::Key("ctrl+o")] {
            let hint = collapse_hint(m);
            assert!(!hint.contains(char::is_numeric), "{hint}");
            assert!(!hint.contains("line"), "{hint}");
        }
    }
    #[test]
    fn spinner_is_periodic_in_time_and_verbs_are_stable_for_a_seed() {
        assert_eq!(spinner_frame(0), spinner_frame(800));
        assert_ne!(spinner_frame(0), spinner_frame(80));
        assert_eq!(verb(Locale::En, 7), verb(Locale::En, 7));
        assert_eq!(VERBS_EN.len(), 30);
        assert_eq!(VERBS_ZH.len(), 30);
    }
    #[test]
    fn durations_switch_units_at_the_right_edges() {
        assert_eq!(fmt_duration_ms(800), "0.8s");
        assert_eq!(fmt_duration_ms(12_000), "12s");
        assert_eq!(fmt_duration_ms(125_000), "2m 05s");
        assert_eq!(worked_for(12_000), "✻ Worked for 12s");
    }
}
