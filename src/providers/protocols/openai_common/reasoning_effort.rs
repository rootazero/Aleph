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
        // `xhigh` and `max` are the SAME rung spelled two ways: OpenAI (and
        // Anthropic 4.7+) call the top level `xhigh`, Kimi (and Anthropic 4.6)
        // call it `max`. Sharing an ordinal is what lets a requested `xhigh`
        // land exactly on `max` for a family that only spells it that way —
        // with distinct ordinals it would tie with `high` at distance 1 and
        // the tie-break would take the cheaper rung, leaving `max` a value no
        // request could ever reach.
        //
        // No pre-existing family list contains `max`, so this widens nothing
        // that was already in use; `map_think_level` never emits it either.
        "xhigh" | "max" => Some(5),
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
    let split = s.len() - 11;
    // `s` is a provider/config-supplied model id. The `-YYYY-MM-DD` suffix is
    // pure ASCII, so a non-char-boundary at `split` means the tail contains a
    // multi-byte char and cannot match the pattern. Bail instead of slicing on
    // a non-boundary, which would panic.
    if !s.is_char_boundary(split) {
        return None;
    }
    let tail = &s[split..];
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
    Some(&s[..split])
}

/// Parse the minor version `n` from `gpt-5.<n>` ids (`None` for plain `gpt-5`).
fn minor_of(id: &str) -> Option<u32> {
    let rest = id.strip_prefix("gpt-5.")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Drop an aggregator's routing prefix (`moonshotai/kimi-k3` → `kimi-k3`).
///
/// [`normalize_for_family`] mirrors openclaw and deliberately does not peel
/// vendor tags, but the Moonshot rows below match on prefixes — without this,
/// `moonshotai/kimi-k3` would be caught by the `moonshot` catch-all and lose
/// the effort it gets when the same model is reached directly.
fn strip_routing_prefix(id: &str) -> &str {
    match id.rfind('/') {
        Some(i) => &id[i + 1..],
        None => id,
    }
}

/// True for the Kimi K3 generation under either of its two id spellings: the
/// open platform serves `kimi-k3`, the Kimi Code subscription endpoint serves
/// bare `k3` / `k3-256k`.
fn is_kimi_k3(id: &str) -> bool {
    id == "k3" || id.starts_with("k3-") || id.starts_with("kimi-k3")
}

/// True for any other Moonshot / Kimi id. Used to keep the vendor **fail-closed**:
/// an id this table has never heard of (a future `kimi-k4`) sends no effort at
/// all rather than inheriting the generic ladder and 400ing.
fn is_other_moonshot(id: &str) -> bool {
    id.starts_with("kimi") || id.starts_with("moonshot") || id == "k2p5"
}

/// Supported `reasoning_effort` values for the given model's family.
///
/// Returns the generic ladder for non-gpt-5 reasoning models. The tables track
/// openclaw's family matrix.
///
/// An **empty** slice means "this model takes no `reasoning_effort` field at
/// all" — [`clamp_effort`] then returns `None` and the caller omits the field.
/// That is the model-granularity twin of the endpoint-level
/// `supports_reasoning_effort` strip.
#[must_use]
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
    // Kimi K3 publishes exactly three levels (`low` / `high` / `max`, default
    // `max`). This is deliberately the INTERSECTION of what the two Kimi
    // endpoints accept: the Kimi Code endpoint also maps `medium`/`ultra`/
    // `xhigh`/`minimum`/`light`, but the open platform documents only these
    // three and 400s on anything unmapped.
    //
    // `none` is excluded on purpose, and it is the important one: on Kimi,
    // "thinking off" is not a setting, it is a MODEL SWAP — the vendor docs
    // state that disabling thinking reroutes the request to K2.6. So a
    // `ThinkLevel::Off` clamps up to `low` (thinking on, cheapest rung, still
    // K3) rather than silently answering from a different model.
    const KIMI_K3: &[&str] = &["low", "high", "max"];
    // "Accepts no effort field." K2.x has a thinking mode but not this knob,
    // and `moonshot-v1` has no reasoning at all.
    const NO_EFFORT: &[&str] = &[];

    let id = normalize_for_family(model);

    // Exact specialized ids first.
    match id.as_str() {
        "gpt-5.1-codex-mini" => return CODEX_MINI,
        "gpt-5.1-codex-max" => return CODEX_MAX,
        "gpt-5-pro" => return GPT_5_PRO,
        _ => {}
    }

    // ── Moonshot / Kimi ──────────────────────────────────────────────────
    // Must precede the generic fallthrough: the endpoint-level gate is now
    // open for this vendor (the endpoint *does* understand the field), so this
    // table is the only thing standing between a K2.6 request and a 400.
    let bare = strip_routing_prefix(&id);
    if is_kimi_k3(bare) {
        return KIMI_K3;
    }
    if is_other_moonshot(bare) {
        return NO_EFFORT;
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
/// exposes `none/low/…` clamps up to `low`, not down to `none`).
///
/// Returns `None` in two cases, both of which mean "omit the field": the token
/// is not a recognized effort, or the family accepts no effort at all
/// (empty [`supported_efforts`] — see the Moonshot rows there).
#[must_use]
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
        .min_by_key(|(_, o)| ((i16::from(*o) - i16::from(want)).unsigned_abs(), *o))
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

    /// K3 publishes `low`/`high`/`max`. The top rung must be *reachable* — a
    /// value in the supported list that no request can produce is the same
    /// defect as not supporting it at all.
    #[test]
    fn kimi_k3_reaches_max_and_clamps_the_rest() {
        for id in ["k3", "k3-256k", "kimi-k3", "Kimi-K3"] {
            assert_eq!(
                clamp_effort(id, "xhigh").as_deref(),
                Some("max"),
                "{id}: xhigh is Kimi's `max` under another spelling"
            );
            assert_eq!(clamp_effort(id, "high").as_deref(), Some("high"));
            assert_eq!(clamp_effort(id, "low").as_deref(), Some("low"));
            // `medium` is not published on the open platform; ties break to
            // the cheaper rung.
            assert_eq!(clamp_effort(id, "medium").as_deref(), Some("low"));
            assert_eq!(clamp_effort(id, "minimal").as_deref(), Some("low"));
        }
    }

    /// On Kimi, "thinking off" is a MODEL SWAP: the vendor reroutes a
    /// thinking-disabled K3 request to K2.6. Aleph must never emit `none`
    /// here — the user asked for cheap, not for a different model.
    #[test]
    fn kimi_k3_never_emits_none() {
        for id in ["k3", "k3-256k", "kimi-k3"] {
            let got = clamp_effort(id, "none");
            assert_eq!(
                got.as_deref(),
                Some("low"),
                "{id}: `none` must clamp up to the cheapest thinking rung"
            );
            assert!(!supported_efforts(id).contains(&"none"));
        }
    }

    /// The endpoint gate is open for this vendor now, so this table is the
    /// only thing keeping the field off models that would 400 on it —
    /// including ids it has never heard of.
    #[test]
    fn non_k3_moonshot_models_take_no_effort_field() {
        for id in [
            "kimi-k2.6",
            "kimi-k2.7-code",
            "kimi-for-coding",
            "kimi-for-coding-highspeed",
            "kimi-latest",
            "moonshot-v1-128k",
            "k2p5",
            // Fail-closed: a generation this table predates must not inherit
            // the generic ladder.
            "kimi-k4",
        ] {
            assert!(
                supported_efforts(id).is_empty(),
                "{id} should accept no reasoning_effort"
            );
            assert_eq!(
                clamp_effort(id, "high"),
                None,
                "{id}: an empty family must omit the field, not pick a value"
            );
        }
    }

    /// Aggregator-hosted Kimi must land on the same row as the direct id.
    /// Without the routing-prefix strip, `moonshotai/kimi-k3` falls into the
    /// `moonshot` catch-all and loses an effort it used to get — a regression
    /// introduced by adding the catch-all, not by anything upstream.
    #[test]
    fn aggregator_routed_kimi_resolves_to_the_same_family() {
        assert_eq!(
            clamp_effort("moonshotai/kimi-k3", "xhigh").as_deref(),
            Some("max")
        );
        assert!(supported_efforts("moonshotai/kimi-k2.6").is_empty());
        // Non-Kimi aggregator ids are untouched by the strip.
        assert_eq!(
            clamp_effort("openai/gpt-5.2", "xhigh").as_deref(),
            Some("high"),
            "the prefix strip must not reroute non-Kimi ids into a gpt-5 row"
        );
    }

    /// `max` entering the ordinal ladder must not change any family that does
    /// not list it.
    #[test]
    fn adding_max_to_the_ladder_leaves_other_families_alone() {
        assert_eq!(clamp_effort("gpt-5", "xhigh").as_deref(), Some("high"));
        assert_eq!(clamp_effort("gpt-5.2", "xhigh").as_deref(), Some("xhigh"));
        assert_eq!(clamp_effort("o3", "xhigh").as_deref(), Some("high"));
        // `max` is now a recognized token, so a family without it clamps
        // rather than dropping the field.
        assert_eq!(clamp_effort("o3", "max").as_deref(), Some("high"));
    }
}
