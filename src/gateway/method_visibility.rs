//! Durable registry + regression net for per-user session visibility (P1
//! data isolation, spec §5.4).
//!
//! Sibling of `method_admin.rs` — same shape, different question.
//! `method_admin.rs` asks "does this method require operator privilege";
//! this file asks "does this method's response depend on WHO is asking, and
//! if so, is that dependency enforced." **This registry is NOT a dispatch
//! gate** — nothing here intercepts a request. Filtering a `sessions.list`
//! response or denying a foreign `session_key` needs per-method data access
//! (the session's own `owner_user_id`), which a generic method-name gate
//! cannot supply. Enforcement lives in each handler (`gateway::visibility`'s
//! predicates, applied at the site — see `session/db_handlers/{query,
//! modify}.rs` and `handlers/chat.rs`). This table exists so a REMOVED or
//! never-added enforcement call is a named test failure, not a silent gap —
//! the same audit-trail role `ADMIN_PREFIXES`/`MEMBER_CARVE_OUTS` play for
//! the admin gate.
//!
//! ## Scope of this table (read before extending it)
//!
//! Task 6 (this file's origin) owns the `sessions.*`/`session.*`/`chat.*`
//! family. Task 7 of the same plan
//! (`docs/superpowers/plans/2026-08-05-p1-data-isolation.md`) owns
//! `memory.*`, `artifacts.*`, `clarification.*`, `subagent.tree`, and
//! `graph.query` — those methods are **not yet enforced** (recon confirmed
//! zero ownership checks on any of them at the time of writing) and are
//! deliberately **absent** from `SCOPED_METHODS` rather than added with a
//! `Treatment` that would misrepresent them as covered. Task 7 registers
//! them here when it lands.
//!
//! ## Enumeration evidence
//!
//! Every method below was found by a mechanical sweep of the same four
//! registration patterns `method_admin.rs` swept for the 74-family admin
//! table: `register_handler!` (files under
//! `src/bin/aleph-server/commands/start/builder/handlers/`), direct
//! `registry.register(...)` placeholders (`src/gateway/handlers/mod.rs`),
//! `server.handlers_mut().register(...)` inline in `agent_init/mod.rs` and
//! `agent_init/common_handlers.rs`, and `commands/start/mod.rs`'s inline
//! registrations — filtered to methods whose handler reads a caller-supplied
//! or caller-scoped `session_key` (or, for `sessions.list`, filters by
//! agent/owner). Read (not guessed) per method:
//!
//! - `sessions.list` → `handle_list_db` sets `SessionFilter.owner_visible_to`
//!   from `visibility::visible_owner_filter()` — **ListFiltered**.
//! - `sessions.history`, `sessions.preview`, `session.usage`,
//!   `sessions.delete`, `sessions.reset` → each resolves a caller-supplied
//!   `session_key` to `SessionMetadata` and calls `visibility::
//!   session_visible` before touching the store — **KeyChecked**.
//! - `chat.send` → session resolution goes through
//!   `visibility::existing_session_is_visible` before the run starts (a
//!   session that doesn't exist yet is not a denial — see that fn's doc);
//!   registered here as **KeyChecked** since the caller-supplied
//!   `session_key`, when it already exists, is checked exactly like the
//!   addressed-key sites above.
//! - `chat.abort`, `chat.history`, `chat.clear`, `chat.rewind` →
//!   `KeyChecked`, same pattern (`chat.abort`'s `session_key` is optional;
//!   absent it does nothing session-scoped, present it is checked).
//! - `session.create`, `sessions.new` → creation/epoch-bump surfaces, not
//!   enumerated by this table (see "Known gaps" below — `sessions.new`
//!   mutates an addressed EXISTING session and is a real gap, just not one
//!   this table can claim `KeyChecked` for since it isn't enforced yet).
//!
//! ## Known gaps NOT covered by this table (found during the sweep, not
//! fixed by Task 6 — flagged here exactly as `method_admin.rs` flags its own
//! `clarification.*`/`subagent.tree` follow-up)
//!
//! `sessions.new` (closes+terminates an addressed session's continuations),
//! `sessions.patch`, `sessions.set_topic`, `sessions.set_project_root`,
//! `session.compact`, `session.truncate`, `sessions.compaction.{list,
//! restore,branch}`, and `chat.context_estimate` all take a caller-supplied
//! `session_key` with no ownership check today. They were out of this
//! task's declared file list (`src/gateway/handlers/session/db_handlers/
//! {query,modify}.rs` + the two named `chat.*` handlers); left unfixed so
//! the reviewed diff stays scoped to what was asked, but recorded here as
//! the durable home for the follow-up (same convention as
//! `method_admin.rs`'s `clarification.*` note). `sessions.new` is the
//! highest-severity of these — it destructively closes a session and stops
//! its autonomous continuations by caller-supplied key alone.
//!
//! ## `OrgShared`
//!
//! No entry in this table is `OrgShared` — every session-addressed method
//! Task 6 touches is genuinely per-user (a session has exactly one owner).
//! `OrgShared` exists for Task 7's methods (e.g. `teams.*` surfaces
//! elsewhere in the codebase are org-level infrastructure — project scoping
//! arrives in P2) and is defined here so the enum is stable across both
//! tasks.

/// How a scoped-data method's response is (or should be) restricted to the
/// calling user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Treatment {
    /// A list/enumeration endpoint filters its result set by
    /// `visibility::visible_owner_filter()` (or the partition equivalent).
    ListFiltered,
    /// An addressed-key endpoint resolves the caller-supplied key and calls
    /// `visibility::session_visible` (or `existing_session_is_visible`)
    /// before doing anything else, denying with `visibility::
    /// not_found_response` on a foreign owner.
    KeyChecked,
    /// A partition-addressed endpoint (e.g. `agent_id="main__u-alice"`)
    /// checks `visibility::partition_visible` before reading/writing that
    /// partition. (Task 7.)
    #[allow(dead_code)] // no Task-6 entry uses this variant yet; Task 7 will.
    PartitionChecked,
    /// Deliberately shared across every user by design — not a gap. Carries
    /// a reason string at the call site (see `org_shared_entries_all_carry_reasons`).
    #[allow(dead_code)] // no Task-6 entry uses this variant; Task 7 populates it.
    OrgShared,
}

/// The durable pin list. One entry per method this file's module doc
/// attributes as enforced — see that doc for the sweep methodology and the
/// (recorded, not silently dropped) gaps this table does not yet cover.
pub const SCOPED_METHODS: &[(&str, Treatment)] = &[
    ("sessions.list", Treatment::ListFiltered),
    ("sessions.history", Treatment::KeyChecked),
    ("sessions.preview", Treatment::KeyChecked),
    ("session.usage", Treatment::KeyChecked),
    ("sessions.delete", Treatment::KeyChecked),
    ("sessions.reset", Treatment::KeyChecked),
    ("chat.send", Treatment::KeyChecked),
    ("chat.abort", Treatment::KeyChecked),
    ("chat.history", Treatment::KeyChecked),
    ("chat.clear", Treatment::KeyChecked),
    ("chat.rewind", Treatment::KeyChecked),
];

/// `OrgShared` entries carry a one-line reason at the point they're listed —
/// currently empty (see module doc: no Task-6 method is `OrgShared`), kept
/// as a separate const so Task 7 can extend it without touching
/// `SCOPED_METHODS`'s shape.
pub const ORG_SHARED_REASONS: &[(&str, &str)] = &[];

#[must_use]
pub fn treatment_of(method: &str) -> Option<Treatment> {
    SCOPED_METHODS
        .iter()
        .find(|(m, _)| *m == method)
        .map(|(_, t)| *t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Curated pin, not a second source of truth — same philosophy as
    /// `method_admin.rs`'s `credential_and_config_methods_require_admin`: a
    /// deletion or typo in `SCOPED_METHODS` should fail a test by name.
    #[test]
    fn every_session_addressed_method_is_registered() {
        for m in [
            "sessions.list",
            "sessions.history",
            "sessions.preview",
            "session.usage",
            "sessions.delete",
            "sessions.reset",
            "chat.send",
            "chat.abort",
            "chat.history",
            "chat.clear",
            "chat.rewind",
        ] {
            assert!(
                treatment_of(m).is_some(),
                "{m} must be registered in SCOPED_METHODS"
            );
        }
    }

    #[test]
    fn list_methods_are_list_filtered_not_key_checked() {
        assert_eq!(treatment_of("sessions.list"), Some(Treatment::ListFiltered));
    }

    #[test]
    fn addressed_key_methods_are_key_checked() {
        for m in [
            "sessions.history",
            "sessions.preview",
            "session.usage",
            "sessions.delete",
            "sessions.reset",
            "chat.send",
            "chat.abort",
            "chat.history",
            "chat.clear",
            "chat.rewind",
        ] {
            assert_eq!(treatment_of(m), Some(Treatment::KeyChecked), "{m}");
        }
    }

    #[test]
    fn unregistered_method_reads_as_none_not_a_default_treatment() {
        // No silent "assume KeyChecked" default — an unlisted method must
        // read as unclassified, not falsely covered.
        assert_eq!(treatment_of("memory.search"), None);
        assert_eq!(treatment_of("sessions.new"), None);
    }

    /// Every `OrgShared` entry must carry a one-line reason. Currently
    /// vacuously true (the table is empty — see module doc); the test stays
    /// so Task 7's additions are checked by construction, not convention.
    #[test]
    fn org_shared_entries_all_carry_reasons() {
        let org_shared: Vec<&str> = SCOPED_METHODS
            .iter()
            .filter(|(_, t)| *t == Treatment::OrgShared)
            .map(|(m, _)| *m)
            .collect();
        for m in org_shared {
            assert!(
                ORG_SHARED_REASONS
                    .iter()
                    .any(|(rm, reason)| *rm == m && !reason.is_empty()),
                "{m} is OrgShared but carries no reason in ORG_SHARED_REASONS"
            );
        }
    }

    /// Cross-check with `method_admin.rs`: every method THIS table claims
    /// must be a member-daily-open surface there (never admin-gated) — a
    /// method cannot simultaneously be "operator only" and "per-user
    /// filtered for members", and if it moved to `ADMIN_PREFIXES` this
    /// table's claim about it would be stale.
    #[test]
    fn every_scoped_method_stays_open_to_members_in_method_admin() {
        for (method, _) in SCOPED_METHODS {
            assert!(
                !crate::gateway::method_admin::method_requires_admin(method),
                "{method} is claimed by SCOPED_METHODS but method_admin.rs \
                 requires operator privilege for it — the two tables disagree"
            );
        }
    }
}
