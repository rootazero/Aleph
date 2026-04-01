//! VFS preset paths and path computation utilities.

/// Preset directory paths for the aleph:// VFS
pub const PRESET_PATHS: &[(&str, &str)] = &[
    ("aleph://user/", "User domain root"),
    ("aleph://user/preferences/", "User preferences"),
    ("aleph://user/personal/", "Personal information"),
    ("aleph://user/plans/", "User plans and goals"),
    ("aleph://knowledge/", "Knowledge domain root"),
    ("aleph://knowledge/learning/", "Learning records"),
    ("aleph://knowledge/projects/", "Project knowledge"),
    ("aleph://agent/", "Agent domain root"),
    ("aleph://agent/tools/", "Tool usage experiences"),
    ("aleph://agent/experiences/", "Cortex experiences"),
    ("aleph://session/", "Session temporary data"),
];

/// Compute parent path from a VFS path
/// "aleph://user/preferences/coding/" -> "aleph://user/preferences/"
/// "aleph://user/preferences/" -> "aleph://user/"
/// "aleph://user/" -> "aleph://"
pub fn compute_parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(pos) => format!("{}/", &trimmed[..pos]),
        None => String::new(),
    }
}
