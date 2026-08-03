//! Per-tool runtime state surface.
//!
//! Hermes lets tools mutate their JSON schema at emit time (depth limits,
//! sandbox availability, rate-limit clocks). Aleph instead emits a
//! `<tool_runtime_state>` block from a dedicated `PromptLayer` at priority
//! 502 — intelligence lives in the prompt (R9), not in wire-format JSON
//! schema.
//!
//! Fragments are produced by the orchestrator from the `ToolHealthCache`
//! (see `orchestrator::harness_bridge::context_blocks::compute_runtime_state_blocks`)
//! and rendered by `ToolRuntimeStateLayer` into the prompt block. Tools
//! do not implement any opt-in trait — the cache is the single source.

use serde::{Deserialize, Serialize};

use crate::thinker::xml_util::{escape_xml, escape_xml_attr};

/// Tool availability for runtime-state purposes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// Tool is callable; hints describe runtime conditions.
    Available,
    /// Tool exists but is dormant; the reason is shown to the model so
    /// it can explain to the user why the capability is unavailable.
    Unavailable { reason: String },
}

/// One tool's contribution to the `<tool_runtime_state>` block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStateFragment {
    pub tool_name: String,
    pub status: ToolStatus,
    /// Free-form one-line hints (e.g. "depth 2 of 4", "sandbox: docker").
    pub hints: Vec<String>,
}

impl RuntimeStateFragment {
    /// Construct an Available fragment from a list of hints.
    pub fn available(tool_name: impl Into<String>, hints: Vec<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            status: ToolStatus::Available,
            hints,
        }
    }

    /// Construct an Unavailable fragment with a reason. Hints can still
    /// be attached (e.g. retry timing).
    pub fn unavailable(tool_name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            status: ToolStatus::Unavailable {
                reason: reason.into(),
            },
            hints: Vec::new(),
        }
    }

    /// Render this fragment as an XML `<tool>` element.
    ///
    /// The output is indented two spaces (the parent `<tool_runtime_state>`
    /// block adds none) so several fragments compose cleanly. Empty `hints`
    /// + Available status emits an empty self-closing element.
    #[must_use]
    pub fn render_xml(&self) -> String {
        let (status_attr, unavail_hint) = match &self.status {
            ToolStatus::Available => (String::new(), None),
            ToolStatus::Unavailable { reason } => (
                " status=\"unavailable\"".to_string(),
                Some(format!("    <hint>{}</hint>\n", escape_xml(reason))),
            ),
        };

        if self.hints.is_empty() && unavail_hint.is_none() {
            return format!(
                "  <tool name=\"{}\"{}/>\n",
                escape_xml_attr(&self.tool_name),
                status_attr
            );
        }

        let mut buf = format!(
            "  <tool name=\"{}\"{}>\n",
            escape_xml_attr(&self.tool_name),
            status_attr
        );
        if let Some(h) = unavail_hint {
            buf.push_str(&h);
        }
        for hint in &self.hints {
            buf.push_str(&format!("    <hint>{}</hint>\n", escape_xml(hint)));
        }
        buf.push_str("  </tool>\n");
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_available_renders_xml() {
        let f = RuntimeStateFragment::available("delegate_task", vec!["depth 2 of 4".to_string()]);
        let xml = f.render_xml();
        assert!(xml.contains("<tool name=\"delegate_task\">"));
        assert!(xml.contains("<hint>depth 2 of 4</hint>"));
        assert!(xml.trim_end().ends_with("</tool>"));
    }

    #[test]
    fn fragment_unavailable_includes_status_and_reason() {
        let f = RuntimeStateFragment::unavailable("send_telegram", "bridge offline");
        let xml = f.render_xml();
        assert!(xml.contains("status=\"unavailable\""));
        assert!(xml.contains("<hint>bridge offline</hint>"));
    }

    #[test]
    fn empty_available_fragment_is_self_closing() {
        let f = RuntimeStateFragment::available("noop", vec![]);
        let xml = f.render_xml();
        assert!(xml.contains("/>"));
        assert!(!xml.contains("</tool>"));
    }

    #[test]
    fn xml_attributes_and_text_are_escaped() {
        let f = RuntimeStateFragment {
            tool_name: "bad&name".into(),
            status: ToolStatus::Unavailable {
                reason: "<bad>&\"reason\"".into(),
            },
            hints: vec!["<hint &val>".into()],
        };
        let xml = f.render_xml();
        assert!(xml.contains("bad&amp;name"));
        assert!(xml.contains("&lt;bad&gt;"));
        assert!(xml.contains("&amp;&quot;reason&quot;") || xml.contains("&amp;\"reason\""));
        assert!(xml.contains("&lt;hint &amp;val&gt;"));
    }

    #[test]
    fn attr_escaping_covers_gt_and_apostrophe() {
        // After folding onto the shared SSOT, the `name` attribute uses the
        // full five-char escaper, so a `>` or `'` in a tool name can no longer
        // break out of the quoted attribute value.
        let f = RuntimeStateFragment::available("a>b'c", vec![]);
        let xml = f.render_xml();
        assert!(xml.contains("name=\"a&gt;b&apos;c\""), "got: {xml}");
    }
}
