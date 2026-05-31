//! Per-model `reasoning_effort` clamp matrix.
//!
//! Ports openclaw's `openai-reasoning-effort.ts` family table to Rust. The
//! Chat (`map_think_level`) and Responses (`build_reasoning`) paths now emit
//! every effort level faithfully (`minimal`/`low`/`medium`/`high`/`xhigh`), but
//! not every model accepts every value: a generic reasoning model rejects
//! `minimal`/`xhigh`, and gpt-5 caps at `high`. Sending an unsupported value is
//! a hard `400`, so the requested effort is clamped to the nearest value the
//! target model's family actually supports.
//!
//! This is a capability constraint (what the API physically accepts), not a
//! reasoning decision — the same class of concern as the endpoint-level
//! `supports_reasoning_effort` strip in [`super::provider_policy`], just at
//! model-family granularity.

/// Ordinal ladder used for nearest-supported clamping.
///
/// `none` is included so families that can disable reasoning sort correctly,
/// even though the caller never *requests* `none` (`ThinkLevel::Off` omits the
/// field entirely upstream).
fn effort_ordinal(effort: &str) -> Option<u8> {
    match effort {
        "none" => Some(0),
        "minimal" => Some(1),
        "low" => Some(2),
        "medium" => Some(3),
        "high" => Some(4),
        "xhigh" => Some(5),
        _ => None,
    }
}

/// Normalize a model id for family matching: lowercase + strip a trailing
/// `-YYYY-MM-DD` snapshot suffix (mirrors openclaw's `normalizeModelId`).
fn normalize_for_family(model: &str) -> String {
    let lower = model.trim().to_ascii_lowercase();
    match strip_date_suffix(&lower) {
        Some(head) => head.to_string(),
        None => lower,
    }
}

/// Strip a trailing `-YYYY-MM-DD` snapshot suffix, returning the head when the
/// pattern matches (e.g. `gpt-5-2025-04-16` → `gpt-5`).
fn strip_date_suffix(s: &str) -> Option<&str> {
    if s.len() < 11 {
        return None;
    }
    let tail = &s[s.len() - 11..];
    // Pattern: '-' DDDD '-' DD '-' DD
    let pat = ['-', 'd', 'd', 'd', 'd', '-', 'd', 'd', '-', 'd', 'd'];
    let mut chars = tail.chars();
    for p in pat {
        let c = chars.next()?;
        let ok = match p {
            '-' => c == '-',
            _ => c.is_ascii_digit(),
        };
        if !ok {
            return None;
        }
    }
    Some(&s[..s.len() - 11])
}

/// Parse the minor version `n` from `gpt-5.<n>` ids (`None` for plain `gpt-5`).
fn minor_of(id: &str) -> Option<u32> {
    let rest = id.strip_prefix("gpt-5.")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Supported `reasoning_effort` values for the given model's family.
///
/// Returns the generic ladder for non-gpt-5 reasoning models. The tables track
/// openclaw's family matrix.
pub fn supported_efforts(model: &str) -> &'static [&'static str] {
    const GPT_5: &[&str] = &["minimal", "low", "medium", "high"];
    const GPT_51: &[&str] = &["none", "low", "medium", "high"];
    const GPT_52: &[&str] = &["none", "low", "medium", "high", "xhigh"];
    const CODEX: &[&str] = &["low", "medium", "high", "xhigh"];
    const PRO: &[&str] = &["medium", "high", "xhigh"];
    const GPT_5_PRO: &[&str] = &["high"];
    const CODEX_MAX: &[&str] = &["none", "medium", "high", "xhigh"];
    const CODEX_MINI: &[&str] = &["medium"];
    const GENERIC: &[&str] = &["low", "medium", "high"];

    let id = normalize_for_family(model);

    // Exact specialized ids first.
    match id.as_str() {
        "gpt-5.1-codex-mini" => return CODEX_MINI,
        "gpt-5.1-codex-max" => return CODEX_MAX,
        "gpt-5-pro" => return GPT_5_PRO,
        _ => {}
    }

    // Anything outside the gpt-5 family uses the generic ladder.
    if !id.starts_with("gpt-5") {
        return GENERIC;
    }

    if id.contains("-codex") {
        return CODEX;
    }

    match minor_of(&id) {
        Some(n) if n >= 2 => {
            if id.contains("-pro") {
                PRO
            } else {
                GPT_52
            }
        }
        Some(1) => GPT_51,
        // Plain `gpt-5` / `gpt-5-mini` / `gpt-5-*` (non-codex, non-pro).
        _ => GPT_5,
    }
}

/// Clamp `requested` to the nearest effort the model's family supports.
///
/// When the requested value is supported it is returned unchanged; otherwise
/// the nearest supported value by ordinal distance is chosen, ties breaking
/// toward the lower (cheaper) effort. `"none"` is never a clamp target — a
/// caller asking for an active effort wants reasoning *on*, so collapsing it to
/// the disabled state would defeat the request (e.g. `minimal` on a family that
/// exposes `none/low/…` clamps up to `low`, not down to `none`). Returns `None`
/// only when `requested` is not a recognized effort token.
pub fn clamp_effort(model: &str, requested: &str) -> Option<String> {
    let supported = supported_efforts(model);
    if supported.contains(&requested) {
        return Some(requested.to_string());
    }
    let want = effort_ordinal(requested)?;
    supported
        .iter()
        .filter(|s| **s != "none")
        .filter_map(|s| effort_ordinal(s).map(|o| (s, o)))
        // Tie-break toward the lower effort by ordering on (distance, ordinal).
        .min_by_key(|(_, o)| ((*o as i16 - want as i16).unsigned_abs(), *o))
        .map(|(s, _)| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt5_family_caps_xhigh_to_high_and_keeps_minimal() {
        assert_eq!(clamp_effort("gpt-5", "xhigh").as_deref(), Some("high"));
        assert_eq!(clamp_effort("gpt-5", "minimal").as_deref(), Some("minimal"));
        assert_eq!(clamp_effort("gpt-5", "medium").as_deref(), Some("medium"));
    }

    #[test]
    fn generic_model_clamps_minimal_up_and_xhigh_down() {
        // Generic reasoning model supports only low/medium/high.
        assert_eq!(clamp_effort("o3", "minimal").as_deref(), Some("low"));
        assert_eq!(clamp_effort("o3", "xhigh").as_deref(), Some("high"));
        assert_eq!(clamp_effort("o4-mini", "low").as_deref(), Some("low"));
    }

    #[test]
    fn gpt52_supports_xhigh() {
        assert_eq!(clamp_effort("gpt-5.2", "xhigh").as_deref(), Some("xhigh"));
        assert_eq!(clamp_effort("gpt-5.2", "minimal").as_deref(), Some("low"));
    }

    #[test]
    fn gpt51_clamps_minimal_to_low() {
        // gpt-5.1: none/low/medium/high — minimal not supported.
        assert_eq!(clamp_effort("gpt-5.1", "minimal").as_deref(), Some("low"));
        assert_eq!(clamp_effort("gpt-5.1", "xhigh").as_deref(), Some("high"));
    }

    #[test]
    fn codex_supports_xhigh_not_minimal() {
        assert_eq!(
            clamp_effort("gpt-5-codex", "xhigh").as_deref(),
            Some("xhigh")
        );
        assert_eq!(
            clamp_effort("gpt-5-codex", "minimal").as_deref(),
            Some("low")
        );
    }

    #[test]
    fn gpt5_pro_pins_to_high() {
        assert_eq!(
            clamp_effort("gpt-5-pro", "minimal").as_deref(),
            Some("high")
        );
        assert_eq!(clamp_effort("gpt-5-pro", "low").as_deref(), Some("high"));
        assert_eq!(clamp_effort("gpt-5-pro", "xhigh").as_deref(), Some("high"));
    }

    #[test]
    fn codex_mini_pins_to_medium() {
        assert_eq!(
            clamp_effort("gpt-5.1-codex-mini", "high").as_deref(),
            Some("medium")
        );
        assert_eq!(
            clamp_effort("gpt-5.1-codex-mini", "low").as_deref(),
            Some("medium")
        );
    }

    #[test]
    fn date_snapshot_suffix_is_stripped() {
        assert_eq!(
            clamp_effort("gpt-5-2025-04-16", "xhigh").as_deref(),
            Some("high")
        );
    }

    #[test]
    fn supported_value_passes_through_unchanged() {
        assert_eq!(clamp_effort("gpt-5.2", "high").as_deref(), Some("high"));
        assert_eq!(clamp_effort("o3", "medium").as_deref(), Some("medium"));
    }

    #[test]
    fn unknown_effort_token_returns_none() {
        assert_eq!(clamp_effort("gpt-5", "bogus"), None);
    }
}
