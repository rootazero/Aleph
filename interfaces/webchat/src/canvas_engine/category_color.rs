//! Map a free-form `category` string to a CSS color expression for the node stripe.

use crate::canvas_engine::fnv1a::fnv1a_32;

/// Returns a CSS color string. Well-known categories → curated variable;
/// anything else → deterministic `hsl(hue, 55%, 65%)`.
#[must_use]
pub fn category_color(category: &str) -> String {
    match category {
        "feedback" => "var(--cat-feedback)".to_string(),
        "project" => "var(--cat-project)".to_string(),
        "reference" => "var(--cat-reference)".to_string(),
        "user" => "var(--cat-user)".to_string(),
        "error" | "broken" | "contradiction" => "var(--cat-error)".to_string(),
        other => {
            let hue = fnv1a_32(other.as_bytes()) % 360;
            format!("hsl({hue}, 55%, 65%)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_categories_map_to_vars() {
        assert_eq!(category_color("feedback"), "var(--cat-feedback)");
        assert_eq!(category_color("project"), "var(--cat-project)");
        assert_eq!(category_color("reference"), "var(--cat-reference)");
        assert_eq!(category_color("user"), "var(--cat-user)");
        assert_eq!(category_color("error"), "var(--cat-error)");
        assert_eq!(category_color("broken"), "var(--cat-error)");
    }

    #[test]
    fn unknown_categories_use_deterministic_hsl() {
        let a = category_color("custom-xyz");
        let b = category_color("custom-xyz");
        assert_eq!(a, b);
        assert!(a.starts_with("hsl("));
    }
}
