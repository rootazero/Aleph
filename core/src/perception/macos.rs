use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use accessibility_sys::{AXUIElementCopyAttributeValue, AXUIElementCreateSystemWide, AXUIElementRef, AXValueGetType, AXValueGetValue, AXValueRef};
use core_foundation::array::CFArray;

// AXValueType constant for CGRect (value = 3)
// Not exported by accessibility-sys 0.1.x, so we define it here
const AX_VALUE_TYPE_CGRECT: u32 = 3;
use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::string::CFString;
use image::GenericImageView;
use once_cell::sync::Lazy;
use tracing::warn;

use crate::config::Config;
use crate::error::{AlephError, Result};
use crate::vision::{VisionConfig, VisionService};

use super::{
    AxLimits, AxNode, AxTree, FocusHint, FocusHintSource, ImageFormat, ImageRef, MergeOptions,
    PerceptionSnapshot, Rect, SnapshotCaptureArgs, SnapshotTarget, VisionBlock,
    VisionLimits,
};

const ERROR_AX_PERMISSION: &str = "AX_PERMISSION_REQUIRED";
const ERROR_SCREEN_PERMISSION: &str = "SCREEN_RECORDING_REQUIRED";
const ERROR_AX_LIMIT: &str = "AX_LIMIT_REACHED";
const ERROR_TIME_BUDGET: &str = "TIME_BUDGET_EXCEEDED";
const ERROR_CAPTURE_FAILED: &str = "CAPTURE_FAILED";
const ERROR_OCR_FAILED: &str = "OCR_FAILED";
const ERROR_MERGE_FAILED: &str = "MERGE_FAILED";

const SCREEN_PROMPT_COOLDOWN_SECS: u64 = 600;

static SCREEN_PROMPT_STATE: Lazy<Mutex<Option<Instant>>> = Lazy::new(|| Mutex::new(None));
static MOUSE_STATE: Lazy<Mutex<MouseSignalState>> = Lazy::new(|| Mutex::new(MouseSignalState::default()));

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGEventCreate(source: *const std::ffi::c_void) -> *const std::ffi::c_void;
    fn CGEventGetLocation(event: *const std::ffi::c_void) -> CGPoint;
    fn CGEventSourceButtonState(state_id: u32, button: u32) -> bool;
    fn CFRelease(obj: *const std::ffi::c_void);
}

#[derive(Default)]
struct MouseSignalState {
    last_pos: Option<CGPoint>,
    last_move_at: Option<Instant>,
}

pub async fn capture_snapshot(args: SnapshotCaptureArgs) -> Result<PerceptionSnapshot> {
    let request = args.resolve();
    let start = Instant::now();

    let snapshot_id = format!("ps_{}", uuid::Uuid::new_v4());
    let mut snapshot = PerceptionSnapshot::new(snapshot_id, request.target);

    let display_height = display_height();

    let mut window_frame: Option<Rect> = None;

    if request.include_ax || matches!(request.target, SnapshotTarget::FrontmostWindow) {
        if budget_exceeded(start, request.max_latency_ms) {
            mark_budget_exceeded(&mut snapshot);
            snapshot.finalize();
            return Ok(snapshot);
        }

        match capture_ax_tree(&request.ax_limits, display_height) {
            Ok(result) => {
                window_frame = result.window_frame;
                if let Some(tree) = result.tree {
                    snapshot.ax_tree = Some(tree);
                }
                if let Some(focus_hint) = result.focus_hint {
                    snapshot.focus_hint = Some(focus_hint);
                }
                if result.reached_limits {
                    snapshot.partial = true;
                    snapshot.push_error(ERROR_AX_LIMIT);
                }
            }
            Err(err) => {
                if err == ERROR_AX_PERMISSION {
                    snapshot.push_error(ERROR_AX_PERMISSION);
                } else {
                    snapshot.push_error(ERROR_CAPTURE_FAILED);
                }
            }
        }
    }

    if snapshot.focus_hint.is_none() {
        if let Some(hint) = infer_mouse_focus(request.focus_window_ms, display_height) {
            snapshot.focus_hint = Some(hint);
        }
    }

    if request.include_vision || request.include_image {
        if budget_exceeded(start, request.max_latency_ms) {
            mark_budget_exceeded(&mut snapshot);
            snapshot.finalize();
            return Ok(snapshot);
        }

        if !screen_recording_granted() {
            warn!("Screen Recording permission required for vision snapshots");
            request_screen_recording_access();
            snapshot.partial = true;
            snapshot.push_error(ERROR_SCREEN_PERMISSION);
            snapshot.finalize();
            return Ok(snapshot);
        }

        let capture_rect = match request.target {
            SnapshotTarget::Region => request.region,
            SnapshotTarget::FrontmostWindow => window_frame,
        };

        let capture_rect = match capture_rect {
            Some(rect) => rect,
            None => {
                snapshot.partial = true;
                snapshot.push_error(ERROR_CAPTURE_FAILED);
                snapshot.finalize();
                return Ok(snapshot);
            }
        };

        let image_bytes = match capture_region_image(&capture_rect, request.image_format) {
            Ok(bytes) => bytes,
            Err(_) => {
                snapshot.partial = true;
                snapshot.push_error(ERROR_CAPTURE_FAILED);
                snapshot.finalize();
                return Ok(snapshot);
            }
        };

        let (image_width, image_height) = image_dimensions(&image_bytes);

        if request.include_image {
            if let Ok(image_ref) = persist_snapshot_image(
                &snapshot.snapshot_id,
                request.image_format,
                &image_bytes,
                image_width,
                image_height,
            ) {
                snapshot.image_ref = Some(image_ref);
            }
        }

        if request.include_vision {
            match ocr_to_vision_blocks(&image_bytes, &request.vision_limits).await {
                Ok(blocks) => {
                    if !blocks.is_empty() {
                        snapshot.vision_blocks = Some(blocks);
                    }
                }
                Err(_) => {
                    snapshot.partial = true;
                    snapshot.push_error(ERROR_OCR_FAILED);
                }
            }
        }
    }

    if request.include_ax && request.include_vision {
        if budget_exceeded(start, request.max_latency_ms) {
            mark_budget_exceeded(&mut snapshot);
            snapshot.finalize();
            return Ok(snapshot);
        }

        if let (Some(ax_tree), Some(vision_blocks)) = (&snapshot.ax_tree, &snapshot.vision_blocks) {
            match merge_shadow_dom(ax_tree, vision_blocks, &request.merge_options) {
                Ok(nodes) => {
                    if !nodes.is_empty() {
                        snapshot.shadow_dom = Some(nodes);
                    }
                }
                Err(_) => {
                    snapshot.partial = true;
                    snapshot.push_error(ERROR_MERGE_FAILED);
                }
            }
        }
    }

    if budget_exceeded(start, request.max_latency_ms) {
        mark_budget_exceeded(&mut snapshot);
    }

    snapshot.finalize();
    Ok(snapshot)
}

fn mark_budget_exceeded(snapshot: &mut PerceptionSnapshot) {
    snapshot.partial = true;
    snapshot.push_error(ERROR_TIME_BUDGET);
}

fn budget_exceeded(start: Instant, max_latency_ms: u64) -> bool {
    start.elapsed() > Duration::from_millis(max_latency_ms)
}

fn screen_recording_granted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

fn request_screen_recording_access() {
    let now = Instant::now();
    if let Ok(mut last) = SCREEN_PROMPT_STATE.lock() {
        if let Some(prev) = *last {
            if now.duration_since(prev) < Duration::from_secs(SCREEN_PROMPT_COOLDOWN_SECS) {
                return;
            }
        }
        *last = Some(now);
    }

    unsafe {
        CGRequestScreenCaptureAccess();
    }
}

fn display_height() -> f64 {
    unsafe {
        let bounds = CGDisplayBounds(CGMainDisplayID());
        bounds.size.height
    }
}

fn capture_region_image(rect: &Rect, format: ImageFormat) -> Result<Vec<u8>> {
    let (x, y, w, h) = (
        rect.x.round() as i64,
        rect.y.round() as i64,
        rect.width.round() as i64,
        rect.height.round() as i64,
    );

    if w <= 0 || h <= 0 {
        return Err(AlephError::tool("Invalid capture region"));
    }

    let tmp = tempfile::Builder::new()
        .prefix("aether_snapshot_")
        .suffix(match format {
            ImageFormat::Png => ".png",
            ImageFormat::Jpeg => ".jpg",
        })
        .tempfile()
        .map_err(|e| AlephError::tool(format!("Failed to create temp file: {}", e)))?;

    let path = tmp.path().to_path_buf();
    let region_arg = format!("{},{},{},{}", x, y, w, h);
    let format_arg = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
    };

    let status = Command::new("screencapture")
        .arg("-x")
        .arg("-t")
        .arg(format_arg)
        .arg("-R")
        .arg(&region_arg)
        .arg(&path)
        .status()
        .map_err(|e| AlephError::tool(format!("Failed to run screencapture: {}", e)))?;

    if !status.success() {
        return Err(AlephError::tool("screencapture failed"));
    }

    let data = fs::read(&path)
        .map_err(|e| AlephError::tool(format!("Failed to read snapshot: {}", e)))?;

    Ok(data)
}

fn persist_snapshot_image(
    snapshot_id: &str,
    format: ImageFormat,
    data: &[u8],
    width: u32,
    height: u32,
) -> Result<ImageRef> {
    let base_dir = default_snapshot_dir()?;
    fs::create_dir_all(&base_dir)
        .map_err(|e| AlephError::tool(format!("Failed to create snapshot dir: {}", e)))?;

    let filename = match format {
        ImageFormat::Png => format!("{}.png", snapshot_id),
        ImageFormat::Jpeg => format!("{}.jpg", snapshot_id),
    };
    let path = base_dir.join(filename);

    fs::write(&path, data)
        .map_err(|e| AlephError::tool(format!("Failed to write snapshot image: {}", e)))?;

    Ok(ImageRef {
        path: path.to_string_lossy().to_string(),
        format,
        width,
        height,
        bytes: data.len() as u64,
    })
}

fn default_snapshot_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| {
        AlephError::tool("Failed to resolve home directory for snapshots")
    })?;
    Ok(home.join(".aleph").join("snapshots"))
}

fn image_dimensions(data: &[u8]) -> (u32, u32) {
    if let Ok(img) = image::load_from_memory(data) {
        (img.width(), img.height())
    } else {
        (0, 0)
    }
}

async fn ocr_to_vision_blocks(
    image_data: &[u8],
    limits: &VisionLimits,
) -> Result<Vec<VisionBlock>> {
    let config = Config::load().unwrap_or_default();
    let vision = VisionService::new(VisionConfig::default());
    let text = vision.extract_text(image_data.to_vec(), &config).await?;

    if text.trim().is_empty() {
        return Ok(Vec::new());
    }

    let (width, height) = image_dimensions(image_data);
    if width == 0 || height == 0 {
        return Ok(Vec::new());
    }

    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return Ok(Vec::new());
    }

    let max_blocks = limits.max_blocks as usize;
    let count = lines.len().min(max_blocks);
    let block_height = height as f64 / count as f64;

    let mut blocks = Vec::new();
    for (idx, line) in lines.into_iter().take(count).enumerate() {
        let confidence = 0.9;
        if confidence < limits.min_confidence {
            continue;
        }
        let bbox = Rect {
            x: 0.0,
            y: block_height * idx as f64,
            width: width as f64,
            height: block_height,
        };
        blocks.push(VisionBlock {
            block_id: format!("vb_{}", idx + 1),
            text: line.to_string(),
            bbox,
            confidence,
            language: None,
        });
    }

    Ok(blocks)
}

fn merge_shadow_dom(
    ax_tree: &AxTree,
    vision_blocks: &[VisionBlock],
    options: &MergeOptions,
) -> Result<Vec<super::ShadowNode>> {
    if vision_blocks.is_empty() {
        return Ok(Vec::new());
    }

    let mut nodes = Vec::new();
    let mut matched_ax = std::collections::HashSet::new();

    for (index, block) in vision_blocks.iter().enumerate() {
        let mut best_iou = 0.0;
        let mut best_ax: Option<&AxNode> = None;
        for ax in &ax_tree.nodes {
            if let Some(frame) = ax.frame {
                let iou = frame.iou(&block.bbox);
                if iou > best_iou {
                    best_iou = iou;
                    best_ax = Some(ax);
                }
            }
        }

        let mut sources = vec![super::ShadowSource {
            ax_node_id: best_ax.map(|n| n.node_id.clone()),
            vision_block_id: Some(block.block_id.clone()),
        }];

        if let Some(ax_node) = best_ax {
            if best_iou >= options.iou_threshold {
                matched_ax.insert(ax_node.node_id.clone());
            }
        }

        nodes.push(super::ShadowNode {
            node_id: format!("sd_{}", index + 1),
            bbox: block.bbox,
            text: Some(block.text.clone()),
            role: best_ax.map(|n| n.role.clone()),
            sources,
        });
    }

    for ax in &ax_tree.nodes {
        if matched_ax.contains(&ax.node_id) {
            continue;
        }
        let Some(frame) = ax.frame else { continue };
        let text = ax.title.clone().or_else(|| ax.value.clone());
        if text.is_none() {
            continue;
        }
        nodes.push(super::ShadowNode {
            node_id: format!("sd_ax_{}", ax.node_id),
            bbox: frame,
            text,
            role: Some(ax.role.clone()),
            sources: vec![super::ShadowSource {
                ax_node_id: Some(ax.node_id.clone()),
                vision_block_id: None,
            }],
        });
    }

    Ok(nodes)
}

struct AxCaptureResult {
    tree: Option<AxTree>,
    window_frame: Option<Rect>,
    focus_hint: Option<FocusHint>,
    reached_limits: bool,
}

fn capture_ax_tree(limits: &AxLimits, display_height: f64) -> std::result::Result<AxCaptureResult, &'static str> {
    if !unsafe { AXIsProcessTrusted() } {
        return Err(ERROR_AX_PERMISSION);
    }

    unsafe {
        let system = AXUIElementCreateSystemWide();
        let focused_app = copy_ax_element(system, "AXFocusedApplication")?;
        let focused_window = copy_ax_element(focused_app, "AXFocusedWindow").ok();
        let focused_element = copy_ax_element(system, "AXFocusedUIElement").ok();

        let window_frame = focused_window
            .and_then(|w| copy_ax_frame(w))
            .map(|rect| to_top_left(rect, display_height));

        let focus_hint = focused_element
            .and_then(|el| copy_ax_frame(el))
            .map(|rect| FocusHint {
                bbox: to_top_left(rect, display_height),
                source: FocusHintSource::KeyboardFocus,
                confidence: 0.7,
                last_event_at: chrono::Utc::now().to_rfc3339(),
            });

        let root_element = focused_window.unwrap_or(focused_app);
        let mut nodes: Vec<AxNode> = Vec::new();
        let mut reached_limits = false;

        let mut stack: Vec<(AXUIElementRef, u32, Option<usize>)> = Vec::new();
        stack.push((root_element, 0, None));

        while let Some((element, depth, parent_index)) = stack.pop() {
            if nodes.len() as u32 >= limits.max_nodes {
                reached_limits = true;
                break;
            }
            if depth > limits.max_depth {
                reached_limits = true;
                continue;
            }

            let node_id = format!("ax_{}", nodes.len() + 1);

            let role = copy_ax_string(element, "AXRole", limits.max_value_bytes)
                .unwrap_or_else(|| "unknown".to_string());
            let title = copy_ax_string(element, "AXTitle", limits.max_value_bytes);
            let value = copy_ax_string(element, "AXValue", limits.max_value_bytes);
            let frame = copy_ax_frame(element).map(|rect| to_top_left(rect, display_height));

            let node = AxNode {
                node_id: node_id.clone(),
                role,
                title,
                value,
                frame,
                children: Vec::new(),
            };

            let current_index = nodes.len();
            nodes.push(node);

            if let Some(parent) = parent_index {
                if let Some(parent_node) = nodes.get_mut(parent) {
                    parent_node.children.push(node_id.clone());
                }
            }

            if let Some(children) = copy_ax_children(element) {
                for child in children {
                    stack.push((child, depth + 1, Some(current_index)));
                }
            }
        }

        let root_id = nodes.first().map(|n| n.node_id.clone()).unwrap_or_default();

        Ok(AxCaptureResult {
            tree: if nodes.is_empty() { None } else { Some(AxTree { root_id, nodes }) },
            window_frame,
            focus_hint,
            reached_limits,
        })
    }
}

unsafe fn copy_ax_element(element: AXUIElementRef, attribute: &str) -> std::result::Result<AXUIElementRef, &'static str> {
    let mut value: CFTypeRef = std::ptr::null();
    let attr = CFString::new(attribute);
    let result = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if result != 0 {
        return Err(ERROR_AX_PERMISSION);
    }
    if value.is_null() {
        return Err(ERROR_CAPTURE_FAILED);
    }
    Ok(value as AXUIElementRef)
}

unsafe fn copy_ax_string(element: AXUIElementRef, attribute: &str, max_bytes: u32) -> Option<String> {
    let mut value: CFTypeRef = std::ptr::null();
    let attr = CFString::new(attribute);
    let result = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if result != 0 || value.is_null() {
        return None;
    }
    let cf_str = CFString::wrap_under_get_rule(value as _);
    let mut string = cf_str.to_string();
    if string.len() as u32 > max_bytes {
        string.truncate(max_bytes as usize);
    }
    Some(string)
}

unsafe fn copy_ax_children(element: AXUIElementRef) -> Option<Vec<AXUIElementRef>> {
    let mut value: CFTypeRef = std::ptr::null();
    let attr = CFString::new("AXChildren");
    let result = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if result != 0 || value.is_null() {
        return None;
    }
    let array = CFArray::<*const std::ffi::c_void>::wrap_under_get_rule(value as _);
    let mut children = Vec::new();
    for idx in 0..array.len() {
        if let Some(item) = array.get(idx) {
            children.push(*item as AXUIElementRef);
        }
    }
    Some(children)
}

unsafe fn copy_ax_frame(element: AXUIElementRef) -> Option<Rect> {
    let mut value: CFTypeRef = std::ptr::null();
    let attr = CFString::new("AXFrame");
    let result = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if result != 0 || value.is_null() {
        return None;
    }

    let ax_value = value as AXValueRef;
    let mut rect = CGRect::default();
    let rect_type = AXValueGetType(ax_value);
    if rect_type != AX_VALUE_TYPE_CGRECT {
        return None;
    }

    let ok = AXValueGetValue(ax_value, rect_type, &mut rect as *mut _ as *mut _);
    if !ok {
        return None;
    }

    Some(Rect {
        x: rect.origin.x,
        y: rect.origin.y,
        width: rect.size.width,
        height: rect.size.height,
    })
}

fn to_top_left(rect: Rect, display_height: f64) -> Rect {
    Rect {
        x: rect.x,
        y: display_height - rect.y - rect.height,
        width: rect.width,
        height: rect.height,
    }
}

fn infer_mouse_focus(focus_window_ms: u64, _display_height: f64) -> Option<FocusHint> {
    let now = Instant::now();
    let point = current_mouse_position()?;

    let mut state = MOUSE_STATE.lock().ok()?;
    let last_pos = state.last_pos;

    if let Some(prev) = last_pos {
        let moved = (prev.x - point.x).abs() > 1.0 || (prev.y - point.y).abs() > 1.0;
        if moved {
            state.last_pos = Some(point);
            state.last_move_at = Some(now);
        }
    } else {
        state.last_pos = Some(point);
        state.last_move_at = Some(now);
    }

    let last_move = state.last_move_at.unwrap_or(now);
    let dwell = now.duration_since(last_move) >= Duration::from_millis(focus_window_ms);
    if dwell {
        return Some(FocusHint {
            bbox: Rect {
                x: point.x,
                y: point.y,
                width: 1.0,
                height: 1.0,
            },
            source: FocusHintSource::MouseDwell,
            confidence: 0.6,
            last_event_at: chrono::Utc::now().to_rfc3339(),
        });
    }

    if mouse_button_down() {
        return Some(FocusHint {
            bbox: Rect {
                x: point.x,
                y: point.y,
                width: 1.0,
                height: 1.0,
            },
            source: FocusHintSource::MouseClick,
            confidence: 0.7,
            last_event_at: chrono::Utc::now().to_rfc3339(),
        });
    }

    None
}

fn current_mouse_position() -> Option<CGPoint> {
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let location = CGEventGetLocation(event);
        CFRelease(event);
        Some(CGPoint {
            x: location.x,
            y: display_height() - location.y,
        })
    }
}

fn mouse_button_down() -> bool {
    const STATE_COMBINED: u32 = 0;
    const MOUSE_LEFT: u32 = 0;
    unsafe { CGEventSourceButtonState(STATE_COMBINED, MOUSE_LEFT) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_top_left() {
        let rect = Rect {
            x: 10.0,
            y: 20.0,
            width: 30.0,
            height: 40.0,
        };
        let converted = to_top_left(rect, 200.0);
        assert_eq!(converted.y, 140.0);
    }
}
