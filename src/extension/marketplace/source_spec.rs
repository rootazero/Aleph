//! Turning a marketplace **source** string into a registration.
//!
//! `plugin.marketplace.add` and `aleph-server plugin marketplace add` both
//! answer the same two questions about a source a human typed — *is this a
//! GitHub slug or a local directory?* and *what is this registration called?*
//! — and until now they answered them with two different hand-rolled
//! heuristics that disagreed on four shapes:
//!
//! | source | RPC said | subcommand said |
//! |---|---|---|
//! | `C:\dir\mk` | github, name `c:\dir\mk` | local, name `mk` |
//! | `./foo/bar` | local | **github** |
//! | `myrepo` | github | local |
//! | `/abs/My Dir` | local, name `my dir` | local, name `My Dir` |
//!
//! And on one shape they agreed and were both wrong: `~/foo` classified as
//! GitHub on both paths, even though [`super::local_source`] carries an
//! `expand_tilde` written for exactly that form — a branch only the resolver
//! supported and no producer could ever reach.
//!
//! The fix is not a third heuristic. The classifier already existed:
//! [`super::github_source::is_valid_owner_repo`] is the function that decides
//! whether the GitHub path can work at all (exactly two non-empty segments of
//! `[A-Za-z0-9_.-]`, neither `.` nor `..`). Asking *it* makes the answer here
//! and the outcome downstream the same answer, instead of two guesses that
//! have to be kept in step.
//!
//! # Why URLs are normalised rather than refused
//!
//! `is_valid_owner_repo` rejects `https://github.com/o/r`, so a URL would
//! classify as Local and die at sync time with "Local marketplace path does
//! not exist: https://…" — a *more* misleading message than the one it
//! replaced. Pasting the browser URL is the likeliest thing an operator does,
//! so the four GitHub URL spellings collapse to the `owner/repo` slug they
//! denote. This is a storage-vs-display split: what is stored is the canonical
//! form the fetcher understands, and it is stored once, here.

use super::github_source::is_valid_owner_repo;
use super::names::reject_unsafe_segment;
use super::types::MarketplaceSourceType;

/// A source string, resolved into everything a registration needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceSpec {
    /// Registration key — also the cache directory name for GitHub sources.
    pub name: String,
    /// The source as it will be stored: canonicalised for GitHub, verbatim
    /// for local paths.
    pub source: String,
    /// Which fetcher [`super::MarketplaceManager::update`] will use.
    pub source_type: MarketplaceSourceType,
}

/// Resolve `raw_source` (and an optional caller-supplied name) into a
/// registration.
///
/// `explicit_name` is taken verbatim when present: the caller said what they
/// want it called, and only *derived* names get a house style applied. Both
/// are validated the same way, because both end up joined onto the cache root.
///
/// # Errors
/// The source is blank, or the resulting name could escape the cache
/// directory (`/`, `\`, `..`).
pub fn classify(raw_source: &str, explicit_name: Option<&str>) -> Result<MarketplaceSpec, String> {
    let trimmed = raw_source.trim();
    if trimmed.is_empty() {
        return Err(
            "A marketplace source is required: an `owner/repo` slug, a GitHub URL, or a local \
             directory path."
                .to_string(),
        );
    }

    let source = canonical_github_slug(trimmed).unwrap_or_else(|| trimmed.to_string());
    let source_type = if is_valid_owner_repo(&source) {
        MarketplaceSourceType::Github
    } else {
        MarketplaceSourceType::Local
    };

    let name = match explicit_name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => derive_name(&source),
    };
    reject_unsafe_segment("marketplace name", &name)?;

    Ok(MarketplaceSpec {
        name,
        source,
        source_type,
    })
}

/// The last path-ish component, lowercased.
///
/// Splits on **both** separators rather than going through `Path::file_name`:
/// on a Unix host `Path::new(r"C:\dir\mk").file_name()` is the whole string,
/// because `\` is not a separator there — which is how `C:\dir\mk` came to be
/// registered under the name `c:\dir\mk`. What the operator meant by the last
/// component does not depend on which host parses it.
fn derive_name(source: &str) -> String {
    source
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(source)
        .to_lowercase()
}

/// Collapse the GitHub URL spellings to the `owner/repo` slug they denote.
///
/// Returns `None` for anything that is not a GitHub URL, and also for a GitHub
/// URL whose path is not a plain `owner/repo` (a tree/blob deep link, say) —
/// inventing a slug from those would fetch the wrong thing, and falling
/// through means the caller reports it as an unusable source instead.
fn canonical_github_slug(source: &str) -> Option<String> {
    let rest = source
        .strip_prefix("https://github.com/")
        .or_else(|| source.strip_prefix("http://github.com/"))
        .or_else(|| source.strip_prefix("ssh://git@github.com/"))
        .or_else(|| source.strip_prefix("git@github.com:"))?;

    let slug = rest.trim_end_matches('/');
    let slug = slug.strip_suffix(".git").unwrap_or(slug);

    is_valid_owner_repo(slug).then(|| slug.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(source: &str) -> MarketplaceSpec {
        classify(source, None).unwrap_or_else(|e| panic!("{source:?} should classify: {e}"))
    }

    /// The four shapes the two old derivations disagreed on. Each row is a
    /// case where one of them was wrong; the point of the table is that there
    /// is now only one answer to be wrong or right.
    #[test]
    fn the_shapes_the_two_derivations_disagreed_on_have_one_answer() {
        // Windows absolute path: the RPC called this GitHub and named it
        // `c:\dir\mk`, which `sync_github_marketplace` would then refuse.
        let win = spec(r"C:\dir\mk");
        assert_eq!(win.source_type, MarketplaceSourceType::Local);
        assert_eq!(win.name, "mk");

        // Relative path: the subcommand called this GitHub because it contains
        // a slash and does not start with one.
        let rel = spec("./foo/bar");
        assert_eq!(rel.source_type, MarketplaceSourceType::Local);
        assert_eq!(rel.name, "bar");

        // A bare word is not `owner/repo`, so it cannot be cloned; the
        // subcommand was right and the RPC was not.
        assert_eq!(spec("myrepo").source_type, MarketplaceSourceType::Local);

        // Derived names get one case convention, not one per surface.
        assert_eq!(spec("/abs/My Dir").name, "my dir");
    }

    /// `expand_tilde` exists in `local_source` for this form and nothing could
    /// reach it, because both derivations classified `~/…` as GitHub.
    #[test]
    fn a_tilde_path_reaches_the_local_resolver_that_was_written_for_it() {
        let s = spec("~/markets/mine");
        assert_eq!(s.source_type, MarketplaceSourceType::Local);
        assert_eq!(s.name, "mine");
    }

    #[test]
    fn an_owner_repo_slug_is_github_and_names_itself_after_the_repo() {
        let s = spec("rootazero/Aleph-plugins");
        assert_eq!(s.source_type, MarketplaceSourceType::Github);
        assert_eq!(s.name, "aleph-plugins");
        assert_eq!(s.source, "rootazero/Aleph-plugins");
    }

    /// Classification asks the fetcher's own predicate, so "classified GitHub"
    /// and "clonable" cannot drift apart.
    #[test]
    fn nothing_is_classified_github_that_the_fetcher_would_refuse() {
        for source in [
            r"C:\dir\mk",
            "./foo/bar",
            "myrepo",
            "/abs/dir",
            "~/markets/mine",
            "https://github.com/o/r/tree/main/sub",
            "a/b/c",
            "../escape",
        ] {
            let s = spec(source);
            if matches!(s.source_type, MarketplaceSourceType::Github) {
                assert!(
                    is_valid_owner_repo(&s.source),
                    "{source:?} was called GitHub but the fetcher would refuse {:?}",
                    s.source
                );
            }
        }
    }

    #[test]
    fn every_github_url_spelling_collapses_to_the_slug_it_denotes() {
        for url in [
            "https://github.com/o/r",
            "https://github.com/o/r.git",
            "https://github.com/o/r/",
            "http://github.com/o/r",
            "git@github.com:o/r.git",
            "ssh://git@github.com/o/r",
        ] {
            let s = spec(url);
            assert_eq!(s.source, "o/r", "{url} did not canonicalise");
            assert_eq!(s.source_type, MarketplaceSourceType::Github, "{url}");
            assert_eq!(s.name, "r", "{url}");
        }
    }

    /// A deep link names a subtree, not a repo. Guessing `o/r` from it would
    /// fetch something the operator did not ask for, so it stays unclassified
    /// and falls through to the local branch, where it fails by name.
    #[test]
    fn a_github_deep_link_is_not_guessed_into_a_slug() {
        assert_eq!(
            spec("https://github.com/o/r/tree/main/sub").source_type,
            MarketplaceSourceType::Local
        );
    }

    #[test]
    fn an_explicit_name_is_taken_verbatim_but_still_validated() {
        let s = classify("o/r", Some("My Market")).unwrap();
        assert_eq!(s.name, "My Market", "an explicit name is not restyled");

        let err = classify("o/r", Some("../escape")).unwrap_err();
        assert!(err.contains("marketplace name"), "got {err}");
    }

    /// The old RPC stored anything at all and only failed at sync time, so a
    /// config file could hold a registration that could never resolve.
    #[test]
    fn a_source_that_yields_an_unusable_name_is_refused_at_the_boundary() {
        assert!(classify("   ", None).is_err(), "blank source");
        assert!(classify("/", None).is_err(), "root yields an empty name");
        assert!(classify("..", None).is_err(), "traversal");
    }
}
