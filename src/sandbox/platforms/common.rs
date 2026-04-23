use std::path::{Path, PathBuf};

pub const LINUX_PLATFORM_DEFAULT_READ_ROOTS: &[&str] = &[
    "/bin",
    "/sbin",
    "/usr",
    "/etc",
    "/lib",
    "/lib64",
    "/nix/store",
    "/run/current-system/sw",
];

pub fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|content| content.to_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

pub fn wsl_version() -> Option<u32> {
    if !is_wsl() {
        return None;
    }

    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|content| if content.contains("WSL2") { 2 } else { 1 })
}

pub fn normalize_path_for_sandbox(path: &Path, cwd: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        Some(cwd.join(path))
    }
}

pub fn path_is_allowed(path: &Path, allowed: &[PathBuf]) -> bool {
    allowed.iter().any(|prefix| path.starts_with(prefix))
}

pub fn glob_to_regex(pattern: &str) -> Option<String> {
    if pattern.is_empty() {
        return None;
    }

    let mut regex = String::with_capacity(pattern.len() * 2);
    regex.push('^');

    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    regex.push_str(".*");
                } else {
                    regex.push_str("[^/]*");
                }
            }
            '?' => regex.push_str("[^/]"),
            '.' => regex.push_str("\\."),
            '+' => regex.push_str("\\+"),
            '(' => regex.push_str("\\("),
            ')' => regex.push_str("\\)"),
            '[' => regex.push_str("\\["),
            ']' => regex.push_str("\\]"),
            '{' => regex.push_str("\\{"),
            '}' => regex.push_str("\\}"),
            '^' => regex.push_str("\\^"),
            '$' => regex.push_str("\\$"),
            '|' => regex.push_str("\\|"),
            '\\' => regex.push_str("\\\\"),
            '/' => regex.push('/'),
            c => regex.push(c),
        }
    }

    regex.push('$');
    Some(regex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_absolute_path() {
        let path = Path::new("/usr/bin/python");
        let cwd = Path::new("/home/user");
        assert_eq!(
            normalize_path_for_sandbox(path, cwd),
            Some(PathBuf::from("/usr/bin/python"))
        );
    }

    #[test]
    fn normalize_relative_path() {
        let path = Path::new("src/main.rs");
        let cwd = Path::new("/home/user/project");
        assert_eq!(
            normalize_path_for_sandbox(path, cwd),
            Some(PathBuf::from("/home/user/project/src/main.rs"))
        );
    }

    #[test]
    fn normalize_empty_path() {
        let path = Path::new("");
        let cwd = Path::new("/home/user");
        assert_eq!(normalize_path_for_sandbox(path, cwd), None);
    }

    #[test]
    fn normalize_dot_path() {
        let path = Path::new(".");
        let cwd = Path::new("/home/user");
        assert_eq!(
            normalize_path_for_sandbox(path, cwd),
            Some(PathBuf::from("/home/user/."))
        );
    }

    #[test]
    fn path_allowed_exact_match() {
        let path = Path::new("/home/user/project/src/main.rs");
        let allowed = vec![PathBuf::from("/home/user/project")];
        assert!(path_is_allowed(path, &allowed));
    }

    #[test]
    fn path_allowed_multiple_prefixes() {
        let path = Path::new("/tmp/test.txt");
        let allowed = vec![
            PathBuf::from("/home/user"),
            PathBuf::from("/tmp"),
            PathBuf::from("/var"),
        ];
        assert!(path_is_allowed(path, &allowed));
    }

    #[test]
    fn path_not_allowed() {
        let path = Path::new("/etc/passwd");
        let allowed = vec![PathBuf::from("/home/user"), PathBuf::from("/tmp")];
        assert!(!path_is_allowed(path, &allowed));
    }

    #[test]
    fn path_allowed_empty_list() {
        let path = Path::new("/home/user/file.txt");
        let allowed: Vec<PathBuf> = vec![];
        assert!(!path_is_allowed(path, &allowed));
    }

    #[test]
    fn glob_star_matches_single_segment() {
        let regex = glob_to_regex("*.rs").unwrap();
        assert_eq!(regex, "^[^/]*\\.rs$");
    }

    #[test]
    fn glob_double_star_matches_any() {
        let regex = glob_to_regex("src/**/*.rs").unwrap();
        assert_eq!(regex, "^src/.*/[^/]*\\.rs$");
    }

    #[test]
    fn glob_question_mark() {
        let regex = glob_to_regex("file?.txt").unwrap();
        assert_eq!(regex, "^file[^/]\\.txt$");
    }

    #[test]
    fn glob_literal_match() {
        let regex = glob_to_regex("hello.txt").unwrap();
        assert_eq!(regex, "^hello\\.txt$");
    }

    #[test]
    fn glob_empty_pattern() {
        assert_eq!(glob_to_regex(""), None);
    }

    #[test]
    fn glob_special_chars_escaped() {
        let regex = glob_to_regex("file(name)+[1].txt").unwrap();
        assert_eq!(regex, "^file\\(name\\)\\+\\[1\\]\\.txt$");
    }

    #[test]
    fn glob_mixed_pattern() {
        let regex = glob_to_regex("src/**/test_*.rs").unwrap();
        assert_eq!(regex, "^src/.*/test_[^/]*\\.rs$");
    }

    #[test]
    fn linux_platform_defaults_not_empty() {
        assert!(!LINUX_PLATFORM_DEFAULT_READ_ROOTS.is_empty());
        assert!(LINUX_PLATFORM_DEFAULT_READ_ROOTS.contains(&"/usr"));
        assert!(LINUX_PLATFORM_DEFAULT_READ_ROOTS.contains(&"/bin"));
    }
}
