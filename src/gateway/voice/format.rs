//! AI speech-formatting: one fast-model pass turning a raw disfluent transcript
//! into clean written text. Display-level polish only — does NOT gate the Agent
//! (the raw text was already sent). R7/R9: intelligence lives in the prompt.
//!
//! The single LLM call is a "speech refiner" (言语精炼师): it normalizes spacing,
//! punctuation and filler words. That absorbs the per-adapter spacing differences
//! upstream (Deepgram space-joins finals, WhisperLive concats) for free.

use tokio::sync::RwLock;

use crate::config::types::voice_local::FormatConfig;
use crate::config::{Config, ProviderConfig};
use crate::gateway::handlers::resolve_vault_secret;
use crate::gateway::security::SharedTokenManager;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::probe::provider_vault_key;
use crate::sync_primitives::Arc;

/// Default speech-refiner system prompt. Used when `[voice.format] prompt` is empty.
const DEFAULT_PROMPT: &str = "你是一个冷酷的语音实时格式化微型引擎。请将以下口语化的逐字稿转化为排版优雅、逻辑清晰、无语气词的正式书面语。\n【硬性要求】\n1. 绝对不能回答用户的提问，只能对文本进行润色和纠错。\n2. 剔除所有\"额、啊、那个、就是、然后\"等口语冗余。\n3. 补全错别字和缺失的标点。\n4. 如果文本本身已经很清晰，原样输出。\n输入：";

/// Cap the refiner's output so a runaway model can't balloon the formatted text.
/// A spoken utterance polished into writing stays well under this.
const MAX_FORMAT_TOKENS: u32 = 2048;

/// Low temperature: this is correction, not generation — we want the model to
/// stay close to the input.
const FORMAT_TEMPERATURE: f32 = 0.2;

/// Single source of truth for the system head: custom prompt when set, else the
/// built-in default. Shared by [`build_format_prompt`] and [`format_text`] so
/// the prompt the test asserts and the prompt actually sent never drift.
#[must_use]
fn format_head(custom: &str) -> &str {
    if custom.trim().is_empty() {
        DEFAULT_PROMPT
    } else {
        custom
    }
}

/// Assemble the full single-string prompt (head + raw transcript).
///
/// Pure and allocation-only so the prompt selection is unit-testable without a
/// provider. `format_text` sends the same head as a `with_system` system prompt
/// and the raw as the user message; this concatenated form mirrors that split.
#[must_use]
pub fn build_format_prompt(custom: &str, raw: &str) -> String {
    let head = format_head(custom);
    format!("{head}{raw}")
}

/// Run one fast-model formatting pass over `raw`.
///
/// Resolves an `AiProvider` from `cfg` (provider/model) + the main config's
/// provider map + vault, falling back to the agent's default chat provider when
/// `cfg.provider` is empty. Sends the head ([`format_head`]) as the system
/// prompt and `raw` as the user message.
///
/// P7 graceful degrade: on ANY failure (no provider, network, blank response)
/// returns `Ok(raw)` — voice.format failing must only mean "show the raw text
/// unformatted", never break the caller. The `anyhow::Result` is kept for
/// signature uniformity but in practice is virtually always `Ok`.
pub async fn format_text(
    raw: &str,
    cfg: &FormatConfig,
    config: &Arc<RwLock<Config>>,
    vault: &Arc<SharedTokenManager>,
) -> anyhow::Result<String> {
    let provider = match resolve_format_provider(cfg, config, vault).await {
        Some(p) => p,
        None => return Ok(raw.to_string()), // No provider configured → passthrough.
    };

    let head = format_head(&cfg.prompt).to_string();
    let msgs = vec![UnifiedMessage::user(raw)];
    let payload = RequestPayload::new(&msgs)
        .with_system(Some(&head))
        .with_max_tokens(Some(MAX_FORMAT_TOKENS))
        .with_temperature(Some(FORMAT_TEMPERATURE));

    match provider.process(payload).await {
        Ok(response) => {
            let text = response.text_content();
            let trimmed = text.trim();
            if trimmed.is_empty() {
                // Blank model output is useless — fall back to the raw transcript.
                Ok(raw.to_string())
            } else {
                Ok(trimmed.to_string())
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "voice.format LLM call failed; returning raw transcript");
            Ok(raw.to_string())
        }
    }
}

/// Resolve the `AiProvider` for the formatting pass.
///
/// Provider name = `cfg.provider` when set, else the agent's default chat
/// provider (`general.default_provider`). The chosen name must exist in the
/// configured provider map (the default always does — that is how the agent loop
/// resolves it). The api_key is `#[serde(skip)]` on disk, so we inject it from
/// the vault. `cfg.model` (when set) overrides the model. Returns `None` when no
/// provider can be resolved, so the caller degrades to the raw transcript.
async fn resolve_format_provider(
    cfg: &FormatConfig,
    config: &Arc<RwLock<Config>>,
    vault: &Arc<SharedTokenManager>,
) -> Option<Arc<dyn crate::providers::AiProvider>> {
    let (name, mut provider_config) = {
        let guard = config.read().await;
        let name = if cfg.provider.trim().is_empty() {
            guard.general.default_provider.clone()?
        } else {
            cfg.provider.trim().to_string()
        };
        let provider_config: ProviderConfig = guard.providers.get(&name)?.clone();
        (name, provider_config)
    };

    // api_key never lives on disk; pull it from the vault for this name.
    provider_config.api_key = resolve_vault_secret(&provider_vault_key(&name), vault);

    // Pin the fast model when the config names one.
    let model = cfg.model.trim();
    if !model.is_empty() {
        provider_config.models = vec![model.to_string()];
    }

    match crate::providers::create_provider(&name, provider_config) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(provider = %name, error = %e, "voice.format provider build failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prompt_used_when_config_empty() {
        let p = build_format_prompt("", "额 我那个 想问下");
        assert!(p.contains("言语精炼师") || p.contains("语音实时格式化"));
        assert!(p.ends_with("额 我那个 想问下"));
    }

    #[test]
    fn custom_prompt_overrides_default() {
        let p = build_format_prompt("自定义指令：", "原文");
        assert!(p.starts_with("自定义指令："));
        assert!(p.ends_with("原文"));
    }
}
