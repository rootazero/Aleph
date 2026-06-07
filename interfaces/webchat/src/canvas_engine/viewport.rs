use super::types::{CanvasNode, Vec2};

/// Extra hit tolerance in **screen pixels**, added to every node's world
/// radius during hit-testing. Two reasons:
///   1. Enhance — tiny DOT-mode nodes (≈10 px) become comfortably clickable
///      and hoverable instead of pixel-perfect targets.
///   2. Stabilise — the renderer draws a hover glow that makes a node look
///      larger than its bare radius, so a bare-radius test toggles hover
///      on/off under sub-pixel pointer jitter at the visual edge. A small
///      padding gives the held node a forgiving zone, killing the flicker.
///
/// Converted to world units via the live zoom so the felt size stays constant
/// at any scale.
const HIT_TOLERANCE_PX: f64 = 6.0;

/// Screen-space half-extents of the hover-retention box around a held node's
/// screen center, sized to cover the Full card footprint. The card is 280 px
/// wide and positioned via `translate3d(x-140, y-60)`, with the excerpt
/// extending downward — hence the asymmetric vertical extents. Used for hover
/// *hysteresis*: entry uses the bare node circle, retention uses this larger
/// region so a held node keeps hover while the pointer rests over its card.
const RETAIN_HALF_W: f64 = 150.0;
const RETAIN_UP: f64 = 70.0;
const RETAIN_DOWN: f64 = 130.0;

#[derive(Debug, Clone)]
pub struct Viewport {
    pub offset: Vec2,
    pub scale: f64,
    pub width: f64,
    pub height: f64,
}

impl Viewport {
    pub fn new(width: f64, height: f64) -> Self {
        Self {
            offset: Vec2::new(width / 2.0, height / 2.0),
            scale: 1.0,
            width,
            height,
        }
    }

    pub fn world_to_screen(&self, world: Vec2) -> Vec2 {
        Vec2 {
            x: world.x * self.scale + self.offset.x,
            y: world.y * self.scale + self.offset.y,
        }
    }

    pub fn screen_to_world(&self, screen: Vec2) -> Vec2 {
        Vec2 {
            x: (screen.x - self.offset.x) / self.scale,
            y: (screen.y - self.offset.y) / self.scale,
        }
    }

    pub fn zoom_at(&mut self, screen_point: Vec2, delta: f64) {
        let old_scale = self.scale;
        self.scale = (self.scale * (1.0 + delta)).clamp(0.1, 5.0);
        let ratio = self.scale / old_scale;
        self.offset.x = screen_point.x - (screen_point.x - self.offset.x) * ratio;
        self.offset.y = screen_point.y - (screen_point.y - self.offset.y) * ratio;
    }

    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.offset.x += dx;
        self.offset.y += dy;
    }

    pub fn center_on(&mut self, world_point: Vec2) {
        self.offset.x = self.width / 2.0 - world_point.x * self.scale;
        self.offset.y = self.height / 2.0 - world_point.y * self.scale;
    }

    pub fn hit_test(&self, screen_point: Vec2, nodes: &[CanvasNode]) -> Option<usize> {
        let world = self.screen_to_world(screen_point);
        // Screen-space padding → world units (scale is clamped ≥ 0.1 by zoom_at,
        // guard anyway so an unexpected tiny scale can't explode the tolerance).
        let tol = HIT_TOLERANCE_PX / self.scale.max(0.1);
        nodes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, node)| world.distance_to(&node.position) <= node.radius + tol)
            .map(|(idx, _)| idx)
    }

    /// Hover hysteresis: returns true if `screen_point` still falls within the
    /// retention region of the already-held node at `node_world`.
    ///
    /// `hit_test` (the bare circle) decides hover *entry*; this larger region
    /// decides *retention*, so a held node keeps hover while the pointer rests
    /// anywhere over its enlarged Full card — killing boundary flicker and
    /// letting the user move onto the card to read it. Below the Dot zoom
    /// threshold (`scale < 0.5`) the node renders as a dot with no enlarged
    /// card, so retention degrades to the same forgiving circle as entry.
    pub fn hover_retains(&self, screen_point: Vec2, node_world: Vec2, node_radius: f64) -> bool {
        let center = self.world_to_screen(node_world);
        if self.scale < 0.5 {
            return screen_point.distance_to(&center) <= node_radius * self.scale + HIT_TOLERANCE_PX;
        }
        let dx = screen_point.x - center.x;
        let dy = screen_point.y - center.y;
        dx >= -RETAIN_HALF_W && dx <= RETAIN_HALF_W && dy >= -RETAIN_UP && dy <= RETAIN_DOWN
    }

    pub fn is_visible(&self, world_point: Vec2, margin: f64) -> bool {
        let screen = self.world_to_screen(world_point);
        screen.x >= -margin
            && screen.x <= self.width + margin
            && screen.y >= -margin
            && screen.y <= self.height + margin
    }

    /// Scale + recentre so all `nodes` fit inside the canvas with `padding_pct`
    /// extra margin on every side. No-op for an empty slice. `padding_pct` is a
    /// fraction (0.10 = 10 %).
    ///
    /// Scale is clamped to `[0.2, 3.0]` so degenerate inputs (single point, vast
    /// outliers) cannot pin the user at unusable zoom levels.
    pub fn fit_to_content(&mut self, nodes: &[CanvasNode], padding_pct: f32) {
        if nodes.is_empty() {
            return;
        }
        let (mut min_x, mut min_y) = (f64::MAX, f64::MAX);
        let (mut max_x, mut max_y) = (f64::MIN, f64::MIN);
        for n in nodes {
            min_x = min_x.min(n.position.x);
            min_y = min_y.min(n.position.y);
            max_x = max_x.max(n.position.x);
            max_y = max_y.max(n.position.y);
        }
        // Avoid div-by-zero for degenerate (single-node) bboxes.
        let bbox_w = (max_x - min_x).max(1.0);
        let bbox_h = (max_y - min_y).max(1.0);
        let pad = 1.0 + padding_pct as f64;
        let scale_x = self.width / (bbox_w * pad);
        let scale_y = self.height / (bbox_h * pad);
        self.scale = scale_x.min(scale_y).clamp(0.2, 3.0);
        let cx = (min_x + max_x) * 0.5;
        let cy = (min_y + max_y) * 0.5;
        self.offset.x = self.width / 2.0 - cx * self.scale;
        self.offset.y = self.height / 2.0 - cy * self.scale;
    }
}

/// Per-Z layer parallax offset. Z=0 → factor 1.0, Z=200 → factor 0.85.
pub fn parallax_factor(z: f32) -> f32 {
    1.0 - 0.15 * (z / 200.0).clamp(0.0, 1.0)
}

/// Compute additional position offset for a node when the viewport is dragged.
pub fn parallax_offset(z: f32, drag_dx: f32, drag_dy: f32) -> (f32, f32) {
    let f = parallax_factor(z);
    (drag_dx * f, drag_dy * f)
}

#[cfg(test)]
mod parallax_tests {
    use super::*;

    #[test]
    fn z0_no_parallax_attenuation() {
        assert!((parallax_factor(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn z200_max_attenuation() {
        assert!((parallax_factor(200.0) - 0.85).abs() < 1e-6);
    }

    #[test]
    fn parallax_offset_proportional_to_drag() {
        let (dx, dy) = parallax_offset(0.0, 100.0, 50.0);
        assert!((dx - 100.0).abs() < 1e-3);
        assert!((dy - 50.0).abs() < 1e-3);
        let (dx2, dy2) = parallax_offset(200.0, 100.0, 50.0);
        assert!((dx2 - 85.0).abs() < 1e-3);
        assert!((dy2 - 42.5).abs() < 1e-3);
    }

    fn make_node(x: f64, y: f64) -> CanvasNode {
        CanvasNode {
            id: "n".into(),
            name: "n".into(),
            category: "".into(),
            color: super::super::types::Color { r: 0, g: 0, b: 0 },
            radius: 6.0,
            position: Vec2::new(x, y),
            velocity: Vec2::zero(),
            z: 0.0,
            hop: 1,
            decay_score: 1.0,
            edge_count: 0,
        }
    }

    #[test]
    fn fit_to_content_empty_is_no_op() {
        let mut v = Viewport::new(800.0, 600.0);
        let before = (v.scale, v.offset.x, v.offset.y);
        v.fit_to_content(&[], 0.10);
        assert_eq!((v.scale, v.offset.x, v.offset.y), before);
    }

    #[test]
    fn fit_to_content_centres_single_node() {
        let mut v = Viewport::new(800.0, 600.0);
        v.fit_to_content(&[make_node(123.0, -45.0)], 0.10);
        // Single node has zero bbox → falls back to scale 1, offset centred on node.
        let cx_world = (v.width / 2.0 - v.offset.x) / v.scale;
        let cy_world = (v.height / 2.0 - v.offset.y) / v.scale;
        assert!((cx_world - 123.0).abs() < 1.0);
        assert!((cy_world + 45.0).abs() < 1.0);
    }

    #[test]
    fn fit_to_content_padding_respected() {
        let mut v = Viewport::new(800.0, 600.0);
        let nodes = vec![make_node(-100.0, -100.0), make_node(100.0, 100.0)];
        v.fit_to_content(&nodes, 0.10);
        // bbox = 200×200, padded to 220×220 → scale ≤ min(800/220, 600/220) ≈ 2.727.
        assert!(v.scale <= 2.728, "scale too large: {}", v.scale);
        assert!(v.scale >= 0.2);
    }

    #[test]
    fn fit_to_content_clamps_scale() {
        let mut v = Viewport::new(800.0, 600.0);
        // Tiny bbox → would compute very high scale; must clamp at 3.0.
        v.fit_to_content(&[make_node(0.0, 0.0), make_node(0.5, 0.5)], 0.10);
        assert!(
            v.scale <= 3.0 + 1e-6,
            "expected scale ≤ 3.0, got {}",
            v.scale
        );

        // Massive bbox → would compute very low scale; must clamp at 0.2.
        let mut v2 = Viewport::new(800.0, 600.0);
        v2.fit_to_content(
            &[
                make_node(-100_000.0, -100_000.0),
                make_node(100_000.0, 100_000.0),
            ],
            0.10,
        );
        assert!(
            v2.scale >= 0.2 - 1e-6,
            "expected scale ≥ 0.2, got {}",
            v2.scale
        );
    }
}

#[cfg(test)]
mod hit_test_tests {
    use super::*;

    fn node_at(x: f64, y: f64, radius: f64) -> CanvasNode {
        CanvasNode {
            id: "n".into(),
            name: "n".into(),
            category: "".into(),
            color: super::super::types::Color { r: 0, g: 0, b: 0 },
            radius,
            position: Vec2::new(x, y),
            velocity: Vec2::zero(),
            z: 0.0,
            hop: 1,
            decay_score: 1.0,
            edge_count: 0,
        }
    }

    #[test]
    fn hit_test_inside_radius_resolves() {
        // Identity viewport (scale 1, origin at canvas centre 400,300).
        let v = Viewport::new(800.0, 600.0);
        let nodes = vec![node_at(0.0, 0.0, 6.0)];
        // World (0,0) maps to screen (400,300); 3 px away is well inside radius 6.
        assert_eq!(v.hit_test(Vec2::new(403.0, 300.0), &nodes), Some(0));
    }

    #[test]
    fn hit_test_tolerance_grabs_just_outside_radius() {
        // A 10 px DOT (radius ~5) just beyond the bare radius must still resolve
        // thanks to HIT_TOLERANCE_PX — this is the "tiny dots are clickable" win.
        let v = Viewport::new(800.0, 600.0);
        let nodes = vec![node_at(0.0, 0.0, 5.0)];
        // 9 px from centre: outside radius 5, inside 5 + 6 (tolerance) = 11.
        assert_eq!(v.hit_test(Vec2::new(409.0, 300.0), &nodes), Some(0));
    }

    #[test]
    fn hit_test_misses_beyond_radius_plus_tolerance() {
        let v = Viewport::new(800.0, 600.0);
        let nodes = vec![node_at(0.0, 0.0, 5.0)];
        // 20 px away: outside radius 5 + tolerance 6 = 11 → no hit.
        assert_eq!(v.hit_test(Vec2::new(420.0, 300.0), &nodes), None);
    }

    #[test]
    fn hit_test_tolerance_shrinks_in_world_when_zoomed_in() {
        // At 2× zoom the 6 px screen tolerance is only 3 world units, so the felt
        // hover zone stays constant on screen rather than ballooning in world space.
        let mut v = Viewport::new(800.0, 600.0);
        v.scale = 2.0;
        let nodes = vec![node_at(0.0, 0.0, 5.0)];
        // World point 7 units away → radius 5 + world-tolerance 3 = 8 → hit.
        assert_eq!(
            v.hit_test(v.world_to_screen(Vec2::new(7.0, 0.0)), &nodes),
            Some(0)
        );
        // World point 9 units away → outside 8 → miss.
        assert_eq!(
            v.hit_test(v.world_to_screen(Vec2::new(9.0, 0.0)), &nodes),
            None
        );
    }

    #[test]
    fn hover_retains_inside_full_card_box() {
        // scale 1.0, node at world origin → screen center (400,300).
        let vp = Viewport::new(800.0, 600.0);
        // A point over the card body (down-right of the node) but well outside
        // the bare node circle still retains hover.
        assert!(vp.hover_retains(Vec2::new(500.0, 350.0), Vec2::zero(), 10.0));
    }

    #[test]
    fn hover_retains_false_outside_box() {
        let vp = Viewport::new(800.0, 600.0);
        // 200 px right of center exceeds RETAIN_HALF_W (150).
        assert!(!vp.hover_retains(Vec2::new(600.0, 300.0), Vec2::zero(), 10.0));
        // 140 px below center exceeds RETAIN_DOWN (130).
        assert!(!vp.hover_retains(Vec2::new(400.0, 440.0), Vec2::zero(), 10.0));
    }

    #[test]
    fn hover_retains_dot_mode_uses_circle() {
        // Below the Dot threshold (scale < 0.5) there is no enlarged card, so
        // retention degrades to the forgiving circle (radius*scale + tol).
        let mut vp = Viewport::new(800.0, 600.0);
        vp.scale = 0.4; // offset stays (400,300); world origin → screen (400,300)
        // radius 10 → 10*0.4 + 6 = 10 px screen tolerance.
        assert!(vp.hover_retains(Vec2::new(405.0, 300.0), Vec2::zero(), 10.0));
        assert!(!vp.hover_retains(Vec2::new(420.0, 300.0), Vec2::zero(), 10.0));
    }
}
