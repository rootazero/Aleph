use super::types::Vec2;

#[derive(Debug, Clone)]
pub enum CanvasEvent {
    SelectNode(String),
    DeselectNode,
    EnterLocalView(String),
    HoverNode(Option<String>),
    DragStart { node_idx: usize },
    DragMove { world_pos: Vec2 },
    DragEnd,
}

pub struct InteractionState {
    pub is_panning: bool,
    pub is_dragging_node: bool,
    pub dragged_node_idx: Option<usize>,
    pub last_mouse_screen: Vec2,
    pub mouse_down_screen: Vec2,
    pub mouse_down_time: f64,
    pub last_click_time: f64,
}

impl InteractionState {
    pub fn new() -> Self {
        Self {
            is_panning: false,
            is_dragging_node: false,
            dragged_node_idx: None,
            last_mouse_screen: Vec2::zero(),
            mouse_down_screen: Vec2::zero(),
            mouse_down_time: 0.0,
            last_click_time: 0.0,
        }
    }

    /// Returns true if the mouse-up position is close enough to mouse-down to be a click.
    pub fn is_click(&self, up_pos: Vec2) -> bool {
        up_pos.distance_to(&self.mouse_down_screen) < 5.0
    }

    /// Returns true if the current time is within the double-click threshold of the last click.
    pub fn is_double_click(&self, now: f64) -> bool {
        now - self.last_click_time < 300.0
    }
}

impl Default for InteractionState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Intent-based interaction layer (hover prefetch + keyboard nav)
// ---------------------------------------------------------------------------

use crate::canvas_engine::prefetch::HoverDebouncer;

pub enum CanvasIntent {
    None,
    SetActive(String),
    PrefetchNeighbor(String),
    ExpandCluster(String),
    BreadcrumbBack,
    BreadcrumbForward,
    OpenSearch,
    ToggleGlobal,
    CloseDetail,
    HoverFocus(Direction),
}

#[allow(dead_code)]
pub enum Direction {
    Next,
    Prev,
}

pub struct CanvasInteractionState {
    pub debounce: HoverDebouncer,
    pub hovered: Option<String>,
}

impl CanvasInteractionState {
    pub fn new() -> Self {
        Self { debounce: HoverDebouncer::new(), hovered: None }
    }

    pub fn on_pointer_move(&mut self, hovered: Option<&str>, now_ms: f64) -> Option<CanvasIntent> {
        self.hovered = hovered.map(str::to_string);
        self.debounce.note_hover(hovered, now_ms).map(CanvasIntent::PrefetchNeighbor)
    }

    pub fn on_click(&mut self, target: ClickTarget) -> CanvasIntent {
        match target {
            ClickTarget::Node(id) => CanvasIntent::SetActive(id),
            ClickTarget::Cluster(id) => CanvasIntent::ExpandCluster(id),
            ClickTarget::Empty => CanvasIntent::CloseDetail,
            ClickTarget::Active => CanvasIntent::None,
        }
    }

    pub fn on_keydown(&self, key: &str, alt: bool, _shift: bool) -> CanvasIntent {
        match (key, alt) {
            ("Tab", _) => CanvasIntent::HoverFocus(Direction::Next),
            ("Enter", _) => match &self.hovered {
                Some(id) => CanvasIntent::SetActive(id.clone()),
                None => CanvasIntent::None,
            },
            ("Escape", _) => CanvasIntent::CloseDetail,
            ("Backspace", _) | ("ArrowLeft", true) => CanvasIntent::BreadcrumbBack,
            ("ArrowRight", true) => CanvasIntent::BreadcrumbForward,
            ("/", _) => CanvasIntent::OpenSearch,
            ("g", _) | ("G", _) => CanvasIntent::ToggleGlobal,
            _ => CanvasIntent::None,
        }
    }
}

pub enum ClickTarget {
    Node(String),
    Cluster(String),
    Active,
    Empty,
}

#[cfg(test)]
mod intent_tests {
    use super::*;

    #[test]
    fn click_node_returns_set_active() {
        let mut s = CanvasInteractionState::new();
        match s.on_click(ClickTarget::Node("x".to_string())) {
            CanvasIntent::SetActive(id) => assert_eq!(id, "x"),
            _ => panic!("expected SetActive"),
        }
    }

    #[test]
    fn click_cluster_returns_expand() {
        let mut s = CanvasInteractionState::new();
        match s.on_click(ClickTarget::Cluster("c1".to_string())) {
            CanvasIntent::ExpandCluster(id) => assert_eq!(id, "c1"),
            _ => panic!("expected ExpandCluster"),
        }
    }

    #[test]
    fn keydown_escape_closes_detail() {
        let s = CanvasInteractionState::new();
        assert!(matches!(s.on_keydown("Escape", false, false), CanvasIntent::CloseDetail));
    }

    #[test]
    fn pointer_move_triggers_prefetch_after_debounce() {
        let mut s = CanvasInteractionState::new();
        assert!(s.on_pointer_move(Some("x"), 0.0).is_none());
        assert!(s.on_pointer_move(Some("x"), 100.0).is_none());
        assert!(matches!(
            s.on_pointer_move(Some("x"), 200.0),
            Some(CanvasIntent::PrefetchNeighbor(_))
        ));
    }
}
