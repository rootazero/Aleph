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
}
