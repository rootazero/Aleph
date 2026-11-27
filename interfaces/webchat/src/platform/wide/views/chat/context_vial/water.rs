//! Linearized shallow-water update with reflecting walls and fixed substeps.

pub(super) const SURFACE_POINTS: usize = 16;
const SUBSTEP: f64 = 1.0 / 120.0;
const MAX_STEPS: usize = 6;
const SPEED: f64 = 20.0;
const FLOW_DAMPING: f64 = 4.5;
const SETTLED: f64 = 0.000_02;

pub(super) struct Surface {
    height: [f64; SURFACE_POINTS],
    flow: [f64; SURFACE_POINTS - 1],
    remainder: f64,
}

impl Default for Surface {
    fn default() -> Self {
        Self {
            height: [0.0; SURFACE_POINTS],
            flow: [0.0; SURFACE_POINTS - 1],
            remainder: 0.0,
        }
    }
}

impl Surface {
    pub(super) const fn offsets(&self) -> &[f64; SURFACE_POINTS] {
        &self.height
    }

    pub(super) fn disturb(&mut self, amount: f64) {
        for (i, flux) in self.flow.iter_mut().enumerate() {
            let location = (i + 1) as f64 / SURFACE_POINTS as f64;
            *flux =
                (*flux + amount.clamp(-1.0, 1.0) * location * (1.0 - location)).clamp(-0.25, 0.25);
        }
    }

    pub(super) fn is_settled(&self) -> bool {
        self.height
            .iter()
            .chain(&self.flow)
            .all(|value| value.abs() < SETTLED)
    }

    pub(super) fn advance(&mut self, seconds: f64) -> usize {
        if self.is_settled() || !seconds.is_finite() || seconds <= 0.0 {
            return 0;
        }
        self.remainder += seconds.min(SUBSTEP * MAX_STEPS as f64);
        let steps = ((self.remainder / SUBSTEP + 1e-9) as usize).min(MAX_STEPS);
        self.remainder = (self.remainder - steps as f64 * SUBSTEP).max(0.0);
        let courant = SPEED * SUBSTEP;
        let decay = 1.0 - FLOW_DAMPING * SUBSTEP;
        for _ in 0..steps {
            for (edge, pair) in self.flow.iter_mut().zip(self.height.windows(2)) {
                *edge = (*edge + courant * (pair[0] - pair[1])) * decay;
            }
            let mut inflow = 0.0;
            for (index, height) in self.height.iter_mut().enumerate() {
                let outflow = self.flow.get(index).copied().unwrap_or(0.0);
                *height += courant * (inflow - outflow);
                inflow = outflow;
            }
        }
        if self.is_settled() {
            *self = Self::default();
        }
        steps
    }
}

#[cfg(test)]
mod tests {
    use super::{Surface, SUBSTEP, SURFACE_POINTS};

    #[test]
    fn equal_level_is_stable() {
        let mut surface = Surface::default();
        assert_eq!(surface.advance(0.5), 0);
        assert_eq!(surface.offsets(), &[0.0; SURFACE_POINTS]);
        assert!(surface.is_settled());
    }

    #[test]
    fn disturbance_settles_with_bounded_substeps() {
        let mut surface = Surface::default();
        surface.disturb(0.75);
        assert_eq!(surface.advance(45.0), 6);
        assert!(surface.offsets().iter().any(|height| height.abs() > 0.001));
        assert!(surface.offsets().iter().sum::<f64>().abs() < 1e-9);
        for _ in 0..720 {
            surface.advance(SUBSTEP);
        }
        assert!(surface.is_settled());
        assert_eq!(surface.offsets(), &[0.0; SURFACE_POINTS]);
    }
}
