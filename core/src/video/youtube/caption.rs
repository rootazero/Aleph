//! Caption fetching logic including yt-dlp fallback

use crate::error::{AlephError, Result};
use crate::runtimes::RuntimeRegistry;
use std::path::PathBuf;
use tracing::debug;

/// Get yt-dlp executable path using RuntimeRegistry
///
/// Auto-installs yt-dlp if not present (lazy installation).
pub async fn get_ytdlp_path() -> Result<PathBuf> {
    let registry = RuntimeRegistry::new()?;
    let ytdlp = registry.require("yt-dlp").await?;
    Ok(ytdlp.executable_path())
}

/// Fetch caption using yt-dlp command-line tool as fallback
///
/// yt-dlp has sophisticated anti-bot bypass mechanisms that often work
/// when direct HTTP requests fail. Will auto-install yt-dlp if not found.
///
/// Tries preferred language first, falls back to English, then any available language.
pub async fn fetch_caption_via_ytdlp(video_id: &str, preferred_lang: &str) -> Result<String> {
    use std::fs;
    use std::process::Command;

    // Get yt-dlp path (auto-installs if not present)
    let ytdlp = get_ytdlp_path().await?;
    let temp_dir = std::env::temp_dir();
    let output_template = temp_dir.join(format!("aleph_sub_{}", video_id));
    let url = format!("https://www.youtube.com/watch?v={}", video_id);

    debug!(video_id = %video_id, lang = %preferred_lang, "Fetching caption via yt-dlp");

    // Build language priority list: preferred language, English, then all others
    // The "all" keyword tells yt-dlp to download any available subtitle as last resort
    let lang_list = if preferred_lang == "en" {
        "en,zh,ja,ko,all".to_string()
    } else if preferred_lang == "zh" {
        "zh,zh-Hans,zh-Hant,en,all".to_string()
    } else {
        format!("{},en,zh,all", preferred_lang)
    };

    // Run yt-dlp to download subtitles with language fallback
    let output = Command::new(&ytdlp)
        .args([
            "--no-check-certificates", // Bypass SSL issues
            "--write-auto-sub",        // Download auto-generated subtitles
            "--write-sub",             // Also try manual subtitles (often better quality)
            "--sub-langs",
            &lang_list, // Try multiple languages with "all" as fallback
            "--sub-format",
            "vtt",
            "--skip-download", // Don't download video
            "-o",
            output_template.to_str().unwrap_or("/tmp/aleph_sub"),
            &url,
        ])
        .output()
        .map_err(|e| AlephError::video(format!("Failed to run yt-dlp: {}", e)))?;

    // Note: yt-dlp may exit successfully even if no subtitles found (just logs a warning)
    // So we need to check for actual subtitle files instead of just exit status
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!(stderr = %stderr, "yt-dlp failed");
        return Err(AlephError::video_with_suggestion(
            "yt-dlp failed to download subtitles",
            "The video may not have captions available, or there may be network issues.",
        ));
    }

    // Find the downloaded subtitle file, trying in priority order
    // Build a list of candidate paths based on language preference
    let mut candidate_paths: Vec<std::path::PathBuf> = Vec::new();

    // Add preferred language and its variants
    if preferred_lang == "zh" {
        candidate_paths.push(temp_dir.join(format!("aleph_sub_{}.zh.vtt", video_id)));
        candidate_paths.push(temp_dir.join(format!("aleph_sub_{}.zh-Hans.vtt", video_id)));
        candidate_paths.push(temp_dir.join(format!("aleph_sub_{}.zh-Hant.vtt", video_id)));
        candidate_paths.push(temp_dir.join(format!("aleph_sub_{}.zh-CN.vtt", video_id)));
        candidate_paths.push(temp_dir.join(format!("aleph_sub_{}.zh-TW.vtt", video_id)));
    } else {
        candidate_paths
            .push(temp_dir.join(format!("aleph_sub_{}.{}.vtt", video_id, preferred_lang)));
    }

    // Add English as fallback
    if preferred_lang != "en" {
        candidate_paths.push(temp_dir.join(format!("aleph_sub_{}.en.vtt", video_id)));
    }

    // Find the first existing subtitle file from candidates
    let subtitle_path = candidate_paths.iter().find(|p| p.exists()).cloned().or_else(|| {
        // Try to find any .vtt file that matches as last resort
        let pattern = format!("aleph_sub_{}.", video_id);
        if let Ok(entries) = fs::read_dir(&temp_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&pattern) && name.ends_with(".vtt") {
                    debug!(file = %name, "Found alternative subtitle file");
                    return Some(entry.path());
                }
            }
        }
        None
    }).ok_or_else(|| {
        // Log the stdout/stderr for debugging
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!(stdout = %stdout, stderr = %stderr, "No subtitle files found after yt-dlp");
        AlephError::video_with_suggestion(
            "No subtitles available for this video",
            "The video may not have captions (auto-generated or manual) in any supported language.",
        )
    })?;

    debug!(path = ?subtitle_path, "Using subtitle file");

    // Read and convert VTT to our format
    let vtt_content = fs::read_to_string(&subtitle_path)
        .map_err(|e| AlephError::video(format!("Failed to read subtitle file: {}", e)))?;

    // Clean up temp file
    let _ = fs::remove_file(&subtitle_path);

    debug!(
        len = vtt_content.len(),
        "Caption fetched via yt-dlp successfully"
    );

    // Return VTT content - will be parsed by parse_transcript_vtt
    Ok(vtt_content)
}
