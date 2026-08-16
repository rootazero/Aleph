//! Orbit camera with damping, fly-to easing, and idle auto-rotate.
//! Pure (no web-sys) so it unit-tests on native.

use super::math::{Mat4, Vec3};

const DAMPING: f32 = 0.12; // approach rate per update toward target angles
const AUTOROTATE_RAD_PER_MS: f32 = 0.4 / 1000.0; // ~0.4 rad/s (ref autoRotateSpeed)
const FLY_RATE: f32 = 0.10; // ease-out approach per update toward fly target

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
    /// Timestamp of the last user interaction, or `None` until the first
    /// rendered frame seeds it. `update()` is fed the ABSOLUTE rAF timestamp
    /// (ms since page load, not since the canvas mounted), so a 0.0 seed would
    /// make the idle grace already expired on frame 1 and the galaxy would
    /// auto-rotate before the user has touched anything.
    last_interaction_ms: Option<f64>,
}

impl OrbitCamera {
    pub const MIN_DIST: f32 = 10.0;
    pub const MAX_DIST: f32 = 50000.0;
    pub const IDLE_MS: f64 = 60_000.0;
    /// Vertical field of view (the horizontal one follows from the aspect).
    const FOVY: f32 = std::f32::consts::PI * 50.0 / 180.0;
    /// Slack left around the fitted bounding sphere (15%).
    pub const FIT_PADDING: f32 = 0.15;
    /// Canonical orbit pose, restored by a Reset command.
    pub const HOME_AZIMUTH: f32 = 0.6;
    pub const HOME_ELEVATION: f32 = 0.35;

    pub fn new(distance: f32) -> Self {
        let d = distance.clamp(Self::MIN_DIST, Self::MAX_DIST);
        Self {
            azimuth: Self::HOME_AZIMUTH,
            elevation: Self::HOME_ELEVATION,
            distance: d,
            center: Vec3::zero(),
            tgt_azimuth: Self::HOME_AZIMUTH,
            tgt_elevation: Self::HOME_ELEVATION,
            tgt_distance: d,
            tgt_center: Vec3::zero(),
            last_interaction_ms: None,
        }
    }

    /// Distance at which a sphere of `radius` centred on the orbit centre exactly
    /// fills the frustum, plus `padding` slack.
    ///
    /// `FOVY` is the VERTICAL fov, so the horizontal half-angle is
    /// `atan(tan(FOVY/2) * aspect)`; the MIN of the two binds (a tall viewport
    /// must not clip left/right, a wide one must not clip top/bottom). `sin`, not
    /// `tan`: the frustum planes are TANGENT to the sphere, they do not cut
    /// through its centre plane.
    pub fn fit_distance(radius: f32, aspect: f32, padding: f32) -> f32 {
        let half_v = Self::FOVY * 0.5;
        let half_h = (half_v.tan() * aspect.max(0.01)).atan();
        let half = half_v.min(half_h);
        (radius * (1.0 + padding) / half.sin()).clamp(Self::MIN_DIST, Self::MAX_DIST)
    }

    pub fn orbit(&mut self, d_az: f32, d_el: f32) {
        self.tgt_azimuth += d_az;
        let lim = std::f32::consts::FRAC_PI_2 - 0.05;
        self.tgt_elevation = (self.tgt_elevation + d_el).clamp(-lim, lim);
    }

    /// Set the orbit angles (damping targets only — the camera eases into them).
    /// Used by the Reset command to restore the canonical pose.
    pub fn set_angles(&mut self, azimuth: f32, elevation: f32) {
        let lim = std::f32::consts::FRAC_PI_2 - 0.05;
        self.tgt_azimuth = azimuth;
        self.tgt_elevation = elevation.clamp(-lim, lim);
    }

    pub fn zoom(&mut self, factor: f32) {
        self.tgt_distance = (self.tgt_distance * factor).clamp(Self::MIN_DIST, Self::MAX_DIST);
    }

    /// Jump the distance immediately — both the live value and its damping target,
    /// so there is no ease-in and nothing left to converge to (the same
    /// both-at-once move `pan_world` makes for the centre).
    ///
    /// Reserved for auto-fit while the force layout is still expanding: the damped
    /// approach cannot outrun the expansion, so the content would spill past the
    /// viewport for the first ~30 frames. Do NOT use this for user-facing camera
    /// moves — a cut where the user expects a move reads as a glitch.
    pub fn set_distance(&mut self, distance: f32) {
        let d = distance.clamp(Self::MIN_DIST, Self::MAX_DIST);
        self.distance = d;
        self.tgt_distance = d;
    }

    pub fn fly_to(&mut self, target: Vec3, distance: f32) {
        self.tgt_center = target;
        self.tgt_distance = distance.clamp(Self::MIN_DIST, Self::MAX_DIST);
    }

    /// Pan: translate the orbit centre (look-at point) by a world-space delta,
    /// carrying the eye with it. Distinct from `orbit` (angles) and `zoom`
    /// (distance). Applied to BOTH the live centre and its damping target so a
    /// pan reads as immediate (no ease-in lag) yet never fights an in-flight
    /// `fly_to` (which retargets `tgt_center` independently).
    pub fn pan_world(&mut self, delta: Vec3) {
        self.center = self.center.add(&delta);
        self.tgt_center = self.tgt_center.add(&delta);
    }

    /// World-space (right, up) unit vectors spanning the view plane (perpendicular
    /// to the view direction). Turns a screen-pixel drag into a world-space pan.
    /// The elevation clamp in `orbit` keeps the view direction off the poles, so
    /// `forward × world_up` never degenerates.
    pub fn screen_basis(&self) -> (Vec3, Vec3) {
        let forward = self.center.sub(&self.eye()).normalize();
        let world_up = Vec3::new(0.0, 1.0, 0.0);
        let right = forward.cross(&world_up).normalize();
        let up = right.cross(&forward).normalize();
        (right, up)
    }

    /// World units spanned by one screen pixel at the look-at plane, for a given
    /// viewport height. Perspective: proportional to `distance * tan(fovy/2)`, so
    /// a pixel of drag pans further when zoomed out — matching what the cursor
    /// sits over.
    pub fn world_per_pixel(&self, viewport_h: f32) -> f32 {
        2.0 * self.distance * (Self::FOVY * 0.5).tan() / viewport_h.max(1.0)
    }

    pub fn note_interaction(&mut self, t_ms: f64) {
        self.last_interaction_ms = Some(t_ms);
    }

    pub fn update(&mut self, t_ms: f64, dt_ms: f32) {
        // Seed the idle clock on the FIRST RENDERED FRAME, not at construction:
        // `t_ms` is the absolute rAF timestamp, so the grace period must start
        // when the galaxy actually appears.
        let last = *self.last_interaction_ms.get_or_insert(t_ms);
        // The first frame carries dt == the absolute timestamp (Scene::last_t
        // starts at 0.0); clamp so it can never teleport the camera.
        let dt = dt_ms.clamp(0.0, 100.0);

        // Idle auto-rotate (only past timeout; advances the damping target).
        if t_ms - last > Self::IDLE_MS {
            self.tgt_azimuth += AUTOROTATE_RAD_PER_MS * dt;
        }
        // Frame-rate-independent exponential approach: at dt = 16 ms these are
        // exactly the old per-frame DAMPING / FLY_RATE, and a 120 Hz display no
        // longer damps (and flies, and spins) twice as fast as a 60 Hz one.
        let damp = 1.0 - (1.0 - DAMPING).powf(dt / 16.0);
        let fly = 1.0 - (1.0 - FLY_RATE).powf(dt / 16.0);
        self.azimuth += (self.tgt_azimuth - self.azimuth) * damp;
        self.elevation += (self.tgt_elevation - self.elevation) * damp;
        self.distance += (self.tgt_distance - self.distance) * damp;
        self.center = self
            .center
            .add(&self.tgt_center.sub(&self.center).scale(fly));
    }

    pub fn eye(&self) -> Vec3 {
        let ce = self.elevation.cos();
        Vec3::new(
            self.center.x + self.distance * ce * self.azimuth.sin(),
            self.center.y + self.distance * self.elevation.sin(),
            self.center.z + self.distance * ce * self.azimuth.cos(),
        )
    }

    pub fn target(&self) -> Vec3 {
        self.center
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let proj = Mat4::perspective(Self::FOVY, aspect.max(0.01), 0.1, 200_000.0);
        let view = Mat4::look_at(self.eye(), self.center, Vec3::new(0.0, 1.0, 0.0));
        proj.mul(&view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::memory::galaxy::gl::math::Vec3;

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
        for _ in 0..120 {
            c.update(0.0, 16.0);
        }
        let e1 = c.eye();
        assert!((e0.x - e1.x).abs() + (e0.y - e1.y).abs() + (e0.z - e1.z).abs() > 1.0);
    }

    #[test]
    fn fly_to_converges_target() {
        let mut c = OrbitCamera::new(100.0);
        c.fly_to(Vec3::new(50.0, 0.0, 0.0), 200.0);
        for _ in 0..300 {
            c.update(0.0, 16.0);
        }
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

    fn approx_eq(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-4);
    }

    #[test]
    fn pan_world_shifts_center_immediately() {
        let mut c = OrbitCamera::new(100.0);
        let t0 = c.target();
        c.pan_world(Vec3::new(10.0, -4.0, 0.0));
        let t1 = c.target();
        // Immediate — no update() needed (both center and tgt_center moved).
        approx_eq(t1.x - t0.x, 10.0);
        approx_eq(t1.y - t0.y, -4.0);
    }

    #[test]
    fn screen_basis_is_orthonormal() {
        let c = OrbitCamera::new(100.0);
        let (r, u) = c.screen_basis();
        approx_eq(r.length(), 1.0);
        approx_eq(u.length(), 1.0);
        assert!(r.dot(&u).abs() < 1e-4, "right must be perpendicular to up");
    }

    #[test]
    fn world_per_pixel_positive_and_scales_with_distance() {
        let near = OrbitCamera::new(100.0);
        let far = OrbitCamera::new(1000.0);
        let h = 800.0;
        assert!(near.world_per_pixel(h) > 0.0);
        assert!(far.world_per_pixel(h) > near.world_per_pixel(h));
    }

    /// The 6 axis + 2 diagonal extremes of a sphere of radius `r` around the origin.
    fn sphere_probes(r: f32) -> Vec<Vec3> {
        let d = r / 3.0_f32.sqrt();
        vec![
            Vec3::new(r, 0.0, 0.0),
            Vec3::new(-r, 0.0, 0.0),
            Vec3::new(0.0, r, 0.0),
            Vec3::new(0.0, -r, 0.0),
            Vec3::new(0.0, 0.0, r),
            Vec3::new(0.0, 0.0, -r),
            Vec3::new(d, d, d),
            Vec3::new(-d, -d, d),
        ]
    }

    /// Project a world point through a view-proj into NDC. `None` if behind.
    fn project(vp: &Mat4, p: &Vec3) -> Option<(f32, f32)> {
        let m = vp.as_slice();
        let cx = m[0] * p.x + m[4] * p.y + m[8] * p.z + m[12];
        let cy = m[1] * p.x + m[5] * p.y + m[9] * p.z + m[13];
        let cw = m[3] * p.x + m[7] * p.y + m[11] * p.z + m[15];
        if cw <= 0.0 {
            return None;
        }
        Some((cx / cw, cy / cw))
    }

    /// Snap the camera onto its damping targets (the fit sets targets, not state).
    fn settle(c: &mut OrbitCamera) {
        c.note_interaction(0.0);
        for _ in 0..400 {
            c.update(0.0, 16.0);
        }
    }

    #[test]
    fn fit_distance_frames_bounding_sphere() {
        for r in [50.0_f32, 500.0, 2000.0] {
            for aspect in [0.6_f32, 1.0, 2.4] {
                let d = OrbitCamera::fit_distance(r, aspect, OrbitCamera::FIT_PADDING);
                let c = OrbitCamera::new(d);
                let vp = c.view_proj(aspect);
                for p in sphere_probes(r) {
                    let (x, y) = project(&vp, &p).expect("probe behind camera");
                    assert!(
                        x.abs() <= 1.0 && y.abs() <= 1.0,
                        "r={r} aspect={aspect} probe {:?} → ndc ({x},{y})",
                        p
                    );
                }
            }
        }
    }

    #[test]
    fn fit_distance_is_orbit_invariant() {
        // The property an AABB-derived fit would fail: the sphere stays framed at
        // every orbit pose, so auto-fit does not clip the moment the user rotates.
        let r = 800.0_f32;
        let aspect = 1.6_f32;
        let d = OrbitCamera::fit_distance(r, aspect, OrbitCamera::FIT_PADDING);
        for az in [0.0_f32, 1.2, -1.2] {
            for el in [-1.4_f32, 0.0, 1.4] {
                let mut c = OrbitCamera::new(d);
                c.set_angles(az, el);
                settle(&mut c);
                let vp = c.view_proj(aspect);
                for p in sphere_probes(r) {
                    let (x, y) = project(&vp, &p).expect("probe behind camera");
                    assert!(
                        x.abs() <= 1.0 && y.abs() <= 1.0,
                        "az={az} el={el} probe {:?} → ndc ({x},{y})",
                        p
                    );
                }
            }
        }
    }

    #[test]
    fn fit_distance_picks_limiting_axis() {
        // A narrow (tall) viewport has the smaller half-angle horizontally, so it
        // must pull back FURTHER than a wide one for the same radius.
        let narrow = OrbitCamera::fit_distance(500.0, 0.5, OrbitCamera::FIT_PADDING);
        let wide = OrbitCamera::fit_distance(500.0, 2.0, OrbitCamera::FIT_PADDING);
        assert!(narrow > wide, "narrow={narrow} wide={wide}");
        // Beyond aspect 1 the vertical fov binds, so widening further changes nothing.
        let wider = OrbitCamera::fit_distance(500.0, 4.0, OrbitCamera::FIT_PADDING);
        approx_eq(wider, wide);
        // Bigger content ⇒ bigger distance.
        assert!(
            OrbitCamera::fit_distance(1000.0, 1.0, OrbitCamera::FIT_PADDING)
                > OrbitCamera::fit_distance(500.0, 1.0, OrbitCamera::FIT_PADDING)
        );
    }

    #[test]
    fn fit_distance_clamps() {
        assert_eq!(
            OrbitCamera::fit_distance(0.001, 1.0, 0.15),
            OrbitCamera::MIN_DIST
        );
        assert_eq!(
            OrbitCamera::fit_distance(1.0e9, 1.0, 0.15),
            OrbitCamera::MAX_DIST
        );
    }

    #[test]
    fn damping_is_frame_rate_independent() {
        // Same wall-clock time, half the frame budget: the camera must land in
        // the same place (it used to arrive twice as fast at 120 Hz).
        let mut a = OrbitCamera::new(100.0);
        let mut b = OrbitCamera::new(100.0);
        a.fly_to(Vec3::new(50.0, 10.0, 0.0), 400.0);
        b.fly_to(Vec3::new(50.0, 10.0, 0.0), 400.0);
        a.orbit(0.9, 0.2);
        b.orbit(0.9, 0.2);
        a.note_interaction(0.0);
        b.note_interaction(0.0);
        for _ in 0..60 {
            a.update(0.0, 16.0);
        }
        for _ in 0..120 {
            b.update(0.0, 8.0);
        }
        let tol = 0.01;
        assert!(
            (a.distance - b.distance).abs() / a.distance < tol,
            "distance {} vs {}",
            a.distance,
            b.distance
        );
        assert!(
            (a.azimuth - b.azimuth).abs() < tol,
            "azimuth {} vs {}",
            a.azimuth,
            b.azimuth
        );
        assert!(
            a.target().sub(&b.target()).length() < tol * 50.0,
            "center {:?} vs {:?}",
            a.target(),
            b.target()
        );
    }

    #[test]
    fn idle_rotate_not_armed_on_first_frame() {
        // The rAF timestamp is ms since PAGE load; the Memory canvas typically
        // first renders long after 60 s of uptime. That must not count as idle.
        let mut c = OrbitCamera::new(800.0);
        let az = c.azimuth;
        c.update(600_000.0, 16.0);
        approx_eq(c.azimuth, az);
        // ...but the grace period still expires 60 s after that first frame.
        c.update(600_000.0 + OrbitCamera::IDLE_MS + 100.0, 16.0);
        assert!(c.azimuth != az);
    }
}
