//! Exact-address mutations. Every precondition and write shares the corpus guard.
use super::*;
use crate::memory::notes::links::{resolve, LinkResolveContext};
use crate::memory::notes::wikilink::rewrite_resolved_targets;
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub(crate) enum CheckedNoteAction {
    Update { content: String },
    Rename { title: String },
    Delete,
}

#[derive(Debug)]
pub(crate) enum CheckedNoteError {
    NotFound,
    Conflict,
    DestinationExists,
    Invalid(AlephError),
    Partial {
        address: String,
        failed_source: Option<String>,
        source: AlephError,
    },
    Failed(AlephError),
}

// Keep one in-flight checked rename per corpus. The plan is published before
// the move and stays available until all disk and index work has completed.
const RENAME_PLAN_FILE: &str = ".checked-rename.json";

#[derive(Deserialize, Serialize)]
struct RenamePlan {
    original: String,
    destination: String,
    revision: String,
    rewrites: Vec<RenameRewrite>,
}

#[derive(Deserialize, Serialize)]
struct RenameRewrite {
    address: String,
    revision: String,
    content: String,
}

impl RenamePlan {
    fn partial(&self, address: &str, error: CheckedNoteError) -> CheckedNoteError {
        let source = match error {
            CheckedNoteError::Partial { source, .. }
            | CheckedNoteError::Invalid(source)
            | CheckedNoteError::Failed(source) => source,
            other => AlephError::other(format!(
                "Checked rename could not resume at {address}: {other:?}"
            )),
        };
        CheckedNoteError::Partial {
            address: self.destination.clone(),
            failed_source: (address != self.destination).then(|| address.to_string()),
            source,
        }
    }
}

async fn checked_file_exists(path: &Path) -> Result<bool, CheckedNoteError> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(CheckedNoteError::Invalid(AlephError::Validation(format!(
            "Expected a regular checked rename file: {}",
            path.display()
        )))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(CheckedNoteError::Failed(io_error(error))),
    }
}

async fn read_checked_file(path: &Path) -> Result<String, CheckedNoteError> {
    fs::read_to_string(path)
        .await
        .map_err(|error| CheckedNoteError::Failed(io_error(error)))
}

pub(crate) fn note_revision(content: &str) -> String {
    sha2_hash(content)
}

fn exact_component(value: &str) -> Result<&str, CheckedNoteError> {
    let safe = sanitize_title(value).map_err(CheckedNoteError::Invalid)?;
    if safe != value {
        return Err(CheckedNoteError::Invalid(AlephError::Validation(format!(
            "Expected an exact note path component: {value:?}"
        ))));
    }
    Ok(value)
}

fn exact_address(address: &str) -> Result<(&str, &str), CheckedNoteError> {
    let (category, title) = address.split_once('/').ok_or_else(|| {
        CheckedNoteError::Invalid(AlephError::Validation(
            "Expected category/title note address".into(),
        ))
    })?;
    Ok((exact_component(category)?, exact_component(title)?))
}

fn io_error(error: std::io::Error) -> AlephError {
    AlephError::other(error.to_string())
}

async fn reject_symlinks(
    root: &Path,
    agent_id: &str,
    address: &str,
) -> Result<(), CheckedNoteError> {
    let (category, _) = exact_address(address)?;
    let corpus = root.join(agent_id);
    let paths = [
        corpus.clone(),
        corpus.join(category),
        corpus.join(format!("{address}.md")),
    ];
    for path in paths {
        let metadata = fs::symlink_metadata(&path).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CheckedNoteError::NotFound
            } else {
                CheckedNoteError::Failed(io_error(error))
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(CheckedNoteError::Invalid(AlephError::Validation(format!(
                "Checked note paths cannot traverse symlinks: {}",
                path.display()
            ))));
        }
    }
    Ok(())
}

impl<S: NoteStore + Send + Sync + 'static> NoteIndexer<S> {
    pub(crate) async fn mutate_note_checked(
        &self,
        agent_id: &str,
        node_id: &str,
        revision: &str,
        action: CheckedNoteAction,
    ) -> Result<String, CheckedNoteError> {
        let _guard = lock_note_corpus(&self.memory_dir, agent_id).await;
        exact_component(agent_id)?;
        let (category, title) = exact_address(node_id)?;
        if let CheckedNoteAction::Rename { title: new_title } = &action {
            exact_component(new_title)?;
            let corpus = self.memory_dir.join(agent_id);
            if let Ok(metadata) = fs::symlink_metadata(&corpus).await {
                if metadata.file_type().is_symlink() {
                    return Err(CheckedNoteError::Invalid(AlephError::Validation(
                        "Checked rename cannot traverse a corpus symlink".into(),
                    )));
                }
            }
            let plan_path = corpus.join(RENAME_PLAN_FILE);
            if checked_file_exists(&plan_path).await? {
                let plan: RenamePlan = serde_json::from_str(&read_checked_file(&plan_path).await?)
                    .map_err(|error| {
                        CheckedNoteError::Failed(AlephError::other(error.to_string()))
                    })?;
                if plan.original != node_id
                    || plan.destination != format!("{category}/{new_title}")
                    || plan.revision != revision
                {
                    return Err(CheckedNoteError::Conflict);
                }
                return self.apply_rename_plan(agent_id, &plan_path, &plan).await;
            }
        }
        reject_symlinks(&self.memory_dir, agent_id, node_id).await?;
        let path = self.memory_dir.join(agent_id).join(format!("{node_id}.md"));
        let content = fs::read_to_string(&path).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CheckedNoteError::NotFound
            } else {
                CheckedNoteError::Failed(io_error(error))
            }
        })?;
        if note_revision(&content) != revision {
            return Err(CheckedNoteError::Conflict);
        }
        match action {
            CheckedNoteAction::Update { content } => {
                // Parsing must succeed before changing the source of truth.
                KnowledgeNote::from_markdown(title, &content).map_err(CheckedNoteError::Invalid)?;
                atomic_write_file(&path, &content)
                    .await
                    .map_err(CheckedNoteError::Failed)?;
                self.checked_project(agent_id, node_id, &content).await?;
                Ok(node_id.to_string())
            }
            CheckedNoteAction::Delete => {
                fs::remove_file(&path)
                    .await
                    .map_err(|error| CheckedNoteError::Failed(io_error(error)))?;
                // Preserve the store's inbound tombstone semantics.
                if let Err(source) = self.store.remove_note_index(node_id, agent_id).await {
                    if let Err(repair_error) = self.store.remove_note_index(node_id, agent_id).await
                    {
                        tracing::warn!(%node_id, %repair_error, "checked delete projection repair failed");
                    }
                    return Err(CheckedNoteError::Partial {
                        address: node_id.to_string(),
                        failed_source: None,
                        source,
                    });
                }
                Ok(node_id.to_string())
            }
            CheckedNoteAction::Rename { title: new_title } => {
                exact_component(&new_title)?;
                let destination = format!("{category}/{new_title}");
                if destination == node_id {
                    return Ok(destination);
                }
                let new_path = self
                    .memory_dir
                    .join(agent_id)
                    .join(format!("{destination}.md"));
                match fs::symlink_metadata(&new_path).await {
                    Ok(_) => return Err(CheckedNoteError::DestinationExists),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(CheckedNoteError::Failed(io_error(error))),
                }
                if self
                    .store
                    .get_note_index(&destination, agent_id)
                    .await
                    .map_err(CheckedNoteError::Failed)?
                    .is_some()
                {
                    return Err(CheckedNoteError::DestinationExists);
                }
                // Snapshot aliases and source bytes before any rename. The same
                // resolver as indexing decides whether a target is unambiguous.
                let entries = self
                    .store
                    .list_notes(agent_id)
                    .await
                    .map_err(CheckedNoteError::Failed)?;
                let mut candidates = Vec::new();
                let mut sources = Vec::new();
                for entry in entries {
                    let (_, filename) = exact_address(&entry.path)?;
                    reject_symlinks(&self.memory_dir, agent_id, &entry.path).await?;
                    let source_path = self
                        .memory_dir
                        .join(agent_id)
                        .join(format!("{}.md", entry.path));
                    let bytes = fs::read_to_string(&source_path)
                        .await
                        .map_err(|error| CheckedNoteError::Failed(io_error(error)))?;
                    let note = KnowledgeNote::from_markdown(filename, &bytes)
                        .map_err(CheckedNoteError::Invalid)?;
                    let active_links: Vec<_> = self
                        .store
                        .get_outgoing_link_rows(&entry.path, agent_id)
                        .await
                        .map_err(CheckedNoteError::Failed)?
                        .into_iter()
                        .filter(|row| row.status == "active")
                        .collect();
                    candidates.push((entry.path.clone(), filename.to_string(), note.aliases));
                    sources.push((entry.path, bytes, active_links));
                }
                // The selected disk note can be awaiting index repair.
                if !sources.iter().any(|(address, _, _)| address == node_id) {
                    let note = KnowledgeNote::from_markdown(title, &content)
                        .map_err(CheckedNoteError::Invalid)?;
                    candidates.push((node_id.to_string(), title.to_string(), note.aliases));
                    sources.push((node_id.to_string(), content, Vec::new()));
                }
                let context = LinkResolveContext::new(candidates);
                let mut rewrites = Vec::new();
                for (address, bytes, active_links) in sources {
                    let rewritten = rewrite_resolved_targets(&bytes, |target| {
                        // A current raw occurrence is required. Prefer its persisted
                        // active address even if the short spelling is ambiguous now.
                        let mut active = active_links.iter().filter(|row| row.to_raw == target);
                        let selected = match active.next() {
                            Some(first) => {
                                first.to_note == node_id && active.all(|row| row.to_note == node_id)
                            }
                            None => {
                                (target == node_id || target == title)
                                    && resolve(target, &context).target.as_deref() == Some(node_id)
                            }
                        };
                        selected.then(|| destination.clone())
                    })
                    .map_err(CheckedNoteError::Invalid)?;
                    if bytes != rewritten || address == node_id {
                        let snapshot_revision = note_revision(&bytes);
                        if address == node_id && snapshot_revision != revision {
                            return Err(CheckedNoteError::Conflict);
                        }
                        rewrites.push(RenameRewrite {
                            address,
                            revision: snapshot_revision,
                            content: rewritten,
                        });
                    }
                }
                // Establish the destination before projecting references to it.
                rewrites.sort_by(|left, right| {
                    (left.address != node_id, &left.address)
                        .cmp(&(right.address != node_id, &right.address))
                });
                let plan = RenamePlan {
                    original: node_id.to_string(),
                    destination,
                    revision: revision.to_string(),
                    rewrites,
                };
                let plan_path = self.memory_dir.join(agent_id).join(RENAME_PLAN_FILE);
                let serialized = serde_json::to_string(&plan).map_err(|error| {
                    CheckedNoteError::Failed(AlephError::other(error.to_string()))
                })?;
                atomic_write_file(&plan_path, &serialized)
                    .await
                    .map_err(CheckedNoteError::Failed)?;
                self.apply_rename_plan(agent_id, &plan_path, &plan).await
            }
        }
    }

    async fn apply_rename_plan(
        &self,
        agent_id: &str,
        plan_path: &Path,
        plan: &RenamePlan,
    ) -> Result<String, CheckedNoteError> {
        let corpus = self.memory_dir.join(agent_id);
        let old_path = corpus.join(format!("{}.md", plan.original));
        let new_path = corpus.join(format!("{}.md", plan.destination));
        let Some((selected, backlinks)) = plan.rewrites.split_first() else {
            return Err(CheckedNoteError::Invalid(AlephError::Validation(
                "Missing selected note in checked rename plan".into(),
            )));
        };
        if selected.address != plan.original
            || selected.revision != plan.revision
            || backlinks.iter().any(|rewrite| {
                rewrite.address == plan.original || rewrite.address == plan.destination
            })
        {
            return Err(CheckedNoteError::Invalid(AlephError::Validation(
                "Invalid checked rename plan addresses".into(),
            )));
        }
        for rewrite in backlinks {
            exact_address(&rewrite.address)?;
        }
        let old_exists = checked_file_exists(&old_path).await?;
        if old_exists {
            reject_symlinks(&self.memory_dir, agent_id, &plan.original).await?;
            if checked_file_exists(&new_path).await? {
                return Err(CheckedNoteError::DestinationExists);
            }
            if note_revision(&read_checked_file(&old_path).await?) != plan.revision {
                return Err(CheckedNoteError::Conflict);
            }
            fs::rename(&old_path, &new_path)
                .await
                .map_err(|error| CheckedNoteError::Failed(io_error(error)))?;
        }

        self.write_rename_rewrite(agent_id, &plan.destination, selected)
            .await
            .map_err(|error| plan.partial(&plan.destination, error))?;
        self.store
            .remove_note_index(&plan.original, agent_id)
            .await
            .map_err(|error| plan.partial(&plan.destination, CheckedNoteError::Failed(error)))?;
        self.checked_project(agent_id, &plan.destination, &selected.content)
            .await
            .map_err(|error| plan.partial(&plan.destination, error))?;

        let mut failure = None;
        for rewrite in backlinks {
            match self
                .write_rename_rewrite(agent_id, &rewrite.address, rewrite)
                .await
            {
                Ok(()) => self
                    .checked_project(agent_id, &rewrite.address, &rewrite.content)
                    .await
                    .map_err(|error| plan.partial(&rewrite.address, error))?,
                Err(error) => {
                    // A changed source must not block independent backlinks.
                    failure.get_or_insert_with(|| plan.partial(&rewrite.address, error));
                }
            }
        }
        self.store
            .relink_unresolved(agent_id)
            .await
            .map_err(|error| plan.partial(&plan.destination, CheckedNoteError::Failed(error)))?;
        if let Some(error) = failure {
            return Err(error);
        }
        fs::remove_file(plan_path).await.map_err(|error| {
            plan.partial(&plan.destination, CheckedNoteError::Failed(io_error(error)))
        })?;
        Ok(plan.destination.clone())
    }

    async fn write_rename_rewrite(
        &self,
        agent_id: &str,
        address: &str,
        rewrite: &RenameRewrite,
    ) -> Result<(), CheckedNoteError> {
        reject_symlinks(&self.memory_dir, agent_id, address).await?;
        let path = self.memory_dir.join(agent_id).join(format!("{address}.md"));
        if !checked_file_exists(&path).await? {
            return Err(CheckedNoteError::NotFound);
        }
        let current = read_checked_file(&path).await?;
        if current != rewrite.content {
            if note_revision(&current) != rewrite.revision {
                return Err(CheckedNoteError::Conflict);
            }
            atomic_write_file(&path, &rewrite.content)
                .await
                .map_err(CheckedNoteError::Failed)?;
        }
        Ok(())
    }

    /// Force the normal raw-write projection again on failure: a failed store
    /// write may already have advanced its hash, so incremental hash skipping
    /// alone cannot repair the remaining link/FTS work. Keep the failure visible.
    async fn checked_project(
        &self,
        agent_id: &str,
        address: &str,
        content: &str,
    ) -> Result<(), CheckedNoteError> {
        let (category, title) = exact_address(address)?;
        let parsed =
            KnowledgeNote::from_markdown(title, content).map_err(CheckedNoteError::Invalid)?;
        let mut keys = vec![title.to_string(), address.to_string()];
        keys.extend(parsed.aliases);
        let result = self
            .finalize_write(agent_id, category, title, content)
            .await;
        let result = match result {
            Ok(()) => self
                .store
                .backfill_inbound_links(agent_id, &keys)
                .await
                .map(|_| ()),
            Err(error) => Err(error),
        };
        if let Err(source) = result {
            if let Err(repair_error) = self
                .finalize_write(agent_id, category, title, content)
                .await
            {
                tracing::warn!(%address, %repair_error, "checked mutation projection repair failed");
            }
            return Err(CheckedNoteError::Partial {
                address: address.to_string(),
                failed_source: None,
                source,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
