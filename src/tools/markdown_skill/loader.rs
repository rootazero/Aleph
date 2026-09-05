//! Skill Loader
//!
//! Batch loading of Markdown skills from directory.

use anyhow::Result;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

use super::parser::parse_skill_file;
use super::spec::{AlephSkillSpec, SandboxMode};
use super::tool_adapter::MarkdownCliTool;

/// Skill loader for scanning and loading Markdown skills
pub struct SkillLoader {
    /// Base directory to scan (e.g., "skills/")
    base_dir: PathBuf,

    /// Whether to scan recursively
    recursive: bool,
}

impl SkillLoader {
    /// Create a new skill loader
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            recursive: true,
        }
    }

    /// Load all skills from the base directory
    ///
    /// Returns (`loaded_tools`, errors) - partial failures are logged but don't abort
    pub async fn load_all(&self) -> (Vec<MarkdownCliTool>, Vec<(PathBuf, anyhow::Error)>) {
        let mut tools = Vec::new();
        let mut errors = Vec::new();

        info!(
            base_dir = %self.base_dir.display(),
            recursive = self.recursive,
            "Scanning for Markdown skills"
        );

        // Find all .md files
        let skill_files = match self.find_skill_files().await {
            Ok(files) => files,
            Err(e) => {
                error!(error = %e, "Failed to scan skill directory");
                return (tools, errors);
            }
        };

        info!(count = skill_files.len(), "Found skill files");

        // Load each file
        for path in skill_files {
            match self.load_skill_file(&path).await {
                Ok(tool) => {
                    info!(
                        skill = %tool.spec.name,
                        path = %path.display(),
                        "Loaded skill"
                    );
                    tools.push(tool);
                }
                Err(e) => {
                    warn!(
                        path = %path.display(),
                        error = %e,
                        "Failed to load skill file"
                    );
                    errors.push((path, e));
                }
            }
        }

        info!(
            loaded = tools.len(),
            failed = errors.len(),
            "Skill loading complete"
        );

        (tools, errors)
    }

    /// Find all SKILL.md files (using walkdir for safety)
    async fn find_skill_files(&self) -> Result<Vec<PathBuf>> {
        if !self.base_dir.exists() {
            info!(
                base_dir = %self.base_dir.display(),
                "Skill directory does not exist, skipping"
            );
            return Ok(Vec::new());
        }

        // Use walkdir (sync) in blocking task to avoid stack issues
        let base_dir = self.base_dir.clone();
        let recursive = self.recursive;

        let skill_files = tokio::task::spawn_blocking(move || {
            let walker = if recursive {
                WalkDir::new(&base_dir)
                    .follow_links(false) // Prevent symlink loops
                    .max_depth(10) // Reasonable limit
            } else {
                WalkDir::new(&base_dir).max_depth(1)
            };

            walker
                .into_iter()
                .filter_entry(|e| {
                    // Skip hidden directories (e.g., .git, .git-cache), but not the root entry
                    if e.depth() > 0 && e.file_type().is_dir() {
                        if let Some(name) = e.file_name().to_str() {
                            return !name.starts_with('.');
                        }
                    }
                    true
                })
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
                .map(|e| e.into_path())
                .filter(|p| Self::is_skill_file_static(p))
                .collect::<Vec<PathBuf>>()
        })
        .await?;

        Ok(skill_files)
    }

    /// Static version for use in `spawn_blocking`
    fn is_skill_file_static(path: &Path) -> bool {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            name.eq_ignore_ascii_case("SKILL.md") || name.to_lowercase().ends_with(".skill.md")
        } else {
            false
        }
    }

    /// Load a single skill file
    async fn load_skill_file(&self, path: &Path) -> Result<MarkdownCliTool> {
        // Read file
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read file: {e}"))?;

        // Parse spec
        let spec = parse_skill_file(&content).map_err(|e| anyhow::anyhow!("Parse error: {e}"))?;

        // Validate binary availability (optional warning)
        self.check_binary_availability(&spec);

        // Create tool
        Ok(MarkdownCliTool::new(spec))
    }

    /// Check if required binaries are available (only for host mode)
    fn check_binary_availability(&self, spec: &AlephSkillSpec) {
        // Only check when running on host
        let is_host_mode = spec
            .metadata
            .aleph
            .as_ref()
            .is_none_or(|a| matches!(a.security.sandbox, SandboxMode::Host)); // Default: OpenClaw style (host execution)

        if !is_host_mode {
            // Docker/VirtualFs mode: binary is in container, not on host
            debug!(
                skill = %spec.name,
                "Skipping host binary check (sandbox mode)"
            );
            return;
        }

        for bin in &spec.metadata.requires.bins {
            match which::which(bin) {
                Ok(path) => {
                    debug!(
                        skill = %spec.name,
                        binary = %bin,
                        path = %path.display(),
                        "Binary found"
                    );
                }
                Err(_) => {
                    warn!(
                        skill = %spec.name,
                        binary = %bin,
                        "Required binary not found in PATH (skill will fail at runtime). \
                        Install it or switch to 'sandbox: docker' mode."
                    );
                }
            }
        }
    }
}

/// What a directory scan produced: the tools that loaded, and the files that
/// did not.
///
/// The errors used to be counted into a `warn!` and then dropped on the floor
/// by `load_skills_from_dir`, which returned only the tools. That turned a
/// fail-closed answer ("I could not parse these files") into a value ("there
/// is nothing here") — and the caller that consumed it,
/// `gateway::handlers::markdown_skills::handle_install`, reported exactly
/// that: a bundle whose every SKILL.md was malformed came back as
/// `No skills found in <path>`, which is what the user also sees for a
/// directory that genuinely contains no skills.
///
/// Returning a struct rather than a bare tuple so a caller that only wants the
/// tools has to name `.tools` — i.e. discarding the failures is now a thing
/// someone typed, not the default.
/// (No `Debug`: `MarkdownCliTool` is `Clone`-only, and a report is inspected
/// through `failure_summary()` rather than printed whole.)
#[derive(Default)]
pub struct SkillLoadReport {
    /// Skills that parsed and are ready to register.
    pub tools: Vec<MarkdownCliTool>,
    /// Per-file failures, in scan order. Non-empty means "some of what is on
    /// disk is not represented in `tools`" — never read an empty `tools` as
    /// "the directory is empty" without checking this.
    pub errors: Vec<(PathBuf, anyhow::Error)>,
}

impl SkillLoadReport {
    /// One-line summary of the failures, for an operator-facing message.
    /// Empty string when nothing failed.
    #[must_use]
    pub fn failure_summary(&self) -> String {
        self.errors
            .iter()
            .map(|(path, e)| format!("{}: {e}", path.display()))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Helper function for convenient loading.
///
/// Returns both halves — see [`SkillLoadReport`] for why the errors are no
/// longer swallowed here.
pub async fn load_skills_from_dir(dir: impl Into<PathBuf>) -> SkillLoadReport {
    let loader = SkillLoader::new(dir);
    let (tools, errors) = loader.load_all().await;

    if !errors.is_empty() {
        warn!(
            failed_count = errors.len(),
            "Some skills failed to load (check logs for details)"
        );
    }

    SkillLoadReport { tools, errors }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_load_valid_skill() {
        let temp_dir = tempfile::tempdir().unwrap();
        let skill_path = temp_dir.path().join("test-skill/SKILL.md");

        fs::create_dir_all(skill_path.parent().unwrap())
            .await
            .unwrap();
        // NOTE: written as one literal on purpose. The previous form used
        // `\`-continuations, and a `\<newline>` in a Rust string eats the
        // newline *and the next line's leading whitespace* — so the fixture's
        // YAML arrived as `metadata:` (null) with `requires:` / `bins:`
        // promoted to top level, `serde_yml` refused it, and this test was red
        // on main before the frontmatter unification touched anything. The
        // splitter, old and new, produces the same (broken) YAML from it.
        fs::write(
            &skill_path,
            "---\nname: test-tool\ndescription: A test\nmetadata:\n  requires:\n    bins: [\"echo\"]\n---\nTest content\n",
        )
        .await
        .unwrap();

        let loader = SkillLoader::new(temp_dir.path());
        let (tools, errors) = loader.load_all().await;

        assert_eq!(tools.len(), 1);
        assert_eq!(errors.len(), 0);
        assert_eq!(tools[0].spec.name, "test-tool");
    }

    #[tokio::test]
    async fn test_partial_failure_resilience() {
        let temp_dir = tempfile::tempdir().unwrap();

        // Valid skill
        let valid = temp_dir.path().join("valid/SKILL.md");
        fs::create_dir_all(valid.parent().unwrap()).await.unwrap();
        fs::write(
            &valid,
            "---\nname: good\ndescription: ok\nmetadata:\n  requires:\n    bins: []\n---\nContent",
        )
        .await
        .unwrap();

        // Invalid skill (malformed YAML)
        let invalid = temp_dir.path().join("invalid/SKILL.md");
        fs::create_dir_all(invalid.parent().unwrap()).await.unwrap();
        fs::write(&invalid, "---\n{{{invalid yaml\n---\n")
            .await
            .unwrap();

        let loader = SkillLoader::new(temp_dir.path());
        let (tools, errors) = loader.load_all().await;

        assert_eq!(tools.len(), 1); // Valid one loaded
        assert_eq!(errors.len(), 1); // Invalid one failed
    }

    /// `load_all` always knew which files failed; `load_skills_from_dir` — the
    /// only entry point any caller uses — threw that list away and returned
    /// the tools alone. So a bundle whose every SKILL.md was malformed was
    /// indistinguishable, at the call site, from an empty directory.
    ///
    /// The assertion is on the *effect*: the failing path must be nameable by
    /// the caller. Asserting only `tools.len() == 0` would pass just as well
    /// against the old, lossy signature.
    #[tokio::test]
    async fn a_directory_whose_skills_all_failed_is_not_reported_as_empty() {
        let temp_dir = tempfile::tempdir().unwrap();
        let broken = temp_dir.path().join("broken/SKILL.md");
        fs::create_dir_all(broken.parent().unwrap()).await.unwrap();
        fs::write(&broken, "---\n{{{invalid yaml\n---\n")
            .await
            .unwrap();

        let report = load_skills_from_dir(temp_dir.path()).await;
        assert!(report.tools.is_empty(), "nothing parsed");
        assert_eq!(
            report.errors.len(),
            1,
            "the caller must be able to tell `all of them failed` from `there were none`"
        );
        assert!(
            report.failure_summary().contains("broken"),
            "the summary must name the offending file, got {:?}",
            report.failure_summary()
        );

        // The contrast that gives the assertion above its meaning.
        let empty_dir = tempfile::tempdir().unwrap();
        let empty = load_skills_from_dir(empty_dir.path()).await;
        assert!(empty.tools.is_empty() && empty.errors.is_empty());
    }

    /// The upstream comma-separated `allowed-tools:` scalar must not fail this
    /// path either. `AlephSkillSpec` has no such field and does not
    /// `deny_unknown_fields`, so the key is ignored — deliberately: a
    /// markdown-CLI skill becomes a *tool*, never a slash command, so it never
    /// reaches `ToolRegistrar::register_skills` where the declaration is
    /// enforced. Honouring the key here would be a parse that reports success
    /// and changes nothing. The requirement is only that it does no harm.
    #[tokio::test]
    async fn an_upstream_allowed_tools_declaration_does_not_break_this_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("scoped/SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(
            &path,
            "---\nname: scoped\ndescription: declares a scope\n\
             allowed-tools: Read, Grep, Bash(cargo *)\n---\nBody",
        )
        .await
        .unwrap();

        let report = load_skills_from_dir(temp_dir.path()).await;
        assert_eq!(
            report.tools.len(),
            1,
            "an `allowed-tools:` key must not delete the skill; errors: {}",
            report.failure_summary()
        );
        assert_eq!(report.tools[0].spec.name, "scoped");
    }

    /// A `---`-prefixed line inside a YAML block scalar used to terminate the
    /// frontmatter early (`find("\n---\n")` matched the substring anywhere),
    /// leaving the rest of the frontmatter to be parsed as markdown body — or,
    /// more often, failing the YAML parse and dropping the skill. The shared
    /// splitter matches only a whole line that *is* the fence.
    ///
    /// The boundary this does NOT cross is recorded in
    /// `skill::frontmatter::split`'s docs and in
    /// `an_indented_bare_fence_still_terminates_documents_a_known_boundary`
    /// there: a line whose entire content is `---`, indented or not, is still
    /// read as the fence. That leniency is pre-existing `skill::manifest`
    /// behaviour and tightening it would start dropping any SKILL.md that
    /// indents its closing fence — which is the failure class this whole
    /// change exists to remove, so it is left alone and stated instead.
    #[tokio::test]
    async fn a_dashed_line_inside_a_yaml_value_does_not_end_the_frontmatter() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("fenced/SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(
            &path,
            "---\nname: fenced\ndescription: |\n  intro\n  --- not a fence\n  outro\n---\nReal body",
        )
        .await
        .unwrap();

        let report = load_skills_from_dir(temp_dir.path()).await;
        assert_eq!(
            report.tools.len(),
            1,
            "errors: {}",
            report.failure_summary()
        );
        let description = &report.tools[0].spec.description;
        assert!(
            description.contains("not a fence") && description.contains("outro"),
            "the whole block scalar belongs to the frontmatter, got {description:?}"
        );
        assert_eq!(report.tools[0].spec.markdown_content, "Real body");
    }
}
