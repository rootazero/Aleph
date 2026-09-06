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

// ============================================================================
// generation_providers.list / .get / .create / .update
// ============================================================================

/// What a provider entry's `enabled` means when nobody wrote it down.
///
/// It lives here because two parsers have to answer identically: the TOML
/// shape in `alephcore` and the wire shape below. A wire type that defaulted
/// this to `false` on its own would disable every provider whose payload left
/// the key out — the same field, two tables, one of them wrong (判据 §1).
#[must_use]
pub const fn default_generation_enabled() -> bool {
    true
}

/// The swatch a provider entry takes when nothing sets one. Same reason.
pub const DEFAULT_GENERATION_COLOR: &str = "#808080";

#[must_use]
pub fn default_generation_color() -> String {
    DEFAULT_GENERATION_COLOR.to_string()
}

/// Per-provider generation defaults, as the wire carries them.
///
/// Every field is optional and omitted when unset: `None` means "this
/// provider has no opinion", which is not the same as any concrete value the
/// field could take.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GenerationDefaultsJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance_scale: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
}

/// A generation provider's configuration on the wire — what a client may set,
/// and what `generation_providers.list` / `.get` return.
///
/// # This is a lossy view, and that is load-bearing
///
/// `alephcore`'s `GenerationProviderConfig` is simultaneously the `config.toml`
/// shape and this response, and it carries fields no client can express —
/// `model_aliases` today, whatever gets added tomorrow. A server that rebuilt
/// its stored entry from this type alone would **erase** them on every save,
/// which is exactly what it did: the Panel's hand copy never declared
/// `model_aliases`, serde supplied an empty map without a word, and any edit to
/// an unrelated field (a colour, a timeout) dropped the operator's aliases to
/// disk. The chat family had the identical bug with `cache_retention` and it
/// only ever showed up on the bill.
///
/// So the contract is two-sided: this type says what a client can *set*, and
/// the server merges it onto the stored entry rather than replacing it. Adding
/// a field here without a UI is not the fix; the merge is.
///
/// `capabilities` is carried as the snake_case modality strings rather than an
/// enum because `alephcore` and the Panel each still own a `GenerationType`
/// with identical variants — a separate §1 instance this type deliberately
/// does not pretend to have closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationProviderConfigJson {
    pub provider_type: String,
    /// Client → server only. The server resolves keys from the vault and never
    /// sends one back, so a populated `api_key` in a *response* would be a bug,
    /// not a feature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Parsed through the SAME tolerance `config.toml` gets — a bare string, a
    /// comma-joined one, an array, or nothing at all. The strict sibling
    /// rejects an empty list, and the Panel's add-custom form legitimately
    /// sends one.
    #[serde(
        default,
        deserialize_with = "super::wire::deserialize_optional_models",
        alias = "model"
    )]
    pub models: Vec<String>,
    #[serde(default = "default_generation_enabled")]
    pub enabled: bool,
    #[serde(default = "default_generation_color")]
    pub color: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// `None` = the operator has not chosen one and the provider keeps the
    /// default it tuned for its own API. Omitting the key is how "unset"
    /// crosses the wire; a client that always sends a number cannot express it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub defaults: GenerationDefaultsJson,
    #[serde(default)]
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voices_url: Option<String>,
}

impl Default for GenerationProviderConfigJson {
    fn default() -> Self {
        Self {
            provider_type: String::new(),
            api_key: None,
            base_url: None,
            models: Vec::new(),
            enabled: default_generation_enabled(),
            color: default_generation_color(),
            capabilities: Vec::new(),
            timeout_seconds: None,
            defaults: GenerationDefaultsJson::default(),
            verified: false,
            edit_url: None,
            voices_url: None,
        }
    }
}

/// One row of `generation_providers.list`, and the body of `.get`.
///
/// `generation_type` and `has_api_key` used to be welded onto the serialized
/// entry with two `serde_json::Map::insert` calls after the fact. That is a
/// hand-weld on a post-serialization payload (判据 §7): nothing typed either
/// key, so a rename on the reading side had nowhere to go red.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationProviderRow {
    pub name: String,
    pub config: GenerationProviderConfigJson,
    /// Modality strings this provider is the default for.
    #[serde(default)]
    pub is_default_for: Vec<String>,
    /// The category the provider is filed under, from the typed map that holds
    /// it. `None` only on payloads older than the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_type: Option<String>,
    #[serde(default)]
    pub has_api_key: bool,
}

impl GenerationProviderRow {
    /// The modality this row belongs to: the server's filing, or the first
    /// declared capability for payloads that predate it.
    #[must_use]
    pub fn effective_modality(&self) -> Option<&str> {
        self.generation_type
            .as_deref()
            .or_else(|| self.config.capabilities.first().map(String::as_str))
    }
}

/// Custom generation providers rank through the same matcher as the presets.
///
/// Two lists on one page filtered by one box have to agree about what a query
/// means, or the box quietly does two different things depending on which half
/// of the page you are looking at. A custom provider has no separate display
/// name — the operator's chosen name is both, and returning it twice is honest
/// where inventing a second label would make the display-name tier fire on
/// rows that have none.
impl Searchable for GenerationProviderRow {
    fn search_id(&self) -> &str {
        &self.name
    }
    fn search_display_name(&self) -> &str {
        &self.name
    }
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

    /// A payload that never mentions `enabled` must not disable the provider.
    ///
    /// This is the trap a wire type walks into on its own: `#[serde(default)]`
    /// on a `bool` is `false`, while the config file this type mirrors has
    /// always defaulted it to `true`. Both parsers now read one function.
    #[test]
    fn an_omitted_enabled_still_means_enabled() {
        let cfg: GenerationProviderConfigJson =
            serde_json::from_value(serde_json::json!({ "provider_type": "openai" }))
                .expect("decode");
        assert!(cfg.enabled);
        assert_eq!(cfg.color, DEFAULT_GENERATION_COLOR);
    }

    /// "Unset" has to survive the wire as an ABSENT key. A client that always
    /// sends a number cannot say it, which is the whole reason the field is an
    /// `Option` on both sides.
    #[test]
    fn an_unset_timeout_is_omitted_rather_than_sent_as_a_number() {
        let cfg = GenerationProviderConfigJson {
            provider_type: "openai".into(),
            ..Default::default()
        };
        let encoded = serde_json::to_value(&cfg).expect("encode");
        assert!(!encoded
            .as_object()
            .expect("object")
            .contains_key("timeout_seconds"));
        let decoded: GenerationProviderConfigJson =
            serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded.timeout_seconds, None);

        let set = GenerationProviderConfigJson {
            timeout_seconds: Some(45),
            ..cfg
        };
        assert_eq!(
            serde_json::to_value(&set)
                .expect("encode")
                .get("timeout_seconds")
                .and_then(serde_json::Value::as_u64),
            Some(45)
        );
    }

    fn provider_row() -> GenerationProviderRow {
        GenerationProviderRow {
            name: "my-dalle".into(),
            config: GenerationProviderConfigJson {
                provider_type: "openai".into(),
                capabilities: vec!["image".into()],
                ..Default::default()
            },
            is_default_for: vec!["image".into()],
            generation_type: Some("image".into()),
            has_api_key: true,
        }
    }

    /// The two welded keys are fields now, so they round-trip like any other.
    #[test]
    fn a_provider_row_round_trips_including_the_two_formerly_welded_keys() {
        let encoded = serde_json::to_value(provider_row()).expect("encode");
        let obj = encoded.as_object().expect("object");
        assert_eq!(
            obj.get("generation_type").and_then(|v| v.as_str()),
            Some("image")
        );
        assert_eq!(
            obj.get("has_api_key").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        let decoded: GenerationProviderRow = serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded, provider_row());
    }

    #[test]
    fn the_servers_filing_outranks_the_capability_fallback() {
        let mut r = provider_row();
        r.generation_type = Some("speech".into());
        r.config.capabilities = vec!["image".into()];
        assert_eq!(r.effective_modality(), Some("speech"));

        r.generation_type = None;
        assert_eq!(r.effective_modality(), Some("image"));

        r.config.capabilities.clear();
        assert_eq!(r.effective_modality(), None);
    }

    /// The tolerance the wire had before this type existed, pinned.
    ///
    /// Each of these payloads reached the server through the config type's own
    /// deserializer when `generation_providers.update` still parsed straight
    /// into it. A DTO with a plain `Vec<String>` accepts none of them, and the
    /// empty-list case is the one a real form produces.
    #[test]
    fn the_models_field_keeps_every_shape_the_config_parser_accepts() {
        let cases: [(serde_json::Value, Vec<&str>); 5] = [
            (serde_json::json!({"models": []}), vec![]),
            (serde_json::json!({"models": null}), vec![]),
            (serde_json::json!({"models": "dall-e-3"}), vec!["dall-e-3"]),
            (
                serde_json::json!({"models": "a, b"}),
                vec!["a", "b"], // comma-joined, as legacy writes stored it
            ),
            (serde_json::json!({"model": "dall-e-2"}), vec!["dall-e-2"]),
        ];
        for (extra, want) in cases {
            let mut body = serde_json::json!({ "provider_type": "openai" });
            for (k, v) in extra.as_object().expect("object") {
                body[k] = v.clone();
            }
            let cfg: GenerationProviderConfigJson = serde_json::from_value(body.clone())
                .unwrap_or_else(|e| panic!("{body} should decode, got {e}"));
            assert_eq!(cfg.models, want, "for {body}");
        }
    }

    #[test]
    fn a_custom_provider_ranks_through_the_shared_matcher() {
        use super::super::search::{rank_rows, MatchRank};
        let rows = vec![provider_row()];
        let ranked = rank_rows(&rows, "dalle");
        assert_eq!(ranked.len(), 1);
        // Name is both id and display name, so a substring hit lands on the id
        // tier — not a second, invented label tier.
        assert_eq!(ranked[0].rank, MatchRank::IdSubstring);
    }
}
