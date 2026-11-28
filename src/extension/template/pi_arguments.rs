use regex::{Captures, Regex};
use std::sync::LazyLock;

static PI_BRACED_ARGUMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{(?:[0-9]+|@|ARGUMENTS)(?:[^}]*\}|[^}]*$)")
        .expect("valid Pi braced-argument detector")
});

pub(crate) fn unsupported_pi_argument(template: &str) -> Option<&str> {
    PI_BRACED_ARGUMENT
        .find_iter(template)
        .find_map(|candidate| {
            let token = candidate.as_str();
            (!PI_ARGUMENT
                .find(token)
                .is_some_and(|matched| matched.as_str() == token))
            .then_some(token)
        })
}

static PI_ARGUMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{([0-9]+|ARGUMENTS|@):-([^}]*)\}|\$\{@:([0-9]+)(?::([0-9]+))?\}|\$(ARGUMENTS|@|[0-9]+)")
        .expect("valid Pi prompt substitution grammar")
});

fn pi_arguments(mut input: &str) -> Vec<String> {
    let mut args = Vec::new();
    while !input.trim_start().is_empty() {
        input = input.trim_start();
        if let Some(quote @ ('\'' | '"')) = input.chars().next() {
            let mut quoted = String::new();
            let mut chars = input[1..].char_indices().peekable();
            let mut suffix = None;
            while let Some((offset, current)) = chars.next() {
                if current == '\\' && chars.peek().is_some_and(|(_, next)| *next == quote) {
                    quoted.push(quote);
                    chars.next();
                } else if current == quote {
                    suffix = Some(&input[offset + 2..]);
                    break;
                } else {
                    quoted.push(current);
                }
            }
            if let Some(suffix) = suffix {
                let end = suffix.find(char::is_whitespace).unwrap_or(suffix.len());
                quoted.push_str(&suffix[..end]);
                args.push(quoted);
                input = &suffix[end..];
                continue;
            }
        }
        let end = input.find(char::is_whitespace).unwrap_or(input.len());
        args.push(input[..end].to_owned());
        input = &input[end..];
    }
    args
}

pub(super) fn substitute_pi_arguments(template: &str, input: &str) -> String {
    let args = pi_arguments(input);
    let all = args.join(" ");
    let selected = |target: &str| match target {
        "@" | "ARGUMENTS" => all.as_str(),
        index => index
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| args.get(n))
            .map_or("", String::as_str),
    };
    PI_ARGUMENT
        .replace_all(template, |c: &Captures<'_>| {
            if let Some(target) = c.get(1) {
                let value = selected(target.as_str());
                return if value.is_empty() {
                    c[2].to_owned()
                } else {
                    value.to_owned()
                };
            }
            if let Some(start) = c.get(3) {
                let index = start
                    .as_str()
                    .parse::<usize>()
                    .unwrap_or(usize::MAX)
                    .saturating_sub(1);
                let length = c.get(4).map_or(usize::MAX, |length| {
                    length.as_str().parse().unwrap_or(usize::MAX)
                });
                let tail = args.get(index..).unwrap_or_default();
                return tail[..length.min(tail.len())].join(" ");
            }
            selected(&c[5]).to_owned()
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_quoted_and_missing_arguments() {
        assert_eq!(unsupported_pi_argument("${@:1:2:3} $1"), Some("${@:1:2:3}"));
        assert_eq!(unsupported_pi_argument("${1:-x} ${@:1:2}"), None);
        assert_eq!(
            pi_arguments(r#""a b" c isn't "say \"hi\"" '' "unterminated rest"#),
            [
                "a b",
                "c",
                "isn't",
                "say \"hi\"",
                "",
                "\"unterminated",
                "rest"
            ]
        );
        assert_eq!(
            substitute_pi_arguments(
                "$1/$2/[$@]/[$ARGUMENTS]/${@:2}/${@:1:1}/${@:0:1}/${@:4}/${@:2:0}",
                r#""a b" c"#,
            ),
            "a b/c/[a b c]/[a b c]/c/a b/a b//"
        );
        assert_eq!(
            substitute_pi_arguments("${1:-one}/${@:-all}/${ARGUMENTS:-entire}/$0/$3", ""),
            "one/all/entire//"
        );
        assert_eq!(
            substitute_pi_arguments("$1 ${2:-$1}", "$ARGUMENTS"),
            "$ARGUMENTS $1"
        );
    }
}
