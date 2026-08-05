use super::MemoryContextProvider;
use crate::memory::curated::{CuratedMemoryStore, CuratedSnapshot};
use crate::sync_primitives::Arc;

impl MemoryContextProvider {
    /// Build the cached `CuratedMemory` + `UserProfile` envelope for
    /// `base_agent_id`'s current-session scope. `base_agent_id` is the BASE
    /// (registered) agent id — this method resolves the effective storage id
    /// itself via `resolve_storage_id` (spec §5.2), so a personal-scoped
    /// session's envelope is composed from ITS OWN MEMORY.md/USER.md/
    /// OPEN_LOOPS.md, not the shared base one. The first call captures a
    /// frozen snapshot, keyed by the RESOLVED id (session scope is immutable
    /// for the session's lifetime, so this stays session-constant — §2.18);
    /// subsequent calls in the same session reuse it. Returns `Ok(None)`
    /// only when both blocks are empty (so the layer can skip emitting an
    /// empty user message).
    pub async fn build_curated_message(
        &self,
        base_agent_id: &str,
        session_key: &str,
    ) -> Result<Option<crate::providers::message::UnifiedMessage>, crate::error::AlephError> {
        let resolved_id = self.resolve_storage_id(base_agent_id);
        let key = (resolved_id, session_key.to_string());
        if let Some(entry) = self.curated_snapshots.read().await.get(&key) {
            return Ok(self.snapshot_to_message(&entry.value));
        }
        let snap = Arc::new(self.capture_curated(base_agent_id).await?);
        // Single write-guard get-or-insert: a concurrent first build that lost
        // the race gets the winner's snapshot back, so both render identical
        // bytes instead of one silently replacing the other's frozen envelope.
        let frozen = super::freeze_into(&mut *self.curated_snapshots.write().await, key, snap);
        Ok(self.snapshot_to_message(&frozen))
    }

    /// Get or lazily load the `CuratedMemoryStore` for `base_agent_id`'s
    /// current-session scope. `base_agent_id` is the BASE (registered) agent
    /// id — resolved to the effective storage id via `resolve_storage_id`
    /// before touching the cache/disk, so a personal-scoped caller (the
    /// `remember` tool, `capture_curated`) always lands on its own instance.
    /// Public so the builtin `remember` tool can resolve the same store
    /// instance the `CuratedMemoryLayer` renders into the system prompt.
    pub async fn get_or_load_curated_store(
        &self,
        base_agent_id: &str,
    ) -> Result<Arc<CuratedMemoryStore>, crate::error::AlephError> {
        let agent_id = self.resolve_storage_id(base_agent_id);
        if let Some(s) = self.curated_stores.get(&agent_id) {
            return Ok(s.clone());
        }
        self.adopt_owner_curated_file(&agent_id, "MEMORY.md").await;
        let path = self.agent_memory_path(&agent_id);
        let s = Arc::new(
            CuratedMemoryStore::load(
                path,
                self.curated_config.memory_char_limit,
                agent_id.clone(),
            )
            .await?,
        );
        self.curated_stores.insert(agent_id, s.clone());
        Ok(s)
    }

    /// Load (or reuse) the `CuratedMemoryStore` for `base_agent_id`'s
    /// current-session scope, render the `CuratedMemory` and `UserProfile`
    /// blocks, and return them as a frozen `CuratedSnapshot`. `base_agent_id`
    /// is the BASE agent id; resolved once here via `resolve_storage_id` and
    /// reused for MEMORY.md + USER.md. OPEN_LOOPS.md resolves SEPARATELY,
    /// pinned to match its writer's more limited scoping — see the comment
    /// at that sub-fetch below for why.
    async fn capture_curated(
        &self,
        base_agent_id: &str,
    ) -> Result<CuratedSnapshot, crate::error::AlephError> {
        use crate::memory::curated::snapshot::{render_agent_block, render_user_block};

        let agent_id = self.resolve_storage_id(base_agent_id);
        // `get_or_load_curated_store` re-resolves from `base_agent_id` — same
        // ambient scope, same result — rather than accepting `agent_id`
        // directly, so its contract stays "always takes a base id" for its
        // other caller (the `remember` tool).
        let store = self.get_or_load_curated_store(base_agent_id).await?;

        let entries = store.current_entries();
        let agent_block = render_agent_block(
            &entries,
            self.curated_config.memory_char_limit,
            self.curated_config.legacy_warn_threshold,
        );

        let user_block = if let Some(ps) = &self.profile {
            match ps.current(&agent_id).await? {
                Some(p) => {
                    let body = super::helpers::strip_frontmatter(&p.raw);
                    let block = render_user_block(
                        body,
                        self.curated_config.user_char_limit,
                        self.curated_config.legacy_warn_threshold,
                    );
                    if block.is_empty() {
                        None
                    } else {
                        Some(block)
                    }
                }
                None => None,
            }
        } else {
            None
        };

        // Open loops from the previous session (Batch 2 open-loop tracking).
        // Opt-in via `set_open_loop_inject`. `SessionReflector` writes
        // `OPEN_LOOPS.md` beside MEMORY.md on session end.
        //
        // Review fix round 2: this read is DELIBERATELY NOT `agent_id` (the
        // fully project+personal-scoped id used above for MEMORY.md/USER.md).
        // The writer, `session_reflection::open_loops_path`, resolves via
        // `session_write_id(agent_id, false, None)` — it can only ever
        // pin personal scope, because `close_session` (where it fires) has
        // no live `current_project_root()` task-local and no persisted
        // project-root column to derive one from, unlike `owner_user_id`/
        // `scope_id` which ARE persisted on the session row. If this read
        // used the fully-scoped `agent_id` instead, a project-scoped
        // (non-personal) session would read `…__proj-<hash>/OPEN_LOOPS.md`
        // while the writer keeps writing the bare `…/OPEN_LOOPS.md` — a
        // silent, permanent split (the block would always render empty for
        // every project-scoped session). Pinning this read to the SAME
        // resolution the writer uses keeps read == write for every scope
        // kind: bare for org/legacy, `u-`-suffixed for personal, and —
        // deliberately, for now — bare (not `proj-`-suffixed) for
        // project-scoped too. MEMORY.md/USER.md are NOT pinned: their
        // writers (`note_manage`, `post_turn_compress`, etc.) DO thread a
        // real `project_scoped`, so `agent_id` above is the right resolution
        // for them. Un-pin this once the write side can persist/resolve a
        // project root at session-close time — until then, do NOT
        // "helpfully" widen just the read side again.
        let open_loops_id =
            crate::memory::project_scope::session_write_id(base_agent_id, false, None);
        const OPEN_LOOPS_CHAR_LIMIT: usize = 2000;
        let open_loops_block = if super::helpers::open_loop_inject() {
            self.adopt_owner_curated_file(&open_loops_id, "OPEN_LOOPS.md")
                .await;
            let path = self
                .agent_memory_path(&open_loops_id)
                .with_file_name("OPEN_LOOPS.md");
            match tokio::fs::read_to_string(&path).await {
                Ok(body) => {
                    let block = crate::memory::curated::snapshot::render_open_loops_block(
                        &body,
                        OPEN_LOOPS_CHAR_LIMIT,
                    );
                    (!block.is_empty()).then_some(block)
                }
                Err(_) => None,
            }
        } else {
            None
        };

        Ok(CuratedSnapshot {
            agent_md_block: agent_block,
            user_md_block: user_block,
            open_loops_block,
        })
    }

    /// Combine the two block strings into a single user-message. Returns
    /// `None` when both blocks are empty (no need to emit an empty turn).
    fn snapshot_to_message(
        &self,
        snap: &CuratedSnapshot,
    ) -> Option<crate::providers::message::UnifiedMessage> {
        let mut combined = String::new();
        if !snap.agent_md_block.is_empty() {
            combined.push_str(&snap.agent_md_block);
        }
        if let Some(ub) = &snap.user_md_block {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(ub);
        }
        if let Some(ol) = &snap.open_loops_block {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(ol);
        }
        if combined.is_empty() {
            return None;
        }
        Some(crate::providers::message::UnifiedMessage::user(combined))
    }

    /// Evict every snapshot whose `session_key` matches. Called on
    /// compression-complete and `SessionEnd` so the next prompt build picks
    /// up disk mutations. Evicts the frozen orientation envelope too — it
    /// rides the same Stable curated zone and must refresh at the same points.
    pub async fn invalidate_curated(&self, session_key: &str) {
        let agents: Vec<String> = {
            let mut snaps = self.curated_snapshots.write().await;
            // Collect the owning agents BEFORE the retain: the store eviction
            // below must be scoped to the agents this session actually
            // rendered, not to every agent the process has ever loaded.
            let agents = snaps
                .keys()
                .filter(|(_, sk)| sk == session_key)
                .map(|(aid, _)| aid.clone())
                .collect();
            snaps.retain(|(_, sk), _| sk != session_key);
            agents
        };
        self.orientation_snapshots
            .write()
            .await
            .retain(|(_, sk), _| sk != session_key);
        // Dropping the snapshot alone re-renders the SAME entries: the per-agent
        // `CuratedMemoryStore` holds the body it was loaded from, and
        // `capture_curated` reads `store.current_entries()` off that in-memory
        // state (only the write path ever re-reads disk). So a hand-edited
        // `~/.aleph/agents/<id>/MEMORY.md` — a supported legacy input — would
        // stay invisible until a compression run or a daemon restart, which is
        // exactly what this method's contract promises it does not. Mirrors
        // `invalidate_curated_for_agent`, which has always done this.
        for agent_id in agents {
            self.curated_stores.remove(&agent_id);
        }
    }

    /// Drop every cached curated snapshot for `agent_id` across every
    /// `session_key`. Spec A Task 18: fired after compression-run completes
    /// for the agent, since compression mutates `MEMORY.md` / `USER.md` on
    /// disk and any per-session cache must rebuild on the next prompt.
    ///
    /// Unlike `build_curated_message`/`get_or_load_curated_store`, `agent_id`
    /// here is the RESOLVED storage id, not a base id — this is an eviction
    /// keyed on whatever the caches actually hold. `PostCompressionHook`'s
    /// caller compresses under the resolved id (`SessionCompactor` writes via
    /// `session_write_id`), so it already has the right value on hand.
    pub async fn invalidate_curated_for_agent(&self, agent_id: &str) {
        self.curated_snapshots
            .write()
            .await
            .retain(|(aid, _), _| aid != agent_id);
        self.orientation_snapshots
            .write()
            .await
            .retain(|(aid, _), _| aid != agent_id);
        // The per-agent CuratedMemoryStore caches the last-loaded body in
        // memory; drop it too so the next `get_or_load_curated_store` call
        // re-reads MEMORY.md from disk.
        self.curated_stores.remove(agent_id);
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryContextProvider;
    use crate::config::types::memory::MemoryInjectionMode;
    use crate::memory::curated::CuratedConfig;
    use crate::sync_primitives::Arc;

    fn provider_rooted_at(root: &std::path::Path) -> MemoryContextProvider {
        MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
            .with_curated_config(CuratedConfig {
                memory_char_limit: 200,
                user_char_limit: 200,
                legacy_warn_threshold: 0.95,
            })
            .with_curated_root_for_test(root.to_path_buf())
    }

    #[tokio::test]
    async fn session_end_invalidation_picks_up_a_hand_edited_memory_md() {
        // The scenario the doc-comment promises: a user hand-edits
        // MEMORY.md between sessions. Evicting only the snapshot re-renders
        // `store.current_entries()` — the body loaded once, at first capture —
        // so the edit stayed invisible until a compression run or a restart.
        let dir = tempfile::tempdir().unwrap();
        let provider = provider_rooted_at(dir.path());

        let agent_dir = dir.path().join("agent-x");
        tokio::fs::create_dir_all(&agent_dir).await.unwrap();
        tokio::fs::write(agent_dir.join("MEMORY.md"), "fact one\n§\n")
            .await
            .unwrap();

        let first = provider
            .build_curated_message("agent-x", "ses-1")
            .await
            .unwrap();
        assert!(format!("{first:?}").contains("fact one"));

        // Hand edit between sessions.
        tokio::fs::write(agent_dir.join("MEMORY.md"), "fact one\n§\nhand edited\n§\n")
            .await
            .unwrap();

        // Session end only — no manual store surgery, which is the whole point.
        provider.invalidate_curated("ses-1").await;

        let second = provider
            .build_curated_message("agent-x", "ses-2")
            .await
            .unwrap();
        assert!(
            format!("{second:?}").contains("hand edited"),
            "session-end eviction must drop the per-agent store too: {second:?}"
        );
    }

    #[tokio::test]
    async fn session_end_invalidation_leaves_other_agents_stores_alone() {
        // Scoping guard: evicting one session must not force every other agent
        // in the process to re-read MEMORY.md from disk.
        let dir = tempfile::tempdir().unwrap();
        let provider = provider_rooted_at(dir.path());

        for agent in ["agent-A", "agent-B"] {
            let d = dir.path().join(agent);
            tokio::fs::create_dir_all(&d).await.unwrap();
            tokio::fs::write(d.join("MEMORY.md"), format!("{agent} original\n§\n"))
                .await
                .unwrap();
        }
        provider
            .build_curated_message("agent-A", "ses-1")
            .await
            .unwrap();
        provider
            .build_curated_message("agent-B", "ses-2")
            .await
            .unwrap();

        // Mutate B's file, then end A's session.
        tokio::fs::write(
            dir.path().join("agent-B").join("MEMORY.md"),
            "agent-B rewritten\n§\n",
        )
        .await
        .unwrap();
        provider.invalidate_curated("ses-1").await;

        // B's store survives, so a fresh session for B still renders the body
        // it was loaded with (B's own eviction points are what refresh it).
        let b = provider
            .build_curated_message("agent-B", "ses-3")
            .await
            .unwrap();
        assert!(
            format!("{b:?}").contains("agent-B original"),
            "agent-B's store must not be evicted by agent-A's session end: {b:?}"
        );
    }

    #[test]
    fn freeze_into_bounds_the_map() {
        // Sessions that never close (a channel DM peer) used to pin an entry
        // for the process lifetime; the map had no ceiling at all.
        use super::super::{freeze_into, MAX_FROZEN_SNAPSHOTS};
        use std::collections::HashMap;

        let mut map = HashMap::new();
        for i in 0..MAX_FROZEN_SNAPSHOTS + 10 {
            freeze_into(&mut map, ("a".to_string(), format!("ses-{i}")), i);
        }
        assert!(
            map.len() <= MAX_FROZEN_SNAPSHOTS,
            "cache must stay bounded, got {}",
            map.len()
        );
    }

    #[test]
    fn freeze_into_keeps_the_first_write() {
        // "Frozen for the session" is what keeps the provider prompt-cache
        // prefix byte-stable: a capture that lost the race must read the
        // winner's value back, not replace it.
        use super::super::freeze_into;
        use std::collections::HashMap;

        let mut map = HashMap::new();
        let key = ("a".to_string(), "ses-1".to_string());
        assert_eq!(freeze_into(&mut map, key.clone(), 1), 1);
        assert_eq!(
            freeze_into(&mut map, key, 2),
            1,
            "the loser must reuse the frozen value, not overwrite it"
        );
    }

    /// Review fix (Finding 1): `build_curated_message` takes a BASE agent id
    /// and must resolve it through the ACTIVE session scope itself — not
    /// trust a pre-composed id from the caller. Proves the composed id
    /// actually reaches the curated store end-to-end under a scoped run: the
    /// base and the personal-scope directories hold DIFFERENT content, and a
    /// personal-scoped call must read the personal one.
    #[tokio::test]
    async fn personal_scope_reaches_the_curated_store_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("main");
        tokio::fs::create_dir_all(&base).await.unwrap();
        tokio::fs::write(base.join("MEMORY.md"), "base-only fact\n§\n")
            .await
            .unwrap();
        let scoped = dir.path().join("main__u-alice");
        tokio::fs::create_dir_all(&scoped).await.unwrap();
        tokio::fs::write(scoped.join("MEMORY.md"), "alice-only fact\n§\n")
            .await
            .unwrap();

        let provider = provider_rooted_at(dir.path());

        // Passing the BASE id "main", exactly as `runner_impl.rs` does
        // (`&spec.agent`) — the composition must come from the ambient scope.
        let msg = crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("u-alice")),
            provider.build_curated_message("main", "ses-1"),
        )
        .await
        .unwrap();
        let rendered = format!("{msg:?}");
        assert!(
            rendered.contains("alice-only fact"),
            "personal scope must reach its own MEMORY.md: {rendered}"
        );
        assert!(
            !rendered.contains("base-only fact"),
            "personal scope must NOT read the shared base MEMORY.md: {rendered}"
        );

        // No active scope at all: must fall back to the base id unchanged
        // (byte-identical pre-P1 behaviour for org/legacy sessions).
        let base_msg = provider
            .build_curated_message("main", "ses-2")
            .await
            .unwrap();
        let base_rendered = format!("{base_msg:?}");
        assert!(
            base_rendered.contains("base-only fact"),
            "no scope → base id: {base_rendered}"
        );
    }

    /// Review fix round 2: a project-scoped (non-personal) session's
    /// OPEN_LOOPS.md read must match the writer's resolution (bare — see the
    /// pin comment on `capture_curated`'s OPEN_LOOPS sub-fetch), even though
    /// MEMORY.md/USER.md DO fully resolve project-scoped. Without the pin,
    /// this read would look under `main__proj-<hash>/OPEN_LOOPS.md` while
    /// `SessionReflector` keeps writing `main/OPEN_LOOPS.md` — permanently
    /// empty for every project-scoped session.
    #[tokio::test]
    async fn project_scoped_open_loops_read_matches_the_bare_writer_while_memory_stays_scoped() {
        crate::thinker::memory_context_provider::set_open_loop_inject(true);

        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path().join("myproject");
        tokio::fs::create_dir_all(&project_root).await.unwrap();
        let ns = crate::memory::project_scope::project_namespace(Some(&project_root));
        let scoped_id = format!("main__{ns}");

        let base = dir.path().join("main");
        tokio::fs::create_dir_all(&base).await.unwrap();
        tokio::fs::write(base.join("MEMORY.md"), "base fact\n§\n")
            .await
            .unwrap();
        // Mirrors the writer exactly: OPEN_LOOPS.md always lands under the
        // BARE dir, even for a project-scoped session (write side pins to
        // `session_write_id(base, false, None)`).
        tokio::fs::write(
            base.join("OPEN_LOOPS.md"),
            "<!-- captured 2026-01-01 -->\n- bare open loop\n",
        )
        .await
        .unwrap();

        let scoped = dir.path().join(&scoped_id);
        tokio::fs::create_dir_all(&scoped).await.unwrap();
        tokio::fs::write(scoped.join("MEMORY.md"), "project-scoped fact\n§\n")
            .await
            .unwrap();
        // A stray OPEN_LOOPS.md under the proj- dir must NEVER be read —
        // proves the pin, not just an accident of the file being absent.
        tokio::fs::write(
            scoped.join("OPEN_LOOPS.md"),
            "<!-- captured 2026-01-01 -->\n- must never be read\n",
        )
        .await
        .unwrap();

        let provider = provider_rooted_at(dir.path()).with_project_scoped(true);

        let msg = crate::projects::with_project_root(
            Some(project_root),
            provider.build_curated_message("main", "ses-1"),
        )
        .await
        .unwrap();
        let rendered = format!("{msg:?}");

        assert!(
            rendered.contains("project-scoped fact"),
            "MEMORY.md must still resolve project-scoped: {rendered}"
        );
        assert!(!rendered.contains("base fact"), "{rendered}");
        assert!(
            rendered.contains("bare open loop"),
            "OPEN_LOOPS.md read must match the bare writer for a project-scoped session: {rendered}"
        );
        assert!(
            !rendered.contains("must never be read"),
            "OPEN_LOOPS.md read must NOT resolve the proj- dir: {rendered}"
        );
    }

    /// P1 owner adoption: the single-machine owner's pre-existing single-user
    /// MEMORY.md + USER.md must become their personal-scope instance the
    /// first time it's loaded under an ACTIVE owner personal-scope run, and a
    /// second load must be a no-op (idempotent). Both files' real roots
    /// differ in production (MEMORY.md under `agents/`, USER.md under the
    /// note-memory dir), so this test points both at the SAME tempdir root —
    /// each store's own adoption hook only ever cares about its own root.
    #[tokio::test]
    async fn owner_adoption_moves_the_trio_once_and_is_idempotent() {
        use crate::memory::notes::profile::synthesizer::FsProfileSynthesizer;
        use crate::providers::recording_mock::RecordingMockProvider;
        use crate::scope::{with_scope, ScopeAttribution};

        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("main");
        tokio::fs::create_dir_all(&bare).await.unwrap();
        tokio::fs::write(bare.join("MEMORY.md"), "fact one\n§\n")
            .await
            .unwrap();
        tokio::fs::write(bare.join("USER.md"), "owner profile body")
            .await
            .unwrap();

        let profile: Arc<dyn crate::memory::notes::profile::synthesizer::ProfileSynthesizer> =
            Arc::new(FsProfileSynthesizer::new(
                dir.path().to_path_buf(),
                Arc::new(RecordingMockProvider::new(String::new())),
            ));
        let provider = provider_rooted_at(dir.path()).with_profile(profile);
        let owner_scope = || {
            Some(ScopeAttribution::personal(
                crate::gateway::security::store::OWNER_USER_ID,
            ))
        };

        // Passing the BASE id "main" under an active owner personal-scope
        // run — mirrors a real `runner_impl.rs` call for the owner's own
        // session, not a pre-composed id handed in by the test.
        let msg = with_scope(
            owner_scope(),
            provider.build_curated_message("main", "ses-1"),
        )
        .await
        .unwrap();
        let rendered = format!("{msg:?}");
        assert!(rendered.contains("fact one"), "{rendered}");
        assert!(rendered.contains("owner profile body"), "{rendered}");

        // Bare files adopted away; scoped files now hold the content.
        assert!(!bare.join("MEMORY.md").exists());
        assert!(!bare.join("USER.md").exists());
        let scoped = dir.path().join("main__u-owner");
        assert!(scoped.join("MEMORY.md").exists());
        assert!(scoped.join("USER.md").exists());

        // Idempotent: a fresh session (caches evicted) re-loads and finds
        // nothing left to adopt — no error, same content.
        provider.invalidate_curated_for_agent("main__u-owner").await;
        let msg2 = with_scope(
            owner_scope(),
            provider.build_curated_message("main", "ses-2"),
        )
        .await
        .unwrap();
        let rendered2 = format!("{msg2:?}");
        assert!(rendered2.contains("fact one"), "{rendered2}");
        assert!(rendered2.contains("owner profile body"), "{rendered2}");
    }

    /// A non-owner personal scope (e.g. a team member) must get a genuinely
    /// fresh curated instance, never the owner's pre-P1 content — and its
    /// own writes must never touch the owner's bare file.
    #[tokio::test]
    async fn member_scope_gets_a_fresh_instance_not_the_owners() {
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("main");
        tokio::fs::create_dir_all(&bare).await.unwrap();
        tokio::fs::write(bare.join("MEMORY.md"), "owner-only fact\n§\n")
            .await
            .unwrap();

        let provider = provider_rooted_at(dir.path());

        // BASE id "main" under alice's active personal scope.
        let store = crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("u-alice")),
            provider.get_or_load_curated_store("main"),
        )
        .await
        .unwrap();
        assert!(
            store.current_entries().is_empty(),
            "a member must start fresh, not inherit the owner's content"
        );
        assert!(
            bare.join("MEMORY.md").exists(),
            "the owner's pre-P1 file must be untouched by a member's load"
        );

        store.add("alice's own fact").await.unwrap();
        let owner_body = tokio::fs::read_to_string(bare.join("MEMORY.md"))
            .await
            .unwrap();
        assert!(owner_body.contains("owner-only fact"));
        assert!(!owner_body.contains("alice's own fact"));
    }
}
