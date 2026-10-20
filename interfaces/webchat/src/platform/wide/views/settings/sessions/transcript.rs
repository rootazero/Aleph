//! Transcript viewer component with markdown rendering

use leptos::prelude::*;
use pulldown_cmark::{html, Options, Parser};

#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
pub struct TranscriptMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: Option<String>,
}

#[component]
pub fn TranscriptViewer(messages: Signal<Vec<TranscriptMessage>>) -> impl IntoView {
    view! {
        <div class="flex-1 overflow-y-auto p-6 space-y-4">
            <For
                each=move || messages.get().into_iter().enumerate()
                key=|(idx, _)| *idx
                children=move |(_, msg)| {
                    view! { <MessageCard message=msg /> }
                }
            />
            <Show when=move || messages.get().is_empty()>
                <div class="flex items-center justify-center h-full text-text-tertiary">
                    "No messages in this session"
                </div>
            </Show>
        </div>
    }
}

#[component]
fn MessageCard(message: TranscriptMessage) -> impl IntoView {
    let role_class = match message.role {
        MessageRole::User => "bg-surface-base border-l-4 border-l-primary",
        MessageRole::Assistant => "bg-surface-raised border-l-4 border-l-success",
        MessageRole::System => "bg-surface-sunken border-l-4 border-l-warning",
    };

    let role_label = match message.role {
        MessageRole::User => "User",
        MessageRole::Assistant => "Assistant",
        MessageRole::System => "System",
    };

    let role_text_class = match message.role {
        MessageRole::User => "text-primary",
        MessageRole::Assistant => "text-success",
        MessageRole::System => "text-warning",
    };

    // Render markdown to HTML
    let html_content = markdown_to_html(&message.content);

    view! {
        <div class=format!("p-4 rounded-lg border border-border {}", role_class)>
            <div class="flex items-center justify-between mb-2">
                <span class=format!("text-sm font-semibold {}", role_text_class)>
                    {role_label}
                </span>
                {message.timestamp.map(|ts| view! {
                    <span class="text-xs text-text-tertiary font-mono">
                        {ts}
                    </span>
                })}
            </div>
            <div
                class="prose prose-sm max-w-none text-text-primary prose-headings:text-text-primary prose-code:text-text-primary prose-pre:bg-surface-sunken prose-pre:border prose-pre:border-border"
                inner_html=html_content
            />
        </div>
    }
}

/// Convert markdown to HTML with syntax highlighting support
fn markdown_to_html(markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);

    let parser = Parser::new_ext(markdown, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);

    html_output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_rendering() {
        let md = "# Hello\n\n```rust\nfn main() {}\n```";
        let html = markdown_to_html(md);
        assert!(html.contains("<h1"));
        assert!(html.contains("<code"));
    }

    #[test]
    fn test_empty_content() {
        let html = markdown_to_html("");
        assert_eq!(html, "");
    }
}
