//! The `generation_providers.list_presets` row and the `generation_config.*`
//! settings body.
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

/// The body of `generation_config.get` and `generation_config.update`.
///
/// # Why this one is shared and the two hand copies are gone
///
/// The Panel declared `output_dir: String` while the server has always sent
/// `Option<String>`, so on any install that never set an output directory the
/// response was `null` and serde failed the **whole** object: all eight
/// settings vanished behind a bare `invalid type: null, expected a string`.
/// Not a missing field — an unloadable panel, from a mismatch in one field.
///
/// `None` means "unset — use the product default", which is the state a fresh
/// install is in and the only value the field had ever taken there. Whether an
/// empty string collapses to `None` is the server's call at the parse
/// boundary, so every client gets the same answer without having to know.
/// No `#[serde(default)]` on the `Option` fields: serde's derive already
/// accepts a missing one as `None`, so the attribute would read as if it were
/// load-bearing while a mutation removing it changes nothing. `output_dir`
/// keeps no `skip_serializing_if` either — an explicit `null` is how the server
/// says "unset", and omitting the key instead would make "the server has no
/// opinion" indistinguishable from "this server is too old to have the field".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_image_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_video_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_audio_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_speech_provider: Option<String>,
    /// Where generated media lands. `None` = unset.
    pub output_dir: Option<String>,
    pub auto_paste_threshold_mb: u32,
    pub background_task_threshold_seconds: u32,
    pub smart_routing_enabled: bool,
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

    fn settings() -> GenerationSettings {
        GenerationSettings {
            default_image_provider: None,
            default_video_provider: None,
            default_audio_provider: None,
            default_speech_provider: None,
            output_dir: None,
            auto_paste_threshold_mb: 5,
            background_task_threshold_seconds: 30,
            smart_routing_enabled: true,
        }
    }

    #[test]
    fn an_unset_output_dir_decodes_instead_of_failing_the_whole_body() {
        // The regression this type exists for: a `String` here made a fresh
        // install's response unparseable, and serde fails the object, not the
        // field — so every other setting went with it.
        let body = serde_json::json!({
            "output_dir": null,
            "auto_paste_threshold_mb": 5,
            "background_task_threshold_seconds": 30,
            "smart_routing_enabled": true,
        });
        let decoded: GenerationSettings = serde_json::from_value(body).expect("decode");
        assert_eq!(decoded, settings());
    }

    /// Tolerance a client can rely on, from serde's handling of `Option` — not
    /// from an attribute, which is why there is no `#[serde(default)]` to
    /// remove. Pinned as behaviour so a future change of the field's type is
    /// caught here rather than by a client that stops loading.
    #[test]
    fn an_omitted_output_dir_decodes_the_same_way_as_an_explicit_null() {
        let body = serde_json::json!({
            "auto_paste_threshold_mb": 5,
            "background_task_threshold_seconds": 30,
            "smart_routing_enabled": true,
        });
        let decoded: GenerationSettings = serde_json::from_value(body).expect("decode");
        assert_eq!(decoded.output_dir, None);
    }

    #[test]
    fn a_settings_body_round_trips() {
        let mut s = settings();
        s.output_dir = Some("/tmp/out".into());
        s.default_image_provider = Some("openai-dalle".into());
        let encoded = serde_json::to_value(&s).expect("encode");
        let decoded: GenerationSettings = serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded, s);
    }
}
