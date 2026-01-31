//! Macros for defining UI hints declaratively.
//!
//! These macros provide an ergonomic way to define groups and field hints
//! without verbose struct initialization.

/// Define configuration groups.
///
/// Creates a `HashMap<String, GroupMeta>` from a declarative syntax.
///
/// # Example
///
/// ```ignore
/// use aethecore::define_groups;
///
/// let groups = define_groups! {
///     "general" => { label: "General", order: 10, icon: "gear" },
///     "providers" => { label: "AI Providers", order: 20 },
/// };
/// ```
#[macro_export]
macro_rules! define_groups {
    (
        $( $id:literal => { label: $label:literal, order: $order:expr $(, icon: $icon:literal)? } ),* $(,)?
    ) => {
        {
            let mut groups = std::collections::HashMap::new();
            $(
                groups.insert(
                    $id.to_string(),
                    $crate::config::ui_hints::GroupMeta {
                        label: $label.to_string(),
                        order: $order,
                        icon: $crate::define_groups!(@icon $($icon)?),
                    },
                );
            )*
            groups
        }
    };
    (@icon $icon:literal) => { Some($icon.to_string()) };
    (@icon) => { None };
}

/// Define field hints.
///
/// Creates a `HashMap<String, FieldHint>` from a declarative syntax.
/// All fields are optional; unspecified fields use their default values.
///
/// # Supported Fields
///
/// - `label`: Human-readable label for the field
/// - `help`: Help text or tooltip
/// - `group`: Group this field belongs to
/// - `order`: Sort order within group (lower = higher priority)
/// - `advanced`: Whether this is an advanced option (default: false)
/// - `sensitive`: Whether this field contains sensitive data (default: false)
/// - `placeholder`: Placeholder text for input fields
///
/// # Example
///
/// ```ignore
/// use aethecore::define_hints;
///
/// let hints = define_hints! {
///     "general.language" => {
///         label: "Language",
///         help: "UI display language",
///         group: "general",
///         order: 1,
///     },
///     "providers.*.api_key" => {
///         label: "API Key",
///         sensitive: true,
///     },
/// };
/// ```
#[macro_export]
macro_rules! define_hints {
    (
        $( $path:literal => {
            $( label: $label:literal, )?
            $( help: $help:literal, )?
            $( group: $group:literal, )?
            $( order: $order:expr, )?
            $( advanced: $advanced:literal, )?
            $( sensitive: $sensitive:literal, )?
            $( placeholder: $placeholder:literal, )?
        } ),* $(,)?
    ) => {
        {
            let mut fields = std::collections::HashMap::new();
            $(
                fields.insert(
                    $path.to_string(),
                    $crate::config::ui_hints::FieldHint {
                        label: $crate::define_hints!(@opt $( $label )?),
                        help: $crate::define_hints!(@opt $( $help )?),
                        group: $crate::define_hints!(@opt $( $group )?),
                        order: $crate::define_hints!(@opt_num $( $order )?),
                        advanced: $crate::define_hints!(@bool $( $advanced )?),
                        sensitive: $crate::define_hints!(@bool $( $sensitive )?),
                        placeholder: $crate::define_hints!(@opt $( $placeholder )?),
                    },
                );
            )*
            fields
        }
    };
    (@opt $val:literal) => { Some($val.to_string()) };
    (@opt) => { None };
    (@opt_num $val:expr) => { Some($val) };
    (@opt_num) => { None };
    (@bool $val:literal) => { $val };
    (@bool) => { false };
}

// Re-export macros at module level for documentation purposes
pub use define_groups;
pub use define_hints;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_define_groups_with_icon() {
        let groups = define_groups! {
            "test" => { label: "Test Group", order: 10, icon: "test-icon" },
        };

        assert_eq!(groups.len(), 1);
        let group = groups.get("test").unwrap();
        assert_eq!(group.label, "Test Group");
        assert_eq!(group.order, 10);
        assert_eq!(group.icon, Some("test-icon".to_string()));
    }

    #[test]
    fn test_define_groups_without_icon() {
        let groups = define_groups! {
            "test" => { label: "Test Group", order: 5 },
        };

        assert_eq!(groups.len(), 1);
        let group = groups.get("test").unwrap();
        assert_eq!(group.label, "Test Group");
        assert_eq!(group.order, 5);
        assert_eq!(group.icon, None);
    }

    #[test]
    fn test_define_groups_multiple() {
        let groups = define_groups! {
            "a" => { label: "A", order: 1 },
            "b" => { label: "B", order: 2, icon: "b-icon" },
            "c" => { label: "C", order: 3 },
        };

        assert_eq!(groups.len(), 3);
    }

    #[test]
    fn test_define_hints_full() {
        let hints = define_hints! {
            "test.field" => {
                label: "Test Field",
                help: "This is help text",
                group: "test",
                order: 1,
                advanced: true,
                sensitive: true,
                placeholder: "Enter value",
            },
        };

        assert_eq!(hints.len(), 1);
        let hint = hints.get("test.field").unwrap();
        assert_eq!(hint.label, Some("Test Field".to_string()));
        assert_eq!(hint.help, Some("This is help text".to_string()));
        assert_eq!(hint.group, Some("test".to_string()));
        assert_eq!(hint.order, Some(1));
        assert!(hint.advanced);
        assert!(hint.sensitive);
        assert_eq!(hint.placeholder, Some("Enter value".to_string()));
    }

    #[test]
    fn test_define_hints_minimal() {
        let hints = define_hints! {
            "test.field" => {
                label: "Test",
            },
        };

        let hint = hints.get("test.field").unwrap();
        assert_eq!(hint.label, Some("Test".to_string()));
        assert_eq!(hint.help, None);
        assert_eq!(hint.group, None);
        assert_eq!(hint.order, None);
        assert!(!hint.advanced);
        assert!(!hint.sensitive);
        assert_eq!(hint.placeholder, None);
    }

    #[test]
    fn test_define_hints_sensitive_only() {
        let hints = define_hints! {
            "api.key" => {
                sensitive: true,
            },
        };

        let hint = hints.get("api.key").unwrap();
        assert!(hint.sensitive);
        assert!(!hint.advanced);
    }

    #[test]
    fn test_define_hints_multiple() {
        let hints = define_hints! {
            "field1" => { label: "Field 1", },
            "field2" => { label: "Field 2", sensitive: true, },
            "field3" => { help: "Help for field 3", },
        };

        assert_eq!(hints.len(), 3);
    }

    #[test]
    fn test_define_hints_empty() {
        let hints = define_hints! {
            "empty.field" => {},
        };

        let hint = hints.get("empty.field").unwrap();
        assert_eq!(hint.label, None);
        assert!(!hint.sensitive);
    }
}
