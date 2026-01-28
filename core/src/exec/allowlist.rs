//! Allowlist pattern matching for executables.

use super::analysis::CommandResolution;
use super::config::AllowlistEntry;
use std::path::Path;

/// Check if a resolution matches any allowlist entry
pub fn match_allowlist<'a>(
    allowlist: &'a [AllowlistEntry],
    resolution: &CommandResolution,
) -> Option<&'a AllowlistEntry> {
    for entry in allowlist {
        if matches_entry(entry, resolution) {
            return Some(entry);
        }
    }
    None
}

/// Check if a resolution matches a single entry
fn matches_entry(entry: &AllowlistEntry, resolution: &CommandResolution) -> bool {
    let pattern = &entry.pattern;

    // Exact executable name match (e.g., "git")
    if !pattern.contains('/') && !pattern.contains('*') {
        return resolution.executable_name.eq_ignore_ascii_case(pattern);
    }

    // Wildcard pattern (e.g., "~/bin/*", "/usr/local/bin/*")
    if pattern.ends_with("/*") {
        let dir_pattern = &pattern[..pattern.len() - 2];
        let expanded = expand_home(dir_pattern);

        if let Some(resolved) = &resolution.resolved_path {
            if let Some(parent) = resolved.parent() {
                return parent.to_string_lossy().eq_ignore_ascii_case(&expanded);
            }
        }
        return false;
    }

    // Glob pattern with * in middle (e.g., "git-*")
    if pattern.contains('*') {
        return glob_match(pattern, &resolution.executable_name)
            || resolution
                .resolved_path
                .as_ref()
                .map(|p| glob_match(pattern, &p.to_string_lossy()))
                .unwrap_or(false);
    }

    // Absolute or relative path match
    let expanded = expand_home(pattern);
    if let Some(resolved) = &resolution.resolved_path {
        return resolved.to_string_lossy().eq_ignore_ascii_case(&expanded);
    }

    // Raw executable match
    resolution.raw_executable.eq_ignore_ascii_case(&expanded)
}

/// Expand ~ to home directory
fn expand_home(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}{}", home.display(), &path[1..]);
        }
    }
    path.to_string()
}

/// Simple glob matching with * wildcard
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let text = text.to_lowercase();

    let parts: Vec<&str> = pattern.split('*').collect();

    if parts.len() == 1 {
        return pattern == text;
    }

    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        if let Some(found) = text[pos..].find(part) {
            if i == 0 && found != 0 {
                return false; // First part must match at start
            }
            pos += found + part.len();
        } else {
            return false;
        }
    }

    // Last part must match at end
    if let Some(last) = parts.last() {
        if !last.is_empty() && !text.ends_with(last) {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(pattern: &str) -> AllowlistEntry {
        AllowlistEntry {
            id: None,
            pattern: pattern.to_string(),
            last_used_at: None,
            last_used_command: None,
            last_resolved_path: None,
        }
    }

    fn resolution(name: &str, path: Option<&str>) -> CommandResolution {
        CommandResolution {
            raw_executable: name.to_string(),
            resolved_path: path.map(PathBuf::from),
            executable_name: name.to_string(),
        }
    }

    #[test]
    fn test_exact_name_match() {
        let entries = vec![entry("git")];
        let res = resolution("git", Some("/usr/bin/git"));

        assert!(match_allowlist(&entries, &res).is_some());
    }

    #[test]
    fn test_exact_name_case_insensitive() {
        let entries = vec![entry("Git")];
        let res = resolution("git", Some("/usr/bin/git"));

        assert!(match_allowlist(&entries, &res).is_some());
    }

    #[test]
    fn test_exact_path_match() {
        let entries = vec![entry("/usr/bin/git")];
        let res = resolution("git", Some("/usr/bin/git"));

        assert!(match_allowlist(&entries, &res).is_some());
    }

    #[test]
    fn test_directory_wildcard() {
        let entries = vec![entry("/usr/bin/*")];
        let res = resolution("git", Some("/usr/bin/git"));

        assert!(match_allowlist(&entries, &res).is_some());
    }

    #[test]
    fn test_directory_wildcard_no_match() {
        let entries = vec![entry("/usr/local/bin/*")];
        let res = resolution("git", Some("/usr/bin/git"));

        assert!(match_allowlist(&entries, &res).is_none());
    }

    #[test]
    fn test_glob_pattern() {
        let entries = vec![entry("git-*")];
        let res = resolution("git-rebase", Some("/usr/bin/git-rebase"));

        assert!(match_allowlist(&entries, &res).is_some());
    }

    #[test]
    fn test_no_match() {
        let entries = vec![entry("npm")];
        let res = resolution("git", Some("/usr/bin/git"));

        assert!(match_allowlist(&entries, &res).is_none());
    }

    #[test]
    fn test_glob_match_simple() {
        assert!(glob_match("git-*", "git-rebase"));
        assert!(glob_match("*-test", "my-test"));
        assert!(glob_match("foo*bar", "fooxyzbar"));
        assert!(!glob_match("git-*", "npm"));
    }
}
