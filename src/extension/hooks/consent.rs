//! Shell-hook consent allowlist.
//!
//! Shell-command hooks (`HookAction::Command`) execute arbitrary code in the
//! agent's environment. Hermes-inspired: before such a hook runs, its command
//! must be explicitly approved by the operator. Un-approved shell hooks are
//! skipped (fail-safe) and recorded as `pending` so the operator can review
//! them via `aleph hooks list` / `aleph hooks test`.
//!
//! Registry file: `~/.aleph/shell-hooks-allowlist.json`. The file's
//! `(mtime, len)` pair is the cache fingerprint — `is_approved` re-reads the
//! registry whenever the file changes on disk, so an approval made via the
//! `aleph hooks` CLI is picked up by a running server without a restart.
//!
//! # What an approval is bound to
//!
//! An entry is keyed by `(owner, project, command)` — see
//! [`ShellHookConsent::fingerprint`]. `project` is the canonical root of the
//! project the hook is bound to: its `ScopeKey::Project`, the same field the
//! executor's fire-time gate (`project_scope_allows`) reads.
//!
//! - A hook that fires everywhere (`ScopeKey::Global`: `~/.aleph/hooks.json`,
//!   a globally installed plugin) has no project and keeps the key it has
//!   always had. Its code lives under its root, and the approval is bound
//!   to that root (below), so the same plugin id installed elsewhere is not
//!   approved by it.
//! - A project hook (a project's `.aleph/hooks{,.local}.json`, a plugin found
//!   under a project) is keyed by its project as well. Every project's files
//!   load under the same `user:project` label, and a byte-identical template
//!   in another repo resolves `${CLAUDE_PLUGIN_ROOT}` to THAT repo's
//!   directory: approving `"${CLAUDE_PLUGIN_ROOT}"/hooks/lint.sh` in repo A
//!   must not run repo B's `lint.sh`. A project-scoped plugin is the same
//!   case — another repo can ship a plugin with the same id.
//!
//! Within its key, an approval also attests to what `aleph hooks test`
//! reviewed: the hook's root directory ([`ConsentEntry::plugin_root`]) and
//! the content of ONE script file — the first word of the command with a
//! script extension, else the first path — when that word is an absolute or
//! `~/` path, a path variable (`${CLAUDE_PLUGIN_ROOT}` and its spellings, a
//! plugin's `_DATA` pair), `$CLAUDE_PROJECT_DIR` in a project-bound hook, or
//! a path relative to the hook's root. Only the named file is hashed — what
//! it sources, imports or reads is not. `$CLAUDE_PROJECT_DIR` is bound only
//! in a session bound to that project: a run with no project of its own
//! still fires the hooks of the project the daemon was started in, and
//! there the child's `$CLAUDE_PROJECT_DIR` names another directory than the
//! one hashed (the run's workspace), or none. Anything else is not
//! content-bound, and the approval is of the command
//! string alone — among it a script reached through `PATH` (`npx`, `uvx`, a
//! bare name) or through any other variable, through command substitution
//! or re-parsing (`$(…)`, backticks, `eval`, `sh -c "…"`), `python3 -m`, a
//! quoted path containing a space (the words split: it binds nothing, or
//! the wrong file), and a script over 1 MiB. So is `$CLAUDE_PROJECT_DIR` in
//! a hook that fires everywhere: every project a session opens supplies its
//! own script to that one approval, and it runs unreviewed. The same key
//! from another root, or an edited bound script, is refused until it is
//! revoked and reviewed again ([`ShellHookConsent::is_approved`]). A pending
//! entry is refreshed by every fire, so what the review runs is what
//! production runs. No approval is ever minted without a root
//! ([`ShellHookConsent::approve`]).
//!
//! **Migration.** An entry recorded before the project binding carries no
//! project. It is kept on disk as it is — never rewritten, never deleted —
//! but a project hook is never looked up under it, so it authorises
//! nothing: each project's hooks come back as `pending` once and are
//! approved again, per project. An unapproved hook does not run, and a
//! skipped interceptor decides nothing — so every approved project GUARD
//! (a `PreToolUse` hook that denies edits to `.env`, blocks `rm -rf` …)
//! stops blocking from the upgrade on: the tool calls it stopped go through
//! until its project's pending entry is approved. `aleph doctor` reports the
//! old approvals from project hook files (`core/hooks-consent`, "Project-hook
//! approvals no longer apply"); an old approval of a plugin installed inside
//! a project cannot be told apart and is not counted — it shows as pending
//! once the hook fires. `aleph hooks list` / `test` mark such a
//! `user:project*` entry ([`ConsentEntry::predates_project_binding`]). An
//! approval of a hook that fires everywhere, recorded before roots were
//! recorded, keeps working from any root until it is revoked, fires once
//! (which records its root) and is approved again — unless the script it
//! runs can now be hashed and it recorded none: that approval attests to no
//! content, so its next fire withdraws it to `pending` for review
//! ([`ConsentEntry::script_fingerprint`]). That second migration reaches
//! every hook written the Claude Code way and approved before its script
//! could be bound (`${CLAUDE_PLUGIN_ROOT}/…`, a relative script, a project
//! hook's `$CLAUDE_PROJECT_DIR`), globally installed plugins included: from
//! its first fire after the upgrade until it is approved again it does not
//! run, so a guard among them stops blocking. The doctor does not count
//! these; the hook shows as pending once it has fired.

use crate::extension::visibility::{canonical_root, ScopeKey};
use crate::sync_primitives::{Arc, RwLock};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

/// On-disk registry schema version.
const REGISTRY_VERSION: u32 = 1;

/// The [`ConsentEntry::event`] of a plugin command's inline shell command
/// (`` !`cmd` `` in `commands/<name>.md`), which is reviewed and approved
/// here like a `hooks.json` command. Written by the gateway's slash-command
/// runner; read by `aleph hooks list` / `test` to tell the two apart.
pub const INLINE_COMMAND_EVENT: &str = "SlashCommand";

/// Cheap change-detection fingerprint for the registry file: `(mtime, len)`.
/// Length is included because some filesystems have coarse mtime resolution —
/// an approval that lands in the same second still changes the file length.
type FileStamp = Option<(SystemTime, u64)>;

/// Consent status of a shell-hook command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsentStatus {
    /// Seen by the server but awaiting operator approval. The hook is NOT run.
    Pending,
    /// Operator-approved. The hook runs normally.
    Approved,
}

/// A single shell-hook consent record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsentEntry {
    /// Stable id: the `sha256` of `(plugin_name, project_root, command)`
    /// ([`ShellHookConsent::fingerprint`]) truncated to 16 hex chars.
    /// Changes when the command text changes — editing a hook revokes consent.
    pub fingerprint: String,
    /// Plugin that registered the hook.
    pub plugin_name: String,
    /// The project this entry is bound to: the canonical root of a hook that
    /// fires only inside one project. Part of the fingerprint, so approving
    /// a template in one project never approves it in another. `None` for a
    /// hook that fires everywhere — and for every entry recorded before
    /// approvals were bound to a project (see the module doc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<PathBuf>,
    /// The exact shell command string.
    pub command: String,
    /// The event name the hook is dispatched with — its registered spelling
    /// (`PreToolUse`, `before_tool_call`), exactly what its payload's
    /// `hook_event_name` carries — so `aleph hooks test` can hand it the same
    /// value. Best-effort: one entry per `(plugin, project, command)`, so a
    /// command bound to two events carries whichever fired last while it is
    /// pending ([`ShellHookConsent::record_pending`] refreshes it), and the
    /// one it was approved with afterwards. An entry recorded before this
    /// field held the spelling carries the enum's Rust name
    /// (`BeforeToolCall`) until it next fires while pending. A plugin
    /// command's inline shell command is not a hook and carries
    /// [`INLINE_COMMAND_EVENT`].
    #[serde(default)]
    pub event: String,
    /// The hook's plugin root, which its path variables
    /// (`${CLAUDE_PLUGIN_ROOT}` …) resolve to, so `aleph hooks test` runs the
    /// command as production does. Once approved, it is the directory the
    /// approval is bound to: the same key from another root is refused
    /// ([`ShellHookConsent::is_approved`]). While pending, the latest fire's.
    /// `None` for entries recorded before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_root: Option<PathBuf>,
    /// Consent status.
    pub status: ConsentStatus,
    /// Unix seconds when the record was first created.
    pub first_seen: u64,
    /// Unix seconds when approved (absent while pending).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<u64>,
    /// `sha256` of the ONE script file the command invokes — the first word
    /// with a script extension, else the first path, resolved as the module
    /// doc lists — captured when the entry was recorded / approved.
    ///
    /// The command STRING alone is a weak thing to consent to: approving
    /// `sh scripts/deploy.sh` once approves whatever that file contains
    /// forever, so an attacker who can write the script (or a `git pull`)
    /// silently inherits the approval. Binding the content narrows that
    /// time-of-check/time-of-use window — [`ShellHookConsent::is_approved`]
    /// re-hashes and refuses on drift — for that one file: only the named
    /// file is hashed, and what it sources, imports or reads is not. A
    /// command whose script cannot be resolved binds nothing: command
    /// substitution, `python3 -m`, a quoted path containing a space, a
    /// script over 1 MiB, anything reached through `PATH` or another
    /// variable (the module doc has the full rule).
    ///
    /// `None` for commands with no resolvable script (`echo hi`), for
    /// unreadable files and ones over 1 MiB, and for entries written before
    /// this field existed.
    /// An approval with `None` keeps the command-string-only semantics only
    /// while the command still names no hashable script; once it does (the
    /// file appears, or it was a `${…}` / relative script approved before
    /// those were resolved), the approval is refused and withdrawn for
    /// re-review ([`ShellHookConsent::is_approved`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_fingerprint: Option<String>,
}

impl ConsentEntry {
    /// The fingerprint this entry's own key derives — what `fingerprint`
    /// must equal unless the registry was edited by hand.
    #[must_use]
    pub fn expected_fingerprint(&self) -> String {
        ShellHookConsent::fingerprint(
            &self.plugin_name,
            self.project_root.as_deref(),
            &self.command,
        )
    }

    /// Whether this is a project-settings entry recorded before approvals
    /// were bound to a project: a `user:project*` owner with no project. It
    /// authorises nothing — no project hook is looked up without its
    /// project. A project-scoped plugin's entry from that time cannot be
    /// told apart from a global plugin's, so it is not marked (it is just as
    /// inert).
    #[must_use]
    pub fn predates_project_binding(&self) -> bool {
        self.project_root.is_none()
            && super::user_settings::PROJECT_LABELS.contains(&self.plugin_name.as_str())
    }

    /// What an approval of an inline command also approves, for the review
    /// surfaces to print; `None` for a hook.
    ///
    /// Every inline command receives the arguments of whoever sends its
    /// `/command` — as `$1 … $N`, `$@` and `$ARGUMENTS` — not only one whose
    /// text spells them (a script it runs can read `$ARGUMENTS` from its
    /// environment). Channel senders can send one. A shell never parses an
    /// argument as code, but a command that passes one to a program hands
    /// the sender that program's options (`git log $1` with `--output=…`).
    #[must_use]
    pub fn invoker_arguments_note(&self) -> Option<&'static str> {
        (self.event == INLINE_COMMAND_EVENT).then_some(
            "inline command of a plugin's slash command: it runs with the arguments of \
             whoever sends that command ($1 … $N, $@, $ARGUMENTS), channel senders \
             included. They arrive as data, never as code, but a program it hands them \
             to takes them as its options.",
        )
    }
}

/// The project a consent key names: the root of a hook bound to one project,
/// `None` for a hook that fires everywhere.
fn bound_project(scope: &ScopeKey) -> Option<&Path> {
    match scope {
        ScopeKey::Global => None,
        ScopeKey::Project(root) => Some(root),
    }
}

/// Whether two spellings name one directory: equal as written, or once
/// canonicalised (`/var` → `/private/var`) the way the scope key is.
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b || canonical_root(a) == canonical_root(b)
}

#[derive(Serialize, Deserialize, Default)]
struct RegistryDoc {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: Vec<ConsentEntry>,
}

struct CacheState {
    entries: BTreeMap<String, ConsentEntry>,
    /// Stamp of the registry file the cache was loaded from.
    stamp: FileStamp,
}

/// Manages the shell-hook consent allowlist on disk.
pub struct ShellHookConsent {
    path: PathBuf,
    cache: RwLock<CacheState>,
}

impl ShellHookConsent {
    /// Compute the stable fingerprint for a `(plugin_name, project_root,
    /// command)` key. `project_root` is the canonical root of a hook bound to
    /// one project, `None` for a hook that fires everywhere.
    ///
    /// Without a project the hashed bytes are `plugin_name \0 command` — the
    /// key every entry had before approvals were bound to a project, so a
    /// global hook's approval survives. With one, a `0xFF` tag and the
    /// length-prefixed root come first: no global key starts with `0xFF` (a
    /// plugin name is UTF-8), so a project key can never equal a global one,
    /// and the prefix keeps one root from running into the name after it.
    #[must_use]
    pub fn fingerprint(plugin_name: &str, project_root: Option<&Path>, command: &str) -> String {
        let mut hasher = Sha256::new();
        if let Some(root) = project_root {
            let root = root.as_os_str().as_encoded_bytes();
            hasher.update([0xFFu8]);
            hasher.update((root.len() as u64).to_le_bytes());
            hasher.update(root);
        }
        hasher.update(plugin_name.as_bytes());
        hasher.update([0u8]);
        hasher.update(command.as_bytes());
        hex16(&hasher.finalize())
    }

    /// The script word of an INLINE command (a plugin command's `` !`cmd` ``,
    /// [`INLINE_COMMAND_EVENT`]) that consent cannot bind to what runs, if any.
    ///
    /// Consent finds a command's script among its candidate words, in its
    /// order, and resolves a relative word against the hook's root — the
    /// directory a hook runs in. An inline command runs in the session's
    /// directory instead, so a relative script word would be reviewed and
    /// hashed as the plugin's copy (or, when the plugin ships none, bound to
    /// nothing) while the session's copy runs. The first candidate that is
    /// relative decides (`Some(word)`); one that is an existing absolute or
    /// `~/` file is what consent binds — correctly — and ends the scan.
    /// `${CLAUDE_PLUGIN_ROOT}/…` is expanded first, so a plugin's own script
    /// stays content-bound and allowed. A path-shaped ARGUMENT to a program
    /// (`git diff src/app.ts`) is refused too: consent cannot tell it from a
    /// script, and binds whichever comes first.
    #[must_use]
    pub fn root_relative_script(
        plugin_name: &str,
        project_root: Option<&Path>,
        plugin_root: Option<&Path>,
        command: &str,
    ) -> Option<String> {
        let context = ScriptContext::new(plugin_name, project_root, plugin_root);
        for token in script_candidates(command, &context) {
            if !token.starts_with("~/") && !Path::new(&token).is_absolute() {
                return Some(token);
            }
            if candidate_path(&token, &context).is_some_and(|path| path.is_file()) {
                return None;
            }
        }
        None
    }

    /// Default registry path: `<config_dir>/shell-hooks-allowlist.json`.
    ///
    /// Resolved through `utils::paths::get_config_dir` like every other piece
    /// of Aleph state, so it follows `ALEPH_HOME`. The former hand-rolled
    /// `dirs::home_dir().join(".aleph")` did not: under a relocated home the
    /// approval registry — and the `hooks/consent` doctor check that reads it
    /// — silently addressed the developer's real `~/.aleph` instead. Identical
    /// bytes when `ALEPH_HOME` is unset.
    #[must_use]
    pub fn default_path() -> PathBuf {
        crate::utils::paths::get_config_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("shell-hooks-allowlist.json")
    }

    /// Process-wide consent instance backed by the default path.
    pub fn shared() -> Arc<Self> {
        static SHARED: OnceLock<Arc<ShellHookConsent>> = OnceLock::new();
        SHARED
            .get_or_init(|| Arc::new(Self::with_path(Self::default_path())))
            .clone()
    }

    /// Construct a consent manager backed by an explicit path (tests).
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (entries, stamp) = Self::read_file(&path);
        Self {
            path,
            cache: RwLock::new(CacheState { entries, stamp }),
        }
    }

    /// Registry file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether a shell hook is approved to run: `command`, owned by
    /// `plugin_name`, from a hook whose visibility key is `scope` — the
    /// hook's own `scope_key`, so a project hook is looked up under its
    /// project and nowhere else (module doc).
    ///
    /// Re-reads the registry first if the file changed on disk, so approvals
    /// made via the `aleph hooks` CLI are honored without a server restart.
    ///
    /// Three conditions, all required:
    ///
    /// 1. an `Approved` entry exists for that key,
    /// 2. if that entry recorded a [`plugin_root`](ConsentEntry::plugin_root),
    ///    it is the directory this hook runs from (`plugin_root`), and
    /// 3. the script the command names — found from this hook's root, see
    ///    `script_path_from_command` — still hashes to the recorded
    ///    [`script_fingerprint`](ConsentEntry::script_fingerprint); and when
    ///    none was recorded, the command still names no script that can be
    ///    hashed.
    ///
    /// (2) binds the approval to the root `aleph hooks test` reviewed: the
    /// same key from another directory — a plugin id installed again
    /// elsewhere — runs other code behind `${CLAUDE_PLUGIN_ROOT}`. (3) is the
    /// TOCTOU guard: `sh scripts/deploy.sh` approved in March must not keep
    /// running after the script is rewritten in April. Either drift fails
    /// SAFE (the hook is skipped, exactly like an un-approved one) and is
    /// logged loudly, because the alternative — running code nobody reviewed
    /// — is the whole thing consent exists to prevent. The way back is
    /// `aleph hooks revoke`: the entry turns pending, the next fire records
    /// what it runs now ([`Self::record_pending`]), and `aleph hooks test`
    /// reviews and approves that.
    ///
    /// An entry with no recorded root predates root recording and keeps the
    /// older, looser meaning for (2). An approval that recorded no script
    /// fingerprint keeps the command-string-only meaning only while there is
    /// still nothing to hash: once its script can be hashed, it is refused,
    /// and withdrawn to `pending` by the fire ([`Self::record_pending`]).
    pub fn is_approved(
        &self,
        plugin_name: &str,
        scope: &ScopeKey,
        plugin_root: &Path,
        command: &str,
    ) -> bool {
        self.reload_if_stale();
        let fp = Self::fingerprint(plugin_name, bound_project(scope), command);
        let (approved_root, recorded) = {
            let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
            match cache.entries.get(&fp) {
                Some(e) if e.status == ConsentStatus::Approved => {
                    (e.plugin_root.clone(), e.script_fingerprint.clone())
                }
                // Missing or still pending — not approved either way.
                _ => return false,
            }
        };

        if let Some(approved_root) = approved_root {
            if !same_dir(&approved_root, plugin_root) {
                warn!(
                    plugin = plugin_name,
                    command,
                    approved_root = %approved_root.display(),
                    root = %plugin_root.display(),
                    "Hook was approved for another directory — refusing to run. \
                     `aleph hooks revoke <fingerprint>`, then review it again with \
                     `aleph hooks test <fingerprint>` after it next fires."
                );
                return false;
            }
        }

        let context = ScriptContext::new(plugin_name, bound_project(scope), Some(plugin_root));
        match (recorded, script_fingerprint(command, &context)) {
            // No script to bind, then or now (`echo hi`): the approval is of
            // the command string alone.
            (None, None) => true,
            (None, Some(_)) => {
                // Approved while its script could not be hashed — before the
                // script was resolvable, or before `${…}` / relative scripts
                // were bound at all — so the approval attests to no content.
                // The fire's `record_pending` withdraws it for re-review.
                warn!(
                    plugin = plugin_name,
                    command,
                    "Hook was approved before its script could be content-bound — refusing \
                     to run. Review it again with `aleph hooks test <fingerprint>`."
                );
                false
            }
            (Some(approved), Some(current)) if approved == current => true,
            (Some(_), Some(_)) => {
                warn!(
                    plugin = plugin_name,
                    command,
                    "Hook script changed since it was approved — refusing to run. \
                     `aleph hooks revoke <fingerprint>`, then review it again with \
                     `aleph hooks test <fingerprint>`."
                );
                false
            }
            (Some(_), None) => {
                // The script hashed cleanly at approval time and cannot be
                // read now (deleted, renamed, permissions). Something moved
                // under us; fail safe rather than assume it's benign.
                warn!(
                    plugin = plugin_name,
                    command, "Approved hook script is no longer readable — refusing to run."
                );
                false
            }
        }
    }

    /// Record an un-approved shell hook as `pending` so the operator can
    /// review it. Best-effort: a write failure is logged, not propagated.
    ///
    /// `scope` is the hook's `scope_key`, keyed exactly as in
    /// [`Self::is_approved`]. `event` is the name the hook is dispatched with
    /// (`HookConfig::event_name`) and `plugin_root` the root its path
    /// variables resolve to — what `aleph hooks test` rebuilds the run from.
    ///
    /// A key already on file is refreshed while it is still `pending`: its
    /// `event` and `plugin_root` become this fire's, so a review runs what
    /// production just ran — not an entry's first-seen root that has since
    /// been deleted or moved, nor a spelling recorded before the spelling
    /// was kept. An `Approved` entry is never rebound: its root and script
    /// are what the approval attests to ([`Self::is_approved`]). The one
    /// thing a fire does to an approved entry is withdraw an approval that
    /// attests to no script content once this fire's script can be hashed
    /// — it turns `pending`, refreshed, for `aleph hooks test` to review —
    /// and only from the root it was approved at (or when it recorded none),
    /// so another install's fire cannot withdraw it.
    pub fn record_pending(
        &self,
        plugin_name: &str,
        scope: &ScopeKey,
        command: &str,
        event: &str,
        plugin_root: &Path,
    ) {
        let project_root = bound_project(scope);
        let fp = Self::fingerprint(plugin_name, project_root, command);
        let script = script_fingerprint(
            command,
            &ScriptContext::new(plugin_name, project_root, Some(plugin_root)),
        );
        // Whether `entry` should be rewritten to this fire; `false` for an
        // approved entry that attests to content and for a pending one that
        // already says what this fire says.
        let stale = |entry: &ConsentEntry| match entry.status {
            ConsentStatus::Pending => {
                entry.event != event || entry.plugin_root.as_deref() != Some(plugin_root)
            }
            ConsentStatus::Approved => {
                script.is_some()
                    && entry.script_fingerprint.is_none()
                    && entry
                        .plugin_root
                        .as_deref()
                        .is_none_or(|approved_at| same_dir(approved_at, plugin_root))
            }
        };
        {
            let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
            if cache.entries.get(&fp).is_some_and(|e| !stale(e)) {
                return;
            }
        }
        let entry = ConsentEntry {
            fingerprint: fp.clone(),
            plugin_name: plugin_name.to_string(),
            project_root: project_root.map(Path::to_path_buf),
            command: command.to_string(),
            event: event.to_string(),
            plugin_root: Some(plugin_root.to_path_buf()),
            status: ConsentStatus::Pending,
            first_seen: now_secs(),
            approved_at: None,
            script_fingerprint: script.clone(),
        };
        if let Err(e) = self.mutate(|entries| match entries.get_mut(&fp) {
            None => {
                entries.insert(fp.clone(), entry);
                true
            }
            Some(known) if stale(known) => {
                known.status = ConsentStatus::Pending;
                known.approved_at = None;
                known.event = event.to_string();
                known.plugin_root = Some(plugin_root.to_path_buf());
                known.script_fingerprint = script.clone();
                true
            }
            Some(_) => false,
        }) {
            warn!(error = %e, "Failed to record pending shell-hook consent");
        }
    }

    /// Approve a hook by fingerprint (or unique prefix). Returns the approved
    /// entry, or `None` when no entry matches the prefix, when the entry's
    /// root is no longer `reviewed_root` — or when there is no root at all.
    ///
    /// `reviewed_root` is the [`plugin_root`](ConsentEntry::plugin_root) the
    /// operator's review ran against (`aleph hooks test` reads it from the
    /// entry). A pending entry follows the hook's latest fire
    /// ([`Self::record_pending`]), so it can move to another root between
    /// that review and this call; approving it anyway would bind the
    /// approval to a directory nobody reviewed. Checked under the registry
    /// lock, with the write.
    ///
    /// No approval is minted without a root: it would bind no directory, and
    /// follow its key to wherever that plugin id is installed next. An entry
    /// without one (recorded before roots were kept) gets its root from its
    /// next fire, and can be approved after that. This is where the line is
    /// drawn for root-less approvals: one that already exists keeps working
    /// ([`Self::is_approved`], module doc — only a hook that fires
    /// everywhere can still be looked up under one), and none is ever added.
    pub fn approve(
        &self,
        fingerprint_prefix: &str,
        reviewed_root: Option<&Path>,
    ) -> io::Result<Option<ConsentEntry>> {
        let mut approved: Option<ConsentEntry> = None;
        self.mutate(|entries| {
            let Some(key) = match_prefix(entries, fingerprint_prefix) else {
                return false;
            };
            match entries.get_mut(&key) {
                Some(entry)
                    if reviewed_root.is_some() && entry.plugin_root.as_deref() == reviewed_root =>
                {
                    // Re-hash at approval time, not record time: the operator
                    // just reviewed (and `aleph hooks test` just RAN) the
                    // script as it exists NOW, so that content is what the
                    // approval attests to. A stale record-time hash would
                    // refuse the very version the operator green-lit. The
                    // script is found from the root that review ran in.
                    let script = script_fingerprint(
                        &entry.command,
                        &ScriptContext::new(
                            &entry.plugin_name,
                            entry.project_root.as_deref(),
                            entry.plugin_root.as_deref(),
                        ),
                    );
                    entry.status = ConsentStatus::Approved;
                    entry.approved_at = Some(now_secs());
                    entry.script_fingerprint = script;
                    approved = Some(entry.clone());
                    true
                }
                // No such entry, no root, or it moved to a root nobody reviewed.
                _ => false,
            }
        })?;
        Ok(approved)
    }

    /// Revoke consent for a hook (sets it back to `pending`). Returns the
    /// affected entry, or `None` when no entry matches the prefix.
    pub fn revoke(&self, fingerprint_prefix: &str) -> io::Result<Option<ConsentEntry>> {
        let mut revoked: Option<ConsentEntry> = None;
        self.mutate(|entries| {
            let Some(key) = match_prefix(entries, fingerprint_prefix) else {
                return false;
            };
            match entries.get_mut(&key) {
                Some(entry) => {
                    entry.status = ConsentStatus::Pending;
                    entry.approved_at = None;
                    revoked = Some(entry.clone());
                    true
                }
                None => false,
            }
        })?;
        Ok(revoked)
    }

    /// Revoke every approved hook (all back to `pending`). Returns the count.
    pub fn revoke_all(&self) -> io::Result<usize> {
        let mut count = 0usize;
        self.mutate(|entries| {
            for entry in entries.values_mut() {
                if entry.status == ConsentStatus::Approved {
                    entry.status = ConsentStatus::Pending;
                    entry.approved_at = None;
                    count += 1;
                }
            }
            count > 0
        })?;
        Ok(count)
    }

    /// All consent entries, fingerprint-sorted. Re-reads if the file changed.
    pub fn entries(&self) -> Vec<ConsentEntry> {
        self.reload_if_stale();
        self.cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .values()
            .cloned()
            .collect()
    }

    /// Look up a single entry by fingerprint prefix.
    pub fn find(&self, fingerprint_prefix: &str) -> Option<ConsentEntry> {
        self.reload_if_stale();
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        match_prefix(&cache.entries, fingerprint_prefix)
            .and_then(|k| cache.entries.get(&k).cloned())
    }

    // ---- internals --------------------------------------------------------

    /// Read + parse the registry file. Missing file ⇒ empty. A parse error is
    /// logged and treated as empty (defensive — a corrupt file must not crash
    /// the server, and re-recording pending hooks self-heals it).
    fn read_file(path: &Path) -> (BTreeMap<String, ConsentEntry>, FileStamp) {
        let stamp = file_stamp(path);
        let raw = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(_) => return (BTreeMap::new(), stamp),
        };
        match serde_json::from_slice::<RegistryDoc>(&raw) {
            Ok(doc) => {
                let map = doc
                    .entries
                    .into_iter()
                    .map(|e| (e.fingerprint.clone(), e))
                    .collect();
                (map, stamp)
            }
            Err(e) => {
                warn!(
                    error = %e,
                    path = %path.display(),
                    "Corrupt shell-hook consent registry — treating as empty"
                );
                (BTreeMap::new(), stamp)
            }
        }
    }

    /// Reload the in-memory cache if the file's `(mtime, len)` stamp changed.
    fn reload_if_stale(&self) {
        let disk_stamp = file_stamp(&self.path);
        {
            let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
            if cache.stamp == disk_stamp {
                return;
            }
        }
        let (entries, stamp) = Self::read_file(&self.path);
        let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
        cache.entries = entries;
        cache.stamp = stamp;
    }

    /// Cross-process-safe read-modify-write: take an exclusive file lock,
    /// re-read the on-disk registry (another process may have changed it),
    /// apply `f`, persist atomically when `f` reports a change, then refresh
    /// the in-memory cache.
    fn mutate<F>(&self, f: F) -> io::Result<()>
    where
        F: FnOnce(&mut BTreeMap<String, ConsentEntry>) -> bool,
    {
        use fs2::FileExt;

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock_path = self.path.with_extension("lock");
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        lock.lock_exclusive()?;

        let (mut entries, _) = Self::read_file(&self.path);
        if f(&mut entries) {
            let doc = RegistryDoc {
                version: REGISTRY_VERSION,
                entries: entries.values().cloned().collect(),
            };
            let bytes = serde_json::to_vec_pretty(&doc)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            let tmp = self.path.with_extension("json.tmp");
            fs::write(&tmp, &bytes)?;
            fs::rename(&tmp, &self.path)?;
        }

        {
            let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
            cache.entries = entries;
            cache.stamp = file_stamp(&self.path);
        }
        let _ = FileExt::unlock(&lock);
        Ok(())
    }
}

/// Find the single registry key matching a fingerprint prefix. Returns `None`
/// when there is no match or the prefix is ambiguous (matches >1 entry).
fn match_prefix(entries: &BTreeMap<String, ConsentEntry>, prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        return None;
    }
    // review(extension): require at least 4 hex chars — the full fingerprint
    // is 16 hex chars (see `fingerprint`), so 4 hex chars still has ~65k
    // possible prefixes per entry and stops a one-character prefix from
    // resolving to whichever entry happens to sort first alphabetically.
    // Without this, `aleph hooks approve a` would approve the first entry
    // starting with `a` and silently skip the matching-by-prefix step.
    if prefix.len() < 4 {
        return None;
    }
    if entries.contains_key(prefix) {
        return Some(prefix.to_string());
    }
    let mut matches = entries.keys().filter(|k| k.starts_with(prefix));
    let first = matches.next()?.clone();
    match matches.next() {
        Some(_) => None, // ambiguous
        None => Some(first),
    }
}

fn file_stamp(path: &Path) -> FileStamp {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    Some((mtime, meta.len()))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn hex16(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(16);
    for b in bytes.iter().take(8) {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Largest script we will hash. A hook driven by a multi-megabyte file is not
/// a thing; the cap just stops a pathological path from being read into memory
/// on every `is_approved` call.
const MAX_SCRIPT_HASH_BYTES: u64 = 1024 * 1024;

/// File extensions that positively identify the script token in a command.
const SCRIPT_EXTENSIONS: [&str; 7] = [".sh", ".bash", ".zsh", ".py", ".js", ".ts", ".rb"];

/// The directories a hook's command can name its script through, as far as
/// consent can know them — the same values `command_hook_invocation` hands
/// that hook's child.
struct ScriptContext<'a> {
    /// The hook's root: its root path variables' value, and the directory
    /// a hook's command runs in (so what a relative path is relative to).
    /// A plugin command's inline command runs in the session's directory
    /// instead, which is why it may not name a relative script
    /// ([`ShellHookConsent::root_relative_script`]).
    root: Option<&'a Path>,
    /// A plugin hook's data directory (`${CLAUDE_PLUGIN_DATA}` …).
    data: Option<PathBuf>,
    /// The project a project-bound hook fires in: `$CLAUDE_PROJECT_DIR`
    /// whenever that hook can fire. `None` for a hook that fires everywhere,
    /// where that variable names a different directory per session.
    project: Option<&'a Path>,
}

impl<'a> ScriptContext<'a> {
    fn new(owner: &str, project: Option<&'a Path>, root: Option<&'a Path>) -> Self {
        let data = root
            .filter(|_| crate::extension::manifest::validate_plugin_id(owner).is_ok())
            .map(|root| {
                crate::extension::plugin_vars::PluginVars::new(owner, root)
                    .data_dir()
                    .to_path_buf()
            });
        Self {
            root,
            data,
            project,
        }
    }

    /// Every variable this context can resolve, with its value.
    fn variables(&self) -> Vec<(&'static str, &Path)> {
        let mut out = Vec::new();
        if let Some(root) = self.root {
            out.extend(super::PLUGIN_ROOT_VARIABLES.map(|name| (name, root)));
        }
        if let Some(data) = &self.data {
            out.extend(super::PLUGIN_DATA_VARIABLES.map(|name| (name, data.as_path())));
        }
        if let Some(project) = self.project {
            out.push(("CLAUDE_PROJECT_DIR", project));
        }
        out
    }

    /// One whitespace-separated word of a command as the path it names, or
    /// `None` when it cannot be a stable path. The quotes a shell removes go,
    /// wherever they sit in the word (`"${CLAUDE_PLUGIN_ROOT}"/hooks/x.sh`),
    /// and each known variable becomes its value. What is judged for `$` and
    /// `*` is the word as written minus the known variables — not their
    /// values: a root named `a$b` is still one path.
    fn expand(&self, raw: &str) -> Option<String> {
        let unquoted: String = raw.chars().filter(|c| !matches!(c, '"' | '\'')).collect();
        let (mut expanded, mut written) = (unquoted.clone(), unquoted);
        for (name, value) in self.variables() {
            let Some(value) = value.to_str() else {
                continue;
            };
            expanded = replace_variable(&expanded, name, value);
            written = replace_variable(&written, name, "");
        }
        let stable = !expanded.is_empty() && !written.contains('$') && !written.contains('*');
        stable.then_some(expanded)
    }
}

/// `text` with every `${name}`, and every `$name` not followed by an
/// identifier character, replaced by `value`.
fn replace_variable(text: &str, name: &str, value: &str) -> String {
    let text = text.replace(&format!("${{{name}}}"), value);
    let bare = format!("${name}");
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find(bare.as_str()) {
        let (before, from) = rest.split_at(at);
        let after = from.get(bare.len()..).unwrap_or_default();
        let continues = after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        out.push_str(before);
        out.push_str(if continues { &bare } else { value });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Extract the script file a shell command invokes, if any.
///
/// Handles the three shapes that cover essentially every hook in practice:
/// `./hook.sh`, `python3 /path/hook.py`, `/usr/bin/env bash hook.sh` — each
/// also written through a path variable (`"${CLAUDE_PLUGIN_ROOT}"/hooks/x.sh`,
/// `"$CLAUDE_PROJECT_DIR"/.claude/hooks/x.sh` for a project-bound hook) or
/// relative to the hook's root, which is where the command runs. Until
/// 2026-09-25 a word holding any `$` was dropped and a relative one was
/// resolved against THIS process's directory, so every hook written the
/// Claude Code way was never content-bound: a `git pull` rewriting its
/// script kept its approval.
///
/// **Two passes, and the order matters.** A known script extension wins over a
/// merely path-shaped token, because a single pass binds to whichever comes
/// first — and in `sh --rcfile /etc/bashrc run.sh` that is the *config file*.
/// Binding to the wrong file inverts the guard: editing the config would
/// revoke consent (harmless but confusing) while editing `run.sh` — the thing
/// that actually executes — would NOT. Extension-first makes the common case
/// bind to the code.
///
/// Conservative throughout: a candidate must also resolve to an existing
/// regular file. Anything else yields `None`, which keeps the entry on
/// command-string-only semantics rather than binding to something wrong.
/// Only the first match is used; a command chaining two scripts binds to the
/// first, and hashing every token would make the common case pay for a shape
/// nobody writes.
fn script_path_from_command(command: &str, context: &ScriptContext<'_>) -> Option<PathBuf> {
    script_candidates(command, context)
        .iter()
        .filter_map(|token| candidate_path(token, context))
        .find(|path| path.is_file())
}

/// The words a command's script is looked for among, in consent's order: the
/// first word that names itself a script, then every path-shaped word — each
/// with the context's known variables expanded ([`ScriptContext::expand`]).
fn script_candidates(command: &str, context: &ScriptContext<'_>) -> Vec<String> {
    // Anything with a glob or a variable nobody here can resolve is not a
    // stable path, so it is dropped entirely.
    let tokens: Vec<String> = command
        .split_whitespace()
        .filter_map(|raw| context.expand(raw))
        .collect();

    // Pass 1: a token that names itself a script.
    let by_extension = tokens
        .iter()
        .find(|t| {
            let lower = t.to_ascii_lowercase();
            SCRIPT_EXTENSIONS.iter().any(|e| lower.ends_with(e))
        })
        .cloned();
    // Pass 2: fall back to anything path-shaped (a bare `./hook`, no extension).
    let path_shaped = tokens
        .iter()
        .filter(|t| t.contains('/') || t.contains('\\'))
        .cloned();
    by_extension.into_iter().chain(path_shaped).collect()
}

/// The file a candidate word names: `~/` from the home directory, an
/// absolute path as written, a relative one from where a hook runs — its
/// root — never from wherever this process happens to be. `None` when that
/// base is unknown.
fn candidate_path(token: &str, context: &ScriptContext<'_>) -> Option<PathBuf> {
    match token.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|home| home.join(rest)),
        None if Path::new(token).is_absolute() => Some(PathBuf::from(token)),
        None => context.root.map(|root| root.join(token)),
    }
}

/// `sha256` (16 hex chars) of the script `command` invokes, or `None` when
/// there is no resolvable/readable script. Never propagates an I/O error: an
/// unreadable file simply has no fingerprint, and the caller decides what that
/// means (record time: no binding; check time: fail safe).
fn script_fingerprint(command: &str, context: &ScriptContext<'_>) -> Option<String> {
    let path = script_path_from_command(command, context)?;
    let meta = fs::metadata(&path).ok()?;
    if meta.len() > MAX_SCRIPT_HASH_BYTES {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(hex16(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_consent() -> (tempfile::TempDir, ShellHookConsent) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shell-hooks-allowlist.json");
        let consent = ShellHookConsent::with_path(path);
        (dir, consent)
    }

    #[test]
    fn fingerprint_is_stable_and_command_sensitive() {
        let a = ShellHookConsent::fingerprint("plug", None, "echo hi");
        let b = ShellHookConsent::fingerprint("plug", None, "echo hi");
        let c = ShellHookConsent::fingerprint("plug", None, "echo HI");
        let d = ShellHookConsent::fingerprint("other", None, "echo hi");
        assert_eq!(a, b);
        assert_ne!(a, c, "command change must change the fingerprint");
        assert_ne!(a, d, "plugin change must change the fingerprint");
        assert_eq!(a.len(), 16);
    }

    /// A global key is the pre-binding key, byte for byte, so every approval
    /// of a hook that fires everywhere survives the change. Pinned value:
    /// `sha256("plug\0echo hi")`, what `286ccf533` computed.
    #[test]
    fn a_global_key_is_the_key_it_always_was() {
        assert_eq!(
            ShellHookConsent::fingerprint("plug", None, "echo hi"),
            "c64b0e5b341bfa92"
        );
    }

    #[test]
    fn a_project_key_names_its_project() {
        let key = |root: Option<&str>| {
            ShellHookConsent::fingerprint("user:project", root.map(Path::new), "lint")
        };
        assert_ne!(
            key(Some("/repo/a")),
            key(Some("/repo/b")),
            "one template in two projects is two keys"
        );
        assert_ne!(
            key(Some("/repo/a")),
            key(None),
            "a project key is never the shared-label key"
        );
    }

    /// The doctor's drift check recomputes each entry's key from its own
    /// fields; a project entry must round-trip through the file, or every
    /// one of them reads as hand-edited.
    #[test]
    fn a_recorded_project_entry_derives_its_own_fingerprint() {
        let (_d, consent) = tmp_consent();
        let root = tempfile::tempdir().expect("tempdir");
        let scope = ScopeKey::project(root.path());
        consent.record_pending("user:project", &scope, "lint", "PreToolUse", root.path());

        let entry = consent.entries().remove(0);
        assert_eq!(
            entry.project_root,
            Some(crate::extension::visibility::canonical_root(root.path()))
        );
        assert_eq!(entry.expected_fingerprint(), entry.fingerprint);
        assert!(!entry.predates_project_binding());
        let reopened = ShellHookConsent::with_path(consent.path());
        assert_eq!(
            reopened.entries()[0].expected_fingerprint(),
            entry.fingerprint
        );
    }

    /// The review surfaces say who picks an inline command's arguments — for
    /// every inline command, whatever its text spells — and say nothing of
    /// the kind for a hook.
    #[test]
    fn only_an_inline_command_entry_carries_the_invoker_arguments_note() {
        let (_d, consent) = tmp_consent();
        let root = tempfile::tempdir().expect("tempdir");
        consent.record_pending(
            "plug",
            &ScopeKey::Global,
            "git status",
            INLINE_COMMAND_EVENT,
            root.path(),
        );
        consent.record_pending("plug", &ScopeKey::Global, "lint", "PreToolUse", root.path());
        let notes: Vec<(String, bool)> = consent
            .entries()
            .iter()
            .map(|e| (e.command.clone(), e.invoker_arguments_note().is_some()))
            .collect();
        assert!(
            notes.contains(&("git status".to_string(), true)),
            "{notes:?}"
        );
        assert!(notes.contains(&("lint".to_string(), false)), "{notes:?}");
    }

    /// Which inline commands name a script consent would resolve in the wrong
    /// directory: a relative script word is refused, the plugin's own
    /// `${CLAUDE_PLUGIN_ROOT}/…` script is not, nor is a command with no
    /// path at all; an existing absolute script binds, and a relative
    /// argument after it is data to that script.
    #[test]
    fn a_relative_script_word_is_named_and_a_plugin_root_script_is_not() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(root.path().join("check.sh"), "true").unwrap();
        let abs = root.path().join("check.sh").display().to_string();
        let relative = |command: &str| {
            ShellHookConsent::root_relative_script("plug", None, Some(root.path()), command)
        };
        assert_eq!(
            relative("sh scripts/check.sh"),
            Some("scripts/check.sh".into())
        );
        assert_eq!(relative("./run"), Some("./run".into()));
        assert_eq!(relative("git diff src/app.ts"), Some("src/app.ts".into()));
        assert_eq!(relative("sh ${CLAUDE_PLUGIN_ROOT}/check.sh"), None);
        assert_eq!(relative("git status --short"), None);
        assert_eq!(relative("gh pr view $1"), None);
        assert_eq!(relative(&format!("sh {abs} src/app.ts")), None);
    }

    /// Recording under `Global` is exactly how every entry looked before the
    /// binding. Only the project-settings labels can be told apart.
    #[test]
    fn only_an_unbound_project_label_entry_predates_the_binding() {
        let (_d, consent) = tmp_consent();
        for (owner, command) in [
            ("user:project", "a"),
            ("user:project-local", "b"),
            ("user:global", "c"),
            ("some-plugin", "d"),
        ] {
            consent.record_pending(owner, &ScopeKey::Global, command, "e", Path::new("/p"));
        }
        let mut marked: Vec<String> = consent
            .entries()
            .into_iter()
            .filter(ConsentEntry::predates_project_binding)
            .map(|e| e.command)
            .collect();
        marked.sort();
        assert_eq!(marked, ["a", "b"]);
    }

    // -- the root an approval attests to ------------------------------------

    fn key_of(consent: &ShellHookConsent) -> String {
        consent.entries()[0].fingerprint.clone()
    }

    #[test]
    fn an_approval_does_not_follow_its_key_to_another_root() {
        let (_d, consent) = tmp_consent();
        let global = ScopeKey::Global;
        consent.record_pending("fmt", &global, "lint", "PreToolUse", Path::new("/a"));
        consent
            .approve(&key_of(&consent), Some(Path::new("/a")))
            .unwrap();

        assert!(consent.is_approved("fmt", &global, Path::new("/a"), "lint"));
        assert!(
            !consent.is_approved("fmt", &global, Path::new("/b"), "lint"),
            "approved for /a, run from /b"
        );
    }

    /// While pending, an entry says what the latest fire ran; once
    /// approved, it keeps what was approved.
    #[test]
    fn a_pending_entry_follows_the_latest_fire_and_an_approved_one_does_not() {
        let (_d, consent) = tmp_consent();
        let global = ScopeKey::Global;
        consent.record_pending("fmt", &global, "lint", "PreToolUse", Path::new("/a"));
        consent.record_pending("fmt", &global, "lint", "before_tool_call", Path::new("/b"));
        let entry = consent.entries().remove(0);
        assert_eq!(
            (entry.event.as_str(), entry.plugin_root.as_deref()),
            ("before_tool_call", Some(Path::new("/b")))
        );

        consent
            .approve(&entry.fingerprint, Some(Path::new("/b")))
            .unwrap();
        consent.record_pending("fmt", &global, "lint", "PreToolUse", Path::new("/c"));
        let entry = consent.entries().remove(0);
        assert_eq!(entry.status, ConsentStatus::Approved);
        assert_eq!(entry.plugin_root.as_deref(), Some(Path::new("/b")));
    }

    /// An entry recorded before the spelling and the root were kept — the
    /// shape every entry on an upgraded install has — is repaired by its
    /// next fire while pending, so it can be reviewed as production runs it.
    #[test]
    fn a_pending_entry_from_before_roots_were_kept_is_filled_in_by_its_next_fire() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shell-hooks-allowlist.json");
        let fp = ShellHookConsent::fingerprint("fmt", None, "lint");
        let legacy = serde_json::json!({ "version": 1, "entries": [{
            "fingerprint": fp, "plugin_name": "fmt", "command": "lint",
            "event": "BeforeToolCall", "status": "pending", "first_seen": 1
        }] });
        std::fs::write(&path, legacy.to_string()).expect("seed legacy registry");
        let consent = ShellHookConsent::with_path(&path);

        consent.record_pending(
            "fmt",
            &ScopeKey::Global,
            "lint",
            "PreToolUse",
            Path::new("/a"),
        );

        let entry = consent.entries().remove(0);
        assert_eq!(entry.event, "PreToolUse");
        assert_eq!(entry.plugin_root.as_deref(), Some(Path::new("/a")));
        assert_eq!(entry.status, ConsentStatus::Pending);
    }

    /// The review ran against `/a`; a fire moved the pending entry to `/b`
    /// before the operator said yes. Approving now would bind `/b`, which
    /// nobody reviewed.
    #[test]
    fn approval_is_refused_when_the_entry_moved_since_it_was_reviewed() {
        let (_d, consent) = tmp_consent();
        let global = ScopeKey::Global;
        consent.record_pending("fmt", &global, "lint", "PreToolUse", Path::new("/a"));
        let reviewed = consent.entries().remove(0);
        consent.record_pending("fmt", &global, "lint", "PreToolUse", Path::new("/b"));

        let outcome = consent
            .approve(&reviewed.fingerprint, reviewed.plugin_root.as_deref())
            .unwrap();
        assert!(outcome.is_none(), "approved a root nobody reviewed");
        assert_eq!(consent.entries()[0].status, ConsentStatus::Pending);
    }

    /// A pending entry recorded before roots were kept cannot be approved as
    /// it is: the approval would bind no directory. Its next fire records the
    /// root, and then it can.
    #[test]
    fn a_pending_entry_without_a_root_cannot_be_approved_until_it_fires() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shell-hooks-allowlist.json");
        let fp = ShellHookConsent::fingerprint("fmt", None, "lint");
        let legacy = serde_json::json!({ "version": 1, "entries": [{
            "fingerprint": fp, "plugin_name": "fmt", "command": "lint",
            "status": "pending", "first_seen": 1
        }] });
        std::fs::write(&path, legacy.to_string()).expect("seed legacy registry");
        let consent = ShellHookConsent::with_path(&path);

        assert!(
            consent.approve(&fp, None).unwrap().is_none(),
            "minted an approval bound to no root"
        );
        assert_eq!(consent.entries()[0].status, ConsentStatus::Pending);

        consent.record_pending(
            "fmt",
            &ScopeKey::Global,
            "lint",
            "PreToolUse",
            Path::new("/a"),
        );
        let entry = consent.approve(&fp, Some(Path::new("/a"))).unwrap();
        assert_eq!(entry.and_then(|e| e.plugin_root), Some(PathBuf::from("/a")));
    }

    /// An approval recorded before roots were kept attests to no root; like
    /// one with no script fingerprint it keeps its older meaning.
    #[test]
    fn an_approval_from_before_roots_were_kept_is_not_bound_to_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shell-hooks-allowlist.json");
        let fp = ShellHookConsent::fingerprint("fmt", None, "lint");
        let legacy = serde_json::json!({ "version": 1, "entries": [{
            "fingerprint": fp, "plugin_name": "fmt", "command": "lint",
            "status": "approved", "first_seen": 1, "approved_at": 2
        }] });
        std::fs::write(&path, legacy.to_string()).expect("seed legacy registry");
        let consent = ShellHookConsent::with_path(&path);
        assert!(consent.is_approved("fmt", &ScopeKey::Global, Path::new("/any"), "lint"));
    }

    #[test]
    fn unknown_hook_is_not_approved() {
        let (_d, consent) = tmp_consent();
        assert!(!consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "rm -rf /"));
    }

    // -- script-content binding (TOCTOU guard) -----------------------------

    /// Write an executable-ish script and return `(dir, command)`.
    fn script_hook(body: &str) -> (tempfile::TempDir, PathBuf, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("hook.sh");
        std::fs::write(&script, body).expect("write script");
        let command = format!("sh {}", script.display());
        (dir, script, command)
    }

    #[test]
    fn approval_binds_to_script_content_and_drift_revokes_it() {
        // The whole point: approving `sh …/hook.sh` must NOT keep approving it
        // after the file is rewritten. Command string is byte-identical
        // throughout, so only content binding can catch this.
        let (_d, consent) = tmp_consent();
        let (_sd, script, command) = script_hook("echo safe\n");

        consent.record_pending(
            "p",
            &ScopeKey::Global,
            &command,
            "before_tool_call",
            Path::new("/p"),
        );
        let fp = consent.entries()[0].fingerprint.clone();
        consent
            .approve(&fp, Some(Path::new("/p")))
            .expect("approve");
        assert!(
            consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), &command),
            "freshly approved"
        );

        std::fs::write(&script, "rm -rf /\n").expect("rewrite script");
        assert!(
            !consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), &command),
            "edited script must lose its approval"
        );

        // Restoring the exact approved bytes restores the approval — the
        // guard keys on content, not on an mtime that any touch would bump.
        std::fs::write(&script, "echo safe\n").expect("restore script");
        assert!(consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), &command));
    }

    #[test]
    fn approving_records_the_content_reviewed_at_approval_time() {
        // `aleph hooks test` runs the script, THEN asks to approve. If the
        // fingerprint were frozen at record time, editing between the two
        // steps would make the just-approved version fail on first fire.
        let (_d, consent) = tmp_consent();
        let (_sd, script, command) = script_hook("echo v1\n");
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            &command,
            "before_tool_call",
            Path::new("/p"),
        );

        std::fs::write(&script, "echo v2\n").expect("edit before approving");
        let fp = consent.entries()[0].fingerprint.clone();
        consent
            .approve(&fp, Some(Path::new("/p")))
            .expect("approve");

        assert!(
            consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), &command),
            "approval must attest to the content present when approving"
        );
    }

    #[test]
    fn deleting_an_approved_script_fails_safe() {
        let (_d, consent) = tmp_consent();
        let (_sd, script, command) = script_hook("echo hi\n");
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            &command,
            "before_tool_call",
            Path::new("/p"),
        );
        let fp = consent.entries()[0].fingerprint.clone();
        consent
            .approve(&fp, Some(Path::new("/p")))
            .expect("approve");

        std::fs::remove_file(&script).expect("delete script");
        assert!(!consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), &command));
    }

    #[test]
    fn commands_without_a_script_keep_string_only_semantics() {
        // `echo hi` has nothing to bind to; it must still approve normally
        // rather than being permanently refused for lack of a fingerprint.
        let (_d, consent) = tmp_consent();
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            "echo hi",
            "before_tool_call",
            Path::new("/p"),
        );
        assert!(consent.entries()[0].script_fingerprint.is_none());
        let fp = consent.entries()[0].fingerprint.clone();
        consent
            .approve(&fp, Some(Path::new("/p")))
            .expect("approve");
        assert!(consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"));
    }

    #[test]
    fn legacy_entries_without_a_script_fingerprint_still_work() {
        // Back-compat: a registry written before this field existed
        // deserializes with `script_fingerprint: None` and must keep running.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shell-hooks-allowlist.json");
        std::fs::write(
            &path,
            r#"{"version":1,"entries":[{
                "fingerprint":"deadbeefdeadbeef","plugin_name":"p",
                "command":"sh /nonexistent/legacy.sh","event":"BeforeToolCall",
                "status":"approved","first_seen":1,"approved_at":2
            }]}"#,
        )
        .expect("seed legacy registry");
        let consent = ShellHookConsent::with_path(&path);

        let entry = &consent.entries()[0];
        assert!(entry.script_fingerprint.is_none());
        // The seeded fingerprint is synthetic, so look it up the way the
        // executor does: by (plugin, command).
        let real_fp = ShellHookConsent::fingerprint("p", None, "sh /nonexistent/legacy.sh");
        assert_ne!(real_fp, entry.fingerprint, "seeded id is deliberately fake");
        // The stored entry is keyed by its recorded fingerprint and stays
        // approved — no forced re-consent for pre-existing registries.
        assert_eq!(entry.status, ConsentStatus::Approved);
    }

    #[test]
    fn script_path_resolution_covers_the_common_command_shapes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("hook.py");
        std::fs::write(&script, "print(1)").expect("write");
        let p = script.display().to_string();

        let resolve = |command: &str| script_path_from_command(command, &NO_CONTEXT);
        assert_eq!(
            resolve(&format!("python3 {p}")).as_ref(),
            Some(&script),
            "interpreter-prefixed"
        );
        assert_eq!(
            resolve(&format!("/usr/bin/env python3 \"{p}\"")).as_ref(),
            Some(&script),
            "env-prefixed and quoted"
        );
        assert_eq!(resolve(&p).as_ref(), Some(&script), "bare path");
        // Nothing resolvable → no binding (not a wrong binding).
        assert!(resolve("echo hi").is_none());
        assert!(resolve("sh /does/not/exist.sh").is_none());
        // Variables nobody here can resolve, and globs, are not stable paths.
        assert!(resolve("sh $HOME/hook.sh").is_none());
        assert!(resolve("sh ./hooks/*.sh").is_none());
    }

    #[test]
    fn a_script_extension_wins_over_an_earlier_path_argument() {
        // Single-pass resolution would bind to `--rcfile`'s target, which
        // INVERTS the guard: editing the config would revoke consent while
        // editing the script that actually runs would not.
        let dir = tempfile::tempdir().expect("tempdir");
        let config = dir.path().join("bashrc");
        let script = dir.path().join("run.sh");
        std::fs::write(&config, "# config").expect("write config");
        std::fs::write(&script, "echo hi").expect("write script");

        let command = format!("sh --rcfile {} {}", config.display(), script.display());
        assert_eq!(
            script_path_from_command(&command, &NO_CONTEXT).as_ref(),
            Some(&script),
            "must bind to the executed script, not an earlier path argument"
        );
    }

    #[test]
    fn extensionless_script_still_resolves_via_the_path_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("hook");
        std::fs::write(&script, "echo hi").expect("write");
        assert_eq!(
            script_path_from_command(&script.display().to_string(), &NO_CONTEXT).as_ref(),
            Some(&script)
        );
    }

    /// No root, data directory or project: only literal absolute and `~/`
    /// paths can be found.
    const NO_CONTEXT: ScriptContext<'static> = ScriptContext {
        root: None,
        data: None,
        project: None,
    };

    /// The shapes a hook written the Claude Code way names its script with —
    /// each resolved from the hook's root (or project), whatever directory
    /// this process runs in.
    #[test]
    fn a_script_named_through_a_path_variable_or_relative_to_the_root_is_found() {
        let root = tempfile::tempdir().expect("tempdir");
        let script = root.path().join("hooks/lint.sh");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "echo hi").unwrap();
        let hook = ScriptContext::new("user:project", None, Some(root.path()));
        let project = ScriptContext::new("user:project", Some(root.path()), None);

        for (command, context) in [
            (r#"sh "${CLAUDE_PLUGIN_ROOT}/hooks/lint.sh""#, &hook),
            (r#"sh "${CLAUDE_PLUGIN_ROOT}"/hooks/lint.sh"#, &hook),
            ("sh $ALEPH_PLUGIN_ROOT/hooks/lint.sh", &hook),
            ("sh ${PLUGIN_ROOT}/hooks/lint.sh", &hook),
            ("sh hooks/lint.sh", &hook),
            ("sh ./hooks/lint.sh", &hook),
            (r#""$CLAUDE_PROJECT_DIR"/hooks/lint.sh"#, &project),
        ] {
            assert_eq!(
                script_path_from_command(command, context).as_ref(),
                Some(&script),
                "{command}"
            );
        }

        // What cannot be resolved stays unbound, not bound to a guess: a
        // variable with no known value, another variable, a relative path
        // with no root, and `$CLAUDE_PROJECT_DIR` for a hook that fires
        // everywhere.
        for (command, context) in [
            ("sh ${CLAUDE_PLUGIN_ROOT}/hooks/lint.sh", &NO_CONTEXT),
            ("sh $CLAUDE_PLUGIN_ROOTX/hooks/lint.sh", &hook),
            ("sh hooks/lint.sh", &NO_CONTEXT),
            ("sh $CLAUDE_PROJECT_DIR/hooks/lint.sh", &hook),
        ] {
            assert!(
                script_path_from_command(command, context).is_none(),
                "{command}"
            );
        }
    }

    /// The template is judged for `$`, not the root: a directory named with
    /// one is still one path.
    #[test]
    fn a_root_named_with_a_dollar_is_still_resolved() {
        let base = tempfile::tempdir().expect("tempdir");
        let root = base.path().join("repo$x");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("lint.sh"), "echo hi").unwrap();
        let context = ScriptContext::new("user:project", None, Some(root.as_path()));
        assert_eq!(
            script_path_from_command("sh ${CLAUDE_PLUGIN_ROOT}/lint.sh", &context),
            Some(root.join("lint.sh"))
        );
    }

    #[test]
    fn record_pending_then_approve_round_trip() {
        let (_d, consent) = tmp_consent();
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            "echo hi",
            "after_tool_call",
            Path::new("/p"),
        );
        assert!(
            !consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"),
            "pending != approved"
        );

        let entries = consent.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, ConsentStatus::Pending);
        let fp = entries[0].fingerprint.clone();

        let approved = consent
            .approve(&fp, Some(Path::new("/p")))
            .expect("approve")
            .expect("entry");
        assert_eq!(approved.status, ConsentStatus::Approved);
        assert!(
            consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"),
            "approved hook runs"
        );
    }

    #[test]
    fn record_pending_is_idempotent() {
        let (_d, consent) = tmp_consent();
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            "echo hi",
            "after_tool_call",
            Path::new("/p"),
        );
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            "echo hi",
            "after_tool_call",
            Path::new("/p"),
        );
        assert_eq!(consent.entries().len(), 1);
    }

    #[test]
    fn revoke_sends_approved_hook_back_to_pending() {
        let (_d, consent) = tmp_consent();
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            "echo hi",
            "before_tool_call",
            Path::new("/p"),
        );
        let fp = consent.entries()[0].fingerprint.clone();
        consent.approve(&fp, Some(Path::new("/p"))).unwrap();
        assert!(consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"));

        let revoked = consent.revoke(&fp).expect("revoke").expect("entry");
        assert_eq!(revoked.status, ConsentStatus::Pending);
        assert!(!consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"));
    }

    #[test]
    fn approve_by_prefix_resolves_unique_match() {
        let (_d, consent) = tmp_consent();
        consent.record_pending("p", &ScopeKey::Global, "echo hi", "e", Path::new("/p"));
        let fp = consent.entries()[0].fingerprint.clone();
        let approved = consent
            .approve(fp.get(..6).unwrap_or(&fp), Some(Path::new("/p")))
            .expect("approve")
            .expect("entry");
        assert_eq!(approved.fingerprint, fp);
    }

    #[test]
    fn approve_unknown_prefix_returns_none() {
        let (_d, consent) = tmp_consent();
        assert!(consent
            .approve("ffffffff", None)
            .expect("approve")
            .is_none());
    }

    #[test]
    fn changes_persist_across_reload() {
        let (_d, consent) = tmp_consent();
        let path = consent.path().to_path_buf();
        consent.record_pending("p", &ScopeKey::Global, "echo hi", "e", Path::new("/p"));
        let fp = consent.entries()[0].fingerprint.clone();
        consent.approve(&fp, Some(Path::new("/p"))).unwrap();

        // A fresh instance reads the same file from disk.
        let reopened = ShellHookConsent::with_path(path);
        assert!(reopened.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"));
    }

    #[test]
    fn external_file_change_is_picked_up_via_stamp() {
        let (_d, consent) = tmp_consent();
        let path = consent.path().to_path_buf();
        consent.record_pending("p", &ScopeKey::Global, "echo hi", "e", Path::new("/p"));
        assert!(!consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"));

        // A second process approves the hook by writing the file directly;
        // the approval grows the file, so the `(mtime, len)` stamp differs.
        let writer = ShellHookConsent::with_path(&path);
        let fp = writer.entries()[0].fingerprint.clone();
        writer.approve(&fp, Some(Path::new("/p"))).unwrap();

        assert!(
            consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"),
            "is_approved must reload after the registry file changes"
        );
    }
}
