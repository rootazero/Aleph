use super::types::{CanvasNode, Vec2};

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
        nodes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, node)| world.distance_to(&node.position) <= node.radius)
            .map(|(idx, _)| idx)
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
        assert!(v.scale <= 3.0 + 1e-6, "expected scale ≤ 3.0, got {}", v.scale);

        // Massive bbox → would compute very low scale; must clamp at 0.2.
        let mut v2 = Viewport::new(800.0, 600.0);
        v2.fit_to_content(
            &[make_node(-100_000.0, -100_000.0), make_node(100_000.0, 100_000.0)],
            0.10,
        );
        assert!(v2.scale >= 0.2 - 1e-6, "expected scale ≥ 0.2, got {}", v2.scale);
    }
}
