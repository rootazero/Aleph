//! Outbound attachment helpers.
use std::path::Path;

/// The multipart form filename for an attachment (basename, fallback "file").
#[must_use]
pub fn attachment_form_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string())
}

#[cfg(test)]
mod tests {
    use super::attachment_form_name;
    use std::path::Path;

    #[test]
    fn form_name_is_basename() {
        assert_eq!(
            attachment_form_name(Path::new("/tmp/a/photo.png")),
            "photo.png"
        );
        assert_eq!(attachment_form_name(Path::new("noext")), "noext");
    }
}
