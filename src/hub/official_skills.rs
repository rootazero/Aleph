//! Cold-start projection of bundled official skills into Hub catalog entries.
//!
//! Projects the compile-time-embedded `BUNDLED_SKILLS` tree into `ExtensionEntry`s
//! for the `aleph-hub` source slot (consumed by `hub::primer`) so official skills
//! are browsable/installable offline and before the remote catalog is fetched.
//! The remote fetch later overwrites the slot wholesale (no peer source, no dedup).

use crate::bundled::{BUNDLED_SKILLS, OFFICIAL_SKILLS_REPO};
use crate::domain::skill::{SkillManifest, SkillSource};
use crate::domain::Entity; // brings `manifest.id()` into scope (status.rs does the same)
use crate::hub::catalog_client::ALEPH_HUB_ID;
use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier};
use crate::skill::manifest::parse_skill_content;

/// Project one bundled skill into a Hub catalog entry. `dir_name` is the bundle
/// directory name (== the Aleph-skills repo subdir); the canonical slug is the
/// manifest's `SkillId` (frontmatter-name-derived), which may differ from it.
fn project_skill(dir_name: &str, manifest: &SkillManifest) -> ExtensionEntry {
    let spec = InstallSpec::GitDir {
        git_url: OFFICIAL_SKILLS_REPO.to_string(),
        subdir: Some(dir_name.to_string()),
        git_ref: None,
        sha256: None,
    };
    ExtensionEntry {
        id: format!("{ALEPH_HUB_ID}:{}", manifest.id()),
        kind: ExtensionKind::Skill,
        category: ExtensionCategory::Other,
        name: manifest.name().to_string(),
        description: manifest.description().to_string(),
        author: None,
        icon: None,
        tags: vec![ExtensionKind::Skill.as_str().to_string()],
        version: None,
        source_id: ALEPH_HUB_ID.to_string(),
        repo_url: Some(OFFICIAL_SKILLS_REPO.to_string()),
        trust_tier: TrustTier::Official,
        requires_config: spec.requires_config(),
        config_schema: None,
        installed: false,
        enabled: false,
        update_available: false,
        via: Some(ALEPH_HUB_ID.to_string()),
        install_spec: Some(spec),
    }
}

/// Project the in-binary bundled official skills into Hub catalog entries.
/// Returns `[]` (logged) when the `skills/` submodule was absent at build time.
pub fn primer_entries() -> Vec<ExtensionEntry> {
    let mut out = Vec::new();
    for dir in BUNDLED_SKILLS.dirs() {
        let Some(dir_name) = dir.path().file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // The SKILL.md directly inside this skill dir (borrowed from the static embed).
        let Some(content) = dir
            .files()
            .find(|f| f.path().file_name().and_then(|n| n.to_str()) == Some("SKILL.md"))
            .and_then(|f| f.contents_utf8())
        else {
            continue;
        };
        match parse_skill_content(content, SkillSource::Bundled) {
            Ok(manifest) => out.push(project_skill(dir_name, &manifest)),
            Err(e) => {
                tracing::warn!(skill = %dir_name, error = %e, "primer: skipping unparseable bundled SKILL.md")
            }
        }
    }
    if out.is_empty() {
        tracing::info!(
            "official skills primer: bundle empty (submodule absent at build) — no skill entries"
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\nname: PDF Tools\ndescription: Work with PDFs.\n---\nBody.";

    fn manifest_from(md: &str) -> SkillManifest {
        parse_skill_content(md, SkillSource::Bundled).expect("sample SKILL.md parses")
    }

    #[test]
    fn project_skill_yields_official_aleph_hub_entry() {
        let m = manifest_from(SAMPLE);
        // dir_name deliberately differs from the SkillId ("pdf-tools") to lock decoupling.
        let e = project_skill("pdf-tools-dir", &m);
        assert_eq!(e.id, "aleph-hub:pdf-tools");
        assert_eq!(e.kind, ExtensionKind::Skill);
        assert_eq!(e.category, ExtensionCategory::Other);
        assert_eq!(e.trust_tier, TrustTier::Official);
        assert_eq!(e.source_id, "aleph-hub");
        assert_eq!(e.via.as_deref(), Some("aleph-hub"));
        assert_eq!(e.name, "PDF Tools");
        assert!(!e.installed);
        match e.install_spec.unwrap() {
            InstallSpec::GitDir {
                git_url,
                subdir,
                git_ref,
                sha256,
            } => {
                assert_eq!(git_url, OFFICIAL_SKILLS_REPO);
                assert_eq!(subdir.as_deref(), Some("pdf-tools-dir"));
                assert!(git_ref.is_none() && sha256.is_none());
            }
            other => panic!("expected GitDir, got {other:?}"),
        }
        assert!(!e.requires_config);
    }

    #[test]
    fn primer_entries_tolerates_absent_bundle() {
        // The skills submodule may be empty in dev/CI; primer_entries must not
        // panic, and whatever it returns must be well-formed official skills.
        let entries = primer_entries();
        for e in &entries {
            assert_eq!(e.kind, ExtensionKind::Skill);
            assert_eq!(e.trust_tier, TrustTier::Official);
            assert!(e.id.starts_with("aleph-hub:"));
        }
    }
}
