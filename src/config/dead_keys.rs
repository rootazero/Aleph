//! Which `config.toml` keys parse but reach no code.
//!
//! [`Config`](crate::config::Config) deliberately does not
//! `deny_unknown_fields`: an existing file carrying a retired key must still
//! boot the daemon, and other subsystems parse their own sections out of this
//! same file. The cost of that stance is that a **misplaced** key is
//! byte-for-byte indistinguishable from a working one — `[browser]` instead of
//! `[general.browser]` parses, saves, and does nothing — and `doctor` answered
//! "config parses" to the question the operator was actually asking, which is
//! "is my config in effect?".
//!
//! This module answers the second question without weakening the first. The
//! scan **never rejects**; it only names the ignored paths, so that
//! `Config::load_from_file` can warn and the `core/config-parse` check can
//! raise a Warning. Both surfaces call this one scanner: two hand-written
//! copies of "what counts as dead" would drift, and the copy that drifted
//! would be the one telling the operator his config is fine.
//!
//! # Out of scope
//!
//! * **Channel table interiors.** `Config.channels` is
//!   `HashMap<String, serde_json::Value>`, so every key under `[channels.<id>]`
//!   is accepted as opaque JSON and serde has nothing to ignore. Catching a
//!   misplaced per-channel policy key needs a per-channel schema — a separate
//!   gap, not something this seam can see.
//! * **Wrong-shape sections.** `[[channels]]` where a table is expected is a
//!   hard parse error, not an ignored key; `core/config-parse` already reports
//!   that as an Error.
//!
//! # Not the same list as `reload_impact::INERT_SECTIONS`
//!
//! That one is hand-maintained and answers a different question — "if the
//! model writes *to* this path, will anything happen?" — for one section it
//! happens to know about. This scan answers "what is in the operator's file
//! that reaches nothing", mechanically, and must therefore keep reporting the
//! sections that list names: they are dead, which is precisely what the
//! operator wants told.
//!
//! # Legacy `[agents.<id>]` is deliberately not tolerated
//!
//! `GatewayConfig` still accepts that shape (`deserialize_agents_compat`), so
//! it is technically read — but its only consumers are a boot banner and a
//! hot-reload log line; agents are configured by `[[agents.list]]`. Tolerating
//! `agents.*` to spare it would blind the scan to every typo under
//! `[[agents.list]]`, which is the section that does the work.

use tracing::debug;

/// A key path the config schema ignores, and why that is not a defect.
///
/// An allowlist without a stated reason rots into a licence — the next reader
/// cannot tell an entry that is still earning its place from one whose reason
/// died two releases ago. Every entry here carries the reason and, for the
/// foreign-owned ones, names the symbol that reads the section; the census test
/// below fails when that symbol disappears.
struct Tolerated {
    /// Dotted key path. A `*` segment matches any single map key. An entry
    /// covers the path itself and everything nested under it, so `gateway`
    /// also tolerates `gateway.auth.mode`.
    path: &'static str,
    /// Why this path is not dead.
    why: &'static str,
}

/// Paths that are ignored by `Config`'s schema on purpose.
///
/// Two categories, and the distinction matters: a *foreign-owned* path is
/// live config that a different parser reads out of the same file — reporting
/// it would cry wolf on every configured deployment. A *retired* path really
/// is inert, and is listed only so that an old file does not produce a warning
/// the operator cannot act on beyond deleting a line.
const TOLERATED: &[Tolerated] = &[
    // ---- Foreign-owned: another parser reads this section from the same file.
    Tolerated {
        path: "gateway",
        why: "read by GatewayConfig::load_default (src/gateway/config.rs) out of this same file; \
              `Config` has no `gateway` field by design",
    },
    Tolerated {
        path: "security.ssrf",
        why: "read by Config::apply_security_ssrf_overrides (src/config/load.rs), a raw-TOML \
              bridge — `ShellSecurityConfig` has no `ssrf` field, so serde ignores what the \
              bridge honours",
    },
    // ---- Retired: knobs removed with their config kept parsing on purpose.
    // `[agent]` and `[cowork]` were removed entirely in 2026-08-17 wire audit
    // (config-002 + config-007). Both spellings are now just unknown top-level
    // sections, so `serde_ignored` reports them AT THE SECTION ROOT (`agent`,
    // not `agent.subagents`) — serde never descends into a section the schema
    // no longer has. The tolerated paths must therefore be the section roots;
    // the per-key entries this replaces could never match, which is exactly
    // why `a_retired_key_is_not_reported_dead` was red on main.
    Tolerated {
        path: "agent",
        why: "the whole [agent] section was retired in the 2026-08-17 wire audit (config-002); \
              reported at the root because serde no longer descends into it",
    },
    Tolerated {
        path: "cowork",
        why: "the whole [cowork] section was retired in the 2026-08-17 wire audit (config-007); \
              reported at the root because serde no longer descends into it",
    },
    Tolerated {
        path: "profiles.*.cache_strategy",
        why: "retired — prompt caching is decided by the protocol adapters, there is no dial; \
              see src/config/types/profile.rs",
    },
    Tolerated {
        path: "profiles.*.system_prompt",
        why: "retired — AGENTS.md is the persona overlay; see src/config/types/profile.rs",
    },
    Tolerated {
        path: "agents.*.system_prompt",
        why: "retired — boot-time system_prompt on AgentInstanceConfig had zero production \
              readers; real injection is via SoulLayer/ProfileLayer/IdentityFilesLayer reading \
              SOUL.md/AGENTS.md/IDENTITY.md each turn. See \
              src/gateway/agent_instance.rs::from_resolved.",
    },
    Tolerated {
        path: "profiles.*.tools",
        why: "retired — the live tool gate is AgentInstanceConfig.tool_whitelist (sourced from \
              agent.skills); see src/config/types/profile.rs",
    },
    Tolerated {
        path: "desktop.presence",
        why: "reporter removed 2026-08-09; see the module doc of src/config/types/desktop.rs",
    },
    Tolerated {
        path: "desktop.mic_level",
        why: "reporter removed 2026-08-09; see the module doc of src/config/types/desktop.rs",
    },
    // These two shipped in `CompoundIngestConfig` from the day the section was
    // written and were read by nothing for their whole life. Note what that
    // means for this scanner: a key the schema *declares* parses, so
    // `serde_ignored` never saw them — "parses but reaches no code" is a
    // different question from "parses and is discarded", and only the second
    // one is mechanically visible here. Removing the fields moves them into
    // this scanner's reach, and these entries keep an existing file from
    // warning about a line we shipped.
    Tolerated {
        path: "memory.compound_ingest.replan_on_hash_conflict",
        why: "retired 2026-08-23 — the hash-conflict replan is exactly one attempt, decided in               src/memory/notes/ingest/ingestor/batch.rs; this knob never reached it",
    },
    Tolerated {
        path: "memory.compound_ingest.failure_cooldown_seconds",
        why: "retired 2026-08-23 — a failed ingest defers its raw rows for RETRY_GRACE_SECS               (src/memory/compression/service.rs); this knob never reached anything",
    },
];

/// Deserialize `contents`, returning the value alongside the dotted key paths
/// that parsed and were then discarded.
///
/// Never fails for an ignored key — only for TOML that does not parse or does
/// not fit the schema, which is `Config::load_from_file`'s existing error.
///
/// Generic over the root only so the tests can exercise the scanner against a
/// small struct; the sole production caller passes `Config`.
pub(crate) fn deserialize_reporting_dead_keys<T>(
    contents: &str,
) -> std::result::Result<(T, Vec<String>), toml::de::Error>
where
    T: serde::de::DeserializeOwned,
{
    let mut dead = Vec::new();
    let value = serde_ignored::deserialize(toml::Deserializer::new(contents), |path| {
        let path = render(&path);
        if path.is_empty() {
            return;
        }
        if let Some(why) = tolerated_reason(&path) {
            debug!(key = %path, reason = why, "Config key is ignored on purpose");
        } else {
            dead.push(path);
        }
    })?;

    // `providers`, `profiles` and `channels` are `HashMap`s, so the callback
    // fires in iteration order. Sort so that the warning block and the doctor
    // finding read the same on every run.
    dead.sort();
    dead.dedup();
    Ok((value, dead))
}

/// The reason `path` is tolerated, or `None` when nothing reads it.
fn tolerated_reason(path: &str) -> Option<&'static str> {
    TOLERATED
        .iter()
        .find(|entry| covers(entry.path, path))
        .map(|entry| entry.why)
}

/// Does `pattern` cover `path` — same segments, `*` matching any one of them,
/// and a shorter pattern covering everything nested beneath it?
///
/// Segment-wise rather than a string prefix: `gateway` must cover
/// `gateway.auth.mode` without also covering a top-level `gateway_extra`.
fn covers(pattern: &str, path: &str) -> bool {
    let mut actual = path.split('.');
    pattern
        .split('.')
        .all(|want| matches!(actual.next(), Some(got) if want == "*" || want == got))
}

/// Render a `serde_ignored` path as the dotted key the operator typed.
///
/// `Path`'s own `Display` emits a `?` segment for each `Option` it descended
/// through, so an unknown key under an optional section arrives as
/// `search.?.foo`. Those segments are an artefact of the schema's shape, not
/// something in the file, and a path the operator cannot find in his own
/// config is worse than no report at all.
fn render(path: &serde_ignored::Path<'_>) -> String {
    path.to_string()
        .split('.')
        .filter(|segment| !segment.is_empty() && *segment != "?")
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, Deserialize)]
    struct Inner {
        #[serde(default)]
        kept: String,
    }

    #[derive(Debug, Deserialize)]
    struct Root {
        #[serde(default)]
        inner: Inner,
        #[serde(default)]
        optional: Option<Inner>,
        #[serde(default)]
        list: Vec<Inner>,
        #[serde(default)]
        map: std::collections::HashMap<String, Inner>,
    }

    fn scan(contents: &str) -> Vec<String> {
        deserialize_reporting_dead_keys::<Root>(contents)
            .expect("fixture parses")
            .1
    }

    /// The scan reports *alongside* the value; it must not cost the caller the
    /// deserialization it wrapped. `Config::load_from_file` depends on this
    /// half as much as on the report.
    #[test]
    fn the_value_is_still_deserialized_next_to_the_report() {
        let (root, dead) = deserialize_reporting_dead_keys::<Root>(
            "[inner]\nkept = \"a\"\n\n[optional]\nkept = \"b\"\n\n[[list]]\nkept = \"c\"\n\n\
             [map.named]\nkept = \"d\"\nstray = 1\n",
        )
        .expect("fixture parses");

        assert_eq!(root.inner.kept, "a");
        assert_eq!(root.optional.expect("optional present").kept, "b");
        assert_eq!(root.list.len(), 1);
        assert_eq!(root.list[0].kept, "c");
        assert_eq!(root.map["named"].kept, "d");
        assert_eq!(dead, vec!["map.named.stray".to_string()]);
    }

    #[test]
    fn a_top_level_key_nothing_reads_is_reported() {
        assert_eq!(scan("stray = 1"), vec!["stray".to_string()]);
    }

    #[test]
    fn a_nested_key_is_reported_with_its_full_path() {
        assert_eq!(
            scan("[inner]\nkept = \"a\"\nstray = 1\n"),
            vec!["inner.stray".to_string()]
        );
    }

    #[test]
    fn a_key_under_an_optional_section_does_not_report_the_option_marker() {
        // Without `render`'s filter this is `optional.?.stray`, a path the
        // operator cannot find anywhere in his file.
        assert_eq!(
            scan("[optional]\nstray = 1\n"),
            vec!["optional.stray".to_string()]
        );
    }

    #[test]
    fn sequence_and_map_entries_keep_their_index_and_key() {
        let dead = scan("[[list]]\nstray = 1\n\n[map.named]\nother = 2\n");
        assert_eq!(
            dead,
            vec!["list.0.stray".to_string(), "map.named.other".to_string()]
        );
    }

    #[test]
    fn a_config_that_uses_only_known_keys_reports_nothing() {
        assert!(scan("[inner]\nkept = \"a\"\n").is_empty());
    }

    #[test]
    fn reported_paths_are_sorted_so_two_runs_read_the_same() {
        let dead = scan("[map.zebra]\nstray = 1\n\n[map.alpha]\nstray = 1\n");
        assert_eq!(
            dead,
            vec!["map.alpha.stray".to_string(), "map.zebra.stray".to_string()]
        );
    }

    #[test]
    fn covers_matches_whole_segments_only() {
        assert!(covers("gateway", "gateway"));
        assert!(covers("gateway", "gateway.auth.mode"));
        assert!(!covers("gateway", "gateway_extra"));
        assert!(!covers("gateway", "other.gateway"));
        assert!(!covers("gateway.auth", "gateway"));
    }

    #[test]
    fn covers_wildcard_spans_exactly_one_segment() {
        assert!(covers(
            "profiles.*.cache_strategy",
            "profiles.dev.cache_strategy"
        ));
        assert!(!covers(
            "profiles.*.cache_strategy",
            "profiles.cache_strategy"
        ));
        assert!(!covers(
            "profiles.*.cache_strategy",
            "profiles.a.b.cache_strategy"
        ));
    }

    #[test]
    fn every_tolerated_entry_states_a_reason() {
        for entry in TOLERATED {
            assert!(
                !entry.path.is_empty() && entry.why.len() > 20,
                "tolerated entry {:?} must say why it is tolerated",
                entry.path
            );
        }
    }

    /// The two foreign-owned entries are only correct while the reader they
    /// name still exists. Delete `apply_security_ssrf_overrides` and
    /// `security.ssrf` silently becomes a licence to ignore live config — the
    /// exact failure this allowlist is supposed to prevent, one level up.
    #[test]
    fn every_foreign_owned_entry_still_has_its_reader() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for (entry_path, source, symbol) in [
            ("gateway", "src/gateway/config.rs", "pub fn load_default("),
            (
                "security.ssrf",
                "src/config/load.rs",
                "fn apply_security_ssrf_overrides(",
            ),
        ] {
            assert!(
                TOLERATED.iter().any(|t| t.path == entry_path),
                "{entry_path} is no longer in TOLERATED; drop its census row too"
            );
            let text = std::fs::read_to_string(root.join(source))
                .unwrap_or_else(|e| panic!("read {source}: {e}"));
            assert!(
                text.contains(symbol),
                "{entry_path} is tolerated because {source} defines `{symbol}`, which is gone — \
                 either restore the reader or stop tolerating the path"
            );
        }
    }
}
