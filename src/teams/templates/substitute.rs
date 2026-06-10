//! Forgiving `{var}` substitution for template fields.
//!
//! Unknown braces are left intact (matches ClawTeam's `_SafeDict` behaviour),
//! so authoring stays low-friction — a typo in a placeholder shows up in the
//! materialized text rather than aborting the whole template.

use std::collections::HashMap;

/// Replace `{key}` occurrences with the matching value. Unknown keys pass
/// through unchanged. Braces are NOT escaped — there's no `{{ }}` literal
/// support because templates don't need it.
#[must_use]
pub fn substitute(input: &str, vars: &HashMap<&str, &str>) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end_rel) = bytes[i + 1..].iter().position(|&b| b == b'}') {
                let end = i + 1 + end_rel;
                let key = &input[i + 1..end];
                if let Some(value) = vars.get(key) {
                    out.push_str(value);
                    i = end + 1;
                    continue;
                }
            }
        }
        // SAFETY: byte index `i` always lies on a UTF-8 char boundary because
        // we only advance past whole `{...}` segments (ASCII) above, otherwise
        // we copy one char at a time below.
        let ch = match input[i..].chars().next() {
            Some(c) => c,
            None => break,
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_known_vars() {
        let mut v = HashMap::new();
        v.insert("goal", "ship feature X");
        v.insert("team_name", "alpha");
        let s = substitute("Build {goal} for team {team_name}", &v);
        assert_eq!(s, "Build ship feature X for team alpha");
    }

    #[test]
    fn leaves_unknown_vars_intact() {
        let v: HashMap<&str, &str> = HashMap::new();
        let s = substitute("Hello {unknown}", &v);
        assert_eq!(s, "Hello {unknown}");
    }

    #[test]
    fn handles_unterminated_brace() {
        let v: HashMap<&str, &str> = HashMap::new();
        let s = substitute("Hello {oops never closes", &v);
        assert_eq!(s, "Hello {oops never closes");
    }

    #[test]
    fn handles_empty_braces() {
        let v: HashMap<&str, &str> = HashMap::new();
        let s = substitute("Empty {} brace", &v);
        assert_eq!(s, "Empty {} brace");
    }

    #[test]
    fn handles_unicode_chars() {
        let mut v = HashMap::new();
        v.insert("name", "中文");
        let s = substitute("你好 {name} 世界", &v);
        assert_eq!(s, "你好 中文 世界");
    }

    #[test]
    fn multiple_substitutions_in_one_pass() {
        let mut v = HashMap::new();
        v.insert("x", "1");
        v.insert("y", "2");
        let s = substitute("{x}+{y}={x}+{y}", &v);
        assert_eq!(s, "1+2=1+2");
    }
}
