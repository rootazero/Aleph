//! One-shot `.obsidian/` vault config so the per-agent note directory opens
//! cleanly in Obsidian (graph view + wikilinks). Idempotent: never overwrites
//! existing user config.

use std::path::Path;

use crate::error::AlephError;

const APP_JSON: &str =
    r#"{"alwaysUpdateLinks":true,"newLinkFormat":"shortest","useMarkdownLinks":false}"#;
const CORE_PLUGINS_JSON: &str = r#"["file-explorer","global-search","graph","backlink","outgoing-link","tag-pane","page-preview"]"#;
const GRAPH_JSON: &str = r#"{"collapse-filter":true,"showTags":true,"showAttachments":false,"hideUnresolved":false,"showOrphans":true}"#;

/// Write `.obsidian/{app,core-plugins,graph}.json` under `agent_dir` if absent.
/// Best-effort: an existing file is left untouched (user owns their config).
pub async fn ensure_obsidian_config(agent_dir: &Path) -> Result<(), AlephError> {
    let dir = agent_dir.join(".obsidian");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AlephError::other(format!("create .obsidian: {e}")))?;
    for (name, body) in [
        ("app.json", APP_JSON),
        ("core-plugins.json", CORE_PLUGINS_JSON),
        ("graph.json", GRAPH_JSON),
    ] {
        let p = dir.join(name);
        if !p.exists() {
            tokio::fs::write(&p, body)
                .await
                .map_err(|e| AlephError::other(format!("write {name}: {e}")))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_three_config_files_idempotently() {
        let d = tempfile::tempdir().unwrap();
        ensure_obsidian_config(d.path()).await.unwrap();
        for f in ["app.json", "core-plugins.json", "graph.json"] {
            assert!(d.path().join(".obsidian").join(f).exists());
        }
        // second call must not error / must not clobber
        tokio::fs::write(d.path().join(".obsidian/app.json"), "USER")
            .await
            .unwrap();
        ensure_obsidian_config(d.path()).await.unwrap();
        let kept = tokio::fs::read_to_string(d.path().join(".obsidian/app.json"))
            .await
            .unwrap();
        assert_eq!(kept, "USER");
    }
}
