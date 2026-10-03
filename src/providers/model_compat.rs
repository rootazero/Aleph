//! Per-model `ModelCompat` resolution helper.
//!
//! Adapters resolve the operator-declared [`ModelDef::compat`] for the model
//! they are about to call, and let each `Some` field override the adapter's
//! built-in heuristic. `None` (or no matching `ModelDef`) leaves the heuristic
//! untouched — request behaviour is byte-identical to before this wiring
//! existed.
//!
//! [`ModelDef::compat`]: crate::config::types::provider::ModelDef::compat

use crate::config::types::provider::{ModelCompat, ModelDef};

/// Resolve the [`ModelCompat`] for `model` from a provider's `model_defs`.
///
/// Matches on the exact [`ModelDef::id`] — the same join key
/// [`ModelRecord::resolve`] uses. Returns `None` when no `ModelDef` matches
/// `model`, or when the matched def declares no `compat` block.
///
/// [`ModelRecord::resolve`]: crate::providers::model_catalog::ModelRecord::resolve
pub fn model_compat_for<'a>(model_defs: &'a [ModelDef], model: &str) -> Option<&'a ModelCompat> {
    model_defs
        .iter()
        .find(|d| d.id == model)
        .and_then(|d| d.compat.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::provider::ModelDef;

    fn def(id: &str, with_compat: bool) -> ModelDef {
        ModelDef {
            id: id.to_string(),
            name: None,
            base_url: None,
            context_window: None,
            max_output_tokens: None,
            input: None,
            reasoning: None,
            thinking_level_map: None,
            cost: None,
            compat: with_compat.then(ModelCompat::default),
        }
    }

    #[test]
    fn no_model_def_returns_none() {
        assert!(model_compat_for(&[], "o3-mini").is_none());
        assert!(model_compat_for(&[def("gpt-4o", true)], "o3-mini").is_none());
    }

    #[test]
    fn model_def_without_compat_block_returns_none() {
        assert!(model_compat_for(&[def("o3-mini", false)], "o3-mini").is_none());
    }

    #[test]
    fn model_def_with_compat_block_is_found() {
        let defs = [def("o3-mini", true)];
        assert!(model_compat_for(&defs, "o3-mini").is_some());
    }
}
