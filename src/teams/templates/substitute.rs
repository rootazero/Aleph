//! Forgiving `{var}` substitution for template fields.
//!
//! Unknown braces are left intact (matches `ClawTeam`'s `_SafeDict` behaviour),
//! so authoring stays low-friction — a typo in a placeholder shows up in the
//! materialized text rather than aborting the whole template.

use std::collections::HashMap;

/// Replace `{key}` occurrences with the matching value. Unknown keys pass
/// through unchanged. Braces are NOT escaped — there's no `{{ }}` literal
/// support because templates don't need it.
#[must_use]
pub fn substitute(input: &str, vars: &HashMap<&str, &str>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let Some(rel_open) = input[i..].find('{') else {
            out.push_str(&input[i..]);
            break;
        };
        let open = i + rel_open;
        out.push_str(&input[i..open]);

        let Some(rel_close) = input[open + 1..].find('}') else {
            out.push_str(&input[open..]);
            break;
        };
        let close = open + 1 + rel_close;
        let key = &input[open + 1..close];

        if let Some(value) = vars.get(key) {
            out.push_str(value);
            i = close + 1;
        } else {
            out.push('{');
            i = open + 1;
        }
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
