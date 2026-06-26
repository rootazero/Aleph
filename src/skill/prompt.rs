//! XML prompt builder — generates `<available_skills>` XML for system prompt injection.

use crate::domain::skill::SkillManifest;
use crate::thinker::xml_util::escape_xml;
use sha2::{Digest, Sha256};
use std::borrow::Cow;

/// Compute a short, stable content version tag for a skill, e.g.
/// `sha256:a1b2c3d4e5f6a7b8`.
///
/// Mirrors openclaw/pi/hermes-agent: the model reads a skill's full body once
/// (via `skill_read`) and caches it across turns. Aleph uniquely lets the model
/// *rewrite* skills mid-session through `skill_manage` (patch/edit), so the
/// cached instructions can silently go stale. Emitting a content digest in the
/// `<available_skills>` index gives the model a cheap signal — when a skill's
/// `<version>` differs from a previous turn, the body changed and must be
/// re-read. Pure scaffolding (a content hash, no reasoning) — R10-compliant.
///
/// The 16-hex prefix (64 bits) is collision-resistant enough for change
/// detection while staying compact in the prompt.
#[must_use]
fn content_version(skill: &SkillManifest) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(skill.content().as_str().as_bytes());
    let mut tag = String::with_capacity("sha256:".len() + 16);
    tag.push_str("sha256:");
    for byte in &digest[..8] {
        let _ = write!(tag, "{byte:02x}");
    }
    tag
}

/// Deferred loading guidance appended after skill index in system prompts.
/// Tells the LLM to call `skill_read` before executing a skill, and carries
/// the self-improvement doctrine (mirrors hermes-agent): the model authors
/// and repairs skills through `skill_manage` instead of relying on a
/// deterministic curator (R7/R9 — the judgment lives in the prompt).
pub const DEFERRED_LOADING_GUIDANCE: &str =
    "To use a skill, first call the `skill_read` tool with the skill name \
     to load its full instructions, then follow those instructions. \
     Use `skill_list` to discover available skills if needed.\n\n\
     When a user's request matches a skill's <when> trigger, proactively \
     invoke that skill without waiting for an explicit request.\n\n\
     Each skill carries a <version> tag. If a skill's <version> differs from \
     when you last read it, its instructions changed — re-read it with \
     `skill_read` before relying on the cached body.\n\n\
     After completing a complex or novel task, consider saving the \
     methodology as a reusable skill via `skill_manage(action='create')`. \
     If a skill's instructions turn out to be outdated or wrong while you \
     use them, repair the skill immediately with `skill_manage` \
     (action='patch' for a targeted fix, action='edit' for a rewrite).";

/// Default cap on the number of skills listed in the injected prompt index.
///
/// Mirrors the budgeting that codex (token budget) and openclaw
/// (`maxSkillsInPrompt`) apply: a large skill library (the host's
/// `~/.aleph/skills` plus `~/.claude/skills` can hold 100+) must not bloat
/// every system prompt. Skills beyond the cap are still fully usable — the
/// model is told to call `skill_list` to enumerate them.
pub const DEFAULT_MAX_SKILLS_IN_PROMPT: usize = 64;

/// Default cap on the total character length of the rendered `<skill>` body.
/// ~12k chars ≈ 3k tokens — a generous ceiling that bounds worst-case bloat.
pub const DEFAULT_MAX_SKILLS_PROMPT_CHARS: usize = 12_000;

/// Budget controlling how many skills (and how many characters) the
/// `<available_skills>` index may consume in a system prompt.
///
/// A field set to `0` means "unlimited" for that dimension.
///
/// Serializable so it can live under `[prompt_budget]` in `skills.toml`
/// ([`crate::skill::config::SkillsConfig`]). The container-level
/// `#[serde(default)]` fills any field omitted from the TOML table from
/// [`Default`] (64 skills / 12k chars), so a partial table stays valid and a
/// default config deserializes to the built-in budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SkillPromptBudget {
    /// Maximum number of `<skill>` entries to render (`0` = unlimited).
    pub max_skills: usize,
    /// Maximum total characters across all `<skill>` fragments (`0` = unlimited).
    pub max_chars: usize,
}

impl Default for SkillPromptBudget {
    fn default() -> Self {
        Self {
            max_skills: DEFAULT_MAX_SKILLS_IN_PROMPT,
            max_chars: DEFAULT_MAX_SKILLS_PROMPT_CHARS,
        }
    }
}

impl SkillPromptBudget {
    /// A budget that imposes no limits (renders every skill).
    #[must_use]
    pub const fn unlimited() -> Self {
        Self {
            max_skills: 0,
            max_chars: 0,
        }
    }
}

/// Render a single skill to its indented `<skill>…</skill>` XML fragment.
fn render_skill_fragment(skill: &SkillManifest) -> String {
    let mut buf = String::from("  <skill>\n");
    buf.push_str("    <name>");
    buf.push_str(&escape_xml(skill.name()));
    buf.push_str("</name>\n");
    buf.push_str("    <description>");
    buf.push_str(&escape_xml(skill.description()));
    buf.push_str("</description>\n");
    if let Some(when) = skill.when_to_use() {
        buf.push_str("    <when>");
        buf.push_str(&escape_xml(when));
        buf.push_str("</when>\n");
    }
    buf.push_str("    <version>");
    buf.push_str(&content_version(skill));
    buf.push_str("</version>\n");
    buf.push_str("  </skill>\n");
    buf
}

/// Render a single skill to a *compact* `<skill>…</skill>` XML fragment that
/// keeps the skill discoverable at a fraction of the character cost.
///
/// Emits only the `<name>` and (when present) `<when>` trigger — the
/// `<description>` is elided. The model still sees the skill exists and what
/// activates it, and the standing [`DEFERRED_LOADING_GUIDANCE`] tells it to
/// call `skill_read` for the full instructions anyway, so dropping the inline
/// description costs no real capability. Used as the second degradation tier
/// when a full render would overflow the prompt budget.
fn render_skill_fragment_compact(skill: &SkillManifest) -> String {
    let mut buf = String::from("  <skill>\n");
    buf.push_str("    <name>");
    buf.push_str(&escape_xml(skill.name()));
    buf.push_str("</name>\n");
    if let Some(when) = skill.when_to_use() {
        buf.push_str("    <when>");
        buf.push_str(&escape_xml(when));
        buf.push_str("</when>\n");
    }
    buf.push_str("    <version>");
    buf.push_str(&content_version(skill));
    buf.push_str("</version>\n");
    buf.push_str("  </skill>\n");
    buf
}

/// Build an XML fragment listing the given skills for injection into a system prompt.
///
/// Applies [`SkillPromptBudget::default`]; see
/// [`build_skills_prompt_xml_with_budget`] for the budgeting semantics.
/// When the skill set fits within the default budget the output is identical
/// to rendering every skill in input order.
///
/// Returns an empty string if the slice is empty.
///
/// Output format:
/// ```xml
/// <available_skills>
///   <skill>
///     <name>Git Commit</name>
///     <description>Helps write commit messages</description>
///   </skill>
/// </available_skills>
/// ```
#[must_use]
pub fn build_skills_prompt_xml(skills: &[&SkillManifest]) -> String {
    build_skills_prompt_xml_with_budget(skills, &SkillPromptBudget::default())
}

/// Build the `<available_skills>` XML, bounded by `budget`.
///
/// Fast path: when the full set fits within both budget dimensions the skills
/// are emitted in their original input order (byte-identical to an uncapped
/// render).
///
/// Over budget, entries are walked by descending [`SkillSource`] priority
/// (Workspace > Plugin > Global > Bundled, then name) so the most specific /
/// user-authored skills keep full detail, then degraded in two tiers rather
/// than hard-dropped:
///
/// 1. **Full** (`<name>` + `<description>` + `<when>`) while the char budget
///    allows — the first entry is always admitted even if it alone exceeds the
///    cap.
/// 2. **Compact** ([`render_skill_fragment_compact`]: `<name>` + `<when>`, the
///    description elided) for the remainder, so a surviving skill stays
///    nameable and the model can still `skill_read` it on demand.
///
/// `max_skills` caps the total number of rendered entries (full and compact
/// alike). Only skills that fit in neither tier are omitted, and a `<note>`
/// element then tells the model how many were dropped and to call `skill_list`.
///
/// [`SkillSource`]: crate::domain::skill::SkillSource
pub fn build_skills_prompt_xml_with_budget(
    skills: &[&SkillManifest],
    budget: &SkillPromptBudget,
) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let fragments: Vec<String> = skills.iter().map(|s| render_skill_fragment(s)).collect();
    let total_chars: usize = fragments.iter().map(String::len).sum();

    let count_ok = budget.max_skills == 0 || skills.len() <= budget.max_skills;
    let chars_ok = budget.max_chars == 0 || total_chars <= budget.max_chars;

    // Fast path: everything fits — preserve input order, render all.
    if count_ok && chars_ok {
        return wrap_skills(fragments.iter().map(String::as_str), 0);
    }

    // Over budget: walk by descending source priority, then name ascending,
    // so higher-value skills keep full detail. Stable on ties via name.
    let mut order: Vec<usize> = (0..skills.len()).collect();
    order.sort_by(|&a, &b| {
        skills[b]
            .priority()
            .cmp(&skills[a].priority())
            .then_with(|| skills[a].name().cmp(skills[b].name()))
    });

    let cap_skills = if budget.max_skills == 0 {
        usize::MAX
    } else {
        budget.max_skills
    };
    let cap_chars = if budget.max_chars == 0 {
        usize::MAX
    } else {
        budget.max_chars
    };

    let mut selected: Vec<Cow<'_, str>> = Vec::new();
    let mut used = 0usize;
    for &idx in &order {
        // `max_skills` caps the total rendered entries (full + compact alike).
        if selected.len() >= cap_skills {
            break;
        }
        let full = fragments[idx].as_str();
        // Tier 1 — full fragment. Always admit the first entry even if it alone
        // exceeds the char cap.
        if selected.is_empty() || used + full.len() <= cap_chars {
            used += full.len();
            selected.push(Cow::Borrowed(full));
            continue;
        }
        // Tier 2 — compact fragment (name + when). Keeps the skill discoverable
        // at a fraction of the cost; omit only if even this overflows. Keep
        // scanning so a shorter lower-priority entry may still fit.
        let compact = render_skill_fragment_compact(skills[idx]);
        if used + compact.len() <= cap_chars {
            used += compact.len();
            selected.push(Cow::Owned(compact));
        }
    }

    let omitted = skills.len() - selected.len();
    wrap_skills(selected.iter().map(|frag| frag.as_ref()), omitted)
}

/// Wrap rendered `<skill>` fragments in the `<available_skills>` envelope,
/// appending an omission `<note>` when `omitted > 0`.
fn wrap_skills<'a>(fragments: impl Iterator<Item = &'a str>, omitted: usize) -> String {
    let mut buf = String::from("<available_skills>\n");
    for frag in fragments {
        buf.push_str(frag);
    }
    if omitted > 0 {
        buf.push_str("  <note>");
        buf.push_str(&format!(
            "{omitted} additional skill(s) omitted to conserve context; \
             call `skill_list` to enumerate all available skills."
        ));
        buf.push_str("</note>\n");
    }
    buf.push_str("</available_skills>");
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{
        InvocationPolicy, PromptScope, SkillContent, SkillId, SkillManifest, SkillSource,
    };

    fn make_skill(name: &str, desc: &str) -> SkillManifest {
        SkillManifest::new(
            SkillId::new(name.to_lowercase().replace(' ', "-")),
            name,
            desc,
            SkillContent::new("content"),
            SkillSource::Bundled,
        )
    }

    #[test]
    fn empty_skills_empty_xml() {
        let result = build_skills_prompt_xml(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn single_skill_xml() {
        let skill = make_skill("Git Commit", "Helps write commit messages");
        let xml = build_skills_prompt_xml(&[&skill]);

        assert!(xml.starts_with("<available_skills>"));
        assert!(xml.ends_with("</available_skills>"));
        assert!(xml.contains("<name>Git Commit</name>"));
        assert!(xml.contains("<description>Helps write commit messages</description>"));
    }

    #[test]
    fn multiple_skills_xml() {
        let s1 = make_skill("Git Commit", "Write commits");
        let s2 = make_skill("Docker Build", "Build images");
        let xml = build_skills_prompt_xml(&[&s1, &s2]);

        // Count <skill> occurrences
        let count = xml.matches("<skill>").count();
        assert_eq!(count, 2);

        assert!(xml.contains("<name>Git Commit</name>"));
        assert!(xml.contains("<name>Docker Build</name>"));
    }

    #[test]
    fn disabled_scope_excluded() {
        // Verify is_model_visible correctly identifies disabled skills
        let mut disabled = make_skill("Hidden", "Not visible");
        disabled.set_scope(PromptScope::Disabled);
        assert!(!disabled.is_model_visible());

        let mut model_disabled = make_skill("Model Hidden", "Not for model");
        model_disabled.set_invocation(InvocationPolicy {
            disable_model_invocation: true,
            ..Default::default()
        });
        assert!(!model_disabled.is_model_visible());

        // A visible skill should pass
        let visible = make_skill("Visible", "Can be seen");
        assert!(visible.is_model_visible());

        // Only include model-visible skills
        let all = [&disabled, &model_disabled, &visible];
        let visible_only: Vec<&&SkillManifest> =
            all.iter().filter(|s| s.is_model_visible()).collect();
        assert_eq!(visible_only.len(), 1);

        let xml = build_skills_prompt_xml(&visible_only.into_iter().copied().collect::<Vec<_>>());
        assert!(xml.contains("<name>Visible</name>"));
        assert!(!xml.contains("Hidden"));
        assert!(!xml.contains("Model Hidden"));
    }

    #[test]
    fn xml_escaping() {
        let skill = make_skill("A & B", "Uses <tags> & stuff");
        let xml = build_skills_prompt_xml(&[&skill]);
        assert!(xml.contains("<name>A &amp; B</name>"));
        assert!(xml.contains("&lt;tags&gt;"));
    }

    #[test]
    fn xml_includes_when_to_use() {
        let mut skill = make_skill("Code Review", "Reviews code quality");
        skill.set_when_to_use("When code has been modified".to_string());
        let xml = build_skills_prompt_xml(&[&skill]);

        assert!(xml.contains("<when>When code has been modified</when>"));
    }

    #[test]
    fn xml_omits_when_tag_if_none() {
        let skill = make_skill("Simple", "A simple skill");
        let xml = build_skills_prompt_xml(&[&skill]);

        assert!(!xml.contains("<when>"));
        assert!(xml.contains("<name>Simple</name>"));
    }

    #[test]
    fn xml_escapes_when_to_use() {
        let mut skill = make_skill("Test", "Test skill");
        skill.set_when_to_use("When <user> asks & needs help".to_string());
        let xml = build_skills_prompt_xml(&[&skill]);

        assert!(xml.contains("<when>When &lt;user&gt; asks &amp; needs help</when>"));
    }

    #[test]
    fn deferred_loading_guidance_includes_proactive_trigger() {
        assert!(
            DEFERRED_LOADING_GUIDANCE.contains("proactively"),
            "Guidance should mention proactive invocation"
        );
        assert!(
            DEFERRED_LOADING_GUIDANCE.contains("<when>"),
            "Guidance should reference the <when> trigger tag"
        );
    }

    fn make_skill_with_source(name: &str, source: SkillSource) -> SkillManifest {
        SkillManifest::new(
            SkillId::new(name.to_lowercase().replace(' ', "-")),
            name,
            format!("{name} description"),
            SkillContent::new("content"),
            source,
        )
    }

    #[test]
    fn under_budget_renders_all_in_input_order_no_note() {
        let s1 = make_skill("Bravo", "second alphabetically");
        let s2 = make_skill("Alpha", "first alphabetically");
        let refs = [&s1, &s2];
        let budget = SkillPromptBudget {
            max_skills: 10,
            max_chars: 0,
        };

        let xml = build_skills_prompt_xml_with_budget(&refs, &budget);
        // No omission note when everything fits.
        assert!(!xml.contains("<note>"));
        // Input order preserved (Bravo before Alpha) — fast path does not sort.
        let bravo = xml.find("Bravo").unwrap();
        let alpha = xml.find("Alpha").unwrap();
        assert!(bravo < alpha, "fast path must preserve input order");
    }

    #[test]
    fn default_wrapper_matches_unlimited_when_small() {
        // The default-budget wrapper must be byte-identical to an explicit
        // unlimited render for a small skill set (backward compatibility).
        let s1 = make_skill("Git Commit", "Write commits");
        let s2 = make_skill("Docker Build", "Build images");
        let refs = [&s1, &s2];

        let via_default = build_skills_prompt_xml(&refs);
        let via_unlimited =
            build_skills_prompt_xml_with_budget(&refs, &SkillPromptBudget::unlimited());
        assert_eq!(via_default, via_unlimited);
    }

    #[test]
    fn count_budget_truncates_and_emits_note() {
        let skills: Vec<SkillManifest> = (0..5)
            .map(|i| make_skill(&format!("Skill{i}"), "d"))
            .collect();
        let refs: Vec<&SkillManifest> = skills.iter().collect();
        let budget = SkillPromptBudget {
            max_skills: 2,
            max_chars: 0,
        };

        let xml = build_skills_prompt_xml_with_budget(&refs, &budget);
        assert_eq!(xml.matches("<skill>").count(), 2);
        assert!(xml.contains("<note>"));
        assert!(xml.contains("3 additional skill(s) omitted"));
        assert!(xml.contains("skill_list"));
    }

    #[test]
    fn char_budget_truncates() {
        // Each fragment is well over 40 chars; a 1-char budget keeps exactly one.
        let skills: Vec<SkillManifest> = (0..4)
            .map(|i| make_skill(&format!("Skill{i}"), "d"))
            .collect();
        let refs: Vec<&SkillManifest> = skills.iter().collect();
        let budget = SkillPromptBudget {
            max_skills: 0,
            max_chars: 1,
        };

        let xml = build_skills_prompt_xml_with_budget(&refs, &budget);
        // Always include at least one even if it alone exceeds the char cap.
        assert_eq!(xml.matches("<skill>").count(), 1);
        assert!(xml.contains("3 additional skill(s) omitted"));
    }

    #[test]
    fn truncation_prefers_higher_source_priority() {
        // Workspace(4) > Plugin(3) > Global(2) > Bundled(1): when truncating to
        // 2, the two highest-priority skills must survive regardless of order.
        let bundled = make_skill_with_source("BundledSkill", SkillSource::Bundled);
        let workspace = make_skill_with_source("WorkspaceSkill", SkillSource::Workspace);
        let global = make_skill_with_source("GlobalSkill", SkillSource::Global);
        let refs = [&bundled, &workspace, &global];
        let budget = SkillPromptBudget {
            max_skills: 2,
            max_chars: 0,
        };

        let xml = build_skills_prompt_xml_with_budget(&refs, &budget);
        assert!(
            xml.contains("WorkspaceSkill"),
            "highest priority must survive"
        );
        assert!(xml.contains("GlobalSkill"), "second priority must survive");
        assert!(
            !xml.contains("BundledSkill"),
            "lowest priority must be dropped"
        );
        assert!(xml.contains("1 additional skill(s) omitted"));
    }

    #[test]
    fn unlimited_budget_renders_everything() {
        let skills: Vec<SkillManifest> = (0..100)
            .map(|i| make_skill(&format!("Skill{i}"), "d"))
            .collect();
        let refs: Vec<&SkillManifest> = skills.iter().collect();

        let xml = build_skills_prompt_xml_with_budget(&refs, &SkillPromptBudget::unlimited());
        assert_eq!(xml.matches("<skill>").count(), 100);
        assert!(!xml.contains("<note>"));
    }

    fn make_skill_with_when(name: &str, desc: &str, when: &str) -> SkillManifest {
        let mut m = SkillManifest::new(
            SkillId::new(name.to_lowercase().replace(' ', "-")),
            name,
            desc,
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        m.set_when_to_use(when.to_string());
        m
    }

    #[test]
    fn compact_fragment_keeps_name_and_when_drops_description() {
        let skill = make_skill_with_when("Deploy", "a very long description", "on release");
        let frag = render_skill_fragment_compact(&skill);
        assert!(frag.contains("<name>Deploy</name>"));
        assert!(frag.contains("<when>on release</when>"));
        assert!(!frag.contains("<description>"));
        // Compact fragments still carry the version so staleness is detectable
        // even after budget degradation.
        assert!(frag.contains("<version>sha256:"));
    }

    fn make_skill_with_content(name: &str, content: &str) -> SkillManifest {
        SkillManifest::new(
            SkillId::new(name.to_lowercase().replace(' ', "-")),
            name,
            format!("{name} description"),
            SkillContent::new(content),
            SkillSource::Bundled,
        )
    }

    #[test]
    fn full_fragment_includes_version_tag() {
        let skill = make_skill("Deploy", "Ships the app");
        let xml = build_skills_prompt_xml(&[&skill]);
        // sha256: prefix + 16 hex chars (8 bytes).
        assert!(xml.contains("<version>sha256:"));
        let tag = content_version(&skill);
        assert_eq!(tag.len(), "sha256:".len() + 16);
        assert!(xml.contains(&format!("<version>{tag}</version>")));
    }

    #[test]
    fn version_tracks_content_not_metadata() {
        // Same name/description, different body → different version (so the
        // model re-reads after a skill_manage edit changed the instructions).
        let v1 = make_skill_with_content("Deploy", "step one: build");
        let v2 = make_skill_with_content("Deploy", "step one: build\nstep two: ship");
        assert_ne!(content_version(&v1), content_version(&v2));

        // Identical body → identical version (stable across turns).
        let v1_again = make_skill_with_content("Deploy", "step one: build");
        assert_eq!(content_version(&v1), content_version(&v1_again));
    }

    #[test]
    fn deferred_guidance_mentions_version_reread() {
        assert!(
            DEFERRED_LOADING_GUIDANCE.contains("<version>"),
            "guidance must teach the model to watch the version tag"
        );
    }

    #[test]
    fn over_char_budget_degrades_tail_to_compact_instead_of_dropping() {
        // Four equal-priority skills with long descriptions. The char budget
        // fits one full fragment plus several compact ones — the old behaviour
        // hard-dropped the tail (3 omitted); graceful degradation keeps every
        // name visible (0 omitted) by eliding descriptions on the tail.
        let long = "d".repeat(400);
        let skills: Vec<SkillManifest> = (0..4)
            .map(|i| make_skill_with_when(&format!("Skill{i}"), &long, &format!("trig{i}")))
            .collect();
        let refs: Vec<&SkillManifest> = skills.iter().collect();

        // One full fragment is ~550 chars (desc + <version> tag); a compact one
        // is ~115. Budget admits the first full (~550) plus the three compact
        // tails (~345) but cannot fit a second full (~1100 total).
        let budget = SkillPromptBudget {
            max_skills: 0,
            max_chars: 900,
        };
        let xml = build_skills_prompt_xml_with_budget(&refs, &budget);

        // Every skill name stays discoverable.
        for i in 0..4 {
            assert!(
                xml.contains(&format!("<name>Skill{i}</name>")),
                "Skill{i} name must remain visible after degradation"
            );
        }
        // Exactly one full description survives; the rest degraded to compact.
        assert_eq!(
            xml.matches("<description>").count(),
            1,
            "only the highest-priority entry keeps its description"
        );
        // Compact tails preserve their <when> trigger for discovery.
        assert!(xml.contains("<when>trig3</when>"));
        // Nothing was actually dropped, so no omission note.
        assert!(!xml.contains("<note>"));
    }

    #[test]
    fn compact_tier_still_omits_and_notes_when_even_compact_overflows() {
        // A 1-char budget cannot fit even a compact fragment, so the tail is
        // truly omitted and the note is emitted — preserving the prior contract.
        let skills: Vec<SkillManifest> = (0..3)
            .map(|i| make_skill_with_when(&format!("Skill{i}"), "d", &format!("trig{i}")))
            .collect();
        let refs: Vec<&SkillManifest> = skills.iter().collect();
        let budget = SkillPromptBudget {
            max_skills: 0,
            max_chars: 1,
        };

        let xml = build_skills_prompt_xml_with_budget(&refs, &budget);
        assert_eq!(xml.matches("<skill>").count(), 1);
        assert!(xml.contains("2 additional skill(s) omitted"));
    }
}
