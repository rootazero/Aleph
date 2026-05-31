//! Runtime metadata footer appended to the final agent reply.
//!
//! Ported from hermes-agent `gateway/runtime_footer.py`. Off by default;
//! when enabled, renders a compact `model · tokens · cwd` line that the
//! final emit step pastes onto the buffer just before the message is sent.
//!
//! Adapter notes vs hermes:
//! - hermes exposes `model`, `context_pct`, `cwd`. Aleph's `RunSummary`
//!   does not carry `context_length` so this port renders `tokens` (an
//!   absolute count) in its place — providers report total_tokens via
//!   the `RunComplete` enriched summary, which is always available.
//! - hermes per-platform overrides live under `display.platforms.*`.
//!   Aleph keeps a flat config knob (`gateway.runtime_footer`) for now;
//!   per-channel overrides can be added when there's a real consumer.

use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Visible separator between footer fields.
pub const SEPARATOR: &str = " · ";

/// Default field order when none is configured.
pub const DEFAULT_FIELDS: &[&str] = &["model", "tokens", "cwd"];

/// Runtime-footer configuration. Disabled by default to keep replies minimal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
}

/// Strip a `vendor/` prefix for readability (`anthropic/claude-opus-4-7` →
/// `claude-opus-4-7`). Returns the input unchanged when no slash is present.
fn model_short(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
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
                    parts.push(format!("{}t", t));
                }
            }
            "cwd" => {
                if let Some(c) = inputs.cwd.filter(|s| !s.is_empty()) {
                    parts.push(home_relative_cwd(c, home));
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
static GLOBAL_FOOTER_CONFIG: OnceLock<RuntimeFooterConfig> = OnceLock::new();

/// Install the global footer configuration. Idempotent — only the first
/// caller wins, subsequent calls are silently ignored. Call this from
/// `aleph-server::start` once `FullGatewayConfig` is loaded.
pub fn set_global_config(cfg: RuntimeFooterConfig) {
    let _ = GLOBAL_FOOTER_CONFIG.set(cfg);
}

/// Read the global footer configuration. Returns a disabled config when
/// nothing has been installed yet (test/boot-without-server paths).
pub fn global_config() -> RuntimeFooterConfig {
    GLOBAL_FOOTER_CONFIG.get().cloned().unwrap_or_default()
}

/// Top-level entry. Returns the rendered footer (with the conventional
/// double-newline separator already attached) or an empty string when
/// disabled / no data.
pub fn build_footer_block(
    cfg: &RuntimeFooterConfig,
    inputs: &RuntimeFooterInputs<'_>,
    home: Option<&str>,
) -> String {
    if !cfg.enabled {
        return String::new();
    }
    match build_footer_line(inputs, &cfg.fields, home) {
        Some(line) => format!("\n\n{}", line),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            },
            &fields(&["model", "tokens", "cwd"]),
            Some("/Users/zoe"),
        )
        .expect("renders");
        assert_eq!(line, "claude-opus-4-7 · 2048t · ~/work");
    }

    #[test]
    fn missing_fields_are_skipped_silently() {
        let line = build_footer_line(
            &RuntimeFooterInputs {
                model: None,
                total_tokens: Some(99),
                cwd: Some("/tmp"),
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
}
