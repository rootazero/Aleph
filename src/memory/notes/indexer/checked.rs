//! Exact-address mutations. Every precondition and write shares the corpus guard.
use super::*;
use crate::memory::notes::links::{resolve, LinkResolveContext};
use crate::memory::notes::wikilink::rewrite_resolved_targets;

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
    Partial { address: String, source: AlephError },
    Failed(AlephError),
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
                    let resolved_targets: Vec<String> = self
                        .store
                        .get_outgoing_link_rows(&entry.path, agent_id)
                        .await
                        .map_err(CheckedNoteError::Failed)?
                        .into_iter()
                        .filter(|row| row.status == "active" && row.to_note == node_id)
                        .map(|row| row.to_raw)
                        .collect();
                    candidates.push((entry.path.clone(), filename.to_string(), note.aliases));
                    sources.push((entry.path, bytes, resolved_targets));
                }
                // The selected disk note can be awaiting index repair.
                if !sources.iter().any(|(address, _, _)| address == node_id) {
                    let note = KnowledgeNote::from_markdown(title, &content)
                        .map_err(CheckedNoteError::Invalid)?;
                    candidates.push((node_id.to_string(), title.to_string(), note.aliases));
                    sources.push((node_id.to_string(), content, Vec::new()));
                }
                let context = LinkResolveContext::new(candidates);
                let mut rewrites: Vec<_> = sources
                    .into_iter()
                    .map(|(address, bytes, resolved_targets)| {
                        let rewritten = rewrite_resolved_targets(&bytes, |target| {
                            // Link rows coalesce multiple raw spellings of one
                            // target. Exact paths/filenames are independently
                            // provable; aliases require a persisted resolution.
                            ((target == node_id
                                || target == title
                                || resolved_targets.iter().any(|raw| raw == target))
                                && resolve(target, &context).target.as_deref() == Some(node_id))
                            .then(|| destination.clone())
                        })?;
                        Ok((address, bytes, rewritten))
                    })
                    .collect::<Result<Vec<_>, AlephError>>()
                    .map_err(CheckedNoteError::Invalid)?;
                // Establish the renamed note before projecting references to it.
                rewrites.sort_by_key(|(address, _, _)| address != node_id);
                fs::rename(&path, &new_path)
                    .await
                    .map_err(|error| CheckedNoteError::Failed(io_error(error)))?;
                if let Err(source) = self.store.remove_note_index(node_id, agent_id).await {
                    if let Err(repair_error) =
                        self.index_file_locked(agent_id, category, &new_path).await
                    {
                        tracing::warn!(%destination, %repair_error, "checked rename incremental repair failed");
                    }
                    return Err(CheckedNoteError::Partial {
                        address: destination,
                        source,
                    });
                }
                for (address, original, rewritten) in rewrites {
                    let actual = if address == node_id {
                        &destination
                    } else {
                        &address
                    };
                    if original == rewritten && address != node_id {
                        continue;
                    }
                    let actual_path = self.memory_dir.join(agent_id).join(format!("{actual}.md"));
                    let write_result = if original != rewritten {
                        atomic_write_file(&actual_path, &rewritten).await
                    } else {
                        Ok(())
                    };
                    if let Err(source) = write_result {
                        // A renamed file can already exist at its new path
                        // while its old projection has been removed.
                        let (actual_category, _) = exact_address(actual)?;
                        if let Err(repair_error) = self
                            .index_file_locked(agent_id, actual_category, &actual_path)
                            .await
                        {
                            tracing::warn!(%actual, %repair_error, "checked rename write repair failed");
                        }
                        return Err(CheckedNoteError::Partial {
                            address: actual.to_string(),
                            source,
                        });
                    }
                    self.checked_project(agent_id, actual, &rewritten).await?;
                }
                self.store
                    .relink_unresolved(agent_id)
                    .await
                    .map_err(|source| CheckedNoteError::Partial {
                        address: destination.clone(),
                        source,
                    })?;
                Ok(destination)
            }
        }
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
                source,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
