use crate::{DesktopError, OcrLine, OcrResult, Result};
use std::io::Write;

pub fn linux_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    let mut child = std::process::Command::new("tesseract")
        .arg("stdin")
        .arg("stdout")
        .arg("-l")
        .arg("chi_sim+eng")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            DesktopError::OcrFailed(format!(
                "Tesseract execution failed (install tesseract-ocr): {e}"
            ))
        })?;

    if let Some(ref mut stdin) = child.stdin {
        stdin
            .write_all(png_bytes)
            .map_err(|e| DesktopError::OcrFailed(format!("Failed to feed PNG to tesseract: {e}")))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| DesktopError::OcrFailed(format!("Tesseract process failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::OcrFailed(format!(
            "Tesseract error: {}",
            stderr.trim()
        )));
    }

    let full_text = String::from_utf8_lossy(&output.stdout).to_string();

    let lines: Vec<OcrLine> = full_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| OcrLine {
            text: line.to_string(),
            bounding_box: None,
            confidence: None,
        })
        .collect();

    Ok(OcrResult { full_text, lines })
}
