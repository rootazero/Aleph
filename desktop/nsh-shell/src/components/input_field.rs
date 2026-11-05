// Multiline message input for the native NSH shell.
//
// Web fallback strategy: a controlled HTML <textarea> with a mod+Enter
// keybinding and a disabled state during generation.

use nsh_core::{
    element::input,
    keymap::When,
    prelude::{on_key_with, IntoElement, IntoReactive, KeyOptions},
};

const INPUT_PLACEHOLDER: &str = "Type a message… (Cmd/Ctrl+Enter to send)";

/// Whether the current draft can be submitted.
fn is_submittable(value: &str, disabled: bool) -> bool {
    !disabled && !value.trim().is_empty()
}

/// A controlled, multiline input that submits with Cmd+Enter on macOS and
/// Ctrl+Enter elsewhere (`mod` follows the native platform convention).
pub fn input_field(
    value: impl IntoReactive<String>,
    on_change: impl Fn(String) + 'static,
    on_submit: impl Fn() + 'static,
    disabled: impl IntoReactive<bool>,
) -> impl IntoElement {
    let value = value.into_reactive();
    let disabled = disabled.into_reactive();
    let value_for_key = value.clone();
    let disabled_for_key = disabled.clone();
    let disabled_for_change = disabled.clone();

    on_key_with(
        "mod+enter",
        KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        },
        move |_| {
            if is_submittable(&value_for_key.get(), disabled_for_key.get()) {
                on_submit();
            }
        },
    );

    input()
        .multiline(true)
        .value(value)
        .placeholder(INPUT_PLACEHOLDER)
        .readonly(move || disabled.get())
        .on_change(move |next: &str| {
            if !disabled_for_change.get() {
                on_change(next.to_owned());
            }
        })
}

#[cfg(test)]
mod tests {
    use super::is_submittable;

    #[test]
    fn submission_requires_enabled_nonblank_input() {
        assert!(is_submittable("hello", false));
        assert!(!is_submittable("   \n", false));
        assert!(!is_submittable("hello", true));
    }
}
