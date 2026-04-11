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
