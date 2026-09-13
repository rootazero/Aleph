//! `InstallStrategy::GithubRelease` — fetch a release asset, verify its sha256
//! against the release metadata, extract one member, make it executable.
//!
//! # Why the digest is not optional
//!
//! The end of this path is `chmod 755` on a ~90 MB binary that Aleph then
//! spawns. GitHub publishes a `digest` for every release asset, so declining
//! to check it would be choosing not to look. A release whose asset carries no
//! digest is therefore a **refusal**, not a permission: "the metadata did not
//! say" is a form of "I do not know", and the one thing an unknown may never
//! be spent as is a go-ahead (判据 §8).
//!
//! # Why the extraction is in-process
//!
//! `Command::new("tar")` would add three failure modes this code does not have
//! — the tool absent, PATH resolving a different `tar`, and an exit code that
//! cannot say which member failed — and it cannot take one member without also
//! writing the ~86 MB `obscura-worker` beside it.
//!
//! # Order is the contract
//!
//! metadata → digest → download → **verify** → extract → rename. Extracting
//! before verifying would put unverified bytes on disk under the exact name
//! [`super::probe`] already searches for.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tracing::warn;

/// Where release assets come from when the operator has named no mirror.
pub const DEFAULT_DOWNLOAD_HOST: &str = "https://github.com";

/// The asset download is large and the link may be slow. Bounded so a wedged
/// socket cannot hold a background install job forever; same order as
/// `bootstrap::BOOTSTRAP_TIMEOUT_SECS` (600, `src/runtimes/bootstrap.rs`).
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// The metadata request is small, so it gets a much tighter bound: a dead
/// mirror must fail in seconds, not in ten minutes.
const METADATA_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("cannot resolve the runtimes directory: {0}")]
    Paths(String),
    #[error("{url}: {source}")]
    Http {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("{url} answered HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("{url} did not return a release document: {detail}")]
    Metadata { url: String, detail: String },
    #[error("release asset {asset}: {detail}")]
    Digest { asset: String, detail: String },
    #[error(
        "sha256 mismatch: the release metadata says {expected}, the {bytes} downloaded bytes \
         hash to {actual}. The download was deleted and nothing was installed. If {mirror} is \
         set, it is serving different bytes than the release it claims to mirror."
    )]
    DigestMismatch {
        expected: String,
        actual: String,
        bytes: usize,
        /// The operator-facing name of the knob to suspect first, built by
        /// [`mirror_clause`] from the same `match` that decides which key is
        /// actually read. A generic "a download_host mirror" here would send
        /// an operator hunting for a key this runtime does not have, and
        /// naming the Chromium one would send them to a key that exists and
        /// does nothing for this download (判据 §17: 错的标签比缺的贵).
        mirror: String,
    },
    #[error("archive {archive}: {detail}")]
    Archive { archive: String, detail: String },
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// `~/.aleph/runtimes/<runtime>/<tag>/`.
///
/// Tag-named on purpose: bumping the pin must not overwrite a binary a running
/// browser is executing, and a rollback must not require a re-download.
pub fn install_dir(runtime: &str, tag: &str) -> Result<PathBuf, ReleaseError> {
    Ok(crate::runtimes::get_runtimes_dir()
        .map_err(|e| ReleaseError::Paths(e.to_string()))?
        .join(runtime)
        .join(tag))
}

/// Where [`install_release`] puts — and [`super::probe`] looks for — the binary.
pub fn installed_binary(
    runtime: &str,
    tag: &str,
    binary_in_archive: &str,
) -> Result<PathBuf, ReleaseError> {
    Ok(install_dir(runtime, tag)?.join(binary_in_archive))
}

/// The release-metadata URL.
///
/// `github.com` keeps its API on a different host; a mirror serves both trees
/// itself. One `format!` for both would work against a fixture server and 404
/// against the real thing — a defect only a real run finds.
#[must_use]
pub fn api_url(host: &str, repo: &str, tag: &str) -> String {
    let host = host.trim_end_matches('/');
    if host == DEFAULT_DOWNLOAD_HOST {
        format!("https://api.github.com/repos/{repo}/releases/tags/{tag}")
    } else {
        format!("{host}/repos/{repo}/releases/tags/{tag}")
    }
}

/// The asset-download URL.
#[must_use]
pub fn asset_url(host: &str, repo: &str, tag: &str, asset: &str) -> String {
    format!(
        "{}/{repo}/releases/download/{tag}/{asset}",
        host.trim_end_matches('/')
    )
}

/// The lowercase hex sha256 the release claims for `asset`.
///
/// Every non-answer is an error naming the asset: absent from the list, no
/// `digest` field, a digest that is not `sha256:`, or one that is not 64 hex
/// characters. None of them may degrade into "install it anyway".
pub fn digest_for_asset(release: &serde_json::Value, asset: &str) -> Result<String, ReleaseError> {
    let assets = release
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ReleaseError::Digest {
            asset: asset.to_string(),
            detail: "the release document has no `assets` array".to_string(),
        })?;
    let entry = assets
        .iter()
        .find(|a| a.get("name").and_then(serde_json::Value::as_str) == Some(asset))
        .ok_or_else(|| ReleaseError::Digest {
            asset: asset.to_string(),
            detail: format!(
                "not in the release's {} asset(s); this tag may not build this platform",
                assets.len()
            ),
        })?;
    let raw = entry
        .get("digest")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ReleaseError::Digest {
            asset: asset.to_string(),
            detail: "the release metadata carries no digest for it, so the bytes cannot be \
                     verified; refusing to install an unverified binary"
                .to_string(),
        })?;
    let hex_part = raw
        .strip_prefix("sha256:")
        .ok_or_else(|| ReleaseError::Digest {
            asset: asset.to_string(),
            detail: format!("digest {raw:?} is not a sha256: digest"),
        })?;
    if hex_part.len() != 64 || !hex_part.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ReleaseError::Digest {
            asset: asset.to_string(),
            detail: format!("digest {raw:?} is not 64 hex characters"),
        });
    }
    Ok(hex_part.to_ascii_lowercase())
}

/// Hash `bytes` and compare. The error names BOTH hashes: an operator has to
/// be able to tell "my mirror is stale" from "the pinned tag moved" without
/// re-running anything.
/// `mirror` is the clause [`mirror_clause`] built for the runtime being
/// installed, so the refusal names the knob an operator can actually turn. It
/// is a parameter rather than a lookup inside because this function is the
/// pure hash compare and knows nothing about runtimes or config sections.
pub fn verify_sha256(bytes: &[u8], expect_hex: &str, mirror: &str) -> Result<(), ReleaseError> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual = hex::encode(hasher.finalize());
    if actual.eq_ignore_ascii_case(expect_hex) {
        return Ok(());
    }
    Err(ReleaseError::DigestMismatch {
        expected: expect_hex.to_ascii_lowercase(),
        actual,
        bytes: bytes.len(),
        mirror: mirror.to_string(),
    })
}

/// Extract exactly the member named `member` from `archive` to `dest`.
///
/// Paths are compared **whole**, never by suffix: `obscura` and `bin/obscura`
/// are different members, and a suffix match takes whichever comes first —
/// the same reasoning `chromium_launch::argv_names_dir` gives for comparing
/// argv tokens rather than scanning a joined string.
///
/// `is_zip` is a PARAMETER, and that is the whole point. The obvious spelling
/// reads the format off `archive.extension()` — but the path this is handed is
/// the download scratch file, `<asset>.part`, so the Windows asset
/// `obscura-x86_64-windows.zip` arrives as `…zip.part` whose extension is
/// `"part"`. A real zip would then be fed to `GzDecoder` and every Windows
/// install would fail with "cannot read tar entries", green in a test suite
/// whose fixtures are all `.tar.gz`. The format is a property of the ASSET, so
/// the asset name is what decides it (判据 §12: derive it where it is known).
pub fn extract_one(
    archive: &Path,
    member: &str,
    dest: &Path,
    is_zip: bool,
) -> Result<(), ReleaseError> {
    let file = std::fs::File::open(archive).map_err(|e| ReleaseError::Io {
        path: archive.display().to_string(),
        source: e,
    })?;
    let found = if is_zip {
        extract_from_zip(file, member, dest, archive)?
    } else {
        extract_from_targz(file, member, dest, archive)?
    };
    if !found {
        return Err(ReleaseError::Archive {
            archive: archive.display().to_string(),
            detail: format!("does not contain a member named {member:?}"),
        });
    }
    make_executable(dest)
}

fn extract_from_targz(
    file: std::fs::File,
    member: &str,
    dest: &Path,
    archive: &Path,
) -> Result<bool, ReleaseError> {
    let mut tarball = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let entries = tarball.entries().map_err(|e| ReleaseError::Archive {
        archive: archive.display().to_string(),
        detail: format!("cannot read tar entries: {e}"),
    })?;
    for entry in entries {
        let mut entry = entry.map_err(|e| ReleaseError::Archive {
            archive: archive.display().to_string(),
            detail: format!("cannot read a tar entry: {e}"),
        })?;
        let path = entry.path().map_err(|e| ReleaseError::Archive {
            archive: archive.display().to_string(),
            detail: format!("a tar entry has an unreadable path: {e}"),
        })?;
        if path.to_string_lossy() != member {
            continue;
        }
        let mut out = std::fs::File::create(dest).map_err(|e| ReleaseError::Io {
            path: dest.display().to_string(),
            source: e,
        })?;
        std::io::copy(&mut entry, &mut out).map_err(|e| ReleaseError::Io {
            path: dest.display().to_string(),
            source: e,
        })?;
        return Ok(true);
    }
    Ok(false)
}

fn extract_from_zip(
    file: std::fs::File,
    member: &str,
    dest: &Path,
    archive: &Path,
) -> Result<bool, ReleaseError> {
    let mut zipfile = zip::ZipArchive::new(file).map_err(|e| ReleaseError::Archive {
        archive: archive.display().to_string(),
        detail: format!("cannot open zip: {e}"),
    })?;
    // The Windows asset holds `obscura.exe`, not `obscura`; accepting either
    // keeps the spec's `binary_in_archive` one string across all platforms.
    let wanted = [member.to_string(), format!("{member}.exe")];
    for i in 0..zipfile.len() {
        let mut entry = zipfile.by_index(i).map_err(|e| ReleaseError::Archive {
            archive: archive.display().to_string(),
            detail: format!("cannot read zip entry {i}: {e}"),
        })?;
        let name = entry.name().to_string();
        if !wanted.iter().any(|w| w == &name) {
            continue;
        }
        let mut out = std::fs::File::create(dest).map_err(|e| ReleaseError::Io {
            path: dest.display().to_string(),
            source: e,
        })?;
        std::io::copy(&mut entry, &mut out).map_err(|e| ReleaseError::Io {
            path: dest.display().to_string(),
            source: e,
        })?;
        return Ok(true);
    }
    Ok(false)
}

/// `chmod 755` on unix; a no-op elsewhere (Windows executability is the
/// extension). Without it the file exists and `probe::is_executable_file`
/// correctly refuses to call it found — an "installed but still Missing" that
/// reads like a path bug.
fn make_executable(path: &Path) -> Result<(), ReleaseError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|e| {
            ReleaseError::Io {
                path: path.display().to_string(),
                source: e,
            }
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// The mirror the operator configured for `runtime`, or [`DEFAULT_DOWNLOAD_HOST`].
///
/// Mirrors `post_install::config_env`'s discipline
/// (`src/runtimes/post_install.rs`): a config we could not read is NOT a config
/// with no mirror, so it warns and proceeds with the default. That is safe here
/// for a reason that does not hold there, and the reason is worth stating: the
/// digest check runs against the bytes whatever host served them, so the worst
/// a wrong host can do is fail to download. It cannot install different bytes.
///
/// **Per runtime, and never `[general.browser.runtime] download_host`.** That
/// key is `PLAYWRIGHT_DOWNLOAD_HOST` (`browser::profile::BrowserRuntimeConfig`,
/// and `runtimes::post_install` is its one reader) — an npmmirror-shaped
/// Playwright CDN mirror, which serves no GitHub release tree at all. Reading
/// it here would let a chromium mirror silently rewrite every obscura URL into
/// a 404. A runtime with no mirror key of its own gets the default host rather
/// than another runtime's mirror, and
/// `every_release_installed_runtime_has_a_mirror_key` is what stops that from
/// becoming a default nobody chose.
#[must_use]
pub fn configured_host(runtime: &str) -> String {
    let Some(key) = mirror_key_for(runtime) else {
        return DEFAULT_DOWNLOAD_HOST.to_string();
    };
    match crate::config::Config::load() {
        Ok(cfg) => (key.read)(&cfg).unwrap_or_else(|| DEFAULT_DOWNLOAD_HOST.to_string()),
        Err(e) => {
            warn!("cannot read config for the {runtime} release download host: {e}");
            DEFAULT_DOWNLOAD_HOST.to_string()
        }
    }
}

/// A runtime's download mirror: how to READ it, and what to CALL it.
///
/// Both halves in one value because they are one fact with two faces (判据 §9)
/// — the key `configured_host` consults and the key a refusal tells the
/// operator to check must never be different keys. Splitting them into a
/// lookup and a message literal is how a remedy ends up naming a knob the code
/// does not read.
struct MirrorKey {
    /// The operator-facing spelling, exactly as it appears in `config.toml`.
    path: &'static str,
    /// How to read it out of the loaded config. A `fn` rather than a dotted
    /// string because the config is a typed tree and the accessor is what
    /// already applies the blank-is-unset rule
    /// ([`crate::browser::profile::ObscuraRuntimeConfig::download_host`]).
    read: fn(&crate::config::Config) -> Option<String>,
}

/// Which config key supplies a release-installed runtime's mirror.
///
/// `None` means "this runtime has no mirror key" — a stated answer, not a
/// fallback onto somebody else's. `every_release_installed_runtime_has_a
/// _mirror_key` is what stops a future `GithubRelease` spec from taking that
/// answer silently.
fn mirror_key_for(runtime: &str) -> Option<MirrorKey> {
    match runtime {
        super::specs::OBSCURA_RUNTIME => Some(MirrorKey {
            path: "[general.browser.obscura] download_host",
            read: |cfg| {
                cfg.general
                    .browser
                    .obscura
                    .download_host()
                    .map(str::to_string)
            },
        }),
        _ => None,
    }
}

/// What a refusal calls the mirror for `runtime`, as a clause that reads
/// correctly whether or not the runtime has a key of its own.
///
/// Derived from [`mirror_key_for`], never written out beside it: a message
/// naming one key while the code reads another is 判据 §1 with the expensive
/// copy in an operator's config file.
#[must_use]
pub fn mirror_clause(runtime: &str) -> String {
    mirror_key_for(runtime).map_or_else(
        || "a download_host mirror".to_string(),
        |k| format!("the {} mirror", k.path),
    )
}

/// Fetch, verify, extract. Returns the absolute path of the installed binary.
pub async fn install_release(
    runtime: &str,
    repo: &str,
    tag: &str,
    asset: &str,
    binary_in_archive: &str,
    host: &str,
) -> Result<PathBuf, ReleaseError> {
    let dir = install_dir(runtime, tag)?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ReleaseError::Io {
            path: dir.display().to_string(),
            source: e,
        })?;

    let client = reqwest::Client::builder()
        .user_agent(concat!("aleph/", env!("ALEPH_VERSION")))
        .build()
        .map_err(|e| ReleaseError::Http {
            url: host.to_string(),
            source: e,
        })?;

    let meta_url = api_url(host, repo, tag);
    let resp = client
        .get(&meta_url)
        .timeout(METADATA_TIMEOUT)
        .send()
        .await
        .map_err(|e| ReleaseError::Http {
            url: meta_url.clone(),
            source: e,
        })?;
    if !resp.status().is_success() {
        return Err(ReleaseError::Status {
            url: meta_url,
            status: resp.status().as_u16(),
        });
    }
    let body = resp.text().await.map_err(|e| ReleaseError::Http {
        url: meta_url.clone(),
        source: e,
    })?;
    let release: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| ReleaseError::Metadata {
            url: meta_url.clone(),
            detail: e.to_string(),
        })?;
    // Before the download, so a platform with no build fails in a second
    // instead of after 90 MB.
    let expect = digest_for_asset(&release, asset)?;

    let dl_url = asset_url(host, repo, tag, asset);
    let resp = client
        .get(&dl_url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|e| ReleaseError::Http {
            url: dl_url.clone(),
            source: e,
        })?;
    if !resp.status().is_success() {
        return Err(ReleaseError::Status {
            url: dl_url,
            status: resp.status().as_u16(),
        });
    }
    let bytes = resp.bytes().await.map_err(|e| ReleaseError::Http {
        url: dl_url,
        source: e,
    })?;

    // `.part` so a crash mid-write never leaves a file the extractor would
    // read as a complete archive.
    let part = dir.join(format!("{asset}.part"));
    tokio::fs::write(&part, &bytes)
        .await
        .map_err(|e| ReleaseError::Io {
            path: part.display().to_string(),
            source: e,
        })?;
    if let Err(e) = verify_sha256(&bytes, &expect, &mirror_clause(runtime)) {
        // Deleted before returning: leaving the bad archive behind invites the
        // next reader to "just extract it manually".
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }

    let dest = dir.join(binary_in_archive);
    let staged = dir.join(format!("{binary_in_archive}.tmp"));
    let (part_for_task, staged_for_task) = (part.clone(), staged.clone());
    let member = binary_in_archive.to_string();
    // From the ASSET name, never from `part`'s extension — that is `"part"`.
    let is_zip = asset
        .rsplit('.')
        .next()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let extracted = tokio::task::spawn_blocking(move || {
        extract_one(&part_for_task, &member, &staged_for_task, is_zip)
    })
    .await;
    let _ = tokio::fs::remove_file(&part).await;
    match extracted {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = tokio::fs::remove_file(&staged).await;
            return Err(e);
        }
        // A panicked or cancelled task is "I do not know whether it worked",
        // and the only safe reading of that is failure with the staging file
        // removed.
        Err(join) => {
            let _ = tokio::fs::remove_file(&staged).await;
            return Err(ReleaseError::Archive {
                archive: asset.to_string(),
                detail: format!("the extraction task did not complete: {join}"),
            });
        }
    }
    // Renamed last: the probe searches this directory by the binary's real
    // name, so that name appears only once the file is complete.
    tokio::fs::rename(&staged, &dest)
        .await
        .map_err(|e| ReleaseError::Io {
            path: dest.display().to_string(),
            source: e,
        })?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A real gzipped tar holding TWO members, in the order the upstream
    /// archive has them (`obscura` then `obscura-worker`, measured with
    /// `tar tzf`). A single-member fixture would let a "take the first entry"
    /// extractor pass.
    fn tiny_targz(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("fixture.tar.gz");
        let file = std::fs::File::create(&path).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut builder = tar::Builder::new(enc);
        for (name, body) in [
            ("obscura", &b"#!/bin/sh\necho obscura 0.2.2\n"[..]),
            ("obscura-worker", &b"worker\n"[..]),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, name, body).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        path
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        hex::encode(h.finalize())
    }

    /// github.com keeps its API on a DIFFERENT host; a mirror serves both
    /// trees itself. One `format!` for both works against a fixture server and
    /// 404s against the real thing — a defect only a real run would find.
    #[test]
    fn urls_split_github_com_into_its_api_host_but_keep_a_mirror_whole() {
        assert_eq!(
            api_url("https://github.com", "o/p", "v1.2.3"),
            "https://api.github.com/repos/o/p/releases/tags/v1.2.3"
        );
        assert_eq!(
            asset_url("https://github.com", "o/p", "v1.2.3", "a.tar.gz"),
            "https://github.com/o/p/releases/download/v1.2.3/a.tar.gz"
        );
        assert_eq!(
            api_url("http://127.0.0.1:9/", "o/p", "v1"),
            "http://127.0.0.1:9/repos/o/p/releases/tags/v1"
        );
        assert_eq!(
            asset_url("http://127.0.0.1:9/", "o/p", "v1", "a.zip"),
            "http://127.0.0.1:9/o/p/releases/download/v1/a.zip"
        );
    }

    /// A release whose asset carries no usable digest must REFUSE. "The
    /// metadata did not say" is a form of "I do not know", and an unknown may
    /// never be spent as a go-ahead — least of all one that ends in `chmod
    /// 755` on 90 MB we then execute (判据 §8).
    #[test]
    fn a_missing_or_malformed_digest_is_a_refusal_not_a_default() {
        let release = serde_json::json!({"assets": [
            {"name": "a.tar.gz", "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"},
            {"name": "b.tar.gz"},
            {"name": "c.tar.gz", "digest": "md5:deadbeef"},
            {"name": "d.tar.gz", "digest": "sha256:nothex"}
        ]});
        assert_eq!(
            digest_for_asset(&release, "a.tar.gz").unwrap(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        for (name, needle) in [
            ("b.tar.gz", "no digest"),
            ("c.tar.gz", "sha256:"),
            ("d.tar.gz", "64 hex"),
            ("nope.tar.gz", "not in the release"),
        ] {
            let err = digest_for_asset(&release, name).unwrap_err().to_string();
            assert!(err.contains(needle), "{name}: {err}");
            assert!(err.contains(name), "the refusal must name the asset: {err}");
        }
    }

    #[test]
    fn verify_sha256_names_both_hashes_when_they_differ() {
        let bytes = b"hello";
        let good = sha256_hex(bytes);
        verify_sha256(bytes, &good, &mirror_clause("obscura"))
            .expect("a matching digest must verify");
        let bad = "f".repeat(64);
        let err = verify_sha256(bytes, &bad, &mirror_clause("obscura"))
            .unwrap_err()
            .to_string();
        assert!(err.contains(&good), "must name what we measured: {err}");
        assert!(err.contains(&bad), "must name what was expected: {err}");
    }

    /// The refusal must name the knob this runtime's installer actually reads.
    /// A generic "a download_host mirror" sends an operator hunting; naming
    /// `[general.browser.runtime]` sends them to a key that exists, is read by
    /// something else, and does nothing for this download (判据 §17).
    #[test]
    fn the_mirror_clause_names_the_key_configured_host_reads() {
        assert_eq!(
            mirror_clause("obscura"),
            "the [general.browser.obscura] download_host mirror"
        );
        assert!(
            !mirror_clause("obscura").contains("[general.browser.runtime]"),
            "the Playwright CDN key cannot serve a GitHub release"
        );
        // A runtime with no key of its own says so generically rather than
        // borrowing obscura's.
        assert_eq!(mirror_clause("node"), "a download_host mirror");
    }

    #[test]
    fn extract_one_takes_the_named_member_and_only_that_one() {
        let dir = TempDir::new().unwrap();
        let archive = tiny_targz(dir.path());
        let dest = dir.path().join("obscura");
        extract_one(&archive, "obscura", &dest, false).unwrap();
        let body = std::fs::read_to_string(&dest).unwrap();
        assert!(body.contains("echo obscura 0.2.2"), "{body}");
        assert!(
            !dir.path().join("obscura-worker").exists(),
            "only the named member is written; obscura-worker is another ~86 MB \
             and only `obscura scrape` uses it"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o755,
                "the extracted binary must be executable"
            );
        }
    }

    /// The whole-path rule, stated as a case a suffix match gets wrong: a
    /// member at `bin/obscura` is NOT the member named `obscura`.
    #[test]
    fn extract_one_compares_whole_paths_not_suffixes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested.tar.gz");
        let file = std::fs::File::create(&path).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut builder = tar::Builder::new(enc);
        let body = &b"nested\n"[..];
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "bin/obscura", body)
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let err = extract_one(&path, "obscura", &dir.path().join("x"), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("obscura"), "{err}");
        assert!(
            !dir.path().join("x").exists(),
            "a suffix match would have taken bin/obscura and written it here"
        );
    }

    /// The Windows path, through the scratch name that broke it. `.zip.part`
    /// has extension `"part"`, so anything that sniffs the path decides "tar"
    /// and hands a zip to `GzDecoder`. Every other fixture in this module is a
    /// `.tar.gz`, so without this test the Windows install ships green and
    /// fails on the first real machine.
    #[test]
    fn a_zip_is_extracted_even_when_the_scratch_path_ends_in_part() {
        let dir = TempDir::new().unwrap();
        let zip_path = dir.path().join("obscura-x86_64-windows.zip.part");
        // The premise this fixture rests on, asserted rather than assumed: a
        // reader that sniffed the format off the path would see "part" here.
        assert_eq!(
            zip_path.extension().and_then(|e| e.to_str()),
            Some("part"),
            "the scratch name must NOT look like a zip, or this test proves nothing"
        );
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut w = zip::ZipWriter::new(file);
            let opts: zip::write::FileOptions<'_, ()> =
                zip::write::FileOptions::default().unix_permissions(0o755);
            w.start_file("obscura.exe", opts).unwrap();
            std::io::Write::write_all(&mut w, b"MZ fake windows binary\n").unwrap();
            w.start_file("obscura-worker.exe", opts).unwrap();
            std::io::Write::write_all(&mut w, b"worker\n").unwrap();
            w.finish().unwrap();
        }
        let dest = dir.path().join("obscura");
        // `binary_in_archive` is the bare name on every platform; the zip
        // branch accepts the `.exe` sibling.
        extract_one(&zip_path, "obscura", &dest, true).unwrap();
        assert!(
            std::fs::read_to_string(&dest)
                .unwrap()
                .contains("fake windows binary"),
            "the zip branch must run for a .zip.part scratch file"
        );
    }

    #[test]
    fn extract_one_refuses_an_archive_that_does_not_hold_the_member() {
        let dir = TempDir::new().unwrap();
        let archive = tiny_targz(dir.path());
        let err = extract_one(&archive, "not-there", &dir.path().join("x"), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not-there"), "{err}");
        assert!(
            !dir.path().join("x").exists(),
            "nothing is written on a miss"
        );
    }

    /// Every runtime this crate installs from a GitHub release must have a
    /// mirror key of its own, or an operator on a network that blocks
    /// `objects.githubusercontent.com` has no door (判据 §14) — and the one
    /// door that exists next door, `[general.browser.runtime] download_host`,
    /// is a Playwright CDN mirror that would 404 every URL built here.
    ///
    /// Derived from `SPECS` rather than from a list of runtime names: adding a
    /// second `GithubRelease` spec without a mirror key goes red here instead
    /// of silently taking the default host (判据 §5).
    #[test]
    fn every_release_installed_runtime_has_a_mirror_key() {
        let mut checked = 0usize;
        for spec in super::super::specs::SPECS {
            for oi in spec.install {
                if matches!(
                    oi.strategy,
                    super::super::specs::InstallStrategy::GithubRelease { .. }
                ) {
                    assert!(
                        mirror_key_for(spec.name).is_some(),
                        "{} installs from a GitHub release but names no download_host key; \
                         a blocked network would have no mirror to point it at",
                        spec.name
                    );
                    checked += 1;
                }
            }
        }
        assert!(
            checked > 0,
            "no GithubRelease spec was found, so this guard measured nothing"
        );
    }

    /// The Playwright mirror is NOT this module's mirror, asserted as an
    /// EFFECT rather than as a property of this file's source text.
    ///
    /// `[general.browser.runtime] download_host` is `PLAYWRIGHT_DOWNLOAD_HOST`;
    /// a host serving Playwright's browser CDN serves no
    /// `/repos/…/releases/tags/…` tree, so reading it here would turn one
    /// operator setting into a 404 on a different subsystem's install
    /// (判据 §1 — one key, two meanings).
    ///
    /// The first version of this test was a source scan over
    /// `include_str!("github_release.rs")`, and it went red on its first run
    /// for the dumbest possible reason: the forbidden string is written **in
    /// the assertion**, so the scan found its own needle (判据 §18 — the
    /// instrument reporting on itself). Replaced rather than patched: a config
    /// the function really reads is the thing being claimed.
    #[tokio::test]
    async fn configured_host_ignores_the_playwright_cdn_mirror() {
        let home = TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());
        std::fs::write(
            home.path().join("config.toml"),
            b"[general.browser.runtime]\ndownload_host = \"https://npmmirror.invalid/playwright\"\n",
        )
        .expect("write config");
        // Precondition: the fixture is hostile only if the config really
        // parsed and really carries that key. A config that failed to load
        // would send `configured_host` down its warn-and-default arm and this
        // test would pass for the wrong reason.
        let cfg = crate::config::Config::load().expect("the fixture config must parse");
        assert_eq!(
            cfg.general.browser.runtime.download_host(),
            Some("https://npmmirror.invalid/playwright"),
            "precondition: the Playwright mirror must actually be set"
        );
        assert_eq!(cfg.general.browser.obscura.download_host(), None);

        assert_eq!(
            configured_host("obscura"),
            DEFAULT_DOWNLOAD_HOST,
            "a Playwright CDN mirror must not be spent as a GitHub release host"
        );
    }

    /// The positive half: obscura's own key IS read, and a runtime with no
    /// mirror key of its own gets the default rather than obscura's.
    #[tokio::test]
    async fn configured_host_reads_the_runtimes_own_mirror_key() {
        let home = TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());
        std::fs::write(
            home.path().join("config.toml"),
            b"[general.browser.obscura]\ndownload_host = \"https://mirror.invalid/gh\"\n",
        )
        .expect("write config");
        let cfg = crate::config::Config::load().expect("the fixture config must parse");
        assert_eq!(
            cfg.general.browser.obscura.download_host(),
            Some("https://mirror.invalid/gh"),
            "precondition: the obscura mirror must actually be set"
        );

        assert_eq!(configured_host("obscura"), "https://mirror.invalid/gh");
        assert_eq!(
            configured_host("node"),
            DEFAULT_DOWNLOAD_HOST,
            "a runtime with no mirror key of its own must not inherit obscura's"
        );
    }

    /// End to end against a local fixture "GitHub". The assertion is the
    /// EFFECT — an executable file at the tag-named path whose CONTENTS are
    /// the archive member's — not that a download happened.
    #[tokio::test]
    async fn install_release_verifies_the_digest_then_lays_down_the_binary() {
        let scratch = TempDir::new().unwrap();
        let archive = tiny_targz(scratch.path());
        let bytes = std::fs::read(&archive).unwrap();
        let digest = sha256_hex(&bytes);
        let server = FixtureRelease::start("o/p", "v0.2.2", "a.tar.gz", bytes, Some(digest)).await;

        let home = TempDir::new().unwrap();
        // `AlephHomeEnvGuard`, not `HomeEnvGuard`: `install_dir` resolves through
        // `$ALEPH_HOME` first and `$HOME` only as a fallback, and the two guards
        // take different mutexes.
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        let out = install_release(
            "obscura",
            "o/p",
            "v0.2.2",
            "a.tar.gz",
            "obscura",
            &server.host(),
        )
        .await
        .expect("install must succeed against the fixture release");
        assert_eq!(
            out,
            install_dir("obscura", "v0.2.2").unwrap().join("obscura")
        );
        assert!(
            std::fs::read_to_string(&out)
                .unwrap()
                .contains("echo obscura 0.2.2"),
            "the installed file must be the archive MEMBER, not the archive"
        );
        assert!(
            !install_dir("obscura", "v0.2.2")
                .unwrap()
                .join("a.tar.gz.part")
                .exists(),
            "the download scratch file must not survive a success"
        );
        assert!(
            !install_dir("obscura", "v0.2.2")
                .unwrap()
                .join("obscura.tmp")
                .exists(),
            "the staging file must not survive a success"
        );
    }

    /// **The zip branch, reached through `install_release` rather than by
    /// handing `extract_one` an `is_zip` a caller chose.**
    ///
    /// `a_zip_is_extracted_even_when_the_scratch_path_ends_in_part` above
    /// passes `is_zip: true` itself, so it proves the extractor handles a zip
    /// — and says nothing about the line that DECIDES. The mutation the whole
    /// `is_zip` parameter exists to catch (compute it from `part.extension()`,
    /// which is `"part"`) leaves that test green. This one is the falsifier:
    /// with the derivation moved to the scratch path, the zip is fed to
    /// `GzDecoder` and this fails (判据 §4 — assert the effect at the surface
    /// that owns the decision).
    #[tokio::test]
    async fn install_release_chooses_the_zip_branch_from_the_asset_name() {
        let scratch = TempDir::new().unwrap();
        let zip_path = scratch.path().join("obscura-x86_64-windows.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut w = zip::ZipWriter::new(file);
            let opts: zip::write::FileOptions<'_, ()> =
                zip::write::FileOptions::default().unix_permissions(0o755);
            w.start_file("obscura.exe", opts).unwrap();
            std::io::Write::write_all(&mut w, b"MZ fake windows binary\n").unwrap();
            w.finish().unwrap();
        }
        let bytes = std::fs::read(&zip_path).unwrap();
        let digest = sha256_hex(&bytes);
        let server = FixtureRelease::start(
            "o/p",
            "v0.2.2",
            "obscura-x86_64-windows.zip",
            bytes,
            Some(digest),
        )
        .await;

        let home = TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        let out = install_release(
            "obscura",
            "o/p",
            "v0.2.2",
            "obscura-x86_64-windows.zip",
            "obscura",
            &server.host(),
        )
        .await
        .expect("a zip asset must install");
        // The scratch file `install_release` wrote was
        // `obscura-x86_64-windows.zip.part`, whose extension is "part" — the
        // premise of this test, and the reason the decision cannot live there.
        assert!(
            std::fs::read_to_string(&out)
                .unwrap()
                .contains("fake windows binary"),
            "the zip branch must be chosen from the ASSET name, not the scratch path"
        );
    }

    /// The point of the digest: a byte-flipped asset installs nothing, the
    /// partial download is gone, and the error names BOTH hashes so an
    /// operator can tell a stale mirror from a moved pin without re-running.
    #[tokio::test]
    async fn a_digest_mismatch_refuses_deletes_the_download_and_installs_nothing() {
        let scratch = TempDir::new().unwrap();
        let archive = tiny_targz(scratch.path());
        let mut bytes = std::fs::read(&archive).unwrap();
        let claimed = sha256_hex(&bytes);
        bytes.push(0x00); // one byte, added AFTER the digest was computed
        let server =
            FixtureRelease::start("o/p", "v0.2.2", "a.tar.gz", bytes, Some(claimed.clone())).await;

        let home = TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        let err = install_release(
            "obscura",
            "o/p",
            "v0.2.2",
            "a.tar.gz",
            "obscura",
            &server.host(),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains(&claimed), "names the expected digest: {err}");
        assert!(err.contains("sha256"), "{err}");
        // …and the knob to check first, which must be obscura's own key and
        // not the Playwright CDN one next door.
        assert!(
            err.contains("[general.browser.obscura] download_host"),
            "the refusal must name the mirror this installer reads: {err}"
        );
        assert!(
            !err.contains("[general.browser.runtime]"),
            "the Playwright CDN key cannot serve a GitHub release: {err}"
        );
        let dir = install_dir("obscura", "v0.2.2").unwrap();
        assert!(!dir.join("obscura").exists(), "no binary may be laid down");
        assert!(
            !dir.join("a.tar.gz.part").exists(),
            "the bad download must be deleted"
        );
    }

    /// An asset the release does not list must fail at the METADATA step,
    /// before anything is downloaded — the network is not the place to
    /// discover that a platform has no build.
    #[tokio::test]
    async fn an_unlisted_asset_fails_before_any_download() {
        let scratch = TempDir::new().unwrap();
        let archive = tiny_targz(scratch.path());
        let bytes = std::fs::read(&archive).unwrap();
        let digest = sha256_hex(&bytes);
        let server = FixtureRelease::start("o/p", "v0.2.2", "a.tar.gz", bytes, Some(digest)).await;

        let home = TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        let err = install_release(
            "obscura",
            "o/p",
            "v0.2.2",
            "other.tar.gz",
            "obscura",
            &server.host(),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("other.tar.gz"), "{err}");
        assert!(err.contains("not in the release"), "{err}");
        assert!(
            !install_dir("obscura", "v0.2.2")
                .unwrap()
                .join("other.tar.gz.part")
                .exists(),
            "nothing may be downloaded once the metadata step has refused"
        );
    }

    /// A minimal HTTP server speaking the two routes `install_release` uses.
    /// `tokio::net::TcpListener` on 127.0.0.1:0 — the shape already used
    /// elsewhere in this tree for local fixtures, so no dev-dependency enters
    /// for it.
    struct FixtureRelease {
        port: u16,
        _task: tokio::task::JoinHandle<()>,
    }

    impl FixtureRelease {
        async fn start(
            repo: &str,
            tag: &str,
            asset: &str,
            body: Vec<u8>,
            digest: Option<String>,
        ) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let api_path = format!("/repos/{repo}/releases/tags/{tag}");
            let asset_path = format!("/{repo}/releases/download/{tag}/{asset}");
            let mut entry = serde_json::json!({ "name": asset });
            if let Some(d) = digest {
                entry["digest"] = serde_json::Value::String(format!("sha256:{d}"));
            }
            let api_body = serde_json::json!({ "assets": [entry] }).to_string();
            let task = tokio::spawn(async move {
                loop {
                    let Ok((mut sock, _)) = listener.accept().await else {
                        return;
                    };
                    let api_path = api_path.clone();
                    let asset_path = asset_path.clone();
                    let api_body = api_body.clone();
                    let body = body.clone();
                    tokio::spawn(async move {
                        use tokio::io::{AsyncReadExt, AsyncWriteExt};
                        let mut buf = vec![0u8; 4096];
                        let n = sock.read(&mut buf).await.unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]).to_string();
                        let target = req.split_whitespace().nth(1).unwrap_or("").to_string();
                        let (status, payload): (&str, Vec<u8>) = if target == api_path {
                            ("200 OK", api_body.into_bytes())
                        } else if target == asset_path {
                            ("200 OK", body)
                        } else {
                            ("404 Not Found", b"no".to_vec())
                        };
                        let head = format!(
                            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\
                             Content-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                            payload.len()
                        );
                        let _ = sock.write_all(head.as_bytes()).await;
                        let _ = sock.write_all(&payload).await;
                        let _ = sock.shutdown().await;
                    });
                }
            });
            Self { port, _task: task }
        }

        fn host(&self) -> String {
            format!("http://127.0.0.1:{}", self.port)
        }
    }
}
