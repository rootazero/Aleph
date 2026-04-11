use std::collections::HashSet;
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

pub fn kind_color(kind: &str) -> Color {
    match kind {
        "person" => Color::new(37, 99, 235),
        "concept" => Color::new(124, 58, 237),
        "project" => Color::new(5, 150, 105),
        "tool" => Color::new(217, 119, 6),
        "skill" => Color::new(220, 38, 38),
        "event" => Color::new(8, 145, 178),
        _ => Color::new(107, 114, 128),
    }
}

pub fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "person" => "\u{1F464}",
        "concept" => "\u{1F4A1}",
        "project" => "\u{1F4C1}",
        "tool" => "\u{1F527}",
        "skill" => "\u{1F3AF}",
        "event" => "\u{1F4C5}",
        _ => "\u{2753}",
    }
}

#[derive(Debug, Clone)]
pub struct CanvasNode {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub icon: &'static str,
    pub color: Color,
    pub radius: f64,
    pub has_wiki: bool,
    pub position: Vec2,
    pub velocity: Vec2,
    pub pinned: bool,
}

#[derive(Debug, Clone)]
pub struct CanvasEdge {
    pub from_idx: usize,
    pub to_idx: usize,
    pub relation: String,
    pub is_wikilink: bool,
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

#[derive(Debug, Clone)]
pub struct ViewState {
    pub mode: ViewMode,
    pub selected_node: Option<String>,
    pub hovered_node: Option<String>,
    pub breadcrumb: Vec<BreadcrumbEntry>,
    pub kind_filter: HashSet<String>,
}

impl ViewState {
    pub fn new() -> Self {
        Self {
            mode: ViewMode::Global { top_k: 100 },
            selected_node: None,
            hovered_node: None,
            breadcrumb: vec![],
            kind_filter: HashSet::new(),
        }
    }
}

impl Default for ViewState {
    fn default() -> Self {
        Self::new()
    }
}
