//! The canonical `EffectVerification` verdict DTO.
//!
//! Moved here from `alephcore::browser::backend` in P0 Task 8 A2: the enum
//! and its pure `as_wire` rendering are contract vocabulary. The
//! engine-aware derivation (`for_engine`) stays in Core as the
//! `EffectVerificationExt` extension trait, because it consults Core's
//! measured capability table, which does not move. Core
//! `crate::browser::backend` publicly re-exports this enum, so existing
//! Core paths keep resolving.

use super::engine::Engine;

/// The effect-arrival verdict of one probed verb (click / type / fill),
/// stamped onto `browser_exec` step results as `effect_verification`.
///
/// The three states are three DIFFERENT facts, and collapsing any two of
/// them is a lie the model acts on (判据 §8):
///
/// * `Verified` — the one-shot listener installed before dispatch observed
///   the expected event at the target element. Earned by the read-back,
///   never asserted by a table.
/// * `Skipped(engine)` — no verdict exists and the label says so: the
///   engine's capability row holds the probe off, the action named a point
///   rather than a node, or the probe's own machinery failed. An honest
///   "I do not know", never a quiet success (「活性≠能干活」).
/// * `Failed` — the probe ran and the effect did not arrive. The action
///   itself already failed as Core's `BrowserError::EffectNotDelivered`; the
///   latch records the same verdict as data so a caller that caught the
///   error can still read it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", content = "engine", rename_all = "snake_case")]
pub enum EffectVerification {
    Verified,
    Skipped(Engine),
    Failed,
}

impl EffectVerification {
    /// The wire token carried in `browser_exec` step results:
    /// `"verified"` / `"skipped(<engine>)"` / `"failed"`.
    #[must_use]
    pub fn as_wire(&self) -> String {
        match self {
            Self::Verified => "verified".into(),
            Self::Skipped(engine) => format!("skipped({})", engine.as_str()),
            Self::Failed => "failed".into(),
        }
    }
}
