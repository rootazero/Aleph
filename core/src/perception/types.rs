//! Perception data structures for SnapshotTool.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Coordinate space identifier for all bounding boxes.
pub const COORDINATE_SPACE_SCREEN_TOP_LEFT: &str = "screen_points_top_left";

/// Snapshot target type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SnapshotTarget {
    #[default]
    FrontmostWindow,
    Region,
}


/// Image format for snapshots.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
}


/// Merge strategy for shadow DOM.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum MergeStrategy {
    #[default]
    Iou,
}


/// Focus hint source type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FocusHintSource {
    MouseDwell,
    MouseClick,
    KeyboardFocus,
}

/// Rectangle in screen coordinates.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn area(&self) -> f64 {
        if self.width <= 0.0 || self.height <= 0.0 {
            return 0.0;
        }
        self.width * self.height
    }

    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.width).min(other.x + other.width);
        let y2 = (self.y + self.height).min(other.y + other.height);

        let w = x2 - x1;
        let h = y2 - y1;
        if w <= 0.0 || h <= 0.0 {
            return None;
        }

        Some(Rect {
            x: x1,
            y: y1,
            width: w,
            height: h,
        })
    }

    pub fn iou(&self, other: &Rect) -> f64 {
        let inter = match self.intersect(other) {
            Some(r) => r.area(),
            None => 0.0,
        };
        let union = self.area() + other.area() - inter;
        if union <= 0.0 {
            0.0
        } else {
            inter / union
        }
    }
}

/// AX capture limits.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AxLimits {
    #[serde(default = "default_ax_max_depth")]
    pub max_depth: u32,
    #[serde(default = "default_ax_max_nodes")]
    pub max_nodes: u32,
    #[serde(default = "default_ax_max_value_bytes")]
    pub max_value_bytes: u32,
}

impl Default for AxLimits {
    fn default() -> Self {
        Self {
            max_depth: default_ax_max_depth(),
            max_nodes: default_ax_max_nodes(),
            max_value_bytes: default_ax_max_value_bytes(),
        }
    }
}

/// Vision OCR limits.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VisionLimits {
    #[serde(default = "default_vision_max_blocks")]
    pub max_blocks: u32,
    #[serde(default = "default_vision_min_confidence")]
    pub min_confidence: f32,
}

impl Default for VisionLimits {
    fn default() -> Self {
        Self {
            max_blocks: default_vision_max_blocks(),
            min_confidence: default_vision_min_confidence(),
        }
    }
}

/// Merge strategy settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MergeOptions {
    #[serde(default)]
    pub strategy: MergeStrategy,
    #[serde(default = "default_iou_threshold")]
    pub iou_threshold: f64,
}

impl Default for MergeOptions {
    fn default() -> Self {
        Self {
            strategy: MergeStrategy::Iou,
            iou_threshold: default_iou_threshold(),
        }
    }
}

/// Input arguments for SnapshotTool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SnapshotCaptureArgs {
    #[serde(default)]
    pub target: Option<SnapshotTarget>,
    #[serde(default)]
    pub region: Option<Rect>,
    #[serde(default)]
    pub include_ax: Option<bool>,
    #[serde(default)]
    pub include_vision: Option<bool>,
    #[serde(default)]
    pub include_image: Option<bool>,
    #[serde(default)]
    pub image_format: Option<ImageFormat>,
    #[serde(default)]
    pub max_latency_ms: Option<u64>,
    #[serde(default)]
    pub focus_window_ms: Option<u64>,
    #[serde(default)]
    pub ax_limits: Option<AxLimits>,
    #[serde(default)]
    pub vision_limits: Option<VisionLimits>,
    #[serde(default)]
    pub merge_strategy: Option<MergeOptions>,
}

/// Resolved snapshot request with defaults applied.
#[derive(Debug, Clone)]
pub struct SnapshotRequest {
    pub target: SnapshotTarget,
    pub region: Option<Rect>,
    pub include_ax: bool,
    pub include_vision: bool,
    pub include_image: bool,
    pub image_format: ImageFormat,
    pub max_latency_ms: u64,
    pub focus_window_ms: u64,
    pub ax_limits: AxLimits,
    pub vision_limits: VisionLimits,
    pub merge_options: MergeOptions,
}

impl SnapshotCaptureArgs {
    pub fn resolve(self) -> SnapshotRequest {
        let include_vision = self.include_vision.unwrap_or(false);
        let include_image = self.include_image.unwrap_or(false);
        let include_ax = self.include_ax.unwrap_or(true);

        let default_latency = if include_vision || include_image { 800 } else { 250 };

        SnapshotRequest {
            target: self.target.unwrap_or_default(),
            region: self.region,
            include_ax,
            include_vision,
            include_image,
            image_format: self.image_format.unwrap_or_default(),
            max_latency_ms: self.max_latency_ms.unwrap_or(default_latency).clamp(50, 2000),
            focus_window_ms: self.focus_window_ms.unwrap_or(1200).clamp(200, 5000),
            ax_limits: self.ax_limits.unwrap_or_default(),
            vision_limits: self.vision_limits.unwrap_or_default(),
            merge_options: self.merge_strategy.unwrap_or_default(),
        }
    }
}

/// Snapshot output object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerceptionSnapshot {
    pub schema_version: u32,
    pub snapshot_id: String,
    pub captured_at: String,
    pub target: SnapshotTarget,
    pub coordinate_space: String,
    pub partial: bool,
    pub ax_tree: Option<AxTree>,
    pub vision_blocks: Option<Vec<VisionBlock>>,
    pub shadow_dom: Option<Vec<ShadowNode>>,
    pub focus_hint: Option<FocusHint>,
    pub image_ref: Option<ImageRef>,
    pub errors: Option<Vec<String>>,
}

impl PerceptionSnapshot {
    pub fn new(snapshot_id: String, target: SnapshotTarget) -> Self {
        Self {
            schema_version: 1,
            snapshot_id,
            captured_at: chrono::Utc::now().to_rfc3339(),
            target,
            coordinate_space: COORDINATE_SPACE_SCREEN_TOP_LEFT.to_string(),
            partial: false,
            ax_tree: None,
            vision_blocks: None,
            shadow_dom: None,
            focus_hint: None,
            image_ref: None,
            errors: None,
        }
    }

    pub fn push_error(&mut self, code: &str) {
        let errors = self.errors.get_or_insert_with(Vec::new);
        if !errors.iter().any(|e| e == code) {
            errors.push(code.to_string());
        }
    }

    pub fn finalize(&mut self) {
        if let Some(errors) = &self.errors {
            if errors.is_empty() {
                self.errors = None;
            }
        }
    }
}

/// AX tree root object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxTree {
    pub root_id: String,
    pub nodes: Vec<AxNode>,
}

/// AX node representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxNode {
    pub node_id: String,
    pub role: String,
    pub title: Option<String>,
    pub value: Option<String>,
    pub frame: Option<Rect>,
    pub children: Vec<String>,
}

/// OCR block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionBlock {
    pub block_id: String,
    pub text: String,
    pub bbox: Rect,
    pub confidence: f32,
    pub language: Option<String>,
}

/// Shadow DOM node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowNode {
    pub node_id: String,
    pub bbox: Rect,
    pub text: Option<String>,
    pub role: Option<String>,
    pub sources: Vec<ShadowSource>,
}

/// Shadow DOM source mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowSource {
    pub ax_node_id: Option<String>,
    pub vision_block_id: Option<String>,
}

/// Focus hint data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusHint {
    pub bbox: Rect,
    pub source: FocusHintSource,
    pub confidence: f32,
    pub last_event_at: String,
}

/// Image reference metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageRef {
    pub path: String,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

fn default_ax_max_depth() -> u32 {
    12
}

fn default_ax_max_nodes() -> u32 {
    1500
}

fn default_ax_max_value_bytes() -> u32 {
    256
}

fn default_vision_max_blocks() -> u32 {
    200
}

fn default_vision_min_confidence() -> f32 {
    0.30
}

fn default_iou_threshold() -> f64 {
    0.60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect_iou() {
        let a = Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        let b = Rect {
            x: 5.0,
            y: 5.0,
            width: 10.0,
            height: 10.0,
        };
        let iou = a.iou(&b);
        assert!(iou > 0.0 && iou < 1.0);
    }

    #[test]
    fn test_snapshot_defaults() {
        let args = SnapshotCaptureArgs::default();
        let resolved = args.resolve();
        assert_eq!(resolved.target, SnapshotTarget::FrontmostWindow);
        assert!(resolved.include_ax);
        assert!(!resolved.include_vision);
        assert_eq!(resolved.max_latency_ms, 250);
    }
}
