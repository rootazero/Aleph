//! Skill content preprocessing — template-variable substitution and opt-in
//! inline-shell expansion applied when a skill's instructions are loaded
//! (Level-2 progressive disclosure, via `skill_read`).
//!
//! This maps hermes-agent's `agent/skill_preprocessing.py` onto Aleph's Rust
//! core. Two capabilities the reference provides that the raw `fs::read_to_string`
//! path lacked:
//!
//! 1. **Template variables** — `${ALEPH_SKILL_DIR}` in the skill's prose
//!    resolves to the skill's own directory, so instructions can point at
//!    bundled scripts/resources. An unknown token is left literal — matching
//!    the reference, which leaves it in place rather than erroring.
//! 2. **Inline shell** — `` !`cmd` `` snippets are executed with the skill
//!    directory as the working directory and replaced by the command's stdout,
//!    so a skill can embed live context (e.g. `` !`git rev-parse HEAD` ``).
//!
//! ## An inline command here is an inline command everywhere
//!
//! A skill's `` !`cmd` `` is the same thing as a plugin command's, and runs
//! under the same three rules (`extension::inline_shell`):
//! - **which text** is a command: [`inline_commands`], on the body as
//!   written, before anything is expanded — never rescanned;
//! - **what the shell parses**: the text as written. `${ALEPH_SKILL_DIR}`
//!   (and, in a plugin's skill, the plugin's path variables) reach it through
//!   the child's environment only ([`inline_shell_command`] with
//!   [`InlineSite::skill_dir`]), so a skill directory named `s $(touch M)` is
//!   one word of data. The prose between commands keeps its textual
//!   expansion;
//! - **who may run one**: only an operator's call whose tool gate would not
//!   deny `bash` (`tools::turn_context::current_inline_shell_refusal`), and
//!   only a text approved in the consent registry, filed per `(owner, skill,
//!   text)` under `SkillRead` ([`SKILL_INLINE_EVENT`]) and reviewed with
//!   `aleph-server hooks list` / `test` — a user's own skill included: the
//!   model can write a skill (`skill_manage`, or `file_write` into a
//!   project's `.aleph/skills`), so a skill's origin is no approval.
//!   Otherwise each command is a placeholder naming why.
//!
//! ## Differences from the reference (Rust advantages)
//!
//! - The common case (no template token, no opt-in) is allocation-free: the
//!   input is returned borrowed and untouched, so every existing skill renders
//!   byte-for-byte identically.
//! - Inline-shell snippets run **concurrently** via `futures::join_all` rather
//!   than the reference's sequential loop — N snippets cost ~1 snippet of
//!   wall-clock instead of N.
//! - Inline shell is gated behind an explicit per-skill frontmatter opt-in
//!   (`allow-inline-shell: true`); without it the shell path is never entered.
//!   This keeps arbitrary command execution off by default (P7 defensive
//!   design) while still giving skill authors the capability when they ask.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crate::extension::hooks::{ShellHookConsent, SKILL_INLINE_EVENT};
use crate::extension::inline_shell::{run_inline_process, withheld, InlineConsent, Withheld};
use crate::extension::visibility::ScopeKey;
use crate::extension::{
    inline_commands, inline_shell_command, run_inline, InlineArgs, InlineShell, InlineSite,
    TemplateCtx, SKILL_DIR_TOKEN,
};
use crate::sync_primitives::Arc;

/// Aggregate cap on everything inline shell splices into a skill body, in
/// bytes. The per-snippet cap bounds one snippet; without an aggregate cap a
/// body with N snippets still grows by `N * per-snippet` — expansion blowup
/// the install-time size cap on SKILL.md cannot see. 256 KiB of spliced
/// stdout is far past any legitimate live-context use; past it the whole
/// expansion is refused rather than truncated, because a half-spliced body
/// silently changes the skill's meaning.
const MAX_TOTAL_SNIPPET_OUTPUT_BYTES: usize = 256 * 1024;

/// Errors raised when preprocessing cannot produce a body safe to hand to
/// the model.
///
/// Surfaced as a hard failure rather than a fallback to the unexpanded body:
/// for an opted-in skill the snippets ARE the instructions, and silently
/// serving the pre-expansion text would leave the model following a
/// different skill than the author shipped.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PreprocessError {
    /// Re-running the content guard on the expanded body found a threat that
    /// the install-time scan of the source could not have seen — it arrived
    /// via snippet stdout. The string lists the findings (`file: pattern`).
    #[error("inline-shell expansion introduced guarded content: {0}")]
    GuardedExpansion(String),
    /// Accumulated snippet stdout blew past [`MAX_TOTAL_SNIPPET_OUTPUT_BYTES`].
    #[error("inline-shell expansion exceeds the {max}-byte aggregate stdout cap ({size} bytes)")]
    ExpansionTooLarge { size: usize, max: usize },
}

/// Context for preprocessing a single skill file.
pub struct SkillPreprocessContext {
    /// Absolute path to the skill's directory (the parent of `SKILL.md`).
    pub skill_dir: PathBuf,
    /// What runs the skill's inline commands, when it opts in: a
    /// [`SkillShell`], or `Withheld` naming why this call may run none.
    pub shell: Box<dyn InlineShell>,
}

impl SkillPreprocessContext {
    /// A context for a skill rooted at `skill_dir` whose inline commands
    /// `shell` runs.
    pub fn new(skill_dir: impl Into<PathBuf>, shell: Box<dyn InlineShell>) -> Self {
        Self {
            skill_dir: skill_dir.into(),
            shell,
        }
    }

    /// The context `skill_read` reads the skill at `skill_dir` with — built
    /// as the command face builds its runner (`render_command`): when this
    /// call may run no inline command
    /// ([`current_inline_shell_refusal`](crate::tools::turn_context::current_inline_shell_refusal)),
    /// every one is withheld naming why; otherwise each answers to `consent`
    /// ([`SkillShell`]).
    pub fn for_read(skill_dir: PathBuf, consent: Arc<ShellHookConsent>) -> Self {
        let shell: Box<dyn InlineShell> =
            match crate::tools::turn_context::current_inline_shell_refusal() {
                Some(reason) => Box::new(Withheld(reason)),
                None => Box::new(SkillShell::new(skill_dir.clone(), consent)),
            };
        Self::new(skill_dir, shell)
    }
}

/// Preprocess a skill file's content.
///
/// Template variables are always expanded in the prose (a no-op for content
/// that contains none). Inline-shell snippets are run only when the skill's
/// frontmatter opts in with `allow-inline-shell: true`, through
/// [`SkillPreprocessContext::shell`]; otherwise the content is returned
/// after template expansion alone.
///
/// The expanded body is re-scanned by the install-time content guard before
/// it is returned (see [`guard_expanded_body`]); a body that trips the guard
/// or the aggregate expansion cap is an error, not a partial result.
pub async fn preprocess_skill_content(
    content: &str,
    ctx: &SkillPreprocessContext,
) -> Result<String, PreprocessError> {
    // Inline shell is opt-in per skill, read from the frontmatter of the
    // content as written.
    if frontmatter_allows_inline_shell(content) {
        let spliced = expand_inline_shell(content, ctx).await?;
        guard_expanded_body(&spliced)?;
        Ok(spliced)
    } else {
        Ok(expand_template_vars(content, &ctx.skill_dir).into_owned())
    }
}

/// Re-run the content guard on the fully spliced body.
///
/// This MUST happen after splicing, never on the source. The install-time
/// scan sees `` !`cmd` `` as inert text, while the model receives whatever
/// stdout the command produced — and that stdout is runtime data shaped by
/// whatever the command inspects (the repository's files, `git log`, the
/// environment), i.e. a channel through which content the install-time
/// guard rejected, or never saw, would otherwise enter the model's system
/// context unexamined. Scanned under [`crate::skill::guard::TrustLevel::Community`]
/// regardless of the skill's install provenance: snippet stdout is no more
/// trustworthy than community content even when the skill itself is bundled.
fn guard_expanded_body(body: &str) -> Result<(), PreprocessError> {
    use crate::skill::guard::{install_allowed, scan_content, TrustLevel};

    let verdict = scan_content("inline-shell-expanded body", body.as_bytes());
    if !install_allowed(verdict.level, TrustLevel::Community) {
        return Err(PreprocessError::GuardedExpansion(
            verdict
                .findings
                .iter()
                .map(|f| format!("{}: {}", f.file, f.pattern_id))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    Ok(())
}

/// Replace `${ALEPH_SKILL_DIR}` tokens in `content` — prose, never an inline
/// command's text ([`expand_inline_shell`] hands this only the stretches
/// between commands).
///
/// Returns the input borrowed and unchanged when no resolvable token is
/// present, so the overwhelmingly common case allocates nothing. Unknown
/// `${ALEPH_SESSION_ID}` tokens are left literal — the wire that would feed
/// them has been severed (the sole production caller never sets one), and the
/// `retain` matches the upstream hermes-agent reference.
#[must_use]
pub fn expand_template_vars<'a>(content: &'a str, skill_dir: &Path) -> Cow<'a, str> {
    if !content.contains(SKILL_DIR_TOKEN) {
        return Cow::Borrowed(content);
    }
    Cow::Owned(content.replace(SKILL_DIR_TOKEN, &skill_dir.to_string_lossy()))
}

/// Whether the skill's YAML frontmatter sets `allow-inline-shell: true`.
///
/// Cheap and self-contained: reuses the manifest frontmatter splitter and
/// parses a single optional boolean. Any parse failure (no frontmatter, bad
/// YAML) is treated as "not allowed".
#[must_use]
pub fn frontmatter_allows_inline_shell(content: &str) -> bool {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct Probe {
        #[serde(default)]
        allow_inline_shell: bool,
    }

    match crate::skill::manifest::split_frontmatter(content) {
        Ok((yaml, _body)) => {
            crate::yaml::from_str::<Probe>(&yaml).is_ok_and(|p| p.allow_inline_shell)
        }
        Err(_) => false,
    }
}

/// Run every inline command concurrently and splice the results back in
/// place, expanding `${ALEPH_SKILL_DIR}` in the prose between them.
///
/// The commands are [`inline_commands`] of `content` as written — the one
/// recogniser, found before any expansion, so neither the skill directory
/// nor a command's output can open or close one. Each runs with its text as
/// written; its expansion is the command face's ([`run_inline`]): stdout, or
/// the runner's placeholder. A failing command never aborts skill loading;
/// only the aggregate output cap is fatal (a body that large is a blowup,
/// not a broken command).
async fn expand_inline_shell(
    content: &str,
    ctx: &SkillPreprocessContext,
) -> Result<String, PreprocessError> {
    let commands = inline_commands(content);
    let args = InlineArgs {
        raw: "",
        positional: &[],
    };
    let template = TemplateCtx {
        shell: Some(&*ctx.shell),
    };
    let results = futures::future::join_all(
        commands
            .iter()
            .map(|(_, cmd)| run_inline(cmd, &args, &template)),
    )
    .await;

    let prose = |range: std::ops::Range<usize>| {
        expand_template_vars(content.get(range).unwrap_or_default(), &ctx.skill_dir)
    };
    let mut out = String::with_capacity(content.len());
    let mut cursor = 0usize;
    let mut spliced_bytes = 0usize;
    for ((whole, _), rendered) in commands.iter().zip(results) {
        spliced_bytes += rendered.len();
        if spliced_bytes > MAX_TOTAL_SNIPPET_OUTPUT_BYTES {
            return Err(PreprocessError::ExpansionTooLarge {
                size: spliced_bytes,
                max: MAX_TOTAL_SNIPPET_OUTPUT_BYTES,
            });
        }
        out.push_str(&prose(cursor..whole.start));
        out.push_str(&rendered);
        cursor = whole.end;
    }
    out.push_str(&prose(cursor..content.len()));
    Ok(out)
}

/// A skill's inline-command runner: the consent entry a command answers to
/// is `(owner, skill, text)` under [`SKILL_INLINE_EVENT`], bound to the
/// skill's directory ([`InlineConsent`]); an approved one runs through the
/// production builder ([`inline_shell_command`]) in the skill's directory,
/// `ALEPH_SKILL_DIR` naming it, `CLAUDE_PROJECT_DIR` the run's directory,
/// and — for a plugin's skill — the plugin's path variables, with the
/// command face's timeout and output cap ([`run_inline_process`]).
///
/// The owner is the plugin whose published `skills/` directory holds the
/// skill (its id and visibility key), or `user` for any other skill, keyed
/// by the skills directory it sits in — so one user or project skill in two
/// repositories is two entries, as a project hook is (P4.4d).
pub struct SkillShell {
    skill_dir: PathBuf,
    /// The consent owner label ([`ShellHookConsent::skill_owner`]).
    owner: String,
    scope: ScopeKey,
    /// The plugin whose skill this is: its id and install root.
    plugin: Option<(String, PathBuf)>,
    /// The run's directory (`CLAUDE_PROJECT_DIR`); `None` outside a run.
    run_dir: Option<PathBuf>,
    consent: Arc<ShellHookConsent>,
}

impl SkillShell {
    /// The runner for the skill at `skill_dir`, in the run this call belongs
    /// to (`sandbox::context::current_exec_workspace`).
    #[must_use]
    pub fn new(skill_dir: PathBuf, consent: Arc<ShellHookConsent>) -> Self {
        let skill = skill_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let base = skill_dir.parent().unwrap_or(&skill_dir).to_path_buf();
        let published = crate::utils::paths::plugin_skill_dirs()
            .into_iter()
            .find(|published| published.dir == base);
        let (owner, scope, plugin) = match published {
            Some(p) => (
                ShellHookConsent::skill_owner(&p.plugin_id, &skill),
                p.scope_key,
                Some((p.plugin_id, p.plugin_root)),
            ),
            None => (
                ShellHookConsent::skill_owner("user", &skill),
                ScopeKey::project(&base),
                None,
            ),
        };
        Self {
            skill_dir,
            owner,
            scope,
            plugin,
            run_dir: crate::sandbox::context::current_exec_workspace(),
            consent,
        }
    }
}

#[async_trait::async_trait]
impl InlineShell for SkillShell {
    async fn run(&self, cmd: &str, args: &InlineArgs<'_>) -> Result<String, String> {
        InlineConsent {
            consent: &self.consent,
            owner: &self.owner,
            scope: &self.scope,
            root: &self.skill_dir,
            event: SKILL_INLINE_EVENT,
        }
        .admit(cmd)?;
        let Some(run_dir) = self.run_dir.as_deref() else {
            return Err(withheld(cmd, "this turn's directory is not known"));
        };
        let site = InlineSite {
            cwd: run_dir,
            plugin: self
                .plugin
                .as_ref()
                .map(|(id, root)| (id.as_str(), root.as_path())),
            skill_dir: Some(&self.skill_dir),
        };
        run_inline_process(cmd, inline_shell_command(cmd, args, &site)).await
    }
}

#[cfg(test)]
mod tests;
