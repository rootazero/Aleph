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
    /// True when the field was *removed from the schema on purpose* (i.e. an
    /// operator's old TOML still parses but the knob no longer exists).
    /// These are surfaced at info! in the load summary so the operator
    /// sees the line and can remove it; non-retired tolerated keys stay at
    /// debug! because they are legitimately read elsewhere.
    retired: bool,
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
        retired: false,
    },
    Tolerated {
        path: "security.ssrf",
        why: "read by Config::apply_security_ssrf_overrides (src/config/load.rs), a raw-TOML \
              bridge — `ShellSecurityConfig` has no `ssrf` field, so serde ignores what the \
              bridge honours",
        retired: false,
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
        retired: true,
    },
    Tolerated {
        path: "cowork",
        why: "the whole [cowork] section was retired in the 2026-08-17 wire audit (config-007); \
              reported at the root because serde no longer descends into it",
        retired: true,
    },
    Tolerated {
        path: "profiles.*.cache_strategy",
        why: "retired — prompt caching is decided by the protocol adapters, there is no dial; \
              see src/config/types/profile.rs",
        retired: true,
    },
    Tolerated {
        path: "profiles.*.system_prompt",
        why: "retired — AGENTS.md is the persona overlay; see src/config/types/profile.rs",
        retired: true,
    },
    Tolerated {
        path: "agents.*.system_prompt",
        why: "retired — boot-time system_prompt on AgentInstanceConfig had zero production \
              readers; real injection is via SoulLayer/ProfileLayer/IdentityFilesLayer reading \
              SOUL.md/AGENTS.md/IDENTITY.md each turn. See \
              src/gateway/agent_instance.rs::from_resolved.",
        retired: true,
    },
    Tolerated {
        path: "profiles.*.tools",
        why: "retired — the live tool gate is AgentInstanceConfig.tool_whitelist (sourced from \
              agent.skills); see src/config/types/profile.rs",
        retired: true,
    },
    Tolerated {
        path: "desktop.presence",
        why: "reporter removed 2026-08-09; see the module doc of src/config/types/desktop.rs",
        retired: true,
    },
    Tolerated {
        path: "desktop.mic_level",
        why: "reporter removed 2026-08-09; see the module doc of src/config/types/desktop.rs",
        retired: true,
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
        retired: true,
    },
    Tolerated {
        path: "memory.compound_ingest.failure_cooldown_seconds",
        why: "retired 2026-08-23 — a failed ingest defers its raw rows for RETRY_GRACE_SECS               (src/memory/compression/service.rs); this knob never reached anything",
        retired: true,
    },
    // Another whole-section retirement, so the same serde-root rule as
    // `[agent]` / `[cowork]` applies: the path here must be `secret_providers`,
    // not `secret_providers.*.account` — `Config` no longer has the field, so
    // serde reports the root and never descends.
    Tolerated {
        path: "secret_providers",
        why: "the whole [secret_providers] table was retired in the 2026-09-05 audit pass \
               (secrets I-3): the SecretProvider trait never grew a `get_secret`, so a configured \
               external provider could never resolve a single secret. Secrets resolve only through \
               the built-in local vault (src/secrets/vault_resolver.rs)",
        retired: true,
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
        match tolerated_entry(&path) {
            Some(entry) if entry.retired => {
                // Surface retired keys at info! so the operator sees the
                // line in the load summary and can remove it from their TOML.
                // Non-retired tolerated keys (foreign-owned sections) stay at
                // debug! because they are legitimately read elsewhere.
                tracing::info!(
                    key = %path,
                    reason = entry.why,
                    "Config key was retired; remove it from your TOML"
                );
            }
            Some(entry) => {
                debug!(key = %path, reason = entry.why, "Config key is ignored on purpose");
            }
            None => {
                dead.push(path);
            }
        }
    })?;

    // `providers`, `profiles` and `channels` are `HashMap`s, so the callback
    // fires in iteration order. Sort so that the warning block and the doctor
    // finding read the same on every run.
    dead.sort();
    dead.dedup();
    Ok((value, dead))
}

/// Every tolerated path with its `retired` flag, as `(path, retired)`.
///
/// Exposed to `config::tests::skill_doc_drift`, which derives "which top-level
/// sections may the bundled `self` skill legitimately document" from this list
/// unioned with `Config`'s schema. `Config` has no `gateway` field by design, so
/// that guard cannot get the answer from serde alone — and a hand-copied list of
/// foreign-owned sections over there would be a second spelling of this one,
/// which is the failure mode the guard exists to catch.
#[cfg(test)]
pub(super) fn tolerated_roots() -> Vec<(&'static str, bool)> {
    TOLERATED.iter().map(|t| (t.path, t.retired)).collect()
}

/// The matched tolerated entry for `path`, or `None` when nothing reads it.
fn tolerated_entry(path: &str) -> Option<&'static Tolerated> {
    TOLERATED.iter().find(|entry| covers(entry.path, path))
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

    /// A retired *section* has to be listed at its serde root, and only the
    /// real `Config` can show that: what makes the entry correct is that
    /// `Config` no longer declares the field, which the `Root` fixture above
    /// cannot model. Scanning the fixture would only re-prove `covers`.
    ///
    /// `[secret_providers]` is the 2026-09-05 case. Two things are asserted
    /// together because either alone is a false green: the table must still
    /// **parse** (`Config` has no `deny_unknown_fields`, so an operator
    /// upgrading past the retirement keeps booting), and it must be reported
    /// as *retired* rather than *dead* — otherwise he gets a bare "config key
    /// reaches no code" warning and a `core/config-parse` Warning finding with
    /// no reason attached to it.
    #[test]
    fn the_retired_secret_providers_table_parses_and_is_not_reported_dead() {
        let (_config, dead) = deserialize_reporting_dead_keys::<crate::config::Config>(
            "[secret_providers.op]\ntype = \"1password\"\naccount = \"acme\"\n",
        )
        .expect("a retired table must still parse: Config has no deny_unknown_fields");

        assert!(
            dead.is_empty(),
            "[secret_providers] must be reported as retired, not dead: {dead:?}"
        );
        let entry = tolerated_entry("secret_providers").expect("secret_providers is tolerated");
        assert!(
            entry.retired,
            "the table is inert, so it must be flagged retired (info!), not foreign-owned (debug!)"
        );
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

    /// Final-review I1 (`31c963e3b..`): six operator-facing sentences named
    /// `[browser.runtime]`, a section `Config` has never had — the real path
    /// is `[general.browser.runtime]` (`GeneralConfig::browser` is not
    /// `#[serde(flatten)]`, so there is no top-level `browser` table). Because
    /// `Config` does not `deny_unknown_fields`, the wrong sentence parses,
    /// saves, and reaches nothing: an operator who follows it exactly gets a
    /// config that keeps failing the same check it was written to fix.
    ///
    /// Pinned here rather than as a fresh string comparison (判据 §10 — that
    /// would only prove two literals agree with each other, not that either
    /// names something real): every bracketed, `browser`-mentioning path
    /// found in these four files is deserialized as a real `Config` fragment
    /// through THIS module's own dead-key scanner, the same one
    /// `core/config-parse` uses to answer "is this key actually read" for an
    /// operator's real file. A path that reaches no field reports itself as
    /// dead here exactly the way it would in a live config.
    fn browser_paths_named_in_operator_facing_text() -> Vec<(&'static str, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut found = Vec::new();
        for rel in [
            "src/browser/error.rs",
            "src/browser/chromium_resolve.rs",
            "src/diagnostics/checks/chromium_missing.rs",
            "src/builtin_tools/runtime_manage.rs",
        ] {
            let text = std::fs::read_to_string(root.join(rel))
                .unwrap_or_else(|e| panic!("read {rel}: {e}"));
            let bytes = text.as_bytes();
            let mut i = 0usize;
            // NOT "find the next `]` anywhere after this `[`" — these files'
            // `#[error(...)]` attributes wrap multi-line strings that
            // themselves contain the target `[general.browser.runtime]`, so
            // the attribute's own opening `[` would greedily pair with the
            // FIRST `]` it finds, which is the section path's closing
            // bracket, not the attribute's — silently swallowing the real
            // site into a giant, filtered-out candidate (measured: this is
            // exactly why the first version of this scan found 6 sites, not
            // 7, and missed `error.rs` entirely). Instead: a `[` only starts
            // a candidate if it is IMMEDIATELY followed by a contiguous run
            // of lowercase/`_`/`.` bytes that ends in `]`, checked without
            // ever searching past unrelated brackets.
            while let Some(off) = text[i..].find('[') {
                let start = i + off;
                let mut j = start + 1;
                while j < bytes.len()
                    && (bytes[j].is_ascii_lowercase() || bytes[j] == b'_' || bytes[j] == b'.')
                {
                    j += 1;
                }
                if j > start + 1 && bytes.get(j) == Some(&b']') {
                    let candidate = &text[start + 1..j];
                    if candidate.contains("browser") {
                        found.push((rel, candidate.to_string()));
                    }
                }
                i = start + 1;
            }
        }
        found
    }

    #[test]
    fn every_operator_facing_browser_config_path_is_actually_read() {
        let found = browser_paths_named_in_operator_facing_text();
        assert!(
            found.len() >= 7,
            "derived only {} operator-facing browser config paths across the \
             four known sites; 7 were measured on 2026-09-06 (one file states \
             the section twice). A scan that stopped matching makes this \
             guard pass by finding nothing: {found:?}",
            found.len()
        );
        let mut wrong: Vec<String> = Vec::new();
        for (rel, path) in &found {
            let toml = format!("[{path}]\nbinary_path = \"/nonexistent\"\ndownload_host = \"https://example.invalid\"\n");
            let (_config, dead): (crate::config::Config, Vec<String>) =
                deserialize_reporting_dead_keys(&toml).unwrap_or_else(|e| {
                    panic!("{rel} names {path:?}, which is not even valid TOML syntax: {e}")
                });
            if !dead.is_empty() {
                wrong.push(format!("{rel} says [{path}] — reaches nothing: {dead:?}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "these operator-facing sentences name a config path Config does \
             not read (parses, saves, fixes nothing):\n  {}",
            wrong.join("\n  ")
        );
    }
}
