//! Linux OCR via the `tesseract` CLI, using its TSV output to recover
//! per-line bounding boxes and confidences (parity with the macOS Vision and
//! Windows `WinRT` backends, which both supply boxes).
//!
//! The previous implementation read tesseract's plain-text stdout and left
//! every `bounding_box` as `None`, which silently disabled text-anchored GUI
//! location on Linux (`builtin_tools::desktop::gui_locate` does
//! `let b = line.bounding_box?;`). Switching to the `tsv` configfile yields
//! word-level geometry that we group back into lines.

use crate::{BoundingBox, DesktopError, OcrLine, OcrResult, Result};
use std::time::Duration;

/// Hard deadline for one tesseract run.
///
/// OCR of a full-screen capture is slow-but-working territory, so this is far
/// above [`crate::script_exec::DESKTOP_QUERY_TIMEOUT`]; the cap exists for the
/// wedged case (a tesseract that never finishes pins the agent turn until the
/// harness ceiling and leaks the child — the same failure shape `xclip`,
/// `notify-send` and `ffmpeg` each taught once). 30 s covers a 4K screen on a
/// slow box with room to spare.
const OCR_TIMEOUT: Duration = Duration::from_secs(30);

/// Only *called* under `#[cfg(target_os = "linux")]` in `perform_ocr`; compiled
/// everywhere so the pure [`parse_tesseract_tsv`] parser stays host-testable.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn linux_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    let mut cmd = std::process::Command::new("tesseract");
    cmd.arg("stdin")
        .arg("stdout")
        .arg("-l")
        .arg("chi_sim+eng")
        // `tsv` is a tesseract configfile that emits a tab-separated table with
        // per-word geometry + confidence instead of plain text.
        .arg("tsv");

    // stdin fed on its own thread, stdout/stderr drained on theirs, hard
    // deadline with kill-and-reap — see `output_capped_blocking_with_stdin`
    // for why hand-rolling any of the three is a known loss.
    let output = crate::script_exec::output_capped_blocking_with_stdin(
        cmd,
        Some(png_bytes),
        OCR_TIMEOUT,
        "tesseract OCR",
    )
    .map_err(|e| {
        if crate::script_exec::is_spawn_failure(&e) {
            DesktopError::OcrFailed(format!(
                "Tesseract execution failed (install tesseract-ocr): {e}"
            ))
        } else {
            DesktopError::OcrFailed(e.to_string())
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::OcrFailed(format!(
            "Tesseract error: {}",
            stderr.trim()
        )));
    }

    let tsv = String::from_utf8_lossy(&output.stdout);
    Ok(parse_tesseract_tsv(&tsv))
}

/// Parse tesseract's TSV output into an [`OcrResult`] with per-line boxes.
///
/// TSV columns (tab-separated, with a header row):
/// `level page block par line word left top width height conf text`
///
/// Only `level == 5` rows carry a recognized word. Words sharing the same
/// `(block, par, line)` tuple form one [`OcrLine`]; their boxes are merged
/// (min-left/top, max-right/bottom) and confidences averaged. `full_text`
/// joins line texts with newlines, matching the other platforms' line model.
fn parse_tesseract_tsv(tsv: &str) -> OcrResult {
    // Accumulate words into ordered line groups keyed by (block, par, line).
    struct LineAccum {
        key: (u32, u32, u32),
        words: Vec<String>,
        min_x: f64,
        min_y: f64,
        max_x: f64,
        max_y: f64,
        conf_sum: f64,
        conf_n: u32,
    }

    let mut groups: Vec<LineAccum> = Vec::new();

    for row in tsv.lines() {
        // Skip the header and any blank line.
        if row.is_empty() || row.starts_with("level") {
            continue;
        }
        let cols: Vec<&str> = row.split('\t').collect();
        if cols.len() < 12 {
            continue;
        }
        // Word-level rows only.
        if cols[0].trim() != "5" {
            continue;
        }
        let text = cols[11].trim();
        if text.is_empty() {
            continue;
        }

        let block: u32 = cols[2].trim().parse().unwrap_or(0);
        let par: u32 = cols[3].trim().parse().unwrap_or(0);
        let line: u32 = cols[4].trim().parse().unwrap_or(0);
        let left: f64 = cols[6].trim().parse().unwrap_or(0.0);
        let top: f64 = cols[7].trim().parse().unwrap_or(0.0);
        let width: f64 = cols[8].trim().parse().unwrap_or(0.0);
        let height: f64 = cols[9].trim().parse().unwrap_or(0.0);
        // tesseract reports conf as a float 0..100; -1 for non-word levels.
        let conf: f64 = cols[10].trim().parse().unwrap_or(-1.0);

        let key = (block, par, line);
        let right = left + width;
        let bottom = top + height;

        match groups.last_mut() {
            Some(acc) if acc.key == key => {
                acc.words.push(text.to_string());
                acc.min_x = acc.min_x.min(left);
                acc.min_y = acc.min_y.min(top);
                acc.max_x = acc.max_x.max(right);
                acc.max_y = acc.max_y.max(bottom);
                if conf >= 0.0 {
                    acc.conf_sum += conf;
                    acc.conf_n += 1;
                }
            }
            _ => groups.push(LineAccum {
                key,
                words: vec![text.to_string()],
                min_x: left,
                min_y: top,
                max_x: right,
                max_y: bottom,
                conf_sum: if conf >= 0.0 { conf } else { 0.0 },
                conf_n: u32::from(conf >= 0.0),
            }),
        }
    }

    let lines: Vec<OcrLine> = groups
        .into_iter()
        .map(|acc| OcrLine {
            text: acc.words.join(" "),
            bounding_box: Some(BoundingBox {
                x: acc.min_x,
                y: acc.min_y,
                w: acc.max_x - acc.min_x,
                h: acc.max_y - acc.min_y,
            }),
            confidence: if acc.conf_n > 0 {
                // Normalize to 0..1 to match the macOS Vision backend.
                Some((acc.conf_sum / f64::from(acc.conf_n)) / 100.0)
            } else {
                None
            },
        })
        .collect();

    let full_text = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    OcrResult { full_text, lines }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TSV: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext
1\t1\t0\t0\t0\t0\t0\t0\t300\t100\t-1\t
2\t1\t1\t0\t0\t0\t10\t10\t280\t40\t-1\t
3\t1\t1\t1\t0\t0\t10\t10\t280\t40\t-1\t
4\t1\t1\t1\t1\t0\t10\t10\t280\t20\t-1\t
5\t1\t1\t1\t1\t1\t10\t10\t100\t20\t95.5\tHello
5\t1\t1\t1\t1\t2\t120\t12\t170\t18\t90.0\tworld
4\t1\t1\t1\t2\t0\t10\t40\t120\t20\t-1\t
5\t1\t1\t1\t2\t1\t10\t40\t120\t20\t88.0\tSecond";

    #[test]
    fn groups_words_into_lines_with_boxes() {
        let result = parse_tesseract_tsv(SAMPLE_TSV);
        assert_eq!(result.lines.len(), 2);

        let first = &result.lines[0];
        assert_eq!(first.text, "Hello world");
        let bb = first.bounding_box.as_ref().expect("line 1 has a box");
        // min-left 10, min-top 10, max-right max(110, 290)=290, max-bottom max(30,30)=30.
        assert_eq!(bb.x, 10.0);
        assert_eq!(bb.y, 10.0);
        assert_eq!(bb.w, 280.0);
        assert_eq!(bb.h, 20.0);
        // Average confidence (95.5 + 90.0) / 2 = 92.75 → /100.
        let conf = first.confidence.expect("confidence present");
        assert!((conf - 0.92750).abs() < 1e-6, "got {conf}");

        assert_eq!(result.lines[1].text, "Second");
    }

    #[test]
    fn full_text_joins_lines_with_newlines() {
        let result = parse_tesseract_tsv(SAMPLE_TSV);
        assert_eq!(result.full_text, "Hello world\nSecond");
    }

    #[test]
    fn empty_or_header_only_yields_no_lines() {
        let header = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n";
        let result = parse_tesseract_tsv(header);
        assert!(result.lines.is_empty());
        assert!(result.full_text.is_empty());

        let result = parse_tesseract_tsv("");
        assert!(result.lines.is_empty());
    }

    #[test]
    fn skips_blank_word_rows_and_short_rows() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext
5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t80.0\t
5\t1\t1\t1\t1\t2\t70\t10\t50\t20\t80.0\tword
too\tshort\trow";
        let result = parse_tesseract_tsv(tsv);
        assert_eq!(result.lines.len(), 1);
        assert_eq!(result.lines[0].text, "word");
    }

    #[test]
    fn missing_confidence_leaves_none() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext
5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t-1\tword";
        let result = parse_tesseract_tsv(tsv);
        assert_eq!(result.lines.len(), 1);
        assert!(result.lines[0].confidence.is_none());
        assert!(result.lines[0].bounding_box.is_some());
    }
}
