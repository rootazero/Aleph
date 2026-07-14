//! CPU twin of the GPU idle drift. Pure (no web-sys) so it unit-tests on native.
//!
//! Every node is displaced in the VERTEX SHADER by a per-axis sine
//! (`shaders::DRIFT_GLSL`), so the position the GPU renders is never the
//! canonical `GalaxyNode::pos`. Picking and the label overlays must project the
//! position the user actually SEES, otherwise clicks miss the star and labels
//! float off it (worse the closer the camera gets).
//!
//! ⚠ TWIN INVARIANT: this function must stay byte-for-byte equivalent to
//! `shaders::DRIFT_GLSL::idle_drift`. If the GLSL constants (amplitude, omega,
//! the two axis phase offsets) ever change, change them HERE in the same commit
//! — a divergence silently reintroduces the off-by-a-star picking bug.

use super::math::Vec3;

/// Drift amplitude per axis, in world units. Mirrors `DRIFT_AMP` in the GLSL.
pub const DRIFT_AMP: f32 = 3.0;

/// The GLSL spells this `6.28318530718`, which is exactly what `TAU` rounds to in
/// f32 — so the constant IS the mirror, not an approximation of one.
const DRIFT_TAU: f32 = std::f32::consts::TAU;
/// Angular frequency: one full cycle every 5000 ms. Mirrors `DRIFT_OMEGA`.
const DRIFT_OMEGA: f32 = DRIFT_TAU / 5.0;

/// The drift OFFSET applied to a node this frame (the GLSL adds it to `base`;
/// here the caller adds it to `node.pos`).
///
/// `phase` is the per-node phase in [0,1) from `nodes::node_phase`; `t_ms` is
/// the same wrapped `u_time` the shaders receive (`Scene::last_u_time`).
pub fn idle_drift(phase: f32, t_ms: f32) -> Vec3 {
    let t = t_ms / 1000.0;
    let ph = phase * DRIFT_TAU;
    Vec3::new(
        DRIFT_AMP * (DRIFT_OMEGA * t + ph).sin(),
        DRIFT_AMP * (DRIFT_OMEGA * t + ph + 0.27 * DRIFT_TAU).sin(),
        DRIFT_AMP * (DRIFT_OMEGA * t + ph + 0.54 * DRIFT_TAU).sin(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_drift_is_deterministic_and_bounded() {
        for phase in [0.0_f32, 0.13, 0.5, 0.99] {
            for t in [0.0_f32, 137.0, 2500.0, 4_999_999.0] {
                let d = idle_drift(phase, t);
                assert_eq!(d, idle_drift(phase, t), "not deterministic");
                for c in [d.x, d.y, d.z] {
                    assert!(
                        c.abs() <= DRIFT_AMP + 1e-4,
                        "component {c} exceeds amplitude"
                    );
                }
            }
        }
    }

    #[test]
    fn idle_drift_matches_hand_computed_values() {
        // t = 0, phase = 0 → sin(0), sin(0.27τ), sin(0.54τ).
        let d = idle_drift(0.0, 0.0);
        assert!(d.x.abs() < 1e-5, "x={}", d.x);
        assert!(
            (d.y - DRIFT_AMP * (0.27 * DRIFT_TAU).sin()).abs() < 1e-5,
            "y={}",
            d.y
        );
        assert!(
            (d.z - DRIFT_AMP * (0.54 * DRIFT_TAU).sin()).abs() < 1e-5,
            "z={}",
            d.z
        );
        // A quarter period (1250 ms) at phase 0 puts x at its positive peak.
        let q = idle_drift(0.0, 1250.0);
        assert!((q.x - DRIFT_AMP).abs() < 1e-3, "x={}", q.x);
    }

    #[test]
    fn idle_drift_is_periodic_over_5000ms() {
        // The 5,000,000 ms u_time wrap in Scene is a whole multiple of this
        // period, so the wrap cannot make the stars pop.
        let a = idle_drift(0.31, 1234.0);
        let b = idle_drift(0.31, 1234.0 + 5000.0);
        assert!((a.x - b.x).abs() < 1e-3);
        assert!((a.y - b.y).abs() < 1e-3);
        assert!((a.z - b.z).abs() < 1e-3);
    }

    #[test]
    fn idle_drift_is_continuous() {
        // Two adjacent frames may not jump: |Δ| is bounded by amp*omega*dt.
        let dt = 16.0_f32;
        let max_step = DRIFT_AMP * DRIFT_OMEGA * (dt / 1000.0) * 1.05;
        let a = idle_drift(0.7, 3000.0);
        let b = idle_drift(0.7, 3000.0 + dt);
        assert!(b.sub(&a).length() <= max_step * 3.0_f32.sqrt());
    }
}
