//! Embedding provider resolution.
//!
//! Maps the configured `active_provider_id` onto a concrete provider with a
//! **local-first `auto` mode** and a model-perceivable [`ResolutionReason`].
//!
//! Aleph's `EmbeddingProvider` is a remote OpenAI-compatible client, but the
//! Ollama preset points at `localhost` — i.e. the embedding never leaves the
//! machine (数据不出本机). `auto` exploits that: when the user has not pinned a
//! specific provider, the resolver prefers an enabled *local* provider and only
//! falls back to a remote one. Pinning `active_provider_id` to a concrete id
//! still wins (`也可按需切到 OpenAI / Ollama`).
//!
//! This layer is pure deterministic routing — no heavy dependency, no LLM
//! reasoning. It is infrastructure plumbing in the spirit of "Provider 多厂商"
//! (R7 赋能层), not intent classification.

use crate::config::types::memory::{EmbeddingPreset, EmbeddingProviderConfig, EmbeddingSettings};

/// Sentinel `active_provider_id` value that triggers `auto` resolution.
/// An empty id (the historical default) is treated the same way — both mean
/// "the user did not pin a provider".
pub const AUTO_SENTINEL: &str = "auto";

/// Where an embedding provider physically runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingLocality {
    /// Runs on the local machine (Ollama today; future in-process backends).
    /// Embedding inputs never leave the host — 数据不出本机.
    Local,
    /// Calls out to a remote API (OpenAI, SiliconFlow, or a custom endpoint).
    Remote,
}

impl EmbeddingLocality {
    /// Classify a preset by storage locality.
    #[must_use]
    pub const fn of(preset: &EmbeddingPreset) -> Self {
        match preset {
            EmbeddingPreset::Ollama => Self::Local,
            EmbeddingPreset::OpenAi | EmbeddingPreset::SiliconFlow | EmbeddingPreset::Custom => {
                Self::Remote
            }
        }
    }
}

/// Why the resolver picked (or failed to pick) a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionReason {
    /// `active_provider_id` matched an enabled provider exactly.
    ExactMatch,
    /// `auto` mode chose an enabled local provider (数据不出本机 preferred).
    AutoLocalFirst,
    /// `auto` mode found no enabled local provider; fell back to a remote one.
    AutoRemoteFallback,
    /// No usable provider — memory runs without semantic vectors (keyword-only).
    Unresolved,
}

impl ResolutionReason {
    /// Stable machine-readable tag (for structured logs / model-perceivable hints).
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ExactMatch => "exact_match",
            Self::AutoLocalFirst => "auto_local_first",
            Self::AutoRemoteFallback => "auto_remote_fallback",
            Self::Unresolved => "unresolved",
        }
    }
}

/// Outcome of resolving [`EmbeddingSettings`] into a concrete provider choice.
///
/// Borrows the chosen config from the settings it was resolved against, so the
/// caller clones what it needs before the settings lock is released.
#[derive(Debug)]
pub struct EmbeddingDecision<'a> {
    /// The `active_provider_id` as configured (verbatim, including `""`/`auto`).
    pub requested_id: String,
    /// The provider the resolver landed on, or `None` when unresolved.
    pub effective: Option<&'a EmbeddingProviderConfig>,
    /// Why this choice was made.
    pub reason: ResolutionReason,
}

/// True when `id` means "no explicit pin" — empty/whitespace or the `auto`
/// sentinel (case-insensitive).
fn is_auto(id: &str) -> bool {
    let trimmed = id.trim();
    trimmed.is_empty() || trimmed.eq_ignore_ascii_case(AUTO_SENTINEL)
}

/// Resolve embedding settings into a concrete provider decision.
///
/// Precedence:
/// 1. **Explicit pin** — a non-`auto` `active_provider_id` that matches an
///    enabled provider wins (`ExactMatch`). A pinned-but-missing/disabled id
///    stays `Unresolved` (no surprise auto-activation of a different backend).
/// 2. **Auto / unpinned** — prefer the first enabled *local* provider
///    (`AutoLocalFirst`), else the first enabled *remote* provider
///    (`AutoRemoteFallback`).
/// 3. Otherwise `Unresolved`.
#[must_use]
pub fn resolve(settings: &EmbeddingSettings) -> EmbeddingDecision<'_> {
    let requested = settings.active_provider_id.clone();

    // 1. Explicit pin → exact match against an enabled provider.
    if !is_auto(&requested) {
        let exact = settings
            .providers
            .iter()
            .find(|p| p.enabled && p.id == requested);
        return EmbeddingDecision {
            requested_id: requested,
            effective: exact,
            // Pinned-but-missing degrades to Unresolved rather than silently
            // swapping in an unrelated provider.
            reason: if exact.is_some() {
                ResolutionReason::ExactMatch
            } else {
                ResolutionReason::Unresolved
            },
        };
    }

    // 2. Auto / unpinned → local-first, then remote.
    if let Some(local) = settings
        .providers
        .iter()
        .find(|p| p.enabled && EmbeddingLocality::of(&p.preset) == EmbeddingLocality::Local)
    {
        return EmbeddingDecision {
            requested_id: requested,
            effective: Some(local),
            reason: ResolutionReason::AutoLocalFirst,
        };
    }
    if let Some(remote) = settings.providers.iter().find(|p| p.enabled) {
        return EmbeddingDecision {
            requested_id: requested,
            effective: Some(remote),
            reason: ResolutionReason::AutoRemoteFallback,
        };
    }

    // 3. Nothing usable.
    EmbeddingDecision {
        requested_id: requested,
        effective: None,
        reason: ResolutionReason::Unresolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(id: &str, preset: EmbeddingPreset, enabled: bool) -> EmbeddingProviderConfig {
        let mut c = match preset {
            EmbeddingPreset::Ollama => EmbeddingProviderConfig::ollama(),
            EmbeddingPreset::OpenAi => EmbeddingProviderConfig::openai(),
            _ => EmbeddingProviderConfig::siliconflow(),
        };
        c.id = id.to_string();
        c.enabled = enabled;
        c
    }

    fn settings(active: &str, providers: Vec<EmbeddingProviderConfig>) -> EmbeddingSettings {
        EmbeddingSettings {
            providers,
            active_provider_id: active.to_string(),
        }
    }

    #[test]
    fn locality_classifies_ollama_as_local() {
        assert_eq!(
            EmbeddingLocality::of(&EmbeddingPreset::Ollama),
            EmbeddingLocality::Local
        );
        assert_eq!(
            EmbeddingLocality::of(&EmbeddingPreset::OpenAi),
            EmbeddingLocality::Remote
        );
        assert_eq!(
            EmbeddingLocality::of(&EmbeddingPreset::SiliconFlow),
            EmbeddingLocality::Remote
        );
    }

    #[test]
    fn explicit_pin_matches_enabled_provider() {
        let s = settings(
            "openai",
            vec![
                cfg("ollama", EmbeddingPreset::Ollama, true),
                cfg("openai", EmbeddingPreset::OpenAi, true),
            ],
        );
        let d = resolve(&s);
        assert_eq!(d.reason, ResolutionReason::ExactMatch);
        assert_eq!(d.effective.unwrap().id, "openai");
    }

    #[test]
    fn pinned_but_missing_stays_unresolved() {
        // No auto fall-through: a typo'd or disabled pin must not silently
        // activate a different backend.
        let s = settings("ghost", vec![cfg("ollama", EmbeddingPreset::Ollama, true)]);
        let d = resolve(&s);
        assert_eq!(d.reason, ResolutionReason::Unresolved);
        assert!(d.effective.is_none());
    }

    #[test]
    fn pinned_but_disabled_stays_unresolved() {
        let s = settings(
            "openai",
            vec![cfg("openai", EmbeddingPreset::OpenAi, false)],
        );
        let d = resolve(&s);
        assert_eq!(d.reason, ResolutionReason::Unresolved);
    }

    #[test]
    fn auto_prefers_local_over_remote() {
        // Remote listed first, but auto must still pick the local one.
        let s = settings(
            "auto",
            vec![
                cfg("openai", EmbeddingPreset::OpenAi, true),
                cfg("ollama", EmbeddingPreset::Ollama, true),
            ],
        );
        let d = resolve(&s);
        assert_eq!(d.reason, ResolutionReason::AutoLocalFirst);
        assert_eq!(d.effective.unwrap().id, "ollama");
    }

    #[test]
    fn auto_falls_back_to_remote_when_no_local() {
        let s = settings("auto", vec![cfg("openai", EmbeddingPreset::OpenAi, true)]);
        let d = resolve(&s);
        assert_eq!(d.reason, ResolutionReason::AutoRemoteFallback);
        assert_eq!(d.effective.unwrap().id, "openai");
    }

    #[test]
    fn empty_active_id_is_treated_as_auto() {
        let s = settings("", vec![cfg("ollama", EmbeddingPreset::Ollama, true)]);
        let d = resolve(&s);
        assert_eq!(d.reason, ResolutionReason::AutoLocalFirst);
        assert_eq!(d.effective.unwrap().id, "ollama");
    }

    #[test]
    fn default_empty_settings_resolve_to_unresolved() {
        // The shipped default (no providers, empty id) must stay byte-identical
        // to the old behaviour: no embeddings.
        let settings = EmbeddingSettings::default();
        let d = resolve(&settings);
        assert_eq!(d.reason, ResolutionReason::Unresolved);
        assert!(d.effective.is_none());
    }

    #[test]
    fn auto_skips_disabled_local_for_enabled_remote() {
        let s = settings(
            "auto",
            vec![
                cfg("ollama", EmbeddingPreset::Ollama, false),
                cfg("openai", EmbeddingPreset::OpenAi, true),
            ],
        );
        let d = resolve(&s);
        assert_eq!(d.reason, ResolutionReason::AutoRemoteFallback);
        assert_eq!(d.effective.unwrap().id, "openai");
    }
}
