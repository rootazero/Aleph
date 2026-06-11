//! Request/response shapes for paid Suno API wrappers (sunoapi.com /
//! sunoaiapi.com / sunoapi.org and forks). Schema converges on this shape:
//!
//! * `POST <base>/api/generate` with `{ prompt, mv, title?, tags?,
//!   make_instrumental?, wait_audio: false }` → array of clip skeletons.
//! * `GET <base>/api/get?ids=<csv>` → array of clip records, each carrying
//!   `status` plus `audio_url` once Suno has finished rendering.
//!
//! Clip status values observed across wrappers: `submitted` → `queued` →
//! `streaming` → `complete` (or `error`). We treat `complete` as terminal-OK
//! and `error` as terminal-failure; everything else means "keep polling".

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct SunoGenerateRequest {
    pub prompt: String,
    /// Model / "music video" identifier. The naming `mv` is unfortunate but
    /// matches every wrapper we tested.
    pub mv: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    #[serde(default)]
    pub make_instrumental: bool,
    /// Tell the wrapper not to block on audio rendering — we own the polling.
    pub wait_audio: bool,
}

/// A single Suno clip record. Field set is the lowest common denominator across
/// `/api/generate` (returns skeletons immediately) and `/api/get` (returns the
/// same record enriched with `audio_url` once render completes).
#[derive(Debug, Clone, Deserialize)]
pub struct SunoClip {
    pub id: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub audio_url: Option<String>,
    #[serde(default)]
    pub video_url: Option<String>,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub lyric: Option<String>,
    #[serde(default)]
    pub duration: Option<f32>,
    #[serde(default, rename = "type")]
    #[allow(dead_code)]
    pub clip_type: Option<String>,
}

impl SunoClip {
    /// True if the wrapper has reported terminal success and the audio URL
    /// is present.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        matches!(self.status.as_deref(), Some("complete"))
            && self.audio_url.as_deref().is_some_and(|u| !u.is_empty())
    }

    #[must_use]
    pub fn is_error(&self) -> bool {
        matches!(self.status.as_deref(), Some("error"))
    }
}

/// Error envelope returned by some wrappers on 4xx. Best-effort — many
/// wrappers just return plain text on failure.
#[derive(Debug, Clone, Deserialize)]
pub struct SunoError {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

impl SunoError {
    #[must_use]
    pub fn best_message(&self) -> Option<String> {
        self.message
            .clone()
            .or_else(|| self.error.clone())
            .or_else(|| self.detail.clone())
    }
}
