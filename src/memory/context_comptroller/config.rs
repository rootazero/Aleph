use serde::{Deserialize, Serialize};

/// Configuration handle for [`ContextComptroller`].
///
/// All knobs previously listed here (`similarity_threshold`, `token_budget`,
/// `fold_threshold`) were dead data: the comptroller's [`arbitrate`] method
/// receives a per-call [`TokenBudget`] instead, and similarity/fold dedup was
/// never wired. The struct is kept as a serialized config handle so callers
/// (and downstream consumers of the serialized form) do not churn; once those
/// knobs are actually implemented, add them back here with `#[serde(default)]`
/// so older configs still deserialize cleanly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComptrollerConfig {}
