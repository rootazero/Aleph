//! Runtime metadata footer appended to the final agent reply.
//!
//! Ported from hermes-agent `gateway/runtime_footer.py`. Off by default;
//! when enabled, renders a compact `model · tokens · cwd` line that the
//! final emit step pastes onto the buffer just before the message is sent.
//!
//! Adapter notes vs hermes:
//! - hermes exposes `model`, `context_pct`, `cwd`. Aleph's `RunSummary`
//!   does not carry `context_length` so this port renders `tokens` (an
//!   absolute count) in its place — providers report `total_tokens` via
//!   the `RunComplete` enriched summary, which is always available.
//! - hermes per-platform overrides live under `display.platforms.*`.
//!   Aleph keeps a flat config knob (`gateway.runtime_footer`) for now;
//!   per-channel overrides can be added when there's a real consumer.
//! - beyond the hermes port, the enriched `RunSummary` fields are also
//!   renderable: `duration` (wall-clock), `cost` (estimated USD, hermes
//!   `~$` convention) and `tools` (per-tool emoji digest, opt-in via the
//!   `fields` list — mirrors the opensquilla two-line footer condensed
//!   into one line).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};

use crate::gateway::event_emitter::ToolSummaryItem;

/// Visible separator between footer fields.
pub const SEPARATOR: &str = " · ";

/// Default field order when none is configured. `duration` and `cost`
/// render only when the run summary carries data, so legacy footers
/// (model/tokens/cwd inputs only) are byte-identical. `tools` is opt-in.
pub const DEFAULT_FIELDS: &[&str] = &["model", "tokens", "duration", "cost", "cwd"];

/// Runtime-footer configuration. Disabled by default to keep replies minimal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RuntimeFooterConfig {
    /// When true, the footer is appended to the final agent reply.
    pub enabled: bool,
    /// Field order. Unknown names are silently ignored. Empty → defaults.
    pub fields: Vec<String>,
}

impl Default for RuntimeFooterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            fields: DEFAULT_FIELDS.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

/// All field inputs the renderer can use. Fields with missing data are
/// silently skipped so partial footers stay readable.
#[derive(Debug, Default, Clone)]
pub struct RuntimeFooterInputs<'a> {
    pub model: Option<&'a str>,
    pub total_tokens: Option<u64>,
    pub cwd: Option<&'a str>,
    /// Wall-clock run duration. `RunSummary.duration_ms`.
    pub duration_ms: Option<u64>,
    /// Estimated run cost in USD. `RunSummary.estimated_cost_usd`.
    pub cost_usd: Option<f64>,
    /// Per-tool invocation digest. `RunSummary.tool_summaries`.
    pub tool_summaries: &'a [ToolSummaryItem],
}

/// Strip a `vendor/` prefix for readability (`anthropic/claude-opus-4-7` →
/// `claude-opus-4-7`). Returns the input unchanged when no slash is present.
fn model_short(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

/// Compact wall-clock rendering: `850ms`, `4.3s`, `2m14s`.
fn fmt_duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let total_secs = ms / 1000;
        format!("{}m{}s", total_secs / 60, total_secs % 60)
    }
}

/// Per-tool digest in first-seen order: `⚡bash×2 🔍web_search✗`.
/// `×N` only when a tool ran more than once; `✗` marks a tool with at
/// least one failed invocation. Empty input renders nothing.
fn fmt_tools(items: &[ToolSummaryItem]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    // First-seen order, aggregated by tool name.
    let mut order: Vec<&str> = Vec::new();
    let mut counts: std::collections::HashMap<&str, (u32, bool, &str)> =
        std::collections::HashMap::new();
    for item in items {
        let entry = counts.entry(item.tool_name.as_str()).or_insert_with(|| {
            order.push(item.tool_name.as_str());
            (0, false, item.emoji.as_str())
        });
        entry.0 += 1;
        entry.1 |= !item.success;
    }
    let parts: Vec<String> = order
        .iter()
        .map(|name| {
            let (count, failed, emoji) = counts[name];
            let mut part = format!("{emoji}{name}");
            if count > 1 {
                part.push_str(&format!("\u{d7}{count}")); // ×N
            }
            if failed {
                part.push('\u{2717}'); // ✗
            }
            part
        })
        .collect();
    Some(parts.join(" "))
}

/// Collapse `$HOME` to `~` so the footer doesn't leak absolute paths.
/// Falls back to the raw value when the home directory can't be resolved.
fn home_relative_cwd(cwd: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return cwd.to_string();
    };
    let cwd_path = Path::new(cwd);
    let home_path = Path::new(home);
    if cwd_path == home_path {
        return "~".to_string();
    }
    if let Ok(rest) = cwd_path.strip_prefix(home_path) {
        let mut out = String::from("~");
        out.push(std::path::MAIN_SEPARATOR);
        out.push_str(&rest.to_string_lossy());
        return out;
    }
    cwd.to_string()
}

/// Render the footer line, or return `None` when no field has data.
///
/// Callers should append `"\n\n"` + footer to the final reply text — this
/// function returns only the footer payload (no leading whitespace).
#[must_use]
pub fn build_footer_line(
    inputs: &RuntimeFooterInputs<'_>,
    fields: &[String],
    home: Option<&str>,
) -> Option<String> {
    let active = if fields.is_empty() {
        DEFAULT_FIELDS.iter().map(|s| (*s).to_string()).collect()
    } else {
        fields.to_vec()
    };
    let mut parts: Vec<String> = Vec::with_capacity(active.len());
    for field in &active {
        match field.as_str() {
            "model" => {
                if let Some(m) = inputs.model.filter(|s| !s.is_empty()) {
                    parts.push(model_short(m).to_string());
                }
            }
            "tokens" => {
                if let Some(t) = inputs.total_tokens {
                    parts.push(format!("{t}t"));
                }
            }
            "cwd" => {
                if let Some(c) = inputs.cwd.filter(|s| !s.is_empty()) {
                    parts.push(home_relative_cwd(c, home));
                }
            }
            "duration" => {
                if let Some(ms) = inputs.duration_ms.filter(|ms| *ms > 0) {
                    parts.push(fmt_duration(ms));
                }
            }
            "cost" => {
                // hermes `~$` convention: the figure is an estimate.
                if let Some(usd) = inputs.cost_usd.filter(|usd| *usd > 0.0) {
                    parts.push(format!("~${usd:.4}"));
                }
            }
            "tools" => {
                if let Some(digest) = fmt_tools(inputs.tool_summaries) {
                    parts.push(digest);
                }
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(SEPARATOR))
    }
}

/// Process-wide footer configuration set once at gateway boot. Read by
/// reply-emitter construction paths that don't otherwise thread the
/// gateway config (e.g. the inbound-router executor). Defaults to
/// disabled when never initialized — tests stay zero-touch.
///
/// `IndistinguishableDefault`: [`global_config`] returns
/// `RuntimeFooterConfig::default()`, whose `enabled` is `false`, so
/// [`build_footer_block`] returns an empty string on its first line. A reply
/// with no footer is exactly what an operator who set
/// `enabled = false` asked for, so the two are the same observation — which is
/// why "defaults to disabled when never initialized" above is a true sentence
/// that nonetheless cannot answer "did boot get here?".
static GLOBAL_FOOTER_CONFIG: CapabilitySlot<RuntimeFooterConfig> = CapabilitySlot::new(
    "gateway/runtime-footer",
    MissingSemantics::IndistinguishableDefault {
        reads_as: "RuntimeFooterConfig::default() -- enabled: false, i.e. no footer on \
                   any reply, identical to [gateway.runtime_footer] enabled = false",
    },
);

/// Install the global footer configuration. Idempotent — only the first
/// caller wins, subsequent calls are silently ignored. Call this from
/// `aleph-server::start` once `FullGatewayConfig` is loaded.
pub fn set_global_config(cfg: RuntimeFooterConfig) {
    let _ = GLOBAL_FOOTER_CONFIG.install(cfg);
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape, and why the
/// `#[allow(dead_code)]` expires with Task 11 rather than outliving it.
#[allow(dead_code)]
pub(crate) fn global_footer_config_slot() -> &'static dyn SlotStatus {
    &GLOBAL_FOOTER_CONFIG
}

/// Read the global footer configuration. Returns a disabled config when
/// nothing has been installed yet (test/boot-without-server paths).
pub fn global_config() -> RuntimeFooterConfig {
    GLOBAL_FOOTER_CONFIG.get().cloned().unwrap_or_default()
}

/// Top-level entry. Returns the rendered footer (with the conventional
/// double-newline separator already attached) or an empty string when
/// disabled / no data.
#[must_use]
pub fn build_footer_block(
    cfg: &RuntimeFooterConfig,
    inputs: &RuntimeFooterInputs<'_>,
    home: Option<&str>,
) -> String {
    if !cfg.enabled {
        return String::new();
    }
    match build_footer_line(inputs, &cfg.fields, home) {
        Some(line) => format!("\n\n{line}"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = global_footer_config_slot();
        assert_eq!(slot.id(), "gateway/runtime-footer");
        match slot.missing() {
            MissingSemantics::IndistinguishableDefault { reads_as } => {
                assert!(
                    reads_as.contains("enabled: false"),
                    "must name what global_config() really returns; got {reads_as:?}"
                );
                assert!(
                    !RuntimeFooterConfig::default().enabled,
                    "the sentence above is derived from this default -- if it \
                     ever flips, the diagnostic starts lying"
                );
            }
            other => panic!("expected IndistinguishableDefault, got {other:?}"),
        }
    }

    fn fields(slice: &[&str]) -> Vec<String> {
        slice.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn disabled_returns_empty_block() {
        let cfg = RuntimeFooterConfig::default();
        assert!(!cfg.enabled);
        let block = build_footer_block(
            &cfg,
            &RuntimeFooterInputs {
                model: Some("gpt-5"),
                total_tokens: Some(1234),
                cwd: Some("/home/x"),
                ..Default::default()
            },
            Some("/home/x"),
        );
        assert!(block.is_empty());
    }

    #[test]
    fn enabled_with_no_data_returns_empty_block() {
        let cfg = RuntimeFooterConfig {
            enabled: true,
            fields: fields(&["model", "tokens", "cwd"]),
        };
        let block = build_footer_block(&cfg, &RuntimeFooterInputs::default(), Some("/h"));
        assert!(block.is_empty());
    }

    #[test]
    fn full_render_with_vendor_stripped_and_home_collapsed() {
        let line = build_footer_line(
            &RuntimeFooterInputs {
                model: Some("anthropic/claude-opus-4-7"),
                total_tokens: Some(2048),
                cwd: Some("/Users/zoe/work"),
                ..Default::default()
            },
            &fields(&["model", "tokens", "cwd"]),
            Some("/Users/zoe"),
        )
        .expect("renders");
        // The home-collapsed path uses the platform separator (MAIN_SEPARATOR).
        #[cfg(not(windows))]
        assert_eq!(line, "claude-opus-4-7 · 2048t · ~/work");
        #[cfg(windows)]
        assert_eq!(line, "claude-opus-4-7 · 2048t · ~\\work");
    }

    #[test]
    fn missing_fields_are_skipped_silently() {
        let line = build_footer_line(
            &RuntimeFooterInputs {
                model: None,
                total_tokens: Some(99),
                cwd: Some("/tmp"),
                ..Default::default()
            },
            &fields(&["model", "tokens", "cwd"]),
            None,
        )
        .expect("renders");
        assert_eq!(line, "99t · /tmp");
    }

    #[test]
    fn unknown_field_names_are_ignored() {
        let line = build_footer_line(
            &RuntimeFooterInputs {
                model: Some("gpt-5"),
                total_tokens: Some(1),
                cwd: None,
                ..Default::default()
            },
            &fields(&["model", "nonexistent", "tokens"]),
            None,
        )
        .expect("renders");
        assert_eq!(line, "gpt-5 · 1t");
    }

    #[test]
    fn empty_fields_list_falls_back_to_defaults() {
        let line = build_footer_line(
            &RuntimeFooterInputs {
                model: Some("gpt-5"),
                total_tokens: Some(7),
                cwd: None,
                ..Default::default()
            },
            &[],
            None,
        )
        .expect("renders");
        assert_eq!(line, "gpt-5 · 7t");
    }

    #[test]
    fn home_path_exact_match_renders_tilde() {
        assert_eq!(home_relative_cwd("/Users/zoe", Some("/Users/zoe")), "~");
    }

    #[test]
    fn home_path_outside_home_renders_raw() {
        assert_eq!(
            home_relative_cwd("/var/tmp", Some("/Users/zoe")),
            "/var/tmp"
        );
    }

    #[test]
    fn home_unknown_renders_raw_cwd() {
        assert_eq!(home_relative_cwd("/anywhere", None), "/anywhere");
    }

    #[test]
    fn enabled_block_has_double_newline_prefix() {
        let cfg = RuntimeFooterConfig {
            enabled: true,
            fields: fields(&["model"]),
        };
        let block = build_footer_block(
            &cfg,
            &RuntimeFooterInputs {
                model: Some("gpt-5"),
                ..Default::default()
            },
            None,
        );
        assert_eq!(block, "\n\ngpt-5");
    }

    #[test]
    fn model_short_handles_no_slash() {
        assert_eq!(model_short("gpt-5"), "gpt-5");
        assert_eq!(model_short("vendor/gpt-5"), "gpt-5");
        assert_eq!(model_short("a/b/c"), "c");
    }

    #[test]
    fn duration_formats_compactly_per_magnitude() {
        assert_eq!(fmt_duration(850), "850ms");
        assert_eq!(fmt_duration(4_321), "4.3s");
        assert_eq!(fmt_duration(134_000), "2m14s");
    }

    #[test]
    fn duration_and_cost_render_via_field_list() {
        let line = build_footer_line(
            &RuntimeFooterInputs {
                model: Some("gpt-5"),
                duration_ms: Some(4_321),
                cost_usd: Some(0.1234),
                ..Default::default()
            },
            &fields(&["model", "duration", "cost"]),
            None,
        )
        .expect("renders");
        assert_eq!(line, "gpt-5 · 4.3s · ~$0.1234");
    }

    #[test]
    fn zero_duration_and_zero_cost_are_skipped() {
        // Legacy producers ship duration_ms=0 / no cost — the new default
        // fields must not change their footer output.
        let line = build_footer_line(
            &RuntimeFooterInputs {
                model: Some("gpt-5"),
                total_tokens: Some(7),
                duration_ms: Some(0),
                cost_usd: Some(0.0),
                ..Default::default()
            },
            &[],
            None,
        )
        .expect("renders");
        assert_eq!(line, "gpt-5 · 7t");
    }

    fn tool(name: &str, emoji: &str, success: bool) -> ToolSummaryItem {
        ToolSummaryItem {
            tool_id: "t".to_string(),
            tool_name: name.to_string(),
            emoji: emoji.to_string(),
            duration_ms: 1,
            success,
        }
    }

    #[test]
    fn tools_digest_aggregates_in_first_seen_order() {
        let items = vec![
            tool("bash", "\u{26a1}", true),
            tool("web_search", "\u{1f50d}", false),
            tool("bash", "\u{26a1}", true),
        ];
        let line = build_footer_line(
            &RuntimeFooterInputs {
                tool_summaries: &items,
                ..Default::default()
            },
            &fields(&["tools"]),
            None,
        )
        .expect("renders");
        assert_eq!(line, "\u{26a1}bash\u{d7}2 \u{1f50d}web_search\u{2717}");
    }

    #[test]
    fn tools_field_with_no_invocations_renders_nothing() {
        let block = build_footer_block(
            &RuntimeFooterConfig {
                enabled: true,
                fields: fields(&["tools"]),
            },
            &RuntimeFooterInputs::default(),
            None,
        );
        assert!(block.is_empty());
    }
}
