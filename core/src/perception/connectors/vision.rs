use super::{ConnectorType, StateConnector};
use crate::perception::state_bus::{AppState, Element, ElementState, ElementSource, Rect, StateSource};
use crate::error::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Vision-based connector using OCR and computer vision
///
/// This connector is the universal fallback for applications that don't
/// support Accessibility API or don't have plugin integration.
///
/// Features:
/// - OCR text extraction
/// - Interactive element detection (buttons, inputs, etc.)
/// - Smart polling (pauses when no changes detected)
/// - Diff-based change detection
pub struct VisionConnector {
    /// Polling interval (default: 2 seconds)
    poll_interval: Duration,

    /// Active monitoring sessions
    monitors: Arc<RwLock<HashMap<String, MonitorSession>>>,
}

struct MonitorSession {
    bundle_id: String,
    last_capture: Instant,
    last_state_hash: u64,
    consecutive_no_change: u32,
}

impl VisionConnector {
    /// Create a new vision connector
    pub fn new() -> Self {
        Self {
            poll_interval: Duration::from_secs(2),
            monitors: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create with custom polling interval
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Capture screenshot and extract state
    async fn capture_screenshot(&self, bundle_id: &str, window_id: &str) -> Result<Vec<u8>> {
        // TODO: Implement actual screenshot capture
        // On macOS: Use CGWindowListCreateImage
        // On Linux: Use X11 or Wayland APIs
        // On Windows: Use Windows.Graphics.Capture

        tracing::warn!(
            "Screenshot capture not implemented for bundle_id={}, window_id={}",
            bundle_id,
            window_id
        );

        Ok(Vec::new())
    }

    /// Extract text using OCR
    async fn extract_text(&self, _image_data: &[u8]) -> Result<Vec<OcrResult>> {
        // TODO: Integrate OCR engine
        // Options:
        // - tesseract-rs (Tesseract wrapper)
        // - paddleocr-rs (PaddleOCR wrapper, better for Chinese)
        // - Apple Vision Framework (macOS only, best quality)

        tracing::warn!("OCR extraction not implemented");

        Ok(Vec::new())
    }

    /// Detect interactive elements using computer vision
    async fn detect_interactive_elements(&self, _image_data: &[u8]) -> Result<Vec<Element>> {
        // TODO: Implement CV-based element detection
        // Strategies:
        // 1. Template matching for common UI patterns (buttons, inputs)
        // 2. Edge detection + contour analysis
        // 3. Color-based segmentation (buttons often have distinct colors)
        // 4. ML-based detection (train on UI screenshots)

        tracing::warn!("Interactive element detection not implemented");

        Ok(Vec::new())
    }

    /// Compute hash of state for change detection
    fn compute_state_hash(state: &AppState) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();

        // Hash element count and positions
        state.elements.len().hash(&mut hasher);
        for element in &state.elements {
            element.id.hash(&mut hasher);
            if let Some(rect) = &element.rect {
                // Hash position as integers to avoid floating point issues
                (rect.x as i32).hash(&mut hasher);
                (rect.y as i32).hash(&mut hasher);
            }
        }

        hasher.finish()
    }

    /// Adjust polling interval based on activity
    fn adjust_poll_interval(&self, consecutive_no_change: u32) -> Duration {
        // Exponential backoff when no changes detected
        // 2s -> 4s -> 8s -> 16s (max)
        let multiplier = 2u32.pow(consecutive_no_change.min(3));
        self.poll_interval * multiplier
    }
}

#[async_trait]
impl StateConnector for VisionConnector {
    fn connector_type(&self) -> ConnectorType {
        ConnectorType::Vision
    }

    async fn can_handle(&self, _bundle_id: &str) -> bool {
        // Vision connector can handle any application (universal fallback)
        true
    }

    async fn capture_state(&self, bundle_id: &str, window_id: &str) -> Result<AppState> {
        // Capture screenshot
        let screenshot = self.capture_screenshot(bundle_id, window_id).await?;

        // Extract text via OCR
        let ocr_results = self.extract_text(&screenshot).await?;

        // Detect interactive elements
        let mut elements = self.detect_interactive_elements(&screenshot).await?;

        // Merge OCR results into elements
        for ocr in ocr_results {
            elements.push(Element {
                id: format!("ocr_{}", elements.len()),
                role: "text".to_string(),
                label: Some(ocr.text.clone()),
                current_value: Some(ocr.text),
                rect: Some(ocr.rect),
                state: ElementState {
                    focused: false,
                    enabled: true,
                    selected: false,
                },
                source: ElementSource::Ocr,
                confidence: ocr.confidence,
            });
        }

        Ok(AppState {
            app_id: bundle_id.to_string(),
            elements,
            app_context: None,
            source: StateSource::Vision,
            confidence: 0.8, // Default confidence for vision-based capture
        })
    }

    async fn start_monitoring(&self, bundle_id: &str) -> Result<()> {
        let session = MonitorSession {
            bundle_id: bundle_id.to_string(),
            last_capture: Instant::now(),
            last_state_hash: 0,
            consecutive_no_change: 0,
        };

        self.monitors
            .write()
            .await
            .insert(bundle_id.to_string(), session);

        tracing::info!("Started vision monitoring for bundle_id={}", bundle_id);

        // TODO: Spawn background polling task
        // let monitors = Arc::clone(&self.monitors);
        // tokio::spawn(async move {
        //     loop {
        //         // Poll and emit events
        //     }
        // });

        Ok(())
    }

    async fn stop_monitoring(&self, bundle_id: &str) -> Result<()> {
        self.monitors.write().await.remove(bundle_id);

        tracing::info!("Stopped vision monitoring for bundle_id={}", bundle_id);

        Ok(())
    }
}

impl Default for VisionConnector {
    fn default() -> Self {
        Self::new()
    }
}

/// OCR result with bounding box
struct OcrResult {
    text: String,
    rect: Rect,
    confidence: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_vision_connector_creation() {
        let connector = VisionConnector::new();
        assert_eq!(connector.connector_type(), ConnectorType::Vision);
    }

    #[tokio::test]
    async fn test_can_handle_any_app() {
        let connector = VisionConnector::new();
        assert!(connector.can_handle("com.example.app").await);
        assert!(connector.can_handle("com.another.app").await);
    }

    #[tokio::test]
    async fn test_custom_poll_interval() {
        let connector = VisionConnector::new()
            .with_poll_interval(Duration::from_secs(5));
        assert_eq!(connector.poll_interval, Duration::from_secs(5));
    }

    #[tokio::test]
    async fn test_monitoring_lifecycle() {
        let connector = VisionConnector::new();
        let bundle_id = "com.example.test";

        // Start monitoring
        connector.start_monitoring(bundle_id).await.unwrap();
        assert!(connector.monitors.read().await.contains_key(bundle_id));

        // Stop monitoring
        connector.stop_monitoring(bundle_id).await.unwrap();
        assert!(!connector.monitors.read().await.contains_key(bundle_id));
    }

    #[test]
    fn test_state_hash_computation() {
        let state1 = AppState {
            app_id: "com.test".to_string(),
            elements: vec![
                Element {
                    id: "btn1".to_string(),
                    role: "button".to_string(),
                    label: Some("Click".to_string()),
                    current_value: None,
                    rect: Some(Rect { x: 10.0, y: 20.0, width: 50.0, height: 30.0 }),
                    state: ElementState {
                        focused: false,
                        enabled: true,
                        selected: false,
                    },
                    source: ElementSource::Ax,
                    confidence: 1.0,
                },
            ],
            app_context: None,
            source: StateSource::Vision,
            confidence: 0.8,
        };

        let state2 = state1.clone();
        let hash1 = VisionConnector::compute_state_hash(&state1);
        let hash2 = VisionConnector::compute_state_hash(&state2);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_poll_interval_adjustment() {
        let connector = VisionConnector::new();

        assert_eq!(
            connector.adjust_poll_interval(0),
            Duration::from_secs(2)
        );
        assert_eq!(
            connector.adjust_poll_interval(1),
            Duration::from_secs(4)
        );
        assert_eq!(
            connector.adjust_poll_interval(2),
            Duration::from_secs(8)
        );
        assert_eq!(
            connector.adjust_poll_interval(3),
            Duration::from_secs(16)
        );
        assert_eq!(
            connector.adjust_poll_interval(10),
            Duration::from_secs(16)
        ); // Max
    }
}
