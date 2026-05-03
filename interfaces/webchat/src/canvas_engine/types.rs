use std::collections::HashMap;
use std::ops::{Add, AddAssign, Mul, Sub};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn zero() -> Self {
        Self { x: 0.0, y: 0.0 }
    }
    pub fn length(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    pub fn normalized(&self) -> Self {
        let len = self.length();
        if len < 1e-10 {
            Self::zero()
        } else {
            Self {
                x: self.x / len,
                y: self.y / len,
            }
        }
    }
    pub fn distance_to(&self, other: &Vec2) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, scalar: f64) -> Vec2 {
        Vec2::new(self.x * scalar, self.y * scalar)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, rhs: Vec2) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
    pub fn to_css(&self) -> String {
        format!("rgb({},{},{})", self.r, self.g, self.b)
    }
    pub fn to_css_alpha(&self, alpha: f64) -> String {
        format!("rgba({},{},{},{})", self.r, self.g, self.b, alpha)
    }
}

/// Obsidian-style single purple color for all nodes.
pub const NOTE_COLOR: Color = Color::new(167, 139, 250); // #a78bfa

/// Compute node radius from link count.
pub fn note_radius(link_count: usize) -> f64 {
    4.0 + (link_count as f64 + 1.0).ln() * 4.0
}

#[derive(Debug, Clone)]
pub struct CanvasNode {
    pub id: String,
    pub name: String,
    pub category: String,
    pub color: Color,
    pub radius: f64,
    pub position: Vec2,
    pub velocity: Vec2,
    pub pinned: bool,
    // Radial navigation fields
    pub z: f32,
    pub hop: u8, // 0 = active centre, 1 = one-hop, 2 = two-hop
    pub decay_score: f32,
    pub edge_count: usize,
}

#[derive(Debug, Clone)]
pub struct CanvasEdge {
    pub from_idx: usize,
    pub to_idx: usize,
    pub relation: String,
    pub is_wikilink: bool,
    pub is_active_link: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ViewMode {
    Global { top_k: usize },
    Local { center_node_id: String, depth: u8 },
}

#[derive(Debug, Clone)]
pub struct BreadcrumbEntry {
    pub node_id: String,
    pub node_name: String,
}

// ---------------------------------------------------------------------------
// Radial navigation types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DepthAttrs {
    pub scale: f32,
    pub opacity: f32,
    pub blur_px: f32,
    pub sat_mul: f32,
    pub glow_alpha: f32,
    pub shadow_offset_y: f32,
}

#[derive(Debug, Clone)]
pub struct ClusterNode {
    pub id: String,
    pub relation: String,
    pub kind: String,
    pub member_ids: Vec<String>,
    pub representative_names: Vec<String>,
    pub aggregated_weight: f32,
    pub radius: f32,
    pub world_pos: Vec2,
    pub z: f32,
    pub expanded: bool,
}

#[derive(Debug, Clone)]
pub struct Neighborhood {
    pub center: CanvasNode,
    pub one_hop: Vec<CanvasNode>,
    pub two_hop: Vec<CanvasNode>,
    /// Nodes outside the current connected component, drawn in dim type-grouped
    /// clusters around the canvas. Click to re-center. Tagged with
    /// `hop = ORPHAN_HOP_SENTINEL`.
    pub orphans: Vec<CanvasNode>,
    pub clusters: Vec<ClusterNode>,
    pub edges: Vec<CanvasEdge>,
    pub target_positions: HashMap<String, Vec3>,
    pub fetched_at_ms: f64, // performance.now() timestamp
}

/// Sentinel hop value identifying ghost-dot orphans (nodes outside the current
/// connected component). The radial layers use 0/1/2; 3+ is reserved for ghosts.
pub const ORPHAN_HOP_SENTINEL: u8 = 3;

/// Z-depth assigned to orphans — beyond two_hop's 140 so they get maximum dim.
pub const ORPHAN_Z: f32 = 200.0;

#[derive(Debug, Clone)]
pub enum NavState {
    Idle,
    Loading {
        target: String,
        since_ms: f64,
    },
    Active {
        node_id: String,
        neighborhood: Neighborhood,
    },
    Animating {
        from_id: String,
        to_id: String,
        from_neighborhood: Neighborhood,
        to_neighborhood: Neighborhood,
        t: f32,
        duration_ms: u32,
        started_at_ms: f64,
    },
    Error {
        target: String,
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec3_new_constructs_correctly() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(v.x, 1.0);
        assert_eq!(v.y, 2.0);
        assert_eq!(v.z, 3.0);
    }
}
