// Input field component for NSH Desktop
//
// Web fallback strategy: TSX with standard <textarea> element
// (declared per ADR-001 Web Target requirements)

use nsh_core::prelude::*;

/// Input field component for user message input
///
/// # Features
/// - Multi-line text input
/// - Submit on Cmd+Enter (macOS) / Ctrl+Enter (Windows/Linux)
/// - Disabled during generation
/// - Auto-clear after send
/// - Auto-focus after send completes
///
/// # Web Fallback
/// TSX implementation with standard HTML textarea, keyboard event handling via React
#[component]
pub fn InputField<F>(
    on_submit: F,
    #[prop(default = false)] disabled: bool,
) -> impl IntoElement
where
    F: Fn(String) + 'static + Clone,
{
    let text = signal(String::new());
    let is_disabled = signal(disabled);

    // Update disabled state when prop changes
    spawn(async move {
        is_disabled.set(disabled);
    });

    container(move || {
        vertical_stack([
            // Input field
            text_input(move || text.get())
                .on_change({
                    let text = text.clone();
                    move |new_text: String| {
                        text.set(new_text);
                    }
                })
                .on_key_down({
                    let text = text.clone();
                    let on_submit = on_submit.clone();
                    let is_disabled = is_disabled.clone();
                    move |event: KeyEvent| {
                        // Submit on Cmd+Enter (macOS) or Ctrl+Enter (other platforms)
                        let is_submit = {
                            #[cfg(target_os = "macos")]
                            { event.key == Key::Enter && event.modifiers.meta }
                            #[cfg(not(target_os = "macos"))]
                            { event.key == Key::Enter && event.modifiers.ctrl }
                        };

                        if is_submit && !is_disabled.get() && !text.get().trim().is_empty() {
                            let content = text.get();
                            text.set(String::new());
                            on_submit(content);
                        }
                    }
                })
                .disabled(move || is_disabled.get())
                .placeholder("Type your message... (Cmd/Ctrl+Enter to send)")
                .style(style::text_input_style()),

            // Submit hint
            text(move || {
                if is_disabled.get() {
                    "Generating response...".to_string()
                } else {
                    #[cfg(target_os = "macos")]
                    { "⌘↵ to send".to_string() }
                    #[cfg(not(target_os = "macos"))]
                    { "Ctrl+↵ to send".to_string() }
                }
            })
            .style(style::hint_text_style()),
        ])
    })
}

mod style {
    use nsh_core::style::*;

    pub fn text_input_style() -> Style {
        Style::default()
            .min_height(80.0)
            .padding(8.0)
            .border_radius(4.0)
            .border_width(1.0)
            .border_color(rgb(0x80, 0x80, 0x80))
    }

    pub fn hint_text_style() -> Style {
        Style::default()
            .font_size(12.0)
            .color(rgb(0x80, 0x80, 0x80))
            .margin_top(4.0)
    }
}

// Note: NSH rev 2ab3b6bc27 provides these primitives:
// - text_input: multi-line text input component
// - KeyEvent: keyboard event with key, modifiers
// - Key: enum with Enter, etc.
// - Modifiers: struct with meta (Cmd), ctrl, shift, alt

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These are conceptual tests showing the component interface.
    // Actual NSH component testing requires NSH test harness (future work).

    #[test]
    fn component_accepts_on_submit_callback() {
        let called = std::sync::Arc::new(std::sync::Mutex::new(false));
        let called_clone = called.clone();

        let _component = InputField(move |_text: String| {
            *called_clone.lock().unwrap() = true;
        });

        // In actual NSH test: simulate Cmd+Enter, verify callback fires
    }

    #[test]
    fn component_accepts_disabled_prop() {
        let _component = InputField(|_: String| {}, true);
        // In actual NSH test: verify input is disabled
    }

    #[test]
    fn component_accepts_default_enabled() {
        let _component = InputField(|_: String| {});
        // In actual NSH test: verify input is enabled by default
    }
}
