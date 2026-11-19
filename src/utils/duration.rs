//! One grammar for the duration strings this process accepts.
//!
//! Three faces read the same operator string: the model-facing tool, the JSON
//! RPC face, and the loop tool. Each carried its own parser, so one input was
//! legal on one face and illegal on another, and a sub-second interval the
//! tool face refused was accepted by the RPC face and then fired on every
//! tick. The floor lives here, with the grammar, so no face can skip it.

/// The smallest interval a scheduler in this process accepts.
///
/// A sub-second interval makes the work it schedules fire on every tick — a
/// resource hazard, not a tuning knob.
pub const MIN_INTERVAL_MS: u64 = 1_000;

/// Parse a human duration into milliseconds.
///
/// Accepts an explicit suffix — "500ms", "30s", "5m", "2h" — or a bare number
/// read as milliseconds ("60000"). Rejects garbage, and rejects anything
/// below [`MIN_INTERVAL_MS`].
pub fn parse_interval_ms(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty interval".to_string());
    }
    let (num, unit_ms): (&str, u64) = if let Some(n) = s.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1_000)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60_000)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3_600_000)
    } else {
        (s, 1)
    };
    let value: u64 = num
        .trim()
        .parse()
        .map_err(|_| format!("invalid interval number in '{s}'"))?;
    let ms = value.saturating_mul(unit_ms);
    if ms < MIN_INTERVAL_MS {
        return Err(format!(
            "interval too short: '{s}' is below the {MIN_INTERVAL_MS}ms minimum"
        ));
    }
    Ok(ms)
}

#[cfg(test)]
mod tests {
    use super::{parse_interval_ms, MIN_INTERVAL_MS};

    #[test]
    fn reads_the_suffix_forms() {
        assert_eq!(parse_interval_ms("30s").unwrap(), 30_000);
        assert_eq!(parse_interval_ms("5m").unwrap(), 300_000);
        assert_eq!(parse_interval_ms("2h").unwrap(), 7_200_000);
        assert_eq!(parse_interval_ms("1500ms").unwrap(), 1_500);
    }

    #[test]
    fn reads_a_bare_number_as_milliseconds() {
        assert_eq!(parse_interval_ms("60000").unwrap(), 60_000);
    }

    #[test]
    fn refuses_anything_below_the_floor() {
        assert!(parse_interval_ms("0").is_err());
        assert!(parse_interval_ms("500").is_err());
        assert!(parse_interval_ms("100ms").is_err());
        assert!(parse_interval_ms("999").is_err());
        assert!(parse_interval_ms(&MIN_INTERVAL_MS.to_string()).is_ok());
    }

    #[test]
    fn refuses_garbage() {
        assert!(parse_interval_ms("").is_err());
        assert!(parse_interval_ms("soon").is_err());
        assert!(parse_interval_ms("5x").is_err());
    }
}
