//! Install provenance ledger: one durable record per extension installed *from
//! the Hub*, keyed by the catalog entry id it came from.
//!
//! Before this existed the install pipeline forgot everything the moment it
//! finished: nothing recorded which catalog entry produced a given MCP server or
//! skill directory, at what version, or under which install spec. The visible
//! cost was `ExtensionEntry.update_available` — a field the Panel renders a badge
//! for and every producer hard-coded to `false`, because no one could answer
//! "is the installed copy older than the catalog's".
//!
//! Scope is deliberately narrow. The ledger answers **"what did we install, and
//! from what"**; it does *not* try to be the join key for "is this currently
//! installed" — that stays with the live reconciliation in
//! [`crate::hub::reconcile`], which reads the real backends. A stale row (user
//! removed the extension by hand) is therefore inert rather than wrong: nothing
//! consults it unless the live set already says installed.
//!
//! Rows live in the same rusqlite file as the catalog cache (`hub_catalog.db`),
//! following this module pair's existing shape — pure free functions over a
//! `&Connection` here, thin async wrappers on `CatalogCache`.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::hub::types::{ExtensionEntry, ExtensionKind, InstallSpec};

/// What a Hub install recorded about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallOrigin {
    /// Catalog entry the install came from (primary key).
    pub entry_id: String,
    pub kind: ExtensionKind,
    /// Catalog slot that served the entry (`ExtensionEntry.source_id`).
    pub source_id: String,
    /// Human provenance label at install time (`ExtensionEntry.via`).
    pub via: Option<String>,
    /// Catalog `version` at install time — the left side of the update check.
    pub version: Option<String>,
    /// Digest of the spec that was actually executed. Catches a catalog entry
    /// whose command/args/url changed without a version bump.
    pub spec_digest: String,
    /// The local handle the install produced: MCP server id, or the on-disk path
    /// for a plugin/skill. Provenance and diagnostics — not a join key.
    pub local_ref: String,
    /// Unix seconds.
    pub installed_at: i64,
}

/// Stable digest of an install spec. Both sides of every comparison are produced
/// by this same serializer, so it only has to be deterministic in-process.
#[must_use]
fn spec_digest(spec: &InstallSpec) -> String {
    use sha2::{Digest, Sha256};
    let json = serde_json::to_string(spec).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(json.as_bytes());
    format!("{:x}", h.finalize())
}

impl InstallOrigin {
    /// Build the record for a just-completed install.
    #[must_use]
    pub fn record(
        entry: &ExtensionEntry,
        spec: &InstallSpec,
        local_ref: impl Into<String>,
        installed_at: i64,
    ) -> Self {
        Self {
            entry_id: entry.id.clone(),
            kind: entry.kind,
            source_id: entry.source_id.clone(),
            via: entry.via.clone(),
            version: entry.version.clone(),
            spec_digest: spec_digest(spec),
            local_ref: local_ref.into(),
            installed_at,
        }
    }
}

/// The local handle an install produced, as the ledger stores it: the MCP server
/// id, or the plugin/skill install directory.
fn outcome_local_ref(o: &crate::hub::install::InstallOutcome) -> &str {
    use crate::hub::install::InstallOutcome;
    match o {
        InstallOutcome::Mcp { id } => id,
        InstallOutcome::Plugin { path } | InstallOutcome::Skill { path } => path,
    }
}

/// Write the provenance row for a completed install. Shared by the
/// `extensions.install` RPC path and the agent-driven `hub_install_run` tool, so
/// both installs are equally traceable and equally update-checkable.
///
/// Best-effort: a ledger write failure costs the update badge, never the install
/// the caller just completed.
pub async fn record_install(
    cache: &crate::hub::cache::CatalogCache,
    entry: &ExtensionEntry,
    spec: &InstallSpec,
    outcome: &crate::hub::install::InstallOutcome,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let origin = InstallOrigin::record(entry, spec, outcome_local_ref(outcome), now);
    if let Err(e) = cache.record_origin(&origin).await {
        tracing::warn!(entry = %entry.id, error = %e, "failed to record install origin");
    }
}

/// True when the catalog now offers something different from what was installed.
///
/// A version bump is the primary signal; when either side has no version (MCP
/// presets carry none), fall back to the install-spec digest so a changed command
/// or endpoint still surfaces. Absence of evidence yields `false` — the badge
/// must never claim an update it cannot point at.
#[must_use]
pub fn update_available(origin: &InstallOrigin, entry: &ExtensionEntry) -> bool {
    if let (Some(installed), Some(offered)) = (&origin.version, &entry.version) {
        if installed != offered {
            return true;
        }
    }
    entry
        .install_spec
        .as_ref()
        .is_some_and(|s| spec_digest(s) != origin.spec_digest)
}

pub(super) fn init_origin_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS install_origin (
            entry_id     TEXT PRIMARY KEY,
            kind         TEXT NOT NULL,
            source_id    TEXT NOT NULL,
            via          TEXT,
            version      TEXT,
            spec_digest  TEXT NOT NULL,
            local_ref    TEXT NOT NULL,
            installed_at INTEGER NOT NULL
        );",
    )
}

/// Record (or re-record, on reinstall) one install. Idempotent by `entry_id`.
pub(super) fn upsert_origin(conn: &Connection, o: &InstallOrigin) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO install_origin
            (entry_id, kind, source_id, via, version, spec_digest, local_ref, installed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(entry_id) DO UPDATE SET
            kind=excluded.kind, source_id=excluded.source_id, via=excluded.via,
            version=excluded.version, spec_digest=excluded.spec_digest,
            local_ref=excluded.local_ref, installed_at=excluded.installed_at",
        params![
            o.entry_id,
            o.kind.as_str(),
            o.source_id,
            o.via,
            o.version,
            o.spec_digest,
            o.local_ref,
            o.installed_at,
        ],
    )?;
    Ok(())
}

fn row_to_origin(row: &rusqlite::Row<'_>) -> rusqlite::Result<InstallOrigin> {
    let kind: String = row.get(1)?;
    let kind =
        serde_json::from_value::<ExtensionKind>(serde_json::Value::String(kind)).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?;
    Ok(InstallOrigin {
        entry_id: row.get(0)?,
        kind,
        source_id: row.get(2)?,
        via: row.get(3)?,
        version: row.get(4)?,
        spec_digest: row.get(5)?,
        local_ref: row.get(6)?,
        installed_at: row.get(7)?,
    })
}

const SELECT_COLS: &str =
    "entry_id, kind, source_id, via, version, spec_digest, local_ref, installed_at";

pub fn all_origins(conn: &Connection) -> rusqlite::Result<Vec<InstallOrigin>> {
    let sql = format!("SELECT {SELECT_COLS} FROM install_origin");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_origin)?;
    rows.collect()
}

pub fn delete_origin(conn: &Connection, entry_id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM install_origin WHERE entry_id = ?1",
        params![entry_id],
    )
}

/// Forget the record for an uninstalled backend object, addressed the way
/// `extensions.uninstall` addresses it (`local:{kind}:{backend}` → `backend`).
///
/// Without this a removed-then-reinstalled-by-hand extension would keep the old
/// version in the ledger and light a false update badge. Matching is exact for
/// MCP (`local_ref` *is* the server id) and by trailing path segment for
/// plugin/skill, whose `local_ref` is the install directory.
pub(super) fn forget_installed(
    conn: &Connection,
    kind: ExtensionKind,
    backend: &str,
) -> rusqlite::Result<usize> {
    let all: Vec<InstallOrigin> = all_origins(conn)?
        .into_iter()
        .filter(|o| o.kind == kind)
        .collect();
    // Resolve exact matches first — this is what MCP uses (local_ref IS the
    // server id, verbatim). For plugin/skill the caller passes a *leaf* name
    // (the install directory), so we also need leaf matches. If multiple
    // rows share the same leaf (two catalog entries installed into the same
    // directory name), an unscoped forget would silently delete BOTH; we
    // avoid that by requiring the leaf match be unambiguous.
    let exact: Vec<String> = all
        .iter()
        .filter(|o| o.local_ref == backend)
        .map(|o| o.entry_id.clone())
        .collect();
    let doomed: Vec<String> = if !exact.is_empty() {
        // Two catalog entries that share a `local_ref` (e.g. a hub-managed
        // MCP server id and a user-installed one with the same id) would
        // both be wiped by an unscoped exact-match delete. Mirror the
        // leaf-match guard below: when multiple rows point at the same
        // backend, surface the ambiguity to the operator and skip rather
        // than silently deleting all of them.
        if exact.len() > 1 {
            tracing::warn!(
                kind = kind.as_str(),
                backend = %backend,
                candidates = %exact.join(","),
                "forget_installed: exact match is ambiguous across multiple ledger rows; skipping"
            );
            return Ok(0);
        }
        exact
    } else {
        let leaf_matches: Vec<String> = all
            .iter()
            .filter(|o| local_ref_addresses(&o.local_ref, backend))
            .map(|o| o.entry_id.clone())
            .collect();
        if leaf_matches.len() > 1 {
            tracing::warn!(
                kind = kind.as_str(),
                backend = %backend,
                candidates = %leaf_matches.join(","),
                "forget_installed: leaf name is ambiguous across multiple ledger rows; skipping"
            );
            return Ok(0);
        }
        leaf_matches
    };
    let mut removed = 0;
    for entry_id in doomed {
        removed += delete_origin(conn, &entry_id)?;
    }
    Ok(removed)
}

/// True when `local_ref` names the backend object `backend` — either verbatim
/// (MCP server id) or as the last path segment (plugin/skill install directory).
///
/// This is the bridge from an `extensions.installed` façade id
/// (`local:{kind}:{backend}`) back to the ledger row that produced it.
#[must_use]
pub fn local_ref_addresses(local_ref: &str, backend: &str) -> bool {
    if local_ref == backend {
        return true;
    }
    local_ref
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|leaf| leaf == backend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::{ExtensionCategory, TrustTier};

    fn entry(id: &str, version: Option<&str>, spec: InstallSpec) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(),
            kind: ExtensionKind::Mcp,
            category: ExtensionCategory::Developer,
            name: "E".into(),
            description: "d".into(),
            author: None,
            icon: None,
            tags: vec![],
            version: version.map(str::to_owned),
            source_id: "aleph-hub".into(),
            repo_url: None,
            trust_tier: TrustTier::Verified,
            requires_config: false,
            config_schema: None,
            installed: false,
            enabled: false,
            update_available: false,
            via: Some("Aleph Hub".into()),
            install_spec: Some(spec),
        }
    }

    fn stdio(args: &[&str]) -> InstallSpec {
        InstallSpec::McpStdio {
            command: "npx".into(),
            args: args.iter().map(|s| (*s).to_string()).collect(),
            env: vec![],
        }
    }

    #[test]
    fn roundtrips_through_sqlite() {
        let conn = Connection::open_in_memory().unwrap();
        init_origin_schema(&conn).unwrap();
        let e = entry("aleph-hub:x", Some("1.0.0"), stdio(&["@x/y"]));
        let spec = e.install_spec.clone().unwrap();
        let o = InstallOrigin::record(&e, &spec, "aleph-hub_x", 1_700_000_000);
        upsert_origin(&conn, &o).unwrap();
        assert_eq!(all_origins(&conn).unwrap(), vec![o.clone()]);
        assert_eq!(delete_origin(&conn, "aleph-hub:x").unwrap(), 1);
        assert!(all_origins(&conn).unwrap().is_empty());
    }

    #[test]
    fn reinstall_overwrites_by_entry_id() {
        let conn = Connection::open_in_memory().unwrap();
        init_origin_schema(&conn).unwrap();
        let e1 = entry("aleph-hub:x", Some("1.0.0"), stdio(&["@x/y"]));
        let s1 = e1.install_spec.clone().unwrap();
        upsert_origin(&conn, &InstallOrigin::record(&e1, &s1, "r", 1)).unwrap();
        let e2 = entry("aleph-hub:x", Some("2.0.0"), stdio(&["@x/y"]));
        let s2 = e2.install_spec.clone().unwrap();
        upsert_origin(&conn, &InstallOrigin::record(&e2, &s2, "r", 2)).unwrap();
        let all = all_origins(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].version.as_deref(), Some("2.0.0"));
    }

    #[test]
    fn update_available_on_version_bump() {
        let installed = entry("x", Some("1.0.0"), stdio(&["@x/y"]));
        let spec = installed.install_spec.clone().unwrap();
        let o = InstallOrigin::record(&installed, &spec, "r", 0);
        // Same version, same spec → nothing to update.
        assert!(!update_available(&o, &installed));
        // Version moved.
        let bumped = entry("x", Some("1.1.0"), stdio(&["@x/y"]));
        assert!(update_available(&o, &bumped));
    }

    #[test]
    fn update_available_on_spec_change_without_version() {
        // MCP presets carry no version; a changed command must still surface.
        let installed = entry("x", None, stdio(&["@x/y"]));
        let spec = installed.install_spec.clone().unwrap();
        let o = InstallOrigin::record(&installed, &spec, "r", 0);
        assert!(!update_available(&o, &installed));
        let respecced = entry("x", None, stdio(&["@x/y", "--new-flag"]));
        assert!(update_available(&o, &respecced));
    }

    #[test]
    fn forget_installed_matches_mcp_id_and_install_dir_leaf() {
        let conn = Connection::open_in_memory().unwrap();
        init_origin_schema(&conn).unwrap();

        let mcp = entry("aleph-hub:gh", Some("1"), stdio(&["@gh"]));
        let spec = mcp.install_spec.clone().unwrap();
        upsert_origin(
            &conn,
            &InstallOrigin::record(&mcp, &spec, "aleph-hub_gh", 0),
        )
        .unwrap();

        let mut skill = entry("aleph-hub:pdf", Some("1"), stdio(&["x"]));
        skill.kind = ExtensionKind::Skill;
        let sspec = skill.install_spec.clone().unwrap();
        upsert_origin(
            &conn,
            &InstallOrigin::record(&skill, &sspec, "/home/u/.aleph/skills/pdf-tools", 0),
        )
        .unwrap();

        // Wrong kind never matches, even on an identical backend string.
        assert_eq!(
            forget_installed(&conn, ExtensionKind::Plugin, "aleph-hub_gh").unwrap(),
            0
        );
        // MCP: the server id is stored verbatim.
        assert_eq!(
            forget_installed(&conn, ExtensionKind::Mcp, "aleph-hub_gh").unwrap(),
            1
        );
        // Skill: addressed by the install directory's trailing segment.
        assert_eq!(
            forget_installed(&conn, ExtensionKind::Skill, "pdf-tools").unwrap(),
            1
        );
        assert!(all_origins(&conn).unwrap().is_empty());
    }

    /// No evidence → no claim. An entry the catalog can no longer describe must
    /// not light the update badge.
    #[test]
    fn update_unavailable_without_a_catalog_spec() {
        let installed = entry("x", Some("1.0.0"), stdio(&["@x/y"]));
        let spec = installed.install_spec.clone().unwrap();
        let o = InstallOrigin::record(&installed, &spec, "r", 0);
        let mut specless = installed.clone();
        specless.install_spec = None;
        assert!(!update_available(&o, &specless));
    }
}
