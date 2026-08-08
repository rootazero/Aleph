//! VFS path computation utilities.

/// Compute parent path from a VFS path
/// "<aleph://user/preferences/coding>/" -> "<aleph://user/preferences>/"
/// "<aleph://user/preferences>/" -> "<aleph://user>/"
/// "<aleph://user>/" -> "aleph://"
#[must_use]
pub fn compute_parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(pos) => format!("{}/", &trimmed[..pos]),
        None => String::new(),
    }
}
