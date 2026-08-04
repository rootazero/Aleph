//! Vision bridge — makes desktop screenshots legible to text-only models.
//!
//! A vision-capable model reads a screenshot directly from the `image_base64`
//! the `screenshot` action returns. A *text-only* model (DeepSeek-chat, a
//! proxied Llama, etc.) cannot — the base64 blob is dropped or stripped before
//! it reaches the model, leaving the agent blind mid computer-use. The
//! reference engine (opencodex) solves this with a "Vision Bridge": it runs the
//! screenshot through a multimodal model, caches the textual description, and
//! injects it so the text-only model can still act.
//!
//! Aleph already owns every piece — [`VisionPipeline`] (OCR + image
//! understanding with provider fallback) and the platform-native OCR engine.
//! This module is the missing *wire*: a thin, cached adapter that turns a
//! captured screenshot into an OCR text layer (offline, always available) plus
//! an optional multimodal scene description.
//!
//! Design notes:
//! - **Opt-in & non-breaking.** The `screenshot` action only consults the
//!   bridge when the caller passes `describe: true`; the default path is
//!   byte-identical to before.
//! - **Graceful degradation (P7).** If no provider supports a capability the
//!   corresponding field is simply absent — never an error. OCR works offline
//!   via the platform engine; the description lights up only once a multimodal
//!   provider is registered in the pipeline.
//! - **Bounded cache (opencodex parity).** Identical re-captures of a static
//!   screen reuse a cached result within a TTL, so a model that screenshots in
//!   a tight loop does not re-pay for OCR / a vision round-trip every step.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use crate::sync_primitives::{Arc, Mutex};
use crate::vision::types::{ImageFormat, ImageInput};
use crate::vision::VisionPipeline;

/// Default time-to-live for a cached augmentation (matches opencodex's 300s).
const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// Hard cap on cached entries — a screenshot loop touches many distinct frames,
/// so the cache is a bounded recency aid, not an unbounded store.
const DEFAULT_MAX_ENTRIES: usize = 32;

/// Prompt handed to the multimodal model when producing a scene description.
///
/// Phrased for an agent that will *act* on the result: it asks for elements,
/// their text, and rough locations rather than prose, so a text-only model has
/// enough to choose a next action.
const DESCRIBE_PROMPT: &str =
    "You are describing a desktop screenshot for an AI agent that controls \
the computer but cannot see images. List the visible UI elements (buttons, fields, menus, links), \
their visible text, and their approximate on-screen location. Be concise and factual.";

/// The textual layers a screenshot can be augmented with. Each is best-effort:
/// `None` means no registered provider could produce it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Augmentation {
    /// Flat OCR text extracted from the screen (offline, platform-native).
    pub ocr_text: Option<String>,
    /// Multimodal scene description (only when `want_description` and a capable
    /// provider is registered).
    pub description: Option<String>,
}

impl Augmentation {
    /// True when neither layer was produced — the caller can skip attaching
    /// empty fields to the tool output.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.ocr_text.is_none() && self.description.is_none()
    }
}

struct CacheEntry {
    stored: Instant,
    value: Augmentation,
}

/// Cached adapter from a captured screenshot to its text layers.
///
/// Cheap to clone-share via `Arc`; holds an `Arc<VisionPipeline>` and a small
/// TTL cache keyed by `(image-hash, want_description)`.
pub struct VisionBridge {
    pipeline: Arc<VisionPipeline>,
    cache: Mutex<HashMap<u64, CacheEntry>>,
    ttl: Duration,
    max_entries: usize,
}

impl VisionBridge {
    /// Build a bridge over the given vision pipeline with default TTL / cap.
    #[must_use]
    pub fn new(pipeline: Arc<VisionPipeline>) -> Self {
        Self {
            pipeline,
            cache: Mutex::new(HashMap::new()),
            ttl: DEFAULT_TTL,
            max_entries: DEFAULT_MAX_ENTRIES,
        }
    }

    /// Override the cache TTL (primarily for tests).
    pub const fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Produce the text layers for a base64-encoded screenshot.
    ///
    /// `want_description` gates the (potentially networked) multimodal call;
    /// OCR is always attempted because it is offline and cheap. Results are
    /// cached for [`Self::ttl`]; a fresh cache hit returns immediately without
    /// touching any provider.
    pub async fn augment(
        &self,
        image_base64: &str,
        format: ImageFormat,
        want_description: bool,
    ) -> Augmentation {
        let key = cache_key(image_base64, want_description);

        if let Some(hit) = self.cache_get(key) {
            return hit;
        }

        let image = ImageInput::Base64 {
            data: image_base64.to_string(),
            format,
        };

        // OCR: offline, platform-native. Absent provider → no text, not error.
        let ocr_text = match self.pipeline.ocr(&image).await {
            Ok(result) => {
                let trimmed = result.full_text.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "vision bridge: OCR layer unavailable");
                None
            }
        };

        let description = if want_description {
            match self
                .pipeline
                .understand_image(&image, DESCRIBE_PROMPT)
                .await
            {
                Ok(result) => {
                    let trimmed = result.description.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "vision bridge: description layer unavailable");
                    None
                }
            }
        } else {
            None
        };

        let value = Augmentation {
            ocr_text,
            description,
        };
        self.cache_put(key, value.clone());
        value
    }

    fn cache_get(&self, key: u64) -> Option<Augmentation> {
        let mut guard = self.cache.lock().unwrap_or_else(|e| {
            tracing::warn!("Recovering from poisoned vision-bridge cache");
            e.into_inner()
        });
        match guard.get(&key) {
            Some(entry) if entry.stored.elapsed() < self.ttl => Some(entry.value.clone()),
            Some(_) => {
                guard.remove(&key);
                None
            }
            None => None,
        }
    }

    fn cache_put(&self, key: u64, value: Augmentation) {
        let mut guard = self.cache.lock().unwrap_or_else(|e| {
            tracing::warn!("Recovering from poisoned vision-bridge cache");
            e.into_inner()
        });
        // Drop expired entries first; if still at capacity, clear the whole map
        // (a coarse but cheap bound — the cache is a recency aid, not a store).
        let ttl = self.ttl;
        guard.retain(|_, e| e.stored.elapsed() < ttl);
        if guard.len() >= self.max_entries {
            guard.clear();
        }
        guard.insert(
            key,
            CacheEntry {
                stored: Instant::now(),
                value,
            },
        );
    }
}

/// Stable hash of `(image bytes, want_description)`. `want_description` is part
/// of the key so an OCR-only result is never mistaken for a description-bearing
/// one (and vice versa).
fn cache_key(image_base64: &str, want_description: bool) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    image_base64.as_bytes().hash(&mut hasher);
    want_description.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision::provider::VisionProvider;
    use crate::vision::types::{OcrResult, VisionCapabilities, VisionResult};
    use crate::vision::VisionError;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Provider that counts calls so tests can assert cache behavior.
    struct CountingProvider {
        ocr_calls: Arc<AtomicUsize>,
        understand_calls: Arc<AtomicUsize>,
        caps: VisionCapabilities,
    }

    #[async_trait]
    impl VisionProvider for CountingProvider {
        async fn understand_image(
            &self,
            _image: &ImageInput,
            _prompt: &str,
        ) -> Result<VisionResult, VisionError> {
            self.understand_calls.fetch_add(1, Ordering::SeqCst);
            Ok(VisionResult {
                description: "a window with an OK button".to_string(),
            })
        }

        async fn ocr(&self, _image: &ImageInput) -> Result<OcrResult, VisionError> {
            self.ocr_calls.fetch_add(1, Ordering::SeqCst);
            Ok(OcrResult {
                full_text: "OK Cancel".to_string(),
            })
        }

        fn capabilities(&self) -> VisionCapabilities {
            self.caps
        }

        fn name(&self) -> &str {
            "counting"
        }
    }

    fn bridge_with(caps: VisionCapabilities) -> (VisionBridge, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let ocr_calls = Arc::new(AtomicUsize::new(0));
        let understand_calls = Arc::new(AtomicUsize::new(0));
        let mut pipeline = VisionPipeline::new();
        pipeline.add_provider(Box::new(CountingProvider {
            ocr_calls: Arc::clone(&ocr_calls),
            understand_calls: Arc::clone(&understand_calls),
            caps,
        }));
        (
            VisionBridge::new(Arc::new(pipeline)),
            ocr_calls,
            understand_calls,
        )
    }

    #[tokio::test]
    async fn ocr_only_when_description_not_requested() {
        let (bridge, ocr_calls, understand_calls) = bridge_with(VisionCapabilities::all());
        let aug = bridge.augment("aGVsbG8=", ImageFormat::Png, false).await;
        assert_eq!(aug.ocr_text.as_deref(), Some("OK Cancel"));
        assert!(aug.description.is_none());
        assert_eq!(understand_calls.load(Ordering::SeqCst), 0);
        assert_eq!(ocr_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn description_requested_runs_both_layers() {
        let (bridge, _, understand_calls) = bridge_with(VisionCapabilities::all());
        let aug = bridge.augment("aGVsbG8=", ImageFormat::Png, true).await;
        assert_eq!(aug.ocr_text.as_deref(), Some("OK Cancel"));
        assert_eq!(
            aug.description.as_deref(),
            Some("a window with an OK button")
        );
        assert_eq!(understand_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn identical_capture_is_cached() {
        let (bridge, ocr_calls, understand_calls) = bridge_with(VisionCapabilities::all());
        let _ = bridge.augment("c2FtZQ==", ImageFormat::Png, true).await;
        let _ = bridge.augment("c2FtZQ==", ImageFormat::Png, true).await;
        // Second call served from cache → providers hit exactly once.
        assert_eq!(ocr_calls.load(Ordering::SeqCst), 1);
        assert_eq!(understand_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn expired_entry_recomputes() {
        let (bridge, ocr_calls, _) = bridge_with(VisionCapabilities::all());
        let bridge = bridge.with_ttl(Duration::from_millis(1));
        let _ = bridge.augment("ZXhw", ImageFormat::Png, false).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        let _ = bridge.augment("ZXhw", ImageFormat::Png, false).await;
        assert_eq!(ocr_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn ocr_only_provider_yields_no_description() {
        // A provider that supports OCR but not image understanding: description
        // degrades to absent (P7), OCR still present.
        let (bridge, _, understand_calls) = bridge_with(VisionCapabilities {
            image_understanding: false,
            ocr: true,
        });
        let aug = bridge.augment("b2Nyb25seQ==", ImageFormat::Png, true).await;
        assert_eq!(aug.ocr_text.as_deref(), Some("OK Cancel"));
        assert!(aug.description.is_none());
        // understand_image is never dispatched when no provider is capable.
        assert_eq!(understand_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn empty_pipeline_degrades_to_nothing() {
        let bridge = VisionBridge::new(Arc::new(VisionPipeline::new()));
        let aug = bridge.augment("ZW1wdHk=", ImageFormat::Png, true).await;
        assert!(aug.is_empty());
    }
}
