//! One partition, one bounded walk over persisted evidence (not wikilinks).

use super::{MemoryTraceArgs, MemoryTraceTool, TraceKind, TraceResult};
use crate::memory::notes::profile::{parse_user_md, PROFILE_FILENAME};
use crate::memory::notes::store::{note_content_path, strip_md_ext, NoteStore};
use crate::memory::notes::{sanitize_title, KnowledgeNote, ProvenanceOrigin};
use aleph_protocol::memory_trace::{EvidenceItem, FactOrigin, NoteSource, NoteSourceStatus};
use anyhow::Context;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::io::{self, ErrorKind};
use std::path::{Component, Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncReadExt;

const MAX_CANDIDATES: usize = 500;
const MAX_EVIDENCE: usize = 500;
const MAX_NOTES: usize = 64;
const MAX_FACTS: usize = 100;
const MAX_MARKDOWN_BYTES: u64 = 4 * 1024 * 1024;
const EXCERPT_CHARS: usize = 800;
const RAW_BATCH_SIZE: usize = 16;

pub(super) fn valid_partition(partition: &str) -> bool {
    !partition.contains(['\\', ':', '\0'])
        && matches!(Path::new(partition).components().next(), Some(Component::Normal(value)) if value == partition)
}

fn note_parts(path: &str) -> Option<(&str, &str)> {
    let (category, filename) = path.split_once('/')?;
    if sanitize_title(category).ok()?.as_str() != category
        || sanitize_title(filename).ok()?.as_str() != strip_md_ext(filename)
    {
        // Accept the shared filename helper's one .md suffix, not a lossy fixup.
        return None;
    }
    Some((category, filename))
}

struct ScopedNotes {
    root: PathBuf,
    partition: PathBuf,
}

impl ScopedNotes {
    async fn open(root: &Path, agent_id: &str) -> io::Result<Self> {
        let root = fs::canonicalize(root).await?;
        let partition = root.join(agent_id);
        if fs::canonicalize(&partition).await? != partition
            || !fs::metadata(&partition).await?.is_dir()
        {
            return Err(io::Error::other("note corpus is not the named partition"));
        }
        // On case-insensitive filesystems, canonicalize can retain the spelling
        // supplied by the caller. Storage IDs and visibility are byte-exact.
        let mut entries = fs::read_dir(&root).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_name() == agent_id {
                return Ok(Self { root, partition });
            }
        }
        Err(io::Error::new(
            ErrorKind::NotFound,
            "named note partition not found",
        ))
    }

    async fn read(&self, file: &Path) -> io::Result<String> {
        let file = fs::canonicalize(file).await?;
        if !file.starts_with(&self.partition) || !fs::metadata(&file).await?.is_file() {
            return Err(io::Error::other(
                "not a regular file in the named note partition",
            ));
        }
        let mut bytes = Vec::new();
        fs::File::open(&file)
            .await?
            .take(MAX_MARKDOWN_BYTES + 1)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() as u64 > MAX_MARKDOWN_BYTES {
            return Err(io::Error::other(
                "memory Markdown exceeds the trace read limit",
            ));
        }
        String::from_utf8(bytes).map_err(|error| io::Error::new(ErrorKind::InvalidData, error))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SourceKind {
    Untyped,
    Raw,
    Note,
}

#[derive(Clone)]
enum RawRead {
    Missing,
    Unavailable,
    Available {
        content: String,
        session: Option<String>,
    },
}

struct Walk<'a> {
    tool: &'a MemoryTraceTool,
    files: io::Result<ScopedNotes>,
    result: TraceResult,
    remaining: usize,
    evidence_limit: usize,
    raw_cache: HashMap<String, RawRead>,
}

pub(super) async fn trace(
    tool: &MemoryTraceTool,
    args: MemoryTraceArgs,
) -> anyhow::Result<TraceResult> {
    let MemoryTraceArgs {
        target,
        kind,
        max_results,
    } = args;
    if kind == TraceKind::Note && note_parts(&target).is_none() {
        return Ok(TraceResult {
            target,
            ..TraceResult::default()
        });
    }
    let mut walk = Walk {
        tool,
        files: ScopedNotes::open(&tool.note_memory_dir, &tool.agent_id).await,
        result: TraceResult {
            target: target.clone(),
            ..TraceResult::default()
        },
        remaining: MAX_CANDIDATES,
        evidence_limit: max_results.unwrap_or(MAX_EVIDENCE).min(MAX_EVIDENCE),
        raw_cache: HashMap::new(),
    };
    match kind {
        TraceKind::Note => walk.notes(vec![target], true).await?,
        TraceKind::Raw => {
            walk.cache_raws(std::slice::from_ref(&target)).await;
            if !matches!(walk.raw_cache[&target], RawRead::Missing) {
                walk.raw_evidence(&target, None);
            }
            walk.result.notes = walk.citing_notes(&target).await?;
            walk.notes(walk.result.notes.clone(), false).await?;
        }
        TraceKind::ProfileSection => {
            walk.profile(&target).await?;
            walk.notes(walk.result.notes.clone(), false).await?;
        }
        TraceKind::WriteDecision => unreachable!("write decisions do not walk notes"),
    }
    Ok(walk.result)
}

impl Walk<'_> {
    fn bound<T>(&mut self, mut rows: Vec<T>) -> Vec<T> {
        if rows.len() > self.remaining {
            self.result.truncated = true;
            rows.truncate(self.remaining);
        }
        self.remaining -= rows.len();
        rows
    }

    async fn citing_notes(&mut self, id: &str) -> anyhow::Result<Vec<String>> {
        let paths = self
            .tool
            .db
            .notes_citing(&self.tool.agent_id, id, self.remaining + 1)
            .await
            .context("notes_citing")?;
        Ok(self.bound(paths))
    }

    async fn profile(&mut self, section: &str) -> anyhow::Result<()> {
        let profile = match &self.files {
            Ok(files) => files.read(&files.partition.join(PROFILE_FILENAME)).await,
            Err(error) => Err(io::Error::from(error.kind())),
        };
        let text = match profile {
            Ok(text) => text,
            Err(error) => {
                self.result.incomplete = error.kind() != ErrorKind::NotFound;
                return Ok(());
            }
        };
        let mut profile = match parse_user_md(&text) {
            Ok(profile) => profile,
            Err(_) => {
                self.result.incomplete = true;
                return Ok(());
            }
        };
        let Some(mut sessions) = profile.sources.remove(section) else {
            return Ok(());
        };
        sessions.sort_unstable();
        sessions.dedup();
        let mut notes = BTreeSet::new();
        for session in sessions {
            if self.remaining == 0 {
                self.result.truncated = true;
                break;
            }
            self.remaining -= 1;
            let ids = self
                .tool
                .raw_store
                .get_raw_ids_by_session(&self.tool.agent_id, &session, self.remaining + 1)
                .await
                .context("get_raw_ids_by_session")?;
            self.result.incomplete |= ids.is_empty();
            let ids = self.bound(ids);
            self.cache_raws(&ids).await;
            for id in ids {
                self.raw_evidence(&id, None);
                // Do not issue further reverse queries once a sentinel has
                // confirmed that a full candidate budget omitted rows.
                if self.remaining > 0 || !self.result.truncated {
                    notes.extend(self.citing_notes(&id).await?);
                }
            }
        }
        self.result.notes = notes.into_iter().collect();
        Ok(())
    }

    async fn notes(&mut self, roots: Vec<String>, show_root_facts: bool) -> anyhow::Result<()> {
        let mut discovered: HashSet<_> = roots.iter().cloned().collect();
        let mut pending = VecDeque::from(roots);
        let mut status = HashMap::new();
        while let Some(path) = pending.pop_front() {
            if status.len() >= MAX_NOTES {
                self.result.truncated = true;
                break;
            }
            let note = self.load_note(&path).await;
            let mut edges = BTreeSet::new();
            if let Some(note) = note {
                status.insert(path.clone(), NoteSourceStatus::Available);
                if show_root_facts && path == self.result.target {
                    self.result.notes.push(path.clone());
                    self.collect_facts(&note, &path);
                }
                self.follow(&path, note_sources(&note), &mut edges).await;
            } else {
                let indexed = note_parts(&path).is_some()
                    && self
                        .tool
                        .db
                        .get_note_index(&path, &self.tool.agent_id)
                        .await
                        .context("get_note_index")?
                        .is_some();
                if show_root_facts && path == self.result.target {
                    if !indexed {
                        continue; // Unknown root has the same shape as a denied root.
                    }
                    self.result.notes.push(path.clone());
                }
                status.insert(path.clone(), NoteSourceStatus::Unavailable);
                self.result.incomplete = true;
                if indexed {
                    let refs = self
                        .tool
                        .db
                        .sources_of(&self.tool.agent_id, &path, self.remaining + 1)
                        .await
                        .context("sources_of fallback")?;
                    self.follow(
                        &path,
                        refs.iter().map(|id| (id.as_str(), SourceKind::Untyped)),
                        &mut edges,
                    )
                    .await;
                }
            }
            for source_path in edges {
                if discovered.insert(source_path.clone()) {
                    pending.push_back(source_path.clone());
                }
                self.result.note_sources.push(NoteSource {
                    note_path: path.clone(),
                    source_path,
                    status: NoteSourceStatus::NotVisited,
                });
            }
        }
        for edge in &mut self.result.note_sources {
            if let Some(found) = status.get(&edge.source_path) {
                edge.status = *found;
            }
        }
        Ok(())
    }

    async fn load_note(&self, path: &str) -> Option<KnowledgeNote> {
        let (category, filename) = note_parts(path)?;
        let files = self.files.as_ref().ok()?;
        let file = note_content_path(&files.root, &self.tool.agent_id, category, filename);
        let content = files.read(&file).await.ok()?;
        KnowledgeNote::from_markdown(strip_md_ext(filename), &content).ok()
    }

    fn collect_facts(&mut self, note: &KnowledgeNote, path: &str) {
        self.result.truncated |= note.facts.len() > MAX_FACTS;
        for (fact_idx, (text, provenance)) in note.facts_with_origin().take(MAX_FACTS).enumerate() {
            let mut chars = text.chars();
            let text = chars.by_ref().take(EXCERPT_CHARS).collect();
            self.result.truncated |= chars.next().is_some();
            self.result.facts.push(FactOrigin {
                note_path: path.to_string(),
                fact_idx,
                text,
                origin: provenance.origin.as_str().to_owned(),
                inferred: provenance.inferred,
                source_id: provenance.source_id,
            });
        }
    }

    async fn follow<'a>(
        &mut self,
        note: &str,
        sources: impl Iterator<Item = (&'a str, SourceKind)>,
        edges: &mut BTreeSet<String>,
    ) {
        // Keep only the ordered prefix plus one overflow sentinel, even when
        // a single Markdown note has a large number of references.
        let mut prefix = BTreeSet::new();
        for source in sources.filter(|(id, _)| !id.trim().is_empty()) {
            prefix.insert(source);
            if prefix.len() > self.remaining + 1 {
                prefix.pop_last();
            }
        }
        let sources = self.bound(prefix.into_iter().collect());
        let raw_ids: Vec<_> = sources
            .iter()
            .filter(|(_, kind)| *kind != SourceKind::Note)
            .map(|(id, _)| id.to_string())
            .collect();
        self.cache_raws(&raw_ids).await;
        let mut seen_raws = HashSet::new();
        for (id, kind) in sources {
            let is_note = kind == SourceKind::Note
                || (kind == SourceKind::Untyped
                    && matches!(self.raw_cache[id], RawRead::Missing)
                    && note_parts(id).is_some());
            if is_note {
                // Explicit prior-note references with a malformed address are
                // unavailable note edges, not raw IDs or path fixups.
                edges.insert(id.to_string());
            } else if seen_raws.insert(id) {
                self.raw_evidence(id, Some(note));
            }
        }
    }

    async fn cache_raws(&mut self, ids: &[String]) {
        let pending: BTreeSet<_> = ids
            .iter()
            .filter(|id| !self.raw_cache.contains_key(*id))
            .cloned()
            .collect();
        let pending: Vec<_> = pending.into_iter().collect();
        for batch in pending.chunks(RAW_BATCH_SIZE) {
            match self
                .tool
                .raw_store
                .get_raws_by_ids(&self.tool.agent_id, batch)
                .await
            {
                Ok(raws) => {
                    for raw in raws {
                        self.raw_cache.insert(
                            raw.id,
                            RawRead::Available {
                                content: raw.content.chars().take(EXCERPT_CHARS).collect(),
                                session: raw.session_id,
                            },
                        );
                    }
                    for id in batch {
                        self.raw_cache.entry(id.clone()).or_insert(RawRead::Missing);
                    }
                }
                Err(_) => {
                    // A corrupt raw can make the Markdown store reject a
                    // batch. Isolate those rows without losing other evidence.
                    for id in batch {
                        let value = match self
                            .tool
                            .raw_store
                            .get_raws_by_ids(&self.tool.agent_id, std::slice::from_ref(id))
                            .await
                        {
                            Ok(rows) => rows.into_iter().find(|raw| raw.id == *id).map_or(
                                RawRead::Missing,
                                |raw| RawRead::Available {
                                    content: raw.content.chars().take(EXCERPT_CHARS).collect(),
                                    session: raw.session_id,
                                },
                            ),
                            Err(_) => RawRead::Unavailable,
                        };
                        self.raw_cache.insert(id.clone(), value);
                    }
                }
            }
        }
    }

    fn raw_evidence(&mut self, id: &str, via_note: Option<&str>) {
        let raw = &self.raw_cache[id];
        let pruned = !matches!(raw, RawRead::Available { .. });
        self.result.incomplete |= pruned;
        if self.result.evidence.len() == self.evidence_limit {
            self.result.truncated = true;
            return;
        }
        let (content, via_session) = match raw {
            RawRead::Available { content, session } => (Some(content.clone()), session.clone()),
            _ => (None, None),
        };
        self.result.evidence.push(EvidenceItem {
            raw_id: id.to_string(),
            via_note: via_note.map(str::to_owned),
            via_session,
            content,
            pruned,
        });
    }
}

fn note_sources(note: &KnowledgeNote) -> impl Iterator<Item = (&str, SourceKind)> {
    note.source_notes
        .iter()
        .map(|id| (id.as_str(), SourceKind::Untyped))
        .chain(note.fact_provenance.iter().filter_map(|origin| {
            let kind = match origin.origin {
                ProvenanceOrigin::RawSource => SourceKind::Raw,
                ProvenanceOrigin::PriorNote => SourceKind::Note,
                _ => return None,
            };
            Some((origin.source_id.as_deref()?, kind))
        }))
}
