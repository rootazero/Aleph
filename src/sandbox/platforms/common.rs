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

/// Truncate captured process output to at most `max_bytes`, never cutting
/// a UTF-8 codepoint in half (project rule P7). Returns the (possibly
/// shortened) buffer and whether truncation occurred.
///
/// The cut index is backed off any UTF-8 continuation byte
/// (`0b10xx_xxxx`), so a multi-byte char is never split — for binary
/// (non-UTF-8) output the worst case drops at most 3 extra bytes.
pub fn truncate_output(mut buf: Vec<u8>, max_bytes: usize) -> (Vec<u8>, bool) {
    if buf.len() <= max_bytes {
        return (buf, false);
    }
    let mut end = max_bytes;
    while end > 0 && (buf[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    buf.truncate(end);
    (buf, true)
}

/// The Unix signal that terminated a child process, if it was killed by a
/// signal rather than exiting normally. `None` for a normal exit. Used to
/// populate `SandboxOutput.signal` so callers can distinguish a SIGSEGV /
/// rlimit-or-cgroup SIGKILL from a clean non-zero exit.
#[cfg(unix)]
pub fn termination_signal(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
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

    #[test]
    fn truncate_output_keeps_short_buffer_intact() {
        let (out, truncated) = truncate_output(b"hello".to_vec(), 1024);
        assert_eq!(out, b"hello");
        assert!(!truncated);
    }

    #[test]
    fn truncate_output_at_exact_len_does_not_truncate() {
        let (out, truncated) = truncate_output(b"hello".to_vec(), 5);
        assert_eq!(out, b"hello");
        assert!(!truncated);
    }

    #[test]
    fn truncate_output_cuts_ascii_at_boundary() {
        let (out, truncated) = truncate_output(b"hello world".to_vec(), 5);
        assert_eq!(out, b"hello");
        assert!(truncated);
    }

    #[test]
    fn truncate_output_never_splits_a_multibyte_codepoint() {
        // "a€b" = 61 E2 82 AC 62 — the euro sign is a 3-byte sequence.
        let buf = "a€b".as_bytes().to_vec();
        // Cutting at 2 lands inside the euro sign → must back off to "a".
        let (out, truncated) = truncate_output(buf.clone(), 2);
        assert_eq!(out, b"a");
        assert!(truncated);
        assert!(
            std::str::from_utf8(&out).is_ok(),
            "result must stay valid UTF-8"
        );
        // Cutting at 4 lands exactly after the euro sign → "a€" kept whole.
        let (out, truncated) = truncate_output(buf, 4);
        assert_eq!(std::str::from_utf8(&out).unwrap(), "a€");
        assert!(truncated);
    }

    #[test]
    fn truncate_output_zero_max_yields_empty() {
        let (out, truncated) = truncate_output(b"x".to_vec(), 0);
        assert!(out.is_empty());
        assert!(truncated);
    }

    #[cfg(unix)]
    #[test]
    fn termination_signal_reports_killed_child() {
        // A child killed by a signal must surface that signal, and have
        // no normal exit code — this is what BUG-9 wires into
        // SandboxOutput.signal.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        child.kill().expect("kill child");
        let status = child.wait().expect("wait for child");
        assert_eq!(termination_signal(&status), Some(9), "SIGKILL is signal 9");
        assert_eq!(status.code(), None, "a signalled process has no exit code");
    }

    #[cfg(unix)]
    #[test]
    fn termination_signal_is_none_for_clean_exit() {
        let status = std::process::Command::new("true")
            .status()
            .expect("run /usr/bin/true");
        assert_eq!(termination_signal(&status), None);
        assert_eq!(status.code(), Some(0));
    }
}
