//! Orbit camera with damping, fly-to easing, and idle auto-rotate.
//! Pure (no web-sys) so it unit-tests on native.

use super::math::{Mat4, Vec3};

const DAMPING: f32 = 0.12;          // approach rate per update toward target angles
const AUTOROTATE_RAD_PER_MS: f32 = 0.4 / 1000.0; // ~0.4 rad/s (ref autoRotateSpeed)
const FLY_RATE: f32 = 0.10;         // ease-out approach per update toward fly target

pub struct OrbitCamera {
    // Current (rendered) orbit.
    pub azimuth: f32,
    pub elevation: f32,
    pub distance: f32,
    center: Vec3,
    // Damping targets.
    tgt_azimuth: f32,
    tgt_elevation: f32,
    tgt_distance: f32,
    tgt_center: Vec3,
    last_interaction_ms: f64,
}

impl OrbitCamera {
    pub const MIN_DIST: f32 = 10.0;
    pub const MAX_DIST: f32 = 50000.0;
    pub const IDLE_MS: f64 = 60_000.0;
    const FOVY: f32 = std::f32::consts::PI * 50.0 / 180.0;

    pub fn new(distance: f32) -> Self {
        let d = distance.clamp(Self::MIN_DIST, Self::MAX_DIST);
        Self {
            azimuth: 0.6, elevation: 0.35, distance: d, center: Vec3::zero(),
            tgt_azimuth: 0.6, tgt_elevation: 0.35, tgt_distance: d, tgt_center: Vec3::zero(),
            last_interaction_ms: 0.0,
        }
    }

    pub fn orbit(&mut self, d_az: f32, d_el: f32) {
        self.tgt_azimuth += d_az;
        let lim = std::f32::consts::FRAC_PI_2 - 0.05;
        self.tgt_elevation = (self.tgt_elevation + d_el).clamp(-lim, lim);
    }

    pub fn zoom(&mut self, factor: f32) {
        self.tgt_distance = (self.tgt_distance * factor).clamp(Self::MIN_DIST, Self::MAX_DIST);
    }

    pub fn fly_to(&mut self, target: Vec3, distance: f32) {
        self.tgt_center = target;
        self.tgt_distance = distance.clamp(Self::MIN_DIST, Self::MAX_DIST);
    }

    pub fn note_interaction(&mut self, t_ms: f64) { self.last_interaction_ms = t_ms; }

    pub fn update(&mut self, t_ms: f64, _dt_ms: f32) {
        // Idle auto-rotate (only past timeout; resets damping target).
        if t_ms - self.last_interaction_ms > Self::IDLE_MS {
            self.tgt_azimuth += AUTOROTATE_RAD_PER_MS * 16.0;
        }
        // Critically-ish damped approach.
        self.azimuth += (self.tgt_azimuth - self.azimuth) * DAMPING;
        self.elevation += (self.tgt_elevation - self.elevation) * DAMPING;
        self.distance += (self.tgt_distance - self.distance) * DAMPING;
        self.center = self.center.add(&self.tgt_center.sub(&self.center).scale(FLY_RATE));
    }

    pub fn eye(&self) -> Vec3 {
        let ce = self.elevation.cos();
        Vec3::new(
            self.center.x + self.distance * ce * self.azimuth.sin(),
            self.center.y + self.distance * self.elevation.sin(),
            self.center.z + self.distance * ce * self.azimuth.cos(),
        )
    }

    pub fn target(&self) -> Vec3 { self.center }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let proj = Mat4::perspective(Self::FOVY, aspect.max(0.01), 0.1, 200_000.0);
        let view = Mat4::look_at(self.eye(), self.center, Vec3::new(0.0, 1.0, 0.0));
        proj.mul(&view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::canvas::gl::math::Vec3;

    #[test]
    fn zoom_clamps_distance() {
        let mut c = OrbitCamera::new(100.0);
        c.zoom(0.0001); // zoom way in
        assert!(c.distance >= OrbitCamera::MIN_DIST);
        c.zoom(100000.0); // zoom way out
        assert!(c.distance <= OrbitCamera::MAX_DIST);
    }

    #[test]
    fn orbit_changes_eye_position() {
        let mut c = OrbitCamera::new(100.0);
        let e0 = c.eye();
        c.orbit(0.5, 0.2);
        // damping means eye moves toward new orbit over update()s
        for _ in 0..120 { c.update(0.0, 16.0); }
        let e1 = c.eye();
        assert!((e0.x - e1.x).abs() + (e0.y - e1.y).abs() + (e0.z - e1.z).abs() > 1.0);
    }

    #[test]
    fn fly_to_converges_target() {
        let mut c = OrbitCamera::new(100.0);
        c.fly_to(Vec3::new(50.0, 0.0, 0.0), 200.0);
        for _ in 0..300 { c.update(0.0, 16.0); }
        let t = c.target();
        assert!((t.x - 50.0).abs() < 1.0, "target.x={}", t.x);
    }

    #[test]
    fn idle_autorotate_after_timeout_only() {
        let mut c = OrbitCamera::new(100.0);
        c.note_interaction(0.0);
        let az_before = c.azimuth;
        c.update(1000.0, 16.0); // 1s < IDLE_MS → no autorotate
        approx_eq(c.azimuth, az_before);
        c.update(OrbitCamera::IDLE_MS + 100.0, 16.0); // past idle → rotates
        assert!(c.azimuth != az_before);
    }

    fn approx_eq(a: f32, b: f32) { assert!((a - b).abs() < 1e-4); }
}
