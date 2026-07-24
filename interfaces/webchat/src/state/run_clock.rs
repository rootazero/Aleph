//! 1s shared clock for live tool-row elapsed timers + formatting helper.

use leptos::prelude::*;

/// 1s-granularity shared clock (epoch ms). Only running tool rows subscribe, avoiding full-list re-renders.
#[derive(Clone, Copy)]
pub struct SecondTick(pub RwSignal<i64>);

/// Silent threshold for displaying elapsed time.
pub const LONG_RUN_THRESHOLD_MS: i64 = 8_000;

/// "12s" / "1m05s" — negative values (clock rollback) clamped to 0.
#[must_use]
pub fn fmt_elapsed(elapsed_ms: i64) -> String {
    let secs = (elapsed_ms.max(0)) / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_elapsed_seconds_and_minutes() {
        assert_eq!(fmt_elapsed(9_400), "9s");
        assert_eq!(fmt_elapsed(65_000), "1m05s");
        assert_eq!(fmt_elapsed(0), "0s");
        assert_eq!(fmt_elapsed(-5), "0s"); // 时钟回拨防御
    }
}
