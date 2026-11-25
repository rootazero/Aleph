//! One note mutation, prepared from Markdown and verified against its projection.

use super::{render_note, sha2_hash, NoteIndexer};
use crate::error::AlephError;
use crate::memory::dreaming::distill_action::DistillAction;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::{sanitize_title, KnowledgeNote, Severity};
use crate::utils::atomic_write::atomic_write_file;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use tokio::fs;

const MAX_NOTE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotePath {
    category: String,
    name: String,
}

impl NotePath {
    fn new(category: &str, name: &str) -> Result<Self, AlephError> {
        Ok(Self {
            category: sanitize_title(category)?,
            name: sanitize_title(name)?,
        })
    }

    fn parse(path: &str) -> Result<Self, AlephError> {
        let (category, name) = path
            .split_once('/')
            .ok_or_else(|| AlephError::other("distill target must be category/name"))?;
        let target = Self::new(category, name)?;
        if target.key() != path {
            return Err(AlephError::other("distill target is not canonical"));
        }
        Ok(target)
    }

    fn key(&self) -> String {
        format!("{}/{}", self.category, self.name)
    }

    fn file(&self, root: &Path) -> PathBuf {
        root.join(&self.category).join(format!("{}.md", self.name))
    }

    fn validate(&self) -> Result<(), AlephError> {
        if Self::new(&self.category, &self.name)? != *self {
            return Err(AlephError::other("prepared distill path is not canonical"));
        }
        Ok(())
    }
}

/// A frozen intent. A retry must use these bytes, not regenerate them from the
/// prompt, the current date, or the note left by a previous partial attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparedDistillEffect {
    destination: NotePath,
    expected_before: Option<String>,
    markdown: String,
    supersession: Option<Supersession>,
    reembed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Supersession {
    original: NotePath,
    original_markdown: String,
    date: chrono::NaiveDate,
    archive_stem: String,
}

impl Supersession {
    fn archive_markdown(&self, destination: &NotePath) -> String {
        format!(
            "{}\n## Superseded by [[{}]] ({})\n",
            self.original_markdown,
            destination.key(),
            self.date
        )
    }

    fn archive_file(&self, root: &Path) -> PathBuf {
        root.join("archive")
            .join(&self.original.category)
            .join(format!("{}.md", self.archive_stem))
    }
}

impl PreparedDistillEffect {
    fn validate(&self) -> Result<(), AlephError> {
        self.destination.validate()?;
        if self.markdown.is_empty() || self.markdown.len() > MAX_NOTE_BYTES {
            return Err(AlephError::other("invalid prepared distill note size"));
        }
        if let Some(before) = &self.expected_before {
            if before.len() != 64 || !before.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(AlephError::other("invalid prepared distill preimage"));
            }
        }
        if let Some(replacement) = &self.supersession {
            replacement.original.validate()?;
            if replacement.original_markdown.is_empty()
                || replacement.original_markdown.len() > MAX_NOTE_BYTES
            {
                return Err(AlephError::other("invalid prepared original note size"));
            }
            let archive = replacement.archive_markdown(&self.destination);
            let alternate = archive_alternate_stem(&replacement.original, &archive);
            if replacement.archive_stem != replacement.original.name
                && replacement.archive_stem != alternate
            {
                return Err(AlephError::other("invalid prepared distill archive name"));
            }
            let expected = (replacement.original == self.destination)
                .then(|| sha2_hash(&replacement.original_markdown));
            if self.expected_before != expected {
                return Err(AlephError::other("invalid prepared replacement preimage"));
            }
        }
        Ok(())
    }
}

fn archive_alternate_stem(original: &NotePath, archive: &str) -> String {
    format!(
        "superseded-{}",
        &sha2_hash(&format!("{}\0{archive}", original.key()))[..24]
    )
}

fn check_corpus_component(agent_id: &str) -> Result<(), AlephError> {
    let mut components = Path::new(agent_id).components();
    if !matches!(components.next(), Some(Component::Normal(value)) if value == agent_id)
        || components.next().is_some()
    {
        return Err(AlephError::other("invalid distill corpus path"));
    }
    Ok(())
}

async fn read_optional(path: &Path) -> Result<Option<String>, AlephError> {
    match fs::symlink_metadata(path).await {
        Ok(meta) if meta.is_file() && meta.len() <= (MAX_NOTE_BYTES + 1024) as u64 => {
            fs::read_to_string(path).await.map(Some).map_err(|error| {
                AlephError::other(format!("distill read {}: {error}", path.display()))
            })
        }
        Ok(_) => Err(AlephError::other(format!(
            "distill path is not a bounded regular file: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AlephError::other(format!(
            "distill inspect {}: {error}",
            path.display()
        ))),
    }
}

async fn publish_new(path: &Path, markdown: &str) -> Result<(), AlephError> {
    let path = path.to_owned();
    let markdown = markdown.to_owned();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("note has no parent"))?;
        std::fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(markdown.as_bytes())?;
        temp.as_file().sync_all()?;
        temp.persist_noclobber(&path).map_err(|error| error.error)?;
        Ok(())
    })
    .await
    .map_err(|error| AlephError::other(format!("distill publication task: {error}")))?
    .map_err(|error| AlephError::other(format!("publish distill file: {error}")))
}

// A valid file is not a durable receipt until its rename and any newly
// created vault directories are persisted. Repeat this on recovery too.
#[cfg(unix)]
async fn sync_note_directories(path: &Path, vault: &Path) -> Result<(), AlephError> {
    let vault_parent = vault
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let mut current = path.parent();
    while let Some(directory) = current {
        let handle = fs::File::open(directory).await.map_err(|error| {
            AlephError::other(format!(
                "open distill directory {}: {error}",
                directory.display()
            ))
        })?;
        handle.sync_all().await.map_err(|error| {
            AlephError::other(format!(
                "sync distill directory {}: {error}",
                directory.display()
            ))
        })?;
        if Some(directory) == vault_parent || (vault_parent.is_none() && directory == vault) {
            break;
        }
        current = directory.parent();
    }
    Ok(())
}

// Directory fsync is not exposed consistently on non-Unix targets. The
// synced staging file and atomic publication still support process restart.
#[cfg(not(unix))]
async fn sync_note_directories(_path: &Path, _vault: &Path) -> Result<(), AlephError> {
    Ok(())
}

fn conflict(path: &Path) -> AlephError {
    AlephError::other(format!("unexpected distill file state: {}", path.display()))
}

struct RuleDraft<'a> {
    title: &'a str,
    rule: &'a str,
    confidence: f32,
    severity: Severity,
    sources: &'a [String],
}

impl RuleDraft<'_> {
    fn render(self, category: &str) -> String {
        let now = chrono::Utc::now().timestamp();
        render_note(&KnowledgeNote {
            title: self.title.to_owned(),
            category: category.to_owned(),
            facts: vec![self.rule.to_owned()],
            confidence: self.confidence,
            severity: self.severity,
            source_notes: self.sources.to_vec(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        })
    }
}

impl<S: NoteStore> NoteIndexer<S> {
    /// Apply a distill decision through the same prepare/verify path used for recovery.
    pub async fn apply_distill_action(
        &self,
        agent_id: &str,
        category: &str,
        action: &DistillAction,
    ) -> Result<(), AlephError> {
        let _guard = super::lock_note_corpus(&self.memory_dir, agent_id).await;
        if let Some(effect) = self
            .prepare_distill_effect_locked(agent_id, category, action)
            .await?
        {
            self.verify_distill_effect_locked(agent_id, &effect).await?;
        }
        Ok(())
    }

    pub(super) async fn prepare_distill_effect_locked(
        &self,
        agent_id: &str,
        category: &str,
        action: &DistillAction,
    ) -> Result<Option<PreparedDistillEffect>, AlephError> {
        if matches!(action, DistillAction::Skip { .. }) {
            return Ok(None);
        }
        check_corpus_component(agent_id)?;
        let root = self.memory_dir.join(agent_id);
        let effect = match action {
            DistillAction::New {
                title,
                rule,
                confidence,
                severity,
                source_facts,
            } => {
                let destination = NotePath::new(category, title)?;
                if self
                    .store
                    .get_note_index(&destination.key(), agent_id)
                    .await?
                    .is_some()
                {
                    return self
                        .prepare_strengthen(agent_id, &root, destination, source_facts, *confidence)
                        .await;
                }
                if read_optional(&destination.file(&root)).await?.is_some() {
                    return Err(conflict(&destination.file(&root)));
                }
                PreparedDistillEffect {
                    markdown: RuleDraft {
                        title,
                        rule,
                        confidence: *confidence,
                        severity: *severity,
                        sources: source_facts,
                    }
                    .render(&destination.category),
                    destination,
                    expected_before: None,
                    supersession: None,
                    reembed: true,
                }
            }
            DistillAction::Strengthen {
                existing_note_path,
                source_facts,
            } => {
                let destination = NotePath::parse(existing_note_path)?;
                return self
                    .prepare_strengthen(agent_id, &root, destination, source_facts, 0.0)
                    .await;
            }
            DistillAction::Supersede {
                old_note_path,
                title,
                rule,
                confidence,
                severity,
                source_facts,
            } => {
                let original = NotePath::parse(old_note_path)?;
                let destination = NotePath::new(category, title)?;
                let original_markdown = self.indexed_markdown(agent_id, &root, &original).await?;
                let markdown = RuleDraft {
                    title,
                    rule,
                    confidence: *confidence,
                    severity: *severity,
                    sources: source_facts,
                }
                .render(&destination.category);
                let expected_before = if original == destination {
                    if markdown == original_markdown {
                        return Ok(None);
                    }
                    Some(sha2_hash(&original_markdown))
                } else {
                    if read_optional(&destination.file(&root)).await?.is_some()
                        || self
                            .store
                            .get_note_index(&destination.key(), agent_id)
                            .await?
                            .is_some()
                    {
                        return Err(conflict(&destination.file(&root)));
                    }
                    None
                };
                let mut supersession = Supersession {
                    archive_stem: original.name.clone(),
                    original,
                    original_markdown,
                    date: chrono::Utc::now().date_naive(),
                };
                let archive = supersession.archive_markdown(&destination);
                if read_optional(&supersession.archive_file(&root))
                    .await?
                    .is_some_and(|bytes| bytes != archive)
                {
                    supersession.archive_stem =
                        archive_alternate_stem(&supersession.original, &archive);
                }
                PreparedDistillEffect {
                    markdown,
                    destination,
                    expected_before,
                    supersession: Some(supersession),
                    reembed: true,
                }
            }
            DistillAction::Skip { .. } => return Ok(None),
        };
        effect.validate()?;
        Ok(Some(effect))
    }

    async fn indexed_markdown(
        &self,
        agent_id: &str,
        root: &Path,
        path: &NotePath,
    ) -> Result<String, AlephError> {
        let file = path.file(root);
        let markdown = read_optional(&file).await?.ok_or_else(|| conflict(&file))?;
        let hash = sha2_hash(&markdown);
        if !self
            .store
            .get_note_index(&path.key(), agent_id)
            .await?
            .is_some_and(|entry| entry.content_hash == hash)
        {
            return Err(conflict(&file));
        }
        Ok(markdown)
    }

    async fn prepare_strengthen(
        &self,
        agent_id: &str,
        root: &Path,
        destination: NotePath,
        sources: &[String],
        floor: f32,
    ) -> Result<Option<PreparedDistillEffect>, AlephError> {
        let existing = self.indexed_markdown(agent_id, root, &destination).await?;
        let mut note = KnowledgeNote::from_markdown(&destination.name, &existing)?;
        let previous_source_count = note.source_notes.len();
        for source in sources {
            if !note.source_notes.contains(source) {
                note.source_notes.push(source.clone());
            }
        }
        let added = note.source_notes.len() - previous_source_count;
        let confidence =
            (note.confidence.max(floor) + Self::STRENGTHEN_STEP * added as f32).min(1.0);
        if added == 0 && confidence == note.confidence {
            return Ok(None);
        }
        note.confidence = confidence;
        note.updated_at = chrono::Utc::now().timestamp();
        let effect = PreparedDistillEffect {
            markdown: render_note(&note),
            destination,
            expected_before: Some(sha2_hash(&existing)),
            supersession: None,
            reembed: false,
        };
        effect.validate()?;
        Ok(Some(effect))
    }

    pub(super) async fn verify_distill_effect_locked(
        &self,
        agent_id: &str,
        effect: &PreparedDistillEffect,
    ) -> Result<(), AlephError> {
        check_corpus_component(agent_id)?;
        effect.validate()?;
        let root = self.memory_dir.join(agent_id);
        let destination = &effect.destination;
        let file = destination.file(&root);
        let current = read_optional(&file).await?;
        let already_published = current.as_deref() == Some(&effect.markdown);
        if !already_published && current.as_deref().map(sha2_hash) != effect.expected_before {
            return Err(conflict(&file));
        }
        let after = sha2_hash(&effect.markdown);
        let index = self
            .store
            .get_note_index(&destination.key(), agent_id)
            .await?;
        if index.as_ref().is_some_and(|row| {
            Some(&row.content_hash) != effect.expected_before.as_ref()
                && !(already_published && row.content_hash == after)
        }) {
            return Err(conflict(&file));
        }
        // Parse and validate the expected note before publishing any of its effects.
        let mut note = KnowledgeNote::from_markdown(&destination.name, &effect.markdown)?;
        crate::memory::notes::governance::supersession::sync_body_to_frontmatter(
            &mut note,
            &effect.markdown,
        );

        if let Some(replacement) = &effect.supersession {
            let old = replacement.original.file(&root);
            if replacement.original != *destination {
                let original = read_optional(&old).await?;
                if original
                    .as_ref()
                    .is_some_and(|bytes| bytes != &replacement.original_markdown)
                    || (original.is_none() && !already_published)
                {
                    return Err(conflict(&old));
                }
            }
            let archive = replacement.archive_file(&root);
            let expected = replacement.archive_markdown(destination);
            match read_optional(&archive).await? {
                Some(bytes) if bytes == expected => {}
                None if !already_published => publish_new(&archive, &expected).await?,
                _ => return Err(conflict(&archive)),
            }
            if read_optional(&archive).await?.as_deref() != Some(&expected) {
                return Err(conflict(&archive));
            }
            sync_note_directories(&archive, &self.memory_dir).await?;
        }

        if !already_published {
            // Recheck the preimage immediately before a replacement. New paths
            // use atomic create, so an intervening writer cannot be clobbered.
            if effect.expected_before.is_some() {
                if read_optional(&file).await?.as_deref().map(sha2_hash) != effect.expected_before {
                    return Err(conflict(&file));
                }
                atomic_write_file(&file, &effect.markdown).await?;
            } else {
                publish_new(&file, &effect.markdown).await?;
            }
        }
        sync_note_directories(&file, &self.memory_dir).await?;
        if index
            .as_ref()
            .is_none_or(|entry| entry.content_hash != after)
        {
            self.store
                .index_note(&note, agent_id, &destination.category)
                .await?;
        }
        if read_optional(&file).await?.as_deref() != Some(&effect.markdown)
            || !self
                .store
                .get_note_index(&destination.key(), agent_id)
                .await?
                .is_some_and(|entry| entry.content_hash == after)
        {
            return Err(conflict(&file));
        }

        if let Some(replacement) = &effect.supersession {
            if replacement.original != *destination {
                self.retire_original(agent_id, &root, replacement).await?;
            }
        }

        // Optional links and vectors are not part of the verified receipt.
        if !already_published || index.is_none_or(|entry| entry.content_hash != after) {
            self.finalize_side_effects(
                agent_id,
                &destination.category,
                &destination.name,
                &effect.markdown,
                &note.aliases,
                effect.reembed,
            )
            .await;
        }
        Ok(())
    }

    async fn retire_original(
        &self,
        agent_id: &str,
        root: &Path,
        replacement: &Supersession,
    ) -> Result<(), AlephError> {
        let original = &replacement.original;
        let file = original.file(root);
        let key = original.key();
        let current = read_optional(&file).await?;
        if current
            .as_ref()
            .is_some_and(|bytes| bytes != &replacement.original_markdown)
        {
            return Err(conflict(&file));
        }
        if let Some(index) = self.store.get_note_index(&key, agent_id).await? {
            if index.content_hash != sha2_hash(&replacement.original_markdown) {
                return Err(conflict(&file));
            }
            // Keep the file present if SQLite rejects retirement. The successor
            // is already independently reachable, so a retry can finish either leg.
            self.store.remove_note_index(&key, agent_id).await?;
            if self.store.get_note_index(&key, agent_id).await?.is_some() {
                return Err(conflict(&file));
            }
        }
        if current.is_some() {
            if read_optional(&file).await?.as_deref() != Some(&replacement.original_markdown) {
                return Err(conflict(&file));
            }
            fs::remove_file(&file).await.map_err(|error| {
                AlephError::other(format!(
                    "retire distill original {}: {error}",
                    file.display()
                ))
            })?;
        }
        sync_note_directories(&file, &self.memory_dir).await
    }
}

#[cfg(test)]
#[path = "distill_effect_tests.rs"]
mod tests;
