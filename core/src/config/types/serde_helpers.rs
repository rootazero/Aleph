//! Shared serde helpers for config type deserialization

use serde::de;

/// Deserializer for required models field.
/// Accepts both `model = "xxx"` (String) and `models = ["xxx", ...]` (Vec<String>).
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
            if value.is_empty() {
                Err(E::custom("model name cannot be empty"))
            } else {
                Ok(vec![value.to_string()])
            }
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<String>, A::Error> {
            let mut models = Vec::new();
            while let Some(s) = seq.next_element::<String>()? {
                let trimmed = s.trim().to_string();
                if !trimmed.is_empty() {
                    models.push(trimmed);
                }
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

/// Deserializer for optional models field (used by GenerationProviderConfig).
/// Accepts String, Vec<String>, or null/missing. Empty vec is valid.
pub fn deserialize_optional_models<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: de::Deserializer<'de>,
{
    struct OptionalModelsVisitor;

    impl<'de> de::Visitor<'de> for OptionalModelsVisitor {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string, array of strings, or null")
        }

        fn visit_none<E: de::Error>(self) -> Result<Vec<String>, E> {
            Ok(Vec::new())
        }

        fn visit_unit<E: de::Error>(self) -> Result<Vec<String>, E> {
            Ok(Vec::new())
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Vec<String>, E> {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(Vec::new())
            } else {
                Ok(vec![trimmed.to_string()])
            }
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<String>, A::Error> {
            let mut models = Vec::new();
            while let Some(s) = seq.next_element::<String>()? {
                let trimmed = s.trim().to_string();
                if !trimmed.is_empty() {
                    models.push(trimmed);
                }
            }
            Ok(models)
        }
    }

    deserializer.deserialize_any(OptionalModelsVisitor)
}
