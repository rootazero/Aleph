//! Pure formatting / parsing helpers shared by the cron view components.

// ============================================================================
// Helper Functions
// ============================================================================

/// Format a schedule into a human-readable summary.
/// Prefers the structured `schedule_kind` JSON object when available,
/// falling back to legacy `kind` + `schedule` string fields.
pub(super) fn format_schedule_summary(
    schedule_kind_obj: &Option<serde_json::Value>,
    kind: &str,
    schedule: &str,
) -> String {
    // Try structured schedule_kind JSON first
    if let Some(obj) = schedule_kind_obj {
        if let Some(k) = obj.get("kind").and_then(|v| v.as_str()) {
            match k {
                "every" => {
                    if let Some(ms) = obj.get("every_ms").and_then(serde_json::Value::as_u64) {
                        return format_ms_interval(ms);
                    }
                }
                "cron" => {
                    // Backend serializes as "expr", panel historically used "expression"
                    if let Some(expr) = obj
                        .get("expr")
                        .or_else(|| obj.get("expression"))
                        .and_then(|v| v.as_str())
                    {
                        return expr.to_string();
                    }
                }
                "at" => {
                    if let Some(dt) = obj.get("datetime").and_then(|v| v.as_str()) {
                        return format!("At {dt}");
                    }
                    // Backend stores ms; convert to seconds for display
                    if let Some(ts_ms) = obj
                        .get("at")
                        .or_else(|| obj.get("at_ms"))
                        .and_then(serde_json::Value::as_i64)
                    {
                        return format!("At {}", format_timestamp(ts_ms / 1000));
                    }
                }
                _ => {}
            }
        }
    }

    // Fallback to legacy string fields
    match kind {
        "every" => {
            let trimmed = schedule.trim();
            if let Some(rest) = trimmed.strip_suffix('m') {
                format!("Every {rest}min")
            } else if let Some(rest) = trimmed.strip_suffix('h') {
                format!("Every {rest}h")
            } else if let Some(rest) = trimmed.strip_suffix('s') {
                format!("Every {rest}s")
            } else {
                format!("Every {trimmed}")
            }
        }
        "at" => format!("At {schedule}"),
        _ => schedule.to_string(),
    }
}

/// Format milliseconds into a human-readable interval string.
pub(super) fn format_ms_interval(ms: u64) -> String {
    if ms < 60_000 {
        format!("Every {}s", ms / 1000)
    } else if ms < 3_600_000 {
        format!("Every {}min", ms / 60_000)
    } else if ms < 86_400_000 {
        format!("Every {}h", ms / 3_600_000)
    } else {
        format!("Every {}d", ms / 86_400_000)
    }
}

/// Format a UNIX timestamp (seconds) as a relative time string.
/// e.g. "5min", "2h", "3d", or the provided `overdue_label`.
pub(super) fn format_relative_time(ts: i64, overdue_label: &str) -> String {
    let now_ms = js_sys::Date::now();
    let now_s = (now_ms / 1000.0) as i64;
    let diff = ts - now_s;

    if diff < 0 {
        return overdue_label.to_string();
    }

    let minutes = diff / 60;
    let hours = diff / 3600;
    let days = diff / 86400;

    if minutes < 1 {
        format!("{diff}s")
    } else if hours < 1 {
        format!("{minutes}min")
    } else if days < 1 {
        format!("{hours}h")
    } else {
        format!("{days}d")
    }
}

/// Format a UNIX timestamp (seconds) as "MM/DD HH:MM".
pub(super) fn format_timestamp(ts: i64) -> String {
    let date = js_sys::Date::new_0();
    date.set_time((ts * 1000) as f64);

    let month = date.get_month() + 1; // 0-indexed
    let day = date.get_date();
    let hours = date.get_hours();
    let minutes = date.get_minutes();

    format!("{month:02}/{day:02} {hours:02}:{minutes:02}")
}

/// Format a duration in milliseconds to a human-readable string.
/// e.g. "200ms", "1.5s", "2.1min".
pub(super) fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{:.1}min", ms as f64 / 60_000.0)
    }
}

/// Parse a string into an optional i64, returning None if empty or invalid.
pub(super) fn parse_optional_i64(s: &str) -> Option<i64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        trimmed.parse().ok()
    }
}

/// Extract schedule type and value from the `schedule_kind` JSON object returned by backend.
///
/// Backend returns tagged enums like:
///   `{"kind":"cron","expr":"0 0 11 * * *","tz":null,"stagger_ms":null}`
///   `{"kind":"every","every_ms":3600000,"anchor_ms":null}`
///   `{"kind":"at","at":1711944000000,"delete_after_run":true}`
///
/// Returns (`kind_str`, `schedule_str`, `anchor_ms_str`, `stagger_ms_str`).
pub(super) fn extract_schedule_from_kind(
    schedule_kind: &Option<serde_json::Value>,
) -> (String, String, Option<String>, Option<String>) {
    let Some(obj) = schedule_kind else {
        return ("cron".to_string(), String::new(), None, None);
    };
    let kind = obj.get("kind").and_then(|v| v.as_str()).unwrap_or("cron");
    match kind {
        "cron" => {
            let expr = obj.get("expr").and_then(|v| v.as_str()).unwrap_or("");
            let stagger = obj
                .get("stagger_ms")
                .and_then(serde_json::Value::as_i64)
                .map(|v| v.to_string());
            (kind.to_string(), expr.to_string(), None, stagger)
        }
        "every" => {
            let every_ms = obj
                .get("every_ms")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let anchor = obj
                .get("anchor_ms")
                .and_then(serde_json::Value::as_i64)
                .map(|v| v.to_string());
            (kind.to_string(), every_ms.to_string(), anchor, None)
        }
        "at" => {
            // Backend stores ms; convert to local datetime string for the form
            let at_ms = obj
                .get("at")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let dt_str = ms_to_datetime_local(at_ms);
            (kind.to_string(), dt_str, None, None)
        }
        _ => ("cron".to_string(), String::new(), None, None),
    }
}

/// Build a `schedule_kind` JSON object from form fields.
/// Returns the tagged JSON like `{"kind":"every","every_ms":60000}`.
pub(super) fn build_schedule_kind_json(
    kind: &str,
    schedule: &str,
    anchor_ms_str: &str,
    stagger_ms_str: &str,
) -> Option<serde_json::Value> {
    match kind {
        "every" => {
            // Try to parse schedule as milliseconds directly, or as interval string
            let every_ms = parse_interval_to_ms(schedule)?;
            let mut obj = serde_json::json!({
                "kind": "every",
                "every_ms": every_ms,
            });
            if let Some(anchor) = parse_optional_i64(anchor_ms_str) {
                obj["anchor_ms"] = serde_json::json!(anchor);
            }
            Some(obj)
        }
        "cron" => {
            let mut obj = serde_json::json!({
                "kind": "cron",
                "expr": schedule,
            });
            if let Some(stagger) = parse_optional_i64(stagger_ms_str) {
                obj["stagger_ms"] = serde_json::json!(stagger);
            }
            Some(obj)
        }
        "at" => {
            // User enters local datetime (YYYY-MM-DDTHH:MM); convert to epoch ms
            let at_ms = datetime_local_to_ms(schedule.trim())?;
            Some(serde_json::json!({
                "kind": "at",
                "at": at_ms,
                "delete_after_run": true,
            }))
        }
        _ => None,
    }
}

/// Convert epoch milliseconds to a `YYYY-MM-DDTHH:MM` string in local timezone.
/// This format is used by `<input type="datetime-local">`.
pub(super) fn ms_to_datetime_local(ms: i64) -> String {
    let date = js_sys::Date::new_0();
    date.set_time(ms as f64);
    let y = date.get_full_year();
    let m = date.get_month() + 1;
    let d = date.get_date();
    let hh = date.get_hours();
    let mm = date.get_minutes();
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}")
}

/// Convert a `YYYY-MM-DDTHH:MM` local datetime string to epoch milliseconds.
pub(super) fn datetime_local_to_ms(s: &str) -> Option<i64> {
    // <input type="datetime-local"> produces "YYYY-MM-DDTHH:MM"
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(s));
    let ms = date.get_time();
    if ms.is_nan() {
        None
    } else {
        Some(ms as i64)
    }
}

/// Parse a human interval string (e.g. "5m", "2h", "30s") to milliseconds.
pub(super) fn parse_interval_to_ms(s: &str) -> Option<u64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Try direct number (assume ms)
    if let Ok(ms) = trimmed.parse::<u64>() {
        return Some(ms);
    }
    if let Some(rest) = trimmed.strip_suffix('s') {
        rest.parse::<u64>().ok().and_then(|v| v.checked_mul(1000))
    } else if let Some(rest) = trimmed.strip_suffix('m') {
        rest.parse::<u64>().ok().and_then(|v| v.checked_mul(60_000))
    } else if let Some(rest) = trimmed.strip_suffix('h') {
        rest.parse::<u64>()
            .ok()
            .and_then(|v| v.checked_mul(3_600_000))
    } else if let Some(rest) = trimmed.strip_suffix('d') {
        rest.parse::<u64>()
            .ok()
            .and_then(|v| v.checked_mul(86_400_000))
    } else {
        None
    }
}

/// Build the `failure_alert` payload from the form fields.
///
/// The keys emitted here (`after` / `cooldown_ms` / `target.kind` / `url` /
/// `channel` / `chat_id`) are the backend's own — the server is the single
/// source for this shape. The previous form invented `after_n` / `cooldown` /
/// `kind` / `channel`, which shared *zero* field names with what the handler
/// parses, so the editor reported success and stored nothing.
///
/// Returns `None` when the target is not filled in, which the handler reads as
/// "leave the existing config alone".
pub(super) fn build_failure_alert_json(
    target_kind: &str,
    endpoint: &str,
    chat_id: &str,
    after: &str,
    cooldown_ms: &str,
) -> Option<serde_json::Value> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return None;
    }
    let target = match target_kind {
        "Webhook" => serde_json::json!({ "kind": "Webhook", "url": endpoint }),
        _ => {
            let chat_id = chat_id.trim();
            if chat_id.is_empty() {
                // A Gateway target without a conversation cannot be delivered;
                // emitting it anyway would be rejected by the server, so treat
                // a half-filled form as "not configured".
                return None;
            }
            serde_json::json!({ "kind": "Gateway", "channel": endpoint, "chat_id": chat_id })
        }
    };
    Some(serde_json::json!({
        "after": after.trim().parse::<u32>().unwrap_or(2),
        "cooldown_ms": cooldown_ms.trim().parse::<i64>().unwrap_or(3_600_000),
        "target": target,
    }))
}

/// Returns the id to render as a "(deleted)" placeholder option when the job's
/// currently-bound agent is no longer in the available list. `None` when the
/// current id is empty or still present.
pub(super) fn stale_agent_option(
    current: &str,
    available: &[crate::api::agents::AgentSummary],
) -> Option<String> {
    if current.is_empty() || available.iter().any(|a| a.id == current) {
        None
    } else {
        Some(current.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::agents::AgentSummary;

    fn agent(id: &str) -> AgentSummary {
        AgentSummary {
            id: id.to_string(),
            name: Some(id.to_string()),
            emoji: None,
            description: None,
            model: None,
            is_default: id == "main",
        }
    }

    #[test]
    fn stale_option_none_when_current_in_list() {
        let list = vec![agent("main"), agent("research")];
        assert_eq!(stale_agent_option("research", &list), None);
    }

    #[test]
    fn stale_option_none_when_current_empty() {
        let list = vec![agent("main")];
        assert_eq!(stale_agent_option("", &list), None);
    }

    #[test]
    fn stale_option_some_when_current_deleted() {
        let list = vec![agent("main")];
        assert_eq!(stale_agent_option("gone", &list), Some("gone".to_string()));
    }

    /// The payload must use the server's field names. Asserting on the exact
    /// keys is the point: the previous form's keys parsed cleanly as JSON and
    /// were discarded wholesale by the handler.
    #[test]
    fn webhook_alert_uses_backend_field_names() {
        let v = build_failure_alert_json("Webhook", "https://example.com/hook", "", "3", "60000")
            .expect("a filled-in webhook target must produce a payload");
        assert_eq!(v["after"], 3);
        assert_eq!(v["cooldown_ms"], 60000);
        assert_eq!(v["target"]["kind"], "Webhook");
        assert_eq!(v["target"]["url"], "https://example.com/hook");
        assert!(v.get("after_n").is_none(), "legacy key must be gone");
        assert!(v.get("channel").is_none(), "legacy key must be gone");
    }

    #[test]
    fn gateway_alert_carries_channel_and_chat_id() {
        let v = build_failure_alert_json("Gateway", "telegram", "12345", "2", "3600000").unwrap();
        assert_eq!(v["target"]["kind"], "Gateway");
        assert_eq!(v["target"]["channel"], "telegram");
        assert_eq!(v["target"]["chat_id"], "12345");
    }

    /// A Gateway target with no conversation cannot be delivered anywhere, so
    /// a half-filled form must not masquerade as a configured alert.
    #[test]
    fn incomplete_target_yields_no_payload() {
        assert!(build_failure_alert_json("Gateway", "telegram", "  ", "2", "3600000").is_none());
        assert!(build_failure_alert_json("Webhook", "   ", "", "2", "3600000").is_none());
    }

    #[test]
    fn unparseable_numbers_fall_back_to_the_backend_defaults() {
        let v = build_failure_alert_json("Webhook", "https://x", "", "", "abc").unwrap();
        assert_eq!(v["after"], 2);
        assert_eq!(v["cooldown_ms"], 3_600_000);
    }
}
