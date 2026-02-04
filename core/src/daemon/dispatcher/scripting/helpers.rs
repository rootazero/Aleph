//! Helper Functions for Rhai Scripts

use chrono::Duration;
use rhai::{Engine, EvalAltResult};

/// Parse duration string to Duration object
/// Examples: "90m" -> 90 minutes, "2h" -> 2 hours, "7d" -> 7 days
pub fn parse_duration(s: &str) -> Result<Duration, Box<EvalAltResult>> {
    if s.is_empty() {
        return Err("Empty duration string".into());
    }

    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: i64 = num_str.parse()
        .map_err(|_| format!("Invalid number in duration: {}", s))?;

    let duration = match unit {
        "s" => Duration::seconds(num),
        "m" => Duration::minutes(num),
        "h" => Duration::hours(num),
        "d" => Duration::days(num),
        _ => return Err(format!("Invalid duration unit: {}", unit).into()),
    };

    Ok(duration)
}

/// Register duration helper functions into Rhai engine
pub fn register_duration_helpers(engine: &mut Engine) {
    // Register duration() constructor
    engine.register_fn("duration", parse_duration);

    // Register Duration methods
    engine.register_fn("num_minutes", |d: &mut Duration| d.num_minutes());
    engine.register_fn("num_seconds", |d: &mut Duration| d.num_seconds());
    engine.register_fn("num_hours", |d: &mut Duration| d.num_hours());
    engine.register_fn("num_days", |d: &mut Duration| d.num_days());

    // Register comparison operators for Duration
    engine.register_fn(">", |lhs: Duration, rhs: Duration| lhs > rhs);
    engine.register_fn("<", |lhs: Duration, rhs: Duration| lhs < rhs);
    engine.register_fn(">=", |lhs: Duration, rhs: Duration| lhs >= rhs);
    engine.register_fn("<=", |lhs: Duration, rhs: Duration| lhs <= rhs);
    engine.register_fn("==", |lhs: Duration, rhs: Duration| lhs == rhs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_minutes() {
        let dur = parse_duration("90m").unwrap();
        assert_eq!(dur.num_minutes(), 90);
    }

    #[test]
    fn test_parse_duration_hours() {
        let dur = parse_duration("2h").unwrap();
        assert_eq!(dur.num_hours(), 2);
    }

    #[test]
    fn test_parse_duration_days() {
        let dur = parse_duration("7d").unwrap();
        assert_eq!(dur.num_days(), 7);
    }

    #[test]
    fn test_parse_duration_seconds() {
        let dur = parse_duration("30s").unwrap();
        assert_eq!(dur.num_seconds(), 30);
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration("invalid").is_err());
        assert!(parse_duration("12x").is_err());
    }

    #[test]
    fn test_rhai_duration_function() {
        let mut engine = crate::daemon::dispatcher::scripting::create_sandboxed_engine();
        register_duration_helpers(&mut engine);

        // Test duration() function in Rhai
        let result: i64 = engine.eval("duration(\"90m\").num_minutes()").unwrap();
        assert_eq!(result, 90);
    }
}
