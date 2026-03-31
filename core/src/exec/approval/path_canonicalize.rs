//! Path canonicalization for secure scope validation.
//!
//! Resolves symlinks, normalizes `..` segments, decodes percent-encoding,
//! and rejects null bytes before checking if a path falls within allowed scopes.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum PathSecurityError {
    #[error("path contains null byte")]
    NullByte,
    #[error("path escapes allowed scope: {path}")]
    ScopeEscape { path: String },
    #[error("empty path")]
    EmptyPath,
}

pub fn validate_path_in_scope(
    path: &str,
    allowed_scopes: &[PathBuf],
) -> Result<PathBuf, PathSecurityError> {
    if path.is_empty() {
        return Err(PathSecurityError::EmptyPath);
    }
    if path.contains('\0') {
        return Err(PathSecurityError::NullByte);
    }

    let decoded = percent_decode(path);
    let canonical = safe_canonicalize(&decoded);

    for scope in allowed_scopes {
        let scope_canonical = safe_canonicalize(&scope.to_string_lossy());
        if canonical.starts_with(&scope_canonical) {
            return Ok(canonical);
        }
    }

    Err(PathSecurityError::ScopeEscape {
        path: canonical.display().to_string(),
    })
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                result.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&result).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn safe_canonicalize(path: &str) -> PathBuf {
    let p = Path::new(path);
    if let Ok(canonical) = std::fs::canonicalize(p) {
        return canonical;
    }

    let mut existing = PathBuf::new();
    let mut remaining = Vec::new();
    let components: Vec<_> = p.components().collect();
    for (i, component) in components.iter().enumerate() {
        let mut test_path = existing.clone();
        test_path.push(component);
        if test_path.exists() {
            existing = std::fs::canonicalize(&test_path).unwrap_or(test_path);
        } else {
            remaining = components[i..].to_vec();
            break;
        }
    }

    if existing.as_os_str().is_empty() {
        return normalize_components(p);
    }

    let mut result = existing;
    for component in remaining {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            Component::Normal(s) => result.push(s),
            other => result.push(other),
        }
    }
    result
}

fn normalize_components(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            other => result.push(other),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rejects_null_byte() {
        let result = validate_path_in_scope("/tmp/file\0.txt", &[PathBuf::from("/tmp")]);
        assert!(matches!(result, Err(PathSecurityError::NullByte)));
    }

    #[test]
    fn test_rejects_empty_path() {
        let result = validate_path_in_scope("", &[PathBuf::from("/tmp")]);
        assert!(matches!(result, Err(PathSecurityError::EmptyPath)));
    }

    #[test]
    fn test_allows_path_in_scope() {
        let result = validate_path_in_scope("/tmp/myfile.txt", &[PathBuf::from("/tmp")]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_blocks_path_traversal() {
        let result = validate_path_in_scope("/tmp/../../etc/passwd", &[PathBuf::from("/tmp")]);
        assert!(matches!(result, Err(PathSecurityError::ScopeEscape { .. })));
    }

    #[test]
    fn test_blocks_percent_encoded_traversal() {
        let result =
            validate_path_in_scope("/tmp/%2e%2e/%2e%2e/etc/passwd", &[PathBuf::from("/tmp")]);
        assert!(matches!(result, Err(PathSecurityError::ScopeEscape { .. })));
    }

    #[test]
    fn test_multiple_scopes() {
        let scopes = vec![PathBuf::from("/tmp"), PathBuf::from("/var/log")];
        assert!(validate_path_in_scope("/var/log/syslog", &scopes).is_ok());
        assert!(validate_path_in_scope("/tmp/test", &scopes).is_ok());
        assert!(matches!(
            validate_path_in_scope("/etc/passwd", &scopes),
            Err(PathSecurityError::ScopeEscape { .. })
        ));
    }

    #[test]
    fn test_normalize_dotdot() {
        let result = normalize_components(Path::new("/a/b/../c"));
        assert_eq!(result, PathBuf::from("/a/c"));
    }

    #[test]
    fn test_percent_decode() {
        assert_eq!(percent_decode("%2e%2e"), "..");
        assert_eq!(percent_decode("normal"), "normal");
        assert_eq!(percent_decode("%2Fetc%2Fpasswd"), "/etc/passwd");
    }
}
