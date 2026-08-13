//! The `generation_providers.list_presets` row.
//!
//! # Why this is a contract type and not a `json!` literal
//!
//! The chat family learned this the expensive way: four crates each kept a
//! hand copy of the `providers.*` shapes and two of the copies were broken from
//! the day they were written — a listing whose every row printed dashes, and a
//! command that had never once been accepted by a server. The fix was to put
//! the shape somewhere both sides compile against.
//!
//! The generation family had the same arrangement with one client instead of
//! three, and it had the same defect: the server has always sent `signup_url`
//! for all 44 presets, the Panel's DTO never declared the field, and serde
//! drops unknown keys without a word. So the one page whose whole job is
//! "you have not linked this vendor yet" could not show you where to get a key
//! — on a page where that is the only action available.
//!
//! Building the response *from* this type rather than parsing into it is the
//! other half: parsing only proves the response is a superset, so a field the
//! server over-sends is structurally invisible in that direction.

use serde::{Deserialize, Serialize};

use super::search::Searchable;

/// One built-in generation preset, as `generation_providers.list_presets`
/// sends it.
///
/// The response is a bare array of these — no envelope, which is the shape
/// this RPC has always had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationPresetRow {
    /// Catalogue id, and the config key a configured preset takes.
    pub id: String,
    /// Which concrete provider implementation instantiates it.
    pub provider_type: String,
    /// The model a fresh setup starts from.
    pub default_model: String,
    /// `None` means "the provider SDK's own default" — not "unset".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub display_name: String,
    /// `image` / `video` / `music` / `speech` / `transcription`. Never empty:
    /// a preset that declares no modality is not listed at all.
    pub modalities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Where to get an API key. The field this type exists to keep attached:
    /// it is the only actionable thing on an unconfigured row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signup_url: Option<String>,
}

impl Searchable for GenerationPresetRow {
    fn search_id(&self) -> &str {
        &self.id
    }
    fn search_display_name(&self) -> &str {
        &self.display_name
    }
    // No aliases: unlike chat presets, generation ids are not resolution keys
    // with vendor nicknames attached.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> GenerationPresetRow {
        GenerationPresetRow {
            id: "openai-dalle".into(),
            provider_type: "openai".into(),
            default_model: "dall-e-3".into(),
            base_url: Some("https://api.openai.com".into()),
            display_name: "OpenAI DALL-E".into(),
            modalities: vec!["image".into()],
            homepage: None,
            notes: None,
            signup_url: Some("https://platform.openai.com/api-keys".into()),
        }
    }

    #[test]
    fn signup_url_survives_a_round_trip() {
        // The whole point of the type. A DTO without the field parses this
        // payload perfectly happily and simply loses the link.
        let encoded = serde_json::to_value(row()).expect("encode");
        assert_eq!(
            encoded.get("signup_url").and_then(|v| v.as_str()),
            Some("https://platform.openai.com/api-keys")
        );
        let decoded: GenerationPresetRow = serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded, row());
    }

    #[test]
    fn absent_optionals_are_omitted_rather_than_null() {
        let mut r = row();
        r.base_url = None;
        r.signup_url = None;
        let encoded = serde_json::to_value(&r).expect("encode");
        let obj = encoded.as_object().expect("object");
        assert!(!obj.contains_key("base_url"));
        assert!(!obj.contains_key("signup_url"));
        // …and a row missing them still decodes.
        let decoded: GenerationPresetRow = serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded, r);
    }

    #[test]
    fn a_preset_grid_ranks_through_the_shared_matcher() {
        use super::super::search::{rank_rows, MatchRank};
        let rows = vec![row()];
        let ranked = rank_rows(&rows, "dall-e");
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].rank, MatchRank::DisplayName);
    }
}
