//! The two browser processes Aleph can drive, and the machinery both need.
//!
//! `Engine` is *what runs the page*; `BrowserDriver` (`super::profile`) is
//! *how Aleph talks to it*. They are orthogonal on purpose (spec §5.4): a
//! profile is an identity — its cookies, its data directory, its policy — and
//! the engine is the means, which the model may swap under a live profile.
//!
//! What lives here is only what BOTH engines need. Anything Chromium-shaped
//! (the `DevToolsActivePort` file, `--use-mock-keychain`) stays in
//! [`chromium`]; anything about a *record* of a launched process (the sidecar
//! registry, the orphan sweep) is in [`process`], because a sweep that had to
//! know which engine wrote a record before it could read it would need a
//! second derivation of that fact (判据 §12).

pub mod chromium;
pub mod process;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Which browser process backs a profile.
///
/// The wire spelling is frozen the day it ships: it is a config value
/// (`[general.browser.profiles.<name>] engine = "obscura"`), a
/// `runtime_manage{capability}` value, and a field in every sidecar record on
/// disk. `snake_case` matches `BrowserDriver`'s existing serde
/// (`super::profile::BrowserDriver`, `profile.rs:22-30`).
///
/// **Deliberately not a variant of `BrowserType`** (`profile.rs:13-19`): that
/// enum answers "which member of the Chromium family", and obscura is not one
/// (spec §6.3). Folding them would make `browser = "obscura"` parse into a
/// value `discovery::find_chromium_preferred` would then hunt for on disk.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// The default engine (spec §0): a small, Aleph-shaped browser installed
    /// from the runtime ledger.
    #[default]
    Obscura,
    /// The escape hatch: a Chromium-family browser, supplied at runtime.
    Chromium,
}

impl Engine {
    /// Every engine, in the order tables and doctor rows render them.
    ///
    /// A named array rather than a hand-written list at each call site: a
    /// third engine must reach every enumerator by adding one entry here, not
    /// by being remembered in N places (判据 §5).
    pub const ALL: [Self; 2] = [Self::Obscura, Self::Chromium];

    /// The wire spelling — the SAME string serde writes, derived from one
    /// place so a `#[serde(rename_all)]` change cannot leave the two
    /// disagreeing.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Obscura => "obscura",
            Self::Chromium => "chromium",
        }
    }

    /// The inverse of [`Self::as_str`], exact-match only.
    ///
    /// No case folding and no trimming: the callers are a config file serde
    /// already validated and a `runtime_manage{capability}` value, and a
    /// lenient parse here would accept a spelling serde rejects, so the two
    /// front doors would disagree about the same string.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == s)
    }

    /// This engine's leaf under `~/.aleph/data/browser/`.
    #[must_use]
    pub const fn data_subdir(self) -> &'static str {
        self.as_str()
    }

    /// The argv switch that names this engine's per-profile data directory.
    ///
    /// **This is the token the orphan sweep matches on before it kills**
    /// ([`process::reap_orphans`]), so it is not cosmetic: Chromium's
    /// `--user-data-dir` and obscura's `--storage-dir` (spec §6.2) are the
    /// only evidence that a pid recorded hours ago is still the process the
    /// record meant. A shared switch would let either engine's record
    /// authorise a SIGKILL against the other's process.
    #[must_use]
    pub const fn data_dir_flag(self) -> &'static str {
        match self {
            Self::Chromium => "--user-data-dir",
            Self::Obscura => "--storage-dir",
        }
    }

    /// The engine a sidecar record with no `engine` key must be read as.
    ///
    /// A separate, *named* function rather than `Default`: the product
    /// default is obscura and the compat default is Chromium, and those are
    /// different questions with different answers. A bare `#[serde(default)]`
    /// on the sidecar field would silently pick up the product default the
    /// day someone changes it, and every pre-upgrade Chromium record would
    /// then be swept with obscura's argv switch — i.e. never matched, never
    /// reaped, forever.
    #[must_use]
    pub const fn chromium_default() -> Self {
        Self::Chromium
    }
}

impl std::fmt::Display for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::Engine;

    /// The two engines are told apart by the argv switch that names their
    /// per-profile data directory, and the sweep kills on that answer. A
    /// single shared switch would let an obscura record authorise a SIGKILL
    /// against a Chromium (and the reverse) purely because both processes
    /// happen to sit under the same profile root.
    #[test]
    fn each_engine_names_its_data_directory_with_its_own_switch() {
        assert_eq!(Engine::Chromium.data_dir_flag(), "--user-data-dir");
        assert_eq!(Engine::Obscura.data_dir_flag(), "--storage-dir");
        assert_ne!(
            Engine::Chromium.data_dir_flag(),
            Engine::Obscura.data_dir_flag()
        );
    }

    /// `as_str` / `parse` are inverses, and `parse` refuses everything else.
    /// The wire spelling is a config value (`engine = "obscura"`) and a
    /// `runtime_manage{capability}` value, so a silent acceptance of a
    /// near-miss would pick an engine the operator did not name.
    #[test]
    fn engine_parses_exactly_its_own_wire_spelling() {
        for e in Engine::ALL {
            assert_eq!(
                Engine::parse(e.as_str()),
                Some(e),
                "{} did not round-trip",
                e.as_str()
            );
            assert_eq!(e.data_subdir(), e.as_str());
        }
        for bad in [
            "",
            "Chromium",
            "OBSCURA",
            "chrome",
            "obscura ",
            "chromium\n",
        ] {
            assert_eq!(Engine::parse(bad), None, "accepted {bad:?}");
        }
    }

    /// The human spelling and the wire spelling are one string, not two.
    ///
    /// `Display` is what error texts interpolate (Task 9's `EngineMismatch`
    /// names two engines; Task 8's `UnsupportedByEngine` names two more), and
    /// `as_str` is what serde, the config file and `runtime_manage`'s
    /// `capability` value use. A hand-written `Display` that said "Obscura" or
    /// "the obscura engine" would put a spelling in front of the operator that
    /// their config file rejects (判据 §1).
    #[test]
    fn display_matches_as_str_for_every_engine() {
        for e in Engine::ALL {
            assert_eq!(
                e.to_string(),
                e.as_str(),
                "{e:?} formats differently than it parses"
            );
            // And the formatted text round-trips back through `parse`, which
            // is the property an error message the reader retypes depends on.
            assert_eq!(Engine::parse(&e.to_string()), Some(e));
        }
    }

    /// The product default is obscura (spec §6.3), and it must NOT be what
    /// an old sidecar record falls back to — those were all Chromium. Two
    /// different defaults, two different names, so neither can be reached
    /// by accident.
    #[test]
    fn the_product_default_is_obscura_and_the_sidecar_compat_default_is_chromium() {
        assert_eq!(Engine::default(), Engine::Obscura);
        assert_eq!(Engine::chromium_default(), Engine::Chromium);
    }
}
