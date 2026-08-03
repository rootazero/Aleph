//! Runtime Capability Module
//!
//! Provides runtime capability descriptions for injection into AI system prompts.
//! This allows the AI to understand which runtimes are available and how to use them.

use super::ledger::CapabilityEntry;
use std::path::PathBuf;

/// Get the LLM usage hint for a runtime from its spec.
///
/// Returns `None` when the capability has no hint (e.g. placeholder specs
/// like `cargo` with `llm_hint: None`) or is not in `SPECS` at all.
fn get_hint_from_spec(runtime_id: &str) -> Option<&'static str> {
    crate::runtimes::specs::find_spec(runtime_id).and_then(|s| s.llm_hint)
}

/// Format capability entries for AI system prompt injection.
///
/// Takes a slice of `&CapabilityEntry` references (as returned by
/// `CapabilityLedger::list_ready()`) and produces markdown text
/// suitable for inclusion in the system prompt.
#[must_use]
pub fn format_entries_for_prompt(entries: &[&CapabilityEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }

    use std::fmt::Write;
    let mut output = String::new();
    for entry in entries {
        let _ = writeln!(output, "**{}**", entry.name);
        if !entry.version.is_empty() {
            let _ = writeln!(output, "- Version: {}", entry.version);
        }
        if !entry.bin_path.as_os_str().is_empty() {
            let _ = writeln!(output, "- Executable: {}", entry.bin_path.display());
        }
        // Reuse existing usage hints from SPECS
        if let Some(hint) = get_hint_from_spec(&entry.name) {
            let _ = writeln!(output, "- {hint}");
        }
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- format_entries_for_prompt tests -----------------------------------

    #[test]
    fn test_format_entries_for_prompt_empty() {
        let entries: Vec<&CapabilityEntry> = vec![];
        let result = format_entries_for_prompt(&entries);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_entries_for_prompt_single() {
        let entry = CapabilityEntry {
            name: "uv".to_string(),
            bin_path: PathBuf::from("/home/user/.aleph/runtimes/uv/bin/uv"),
            version: "0.5.14".to_string(),
            status: super::super::ledger::CapabilityStatus::Ready,
            source: super::super::ledger::CapabilitySource::AlephManaged,
            last_probed: 0,
        };
        let entries: Vec<&CapabilityEntry> = vec![&entry];
        let result = format_entries_for_prompt(&entries);

        assert!(result.contains("**uv**"), "should contain bold name");
        assert!(result.contains("Version: 0.5.14"), "should contain version");
        assert!(
            result.contains("/home/user/.aleph/runtimes/uv/bin/uv"),
            "should contain bin path"
        );
        assert!(
            result.contains("Python package manager (uv)"),
            "should contain SPECS hint for uv"
        );
        assert!(
            result.contains("uv pip install"),
            "should contain uv pip install hint"
        );
    }

    #[test]
    fn test_format_entries_for_prompt_no_version() {
        let entry = CapabilityEntry {
            name: "uv".to_string(),
            bin_path: PathBuf::from("/home/user/.aleph/runtimes/uv/bin/uv"),
            version: String::new(),
            status: super::super::ledger::CapabilityStatus::Ready,
            source: super::super::ledger::CapabilitySource::AlephManaged,
            last_probed: 0,
        };
        let entries: Vec<&CapabilityEntry> = vec![&entry];
        let result = format_entries_for_prompt(&entries);

        assert!(result.contains("**uv**"));
        assert!(
            !result.contains("Version:"),
            "empty version should be omitted"
        );
        assert!(
            result.contains("Python package manager (uv)"),
            "should contain SPECS hint for uv"
        );
    }

    #[test]
    fn test_format_entries_for_prompt_unknown_runtime() {
        let entry = CapabilityEntry {
            name: "custom-tool".to_string(),
            bin_path: PathBuf::from("/opt/bin/custom-tool"),
            version: "2.0".to_string(),
            status: super::super::ledger::CapabilityStatus::Ready,
            source: super::super::ledger::CapabilitySource::System,
            last_probed: 0,
        };
        let entries: Vec<&CapabilityEntry> = vec![&entry];
        let result = format_entries_for_prompt(&entries);

        assert!(result.contains("**custom-tool**"));
        assert!(result.contains("Version: 2.0"));
        // No usage hints for unknown runtimes — just name/version/path
    }
}
