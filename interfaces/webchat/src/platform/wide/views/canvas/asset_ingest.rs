//! Pure logic behind image drops and HTML frames — no DOM, fully unit-tested.
//!
//! Three small pieces the editor and the shape views wire up:
//!
//! - [`data_url_base64`]: the payload half of the composer `attachments.rs`
//!   `FileReader` → data-URL dance (the mime comes from `File::type_()`, not
//!   from the URL header — same decision as the composer).
//! - [`display_size`]: how big a freshly dropped image lands on the canvas.
//!   The *natural* size is stored verbatim on the shape (`natural_w/h`); the
//!   display bbox caps the long edge so a 4000-px photo does not swallow the
//!   viewport.
//! - [`SrcdocCache`]: the fetch-dedup state machine for `Shape::Html`
//!   srcdoc content. Keyed by **asset id alone**: assets are
//!   content-addressed (`<sha256>.<ext>`), so one id can only ever name one
//!   byte sequence — a canvas id in the key would only force refetches of
//!   identical content. The cache lives in the editor's scope
//!   (`StoredValue`), which unmounts per open canvas, so it cannot outlive
//!   the capability context it was filled under.
//!
//! # Why the client does NOT carry a mime whitelist
//!
//! The server's `ASSET_MIME_TABLE` (`src/canvas/assets.rs`) is the one
//! authority on what an asset may be; a copy here would be the §0 "supported
//! set forked into two drifting copies" bug. [`is_image_mime`] is a *shape*
//! gate (`image/*` — "is this file even an image"), not a whitelist: an
//! `image/x-icon` drop goes to the server and comes back as the server's own
//! refusal, surfaced through the normal error path.

use std::collections::{HashMap, HashSet};

/// True for any `image/*` mime — the drop/paste filter. Deliberately not the
/// server's allowlist (module doc).
#[must_use]
pub(super) fn is_image_mime(mime: &str) -> bool {
    mime.trim().to_ascii_lowercase().starts_with("image/")
}

/// The base64 payload of a `data:` URL (`data:<mime>;base64,<payload>`), or
/// `None` when there is no payload separator. Empty payloads answer `None`
/// too: `FileReader` yields them for zero-byte files, and an empty asset is
/// nothing the server would store.
#[must_use]
pub(super) fn data_url_base64(data_url: &str) -> Option<&str> {
    let payload = data_url.split_once(',')?.1;
    if payload.is_empty() {
        None
    } else {
        Some(payload)
    }
}

/// Longest display edge for a dropped image, world units.
const MAX_IMAGE_EDGE: f64 = 600.0;
/// Display bbox when the natural size is unknown (decode failed / zero).
const FALLBACK_IMAGE_SIZE: (f64, f64) = (320.0, 240.0);

/// Display size for a dropped image: the natural size, aspect-preserved and
/// capped to [`MAX_IMAGE_EDGE`] on the longer edge. A natural size that is
/// unusable (zero, negative, non-finite — `decode()` failed or the format
/// reports nothing) answers [`FALLBACK_IMAGE_SIZE`] instead of a degenerate
/// bbox no gesture could ever grab.
#[must_use]
pub(super) fn display_size(natural_w: f64, natural_h: f64) -> (f64, f64) {
    if !(natural_w.is_finite() && natural_h.is_finite()) || natural_w < 1.0 || natural_h < 1.0 {
        return FALLBACK_IMAGE_SIZE;
    }
    let long = natural_w.max(natural_h);
    if long <= MAX_IMAGE_EDGE {
        return (natural_w, natural_h);
    }
    let scale = MAX_IMAGE_EDGE / long;
    (natural_w * scale, natural_h * scale)
}

/// Fetch-dedup cache for `Shape::Html` srcdoc content, keyed by asset id
/// (content-addressed — module doc).
///
/// The protocol is claim → resolve: [`Self::begin_fetch`] hands the fetch to
/// exactly one caller (the render loop may ask many times per frame), and
/// the claim is resolved by [`Self::insert`] (success) or [`Self::abandon`]
/// (failure — the id becomes claimable again, so a later doc change retries
/// instead of wedging the frame on one transient error).
#[derive(Default)]
pub(super) struct SrcdocCache {
    ready: HashMap<String, String>,
    inflight: HashSet<String>,
}

impl SrcdocCache {
    #[must_use]
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Claim the fetch for `asset_id`. `true` exactly once per outstanding
    /// claim: already-ready and already-in-flight ids answer `false`.
    #[must_use]
    pub(super) fn begin_fetch(&mut self, asset_id: &str) -> bool {
        if self.ready.contains_key(asset_id) || self.inflight.contains(asset_id) {
            return false;
        }
        self.inflight.insert(asset_id.to_string());
        true
    }

    /// Resolve a claim with content. Assets are immutable, so a later insert
    /// under the same id can only carry identical bytes — last write wins is
    /// indistinguishable from first write wins.
    pub(super) fn insert(&mut self, asset_id: &str, srcdoc: String) {
        self.inflight.remove(asset_id);
        self.ready.insert(asset_id.to_string(), srcdoc);
    }

    /// Resolve a claim with failure: the id becomes claimable again.
    pub(super) fn abandon(&mut self, asset_id: &str) {
        self.inflight.remove(asset_id);
    }

    #[must_use]
    pub(super) fn get(&self, asset_id: &str) -> Option<&str> {
        self.ready.get(asset_id).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_image_gate_is_a_shape_check_not_a_whitelist() {
        assert!(is_image_mime("image/png"));
        assert!(is_image_mime("IMAGE/JPEG"), "case-insensitive");
        // An image mime the server may well refuse still passes here — the
        // server's allowlist is the single authority (module doc).
        assert!(is_image_mime("image/x-icon"));
        assert!(!is_image_mime("text/html"));
        assert!(!is_image_mime("application/octet-stream"));
        assert!(!is_image_mime(""));
    }

    #[test]
    fn the_base64_payload_is_everything_after_the_first_comma() {
        assert_eq!(data_url_base64("data:image/png;base64,AAAA"), Some("AAAA"));
        // Base64 itself never contains a comma, so "first comma" is exact —
        // but a payload that somehow carried one must survive whole.
        assert_eq!(data_url_base64("data:x;base64,AA,BB"), Some("AA,BB"));
        assert_eq!(data_url_base64("no separator"), None);
        assert_eq!(
            data_url_base64("data:image/png;base64,"),
            None,
            "a zero-byte file has no asset to store"
        );
    }

    #[test]
    fn display_size_caps_the_long_edge_and_preserves_aspect() {
        // Small images land at natural size.
        assert_eq!(display_size(200.0, 100.0), (200.0, 100.0));
        // Oversized: long edge pinned to the cap, aspect kept.
        let (w, h) = display_size(3000.0, 1500.0);
        assert_eq!(w, 600.0);
        assert_eq!(h, 300.0);
        // Portrait: the cap follows the longer edge.
        let (w, h) = display_size(1000.0, 4000.0);
        assert_eq!(h, 600.0);
        assert_eq!(w, 150.0);
    }

    #[test]
    fn an_unusable_natural_size_answers_the_fallback_not_a_degenerate_bbox() {
        assert_eq!(display_size(0.0, 0.0), FALLBACK_IMAGE_SIZE);
        assert_eq!(display_size(f64::NAN, 100.0), FALLBACK_IMAGE_SIZE);
        assert_eq!(display_size(100.0, f64::INFINITY), FALLBACK_IMAGE_SIZE);
        assert_eq!(display_size(-5.0, 100.0), FALLBACK_IMAGE_SIZE);
    }

    #[test]
    fn the_srcdoc_cache_hands_each_fetch_to_exactly_one_claimant() {
        let mut cache = SrcdocCache::new();
        assert!(cache.begin_fetch("a.html"), "first claim wins");
        assert!(!cache.begin_fetch("a.html"), "second claim is refused");
        assert!(cache.get("a.html").is_none(), "in flight is not ready");

        cache.insert("a.html", "<p>hi</p>".to_string());
        assert_eq!(cache.get("a.html"), Some("<p>hi</p>"));
        assert!(
            !cache.begin_fetch("a.html"),
            "ready content is never refetched — assets are immutable"
        );
    }

    #[test]
    fn an_abandoned_claim_is_claimable_again_so_failures_can_retry() {
        let mut cache = SrcdocCache::new();
        assert!(cache.begin_fetch("a.html"));
        cache.abandon("a.html");
        assert!(cache.get("a.html").is_none());
        assert!(cache.begin_fetch("a.html"), "failure must not wedge the id");
    }

    /// The cache key is the asset id alone — content-addressed ids make a
    /// canvas-id component pure refetch overhead (module doc).
    #[test]
    fn the_cache_key_is_the_asset_id_alone() {
        let mut cache = SrcdocCache::new();
        cache.insert("aaaa.html", "<p>a</p>".to_string());
        assert_eq!(cache.get("aaaa.html"), Some("<p>a</p>"));
        assert!(cache.get("bbbb.html").is_none());
    }
}
