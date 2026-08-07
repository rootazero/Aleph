//! Conversation transcript export — render the current chat thread to
//! Markdown and hand it to the browser as a download.
//!
//! Mirrors hermes-desktop's `transcriptUtils.ts` (export-to-markdown). The
//! rendering core is a pure, host-testable fn so the wire format is locked by
//! unit tests independent of the WASM download glue; the download path reuses
//! the exact blob → object-URL → anchor idiom already proven in
//! [`crate::views::home`] (memory export).

use super::state::ChatMessage;

/// Render a chat thread to a Markdown transcript.
///
/// Pure + deterministic (no clock / DOM reads) so it is fully unit-testable on
/// the host target. Intermediate "scratch step" bubbles and empty streaming
/// placeholders are skipped — the export captures the *conversation*, not the
/// agent's internal step trace (that lives in the workspace pane).
#[must_use]
pub fn transcript_markdown(messages: &[ChatMessage]) -> String {
    let mut out = String::from("# Aleph 对话记录\n\n");
    for msg in messages {
        if msg.is_intermediate {
            continue;
        }
        let body = msg.content.trim();
        if body.is_empty() {
            continue;
        }
        let speaker = if msg.role == "user" {
            "🧑 User"
        } else {
            "🤖 Aleph"
        };
        out.push_str("## ");
        out.push_str(speaker);
        out.push_str("\n\n");
        out.push_str(body);
        out.push_str("\n\n");
    }
    out
}

/// Build the transcript and trigger a browser download as `aleph-chat-*.md`.
///
/// WASM-only glue — the Markdown body comes from [`transcript_markdown`]. The
/// blob/anchor dance mirrors `views/home.rs` (memory export) verbatim, so the
/// failure modes (no window/document, blob alloc) are handled identically with
/// early returns rather than `unwrap`.
pub fn download_transcript(messages: &[ChatMessage]) {
    use wasm_bindgen::JsCast;

    let markdown = transcript_markdown(messages);
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let blob = match web_sys::Blob::new_with_str_sequence(&js_sys::Array::of1(&markdown.into())) {
        Ok(b) => b,
        Err(_) => return,
    };
    let url = web_sys::Url::create_object_url_with_blob(&blob).unwrap_or_default();
    let link = match document.create_element("a") {
        Ok(el) => match el.dyn_into::<web_sys::HtmlAnchorElement>() {
            Ok(anchor) => anchor,
            Err(_) => return,
        },
        Err(_) => return,
    };
    let timestamp = js_sys::Date::new_0()
        .to_iso_string()
        .as_string()
        .unwrap_or_default()
        .replace(':', "-");
    link.set_href(&url);
    link.set_download(&format!("aleph-chat-{timestamp}.md"));
    let _ = document.body().map(|body| body.append_child(&link));
    link.click();
    let _ = document.body().map(|body| body.remove_child(&link));
    let _ = web_sys::Url::revoke_object_url(&url);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str, intermediate: bool) -> ChatMessage {
        ChatMessage {
            id: format!("{role}-x"),
            role: role.into(),
            content: content.into(),
            tool_calls: vec![],
            is_streaming: false,
            is_intermediate: intermediate,
            error: None,
            model_info: None,
            timestamp: None,
            iteration: None,
            is_final: false,
            text_finalized: false,
            agent_id: None,
            plan_archive: None,
            author_user_id: None,
        }
    }

    #[test]
    fn renders_user_and_assistant_turns() {
        let msgs = vec![
            msg("user", "hello", false),
            msg("assistant", "hi there", false),
        ];
        let md = transcript_markdown(&msgs);
        assert!(md.starts_with("# Aleph"));
        assert!(md.contains("🧑 User"));
        assert!(md.contains("hello"));
        assert!(md.contains("🤖 Aleph"));
        assert!(md.contains("hi there"));
        assert_eq!(md.matches("## ").count(), 2);
    }

    #[test]
    fn skips_intermediate_and_empty_bubbles() {
        let msgs = vec![
            msg("assistant", "scratch step", true),
            msg("assistant", "   ", false),
            msg("user", "real question", false),
        ];
        let md = transcript_markdown(&msgs);
        assert!(!md.contains("scratch step"));
        assert!(md.contains("real question"));
        // Only the single real turn produces a heading.
        assert_eq!(md.matches("## ").count(), 1);
    }
}
