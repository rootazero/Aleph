//! Shared serde helpers for config type deserialization

use serde::de;

/// Split a possibly comma-separated model string into individual model names.
///
/// The provider settings UI exposes a single model field but instructs users to
/// enter multiple models separated by commas (see the `model_hint` copy). A bare
/// name without commas yields a one-element vec; surrounding whitespace and empty
/// segments are dropped. Model names never contain commas, so this is unambiguous.
fn split_csv_models(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Deserializer for required models field.
/// Accepts both `model = "xxx"` (String) and `models = ["xxx", ...]` (Vec<String>).
/// A comma-separated string is split into multiple models.
/// Rejects empty lists and empty strings.
pub fn deserialize_models<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: de::Deserializer<'de>,
{
    struct ModelsVisitor;

    impl<'de> de::Visitor<'de> for ModelsVisitor {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or array of strings")
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Vec<String>, E> {
            let models = split_csv_models(value);
            if models.is_empty() {
                Err(E::custom("model name cannot be empty"))
            } else {
                Ok(models)
            }
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<String>, A::Error> {
            let mut models = Vec::new();
            while let Some(s) = seq.next_element::<String>()? {
                // An element may itself be a comma-joined string (legacy/tool
                // writes stored `["a,b,c"]`); fan it out so each model is distinct.
                models.extend(split_csv_models(&s));
            }
            if models.is_empty() {
                Err(de::Error::custom("models list cannot be empty"))
            } else {
                Ok(models)
            }
        }
    }

    deserializer.deserialize_any(ModelsVisitor)
}

/// Deserializer for the optional models field (used by
/// `GenerationProviderConfig`). Accepts String, Vec<String>, or null/missing;
/// an empty vec is a value, not an error.
///
/// Re-exported from `aleph_protocol` rather than implemented here. That same
/// field is parsed twice — out of `config.toml` by this crate, and out of a
/// JSON payload by `GenerationProviderConfigJson` — and the Panel is forbidden
/// from depending on `alephcore`, so a copy on this side would be a second
/// tolerance for one field: a value the file accepts and the wire rejects
/// (判据 §1). The tests below still exercise it through this path.
pub use aleph_protocol::providers::deserialize_optional_models;

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Deserialize)]
    struct Required {
        #[serde(deserialize_with = "deserialize_models", alias = "model")]
        models: Vec<String>,
    }

    #[derive(Deserialize)]
    struct Optional {
        #[serde(default, deserialize_with = "deserialize_optional_models")]
        models: Vec<String>,
    }

    #[test]
    fn single_model_string_yields_one_entry() {
        let r: Required = serde_json::from_value(json!({ "model": "gpt-5.4" })).unwrap();
        assert_eq!(r.models, vec!["gpt-5.4"]);
    }

    #[test]
    fn comma_separated_string_splits_into_multiple() {
        // The panel sends the single `model` field verbatim; the comma-separated
        // value must fan out so the picker renders one row per model.
        let r: Required =
            serde_json::from_value(json!({ "model": "gpt-5.4, gpt-5.3-codex, gpt-5.4-mini" }))
                .unwrap();
        assert_eq!(r.models, vec!["gpt-5.4", "gpt-5.3-codex", "gpt-5.4-mini"]);
    }

    #[test]
    fn array_form_still_supported() {
        let r: Required = serde_json::from_value(json!({ "models": ["a", " b ", "c"] })).unwrap();
        assert_eq!(r.models, vec!["a", "b", "c"]);
    }

    #[test]
    fn array_element_with_commas_fans_out() {
        // Legacy/tool-written configs stored a comma-joined value as a single
        // array element (e.g. `models = ["gpt-5.4,gpt-5.3-codex,gpt-5.4-mini"]`).
        // It must split so the picker renders one selectable row per model.
        let r: Required =
            serde_json::from_value(json!({ "models": ["gpt-5.4,gpt-5.3-codex,gpt-5.4-mini"] }))
                .unwrap();
        assert_eq!(r.models, vec!["gpt-5.4", "gpt-5.3-codex", "gpt-5.4-mini"]);
    }

    #[test]
    fn optional_array_element_with_commas_fans_out() {
        let o: Optional = serde_json::from_value(json!({ "models": ["a,b", "c"] })).unwrap();
        assert_eq!(o.models, vec!["a", "b", "c"]);
    }

    #[test]
    fn required_rejects_blank_and_comma_only() {
        assert!(serde_json::from_value::<Required>(json!({ "model": "" })).is_err());
        assert!(serde_json::from_value::<Required>(json!({ "model": "  ,  " })).is_err());
    }

    #[test]
    fn optional_comma_string_splits() {
        let o: Optional = serde_json::from_value(json!({ "models": "x, y" })).unwrap();
        assert_eq!(o.models, vec!["x", "y"]);
    }

    #[test]
    fn optional_empty_string_is_empty_vec() {
        let o: Optional = serde_json::from_value(json!({ "models": "" })).unwrap();
        assert!(o.models.is_empty());
    }
}
