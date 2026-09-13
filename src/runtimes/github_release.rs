//! `InstallStrategy::GithubRelease` — fetch a release asset, verify its sha256
//! against the release metadata, extract one member, make it executable.
//!
//! # The trust model, stated as what it does and does not cover
//!
//! The end of this path is `chmod 755` on a ~90 MB binary that Aleph then
//! spawns, so the sentence describing its integrity has to be exactly true.
//!
//! **The checksum and the bytes come from different hosts, on purpose.** The
//! release metadata — and therefore the expected sha256 — is fetched from
//! GitHub's API **always**, regardless of `download_host`
//! ([`ReleaseSource::for_runtime`]). Only the asset bytes follow the
//! operator's mirror. That is the whole of what the digest check buys: a
//! mirror can fail to serve the bytes, or serve the wrong ones and be caught,
//! but it cannot choose the hash it is checked against.
//!
//! **What it does NOT cover**, said plainly because the previous version of
//! this module claimed otherwise: a compromised or spoofed `api.github.com`
//! can substitute the metadata and the asset together, and no check here would
//! notice — the digest is not pinned in this repository. Pinning digests
//! beside the tag in `SPECS` is the strictly stronger design; it is deferred
//! because only one of the five platform archives has a measured digest, and a
//! guard covering one of five reads as covering all five.
//!
//! **A release whose asset carries no digest is a refusal**, not a permission:
//! "the metadata did not say" is a form of "I do not know", and the one thing
//! an unknown may never be spent as is a go-ahead (判据 §8).
//!
//! # Why the extraction is in-process, and from memory
//!
//! `Command::new("tar")` would add three failure modes this code does not have
//! — the tool absent, PATH resolving a different `tar`, and an exit code that
//! cannot say which member failed — and it cannot take one member without also
//! writing the ~86 MB `obscura-worker` beside it.
//!
//! The archive is extracted from **the same buffer that was hashed**. An
//! earlier version wrote the download to `<asset>.part` and re-opened it, so
//! the verified object and the consumed object were different objects across a
//! filesystem round trip, at a boundary ending in `chmod 755`. Extracting from
//! a [`std::io::Cursor`] over the verified bytes deletes the write, the
//! read-back, and both scratch-file cleanup paths.
//!
//! # Order is the contract
//!
//! metadata → digest + size → download (bounded) → **verify** → extract →
//! rename. The binary appears under the name [`super::probe`] searches for only
//! once it is complete and verified.

use std::io::Read;
use std::path::{Path, PathBuf};

use futures::StreamExt;
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

/// The hard ceiling on anything this module holds in memory or writes to disk,
/// for one asset and for one extracted member.
///
/// **This bounds memory, not authenticity.** The per-asset `size` the metadata
/// declares is the tighter, derived bound (see [`AssetMeta`]) — but it comes
/// from the same document as the digest, so it is only as trustworthy as that
/// document, and a metadata response saying `size: 9999999999` must be
/// *refused* rather than honoured. That is what this constant is for: a number
/// this code chose, which no response can raise.
///
/// 512 MiB against a largest real asset of 76 MB (`obscura-aarch64-macos`,
/// 76_038_298 bytes, measured) and a largest real member of ~86 MB
/// (`obscura-worker`, which this ledger deliberately does not extract). Roughly
/// 6x headroom, and still a bound a single allocation cannot cross.
const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;

/// Which of the two fetches an error is about.
///
/// The two are different operator actions — one says "your network cannot
/// reach GitHub's API", the other says "your mirror is not serving this" — and
/// a single message covering both is a label that is wrong half the time
/// (判据 §17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetch {
    /// The release metadata, i.e. the checksum. Always from GitHub's API.
    Checksum,
    /// The asset bytes. From the operator's mirror when one is configured.
    Asset,
}

impl std::fmt::Display for Fetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Checksum => f.write_str(
                "the release checksum, which is always read from GitHub's API and never from a \
                 download_host mirror",
            ),
            Self::Asset => f.write_str("the release asset bytes"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("cannot resolve the runtimes directory: {0}")]
    Paths(String),
    #[error("could not reach {url} for {what}: {source}")]
    Http {
        url: String,
        what: Fetch,
        #[source]
        source: reqwest::Error,
    },
    #[error("{url} answered HTTP {status} while fetching {what}")]
    Status {
        url: String,
        what: Fetch,
        status: u16,
    },
    #[error("{url} did not return a release document: {detail}")]
    Metadata { url: String, detail: String },
    #[error("release asset {asset}: {detail}")]
    Digest { asset: String, detail: String },
    #[error(
        "sha256 mismatch: the release metadata says {expected}, the {bytes} downloaded bytes \
         hash to {actual}. Nothing was installed. {advice}"
    )]
    DigestMismatch {
        expected: String,
        actual: String,
        bytes: usize,
        /// What an operator should look at first, chosen by
        /// [`digest_mismatch_advice`] from whether a mirror is configured at
        /// all. With a mirror it names that mirror's key; without one it says
        /// so, rather than handing the operator a conditional about a knob
        /// they never set.
        advice: String,
    },
    /// The asset is over **this installer's own** ceiling — the number this
    /// code chose, which no response can raise.
    ///
    /// `ceiling` is [`MAX_ASSET_BYTES`] at every construction site, and that is
    /// the whole point of the variant: it used to be reachable with
    /// `ceiling: declared.min(MAX_ASSET_BYTES)`, so the real 76 MB asset
    /// rendered "over this installer's 76038298-byte ceiling" when the
    /// installer's ceiling is 536_870_912 — one label over two different facts,
    /// showing the number that is not the ceiling (判据 §17).
    #[error(
        "release asset {asset} is {declared} bytes, over this installer's {ceiling}-byte ceiling"
    )]
    TooLarge {
        asset: String,
        declared: u64,
        ceiling: u64,
    },
    /// One member expands past the ceiling. Separate from [`Self::TooLarge`]
    /// because the asset's declared size bounds the **compressed** bytes and
    /// says nothing about what they expand to — and because `asset` there is an
    /// asset name, while this needs an archive and a member (判据 §9: one
    /// field, one kind of value).
    ///
    /// **Deliberately not exercised by a test, and the reason is ordering, not
    /// cost.** Extraction runs *after* verification — `verify_sha256` then
    /// `spawn_blocking(extract_one)` in [`install_release`] — so a member that
    /// expands past the ceiling can only reach [`copy_member_capped`] inside an
    /// archive that **hashes to the digest `api.github.com` published**. An
    /// attacker-supplied decompression bomb cannot get here at all unless the
    /// API itself served it, and that is exactly the case this module's
    /// trust-model doc already concedes it does not cover. So this arm is
    /// defence-in-depth against (a) a threat the layer above concedes and
    /// (b) the upstream binary genuinely growing 6x. Both are worth bounding
    /// and neither is worth a >512 MiB fixture: a test here would measure the
    /// fixture, not the property.
    #[error(
        "archive {archive}: member {member} expands past this installer's {ceiling}-byte ceiling"
    )]
    MemberTooLarge {
        archive: String,
        member: String,
        ceiling: u64,
    },
    /// The host served **more bytes than the release document declares**.
    ///
    /// Its own variant because it is not a ceiling breach and must not be
    /// labelled as one: nothing here is too big for this installer, the two
    /// hosts simply disagree about the asset's length. It carries the same
    /// `advice` a [`Self::DigestMismatch`] would, because it means the same
    /// thing and an operator needs the same next step — the bytes are
    /// abandoned **before** they can reach `verify_sha256`, so without this the
    /// path would surface with no advice, no mirror key and no next step. **A
    /// length-changing substitution is the more likely stale-mirror shape than
    /// an equal-length one**, so this is the path a stale mirror is most likely
    /// to take, not the rare one.
    #[error(
        "{asset}: the release document declares {declared} bytes and the download host served \
         more, so the download was abandoned before it could be checked against the release \
         checksum. {advice}"
    )]
    LongerThanDeclared {
        asset: String,
        declared: u64,
        advice: String,
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

/// The release-metadata URL.
///
/// `github.com` keeps its API on a different host; a fixture serves both trees
/// itself. One `format!` for both would work against a fixture server and 404
/// against the real thing — a defect only a real run finds.
///
/// In production this is only ever called with [`DEFAULT_DOWNLOAD_HOST`], so
/// the first arm is the only one that ships; the second exists because a test
/// fixture has to be able to answer this route. [`ReleaseSource::for_runtime`]
/// is what makes that true, and `the_checksum_host_never_follows_the_mirror`
/// is what keeps it true.
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

/// What the release document says about one asset.
///
/// Both fields come from the **same entry**, looked up once: a second scan for
/// the size could find a different entry than the digest did if the document
/// ever carried two assets with one name (判据 §12 — derive them where the
/// answer is already known).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetMeta {
    /// Lowercase hex sha256, already validated as 64 hex characters.
    pub digest: String,
    /// The declared byte length. **Bounds memory, not authenticity** — it
    /// arrives in the same document as `digest`, so it is exactly as
    /// trustworthy as that document, and [`MAX_ASSET_BYTES`] is the bound it
    /// cannot raise.
    pub size: u64,
}

/// What the release claims for `asset`: its sha256 and its length.
///
/// Every non-answer is an error naming the asset: absent from the list, no
/// `digest` field, a digest that is not `sha256:`, or one that is not 64 hex
/// characters. None of them may degrade into "install it anyway".
///
/// A missing or unreadable `size` is **not** fatal — it costs the derived
/// bound, not the integrity check — so it falls back to [`MAX_ASSET_BYTES`],
/// which is the bound that was always going to be enforced anyway.
pub fn asset_meta(release: &serde_json::Value, asset: &str) -> Result<AssetMeta, ReleaseError> {
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
    let size = entry
        .get("size")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(MAX_ASSET_BYTES);
    if size > MAX_ASSET_BYTES {
        return Err(ReleaseError::TooLarge {
            asset: asset.to_string(),
            declared: size,
            ceiling: MAX_ASSET_BYTES,
        });
    }
    Ok(AssetMeta {
        digest: hex_part.to_ascii_lowercase(),
        size,
    })
}

/// Hash `bytes` and compare. The error names BOTH hashes: an operator has to
/// be able to tell "my mirror is stale" from "the pinned tag moved" without
/// re-running anything.
///
/// `advice` is built by the caller, which is the only layer that knows whether
/// a mirror is configured. It is a parameter rather than a lookup inside
/// because this function is the pure hash compare and knows nothing about
/// runtimes, hosts or config sections.
pub fn verify_sha256(bytes: &[u8], expect_hex: &str, advice: &str) -> Result<(), ReleaseError> {
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
        advice: advice.to_string(),
    })
}

/// Extract exactly the member named `member` out of `archive` to `dest`.
///
/// `archive` is the **verified buffer**, not a path. That is the fix for a
/// TOCTOU: the previous version hashed a buffer and then re-opened a file, so
/// what was checked and what was executed were different objects. A
/// [`std::io::Cursor`] over the same bytes removes the gap, and with it the
/// scratch file, its cleanup paths, and their discarded errors.
///
/// Paths are compared **whole**, never by suffix: `obscura` and `bin/obscura`
/// are different members, and a suffix match takes whichever comes first.
///
/// `is_zip` is a PARAMETER, and that is the point. The obvious spelling sniffs
/// the format off a filename — and the name this used to be handed was the
/// download scratch file `<asset>.part`, whose extension is `"part"`, so a zip
/// went to `GzDecoder` and every Windows install failed, green in a test suite
/// whose fixtures are all `.tar.gz`. The format is a property of the ASSET, so
/// the asset name decides it (判据 §12).
///
/// `label` names the archive in error messages; it is the asset name, not a
/// path, because there is no longer a path.
pub fn extract_one(
    archive: &[u8],
    label: &str,
    member: &str,
    dest: &Path,
    is_zip: bool,
) -> Result<(), ReleaseError> {
    let found = if is_zip {
        extract_from_zip(archive, member, dest, label)?
    } else {
        extract_from_targz(archive, member, dest, label)?
    };
    if !found {
        return Err(ReleaseError::Archive {
            archive: label.to_string(),
            detail: format!("does not contain a member named {member:?}"),
        });
    }
    make_executable(dest)
}

/// Copy at most [`MAX_ASSET_BYTES`] from `src` into `dest`, refusing rather
/// than truncating if the member is larger.
///
/// An uncapped `io::copy` here is the decompression half of the bounds
/// question: the asset's own declared size bounds the *compressed* bytes and
/// says nothing about what they expand to.
fn copy_member_capped(
    src: &mut impl Read,
    dest: &Path,
    label: &str,
    member: &str,
) -> Result<(), ReleaseError> {
    let mut out = std::fs::File::create(dest).map_err(|e| ReleaseError::Io {
        path: dest.display().to_string(),
        source: e,
    })?;
    // `+ 1` so that hitting the ceiling exactly is distinguishable from
    // exceeding it: a member of exactly MAX_ASSET_BYTES is legal, one byte
    // more is not, and `take(MAX)` alone cannot tell those apart.
    let mut limited = src.take(MAX_ASSET_BYTES + 1);
    let copied = std::io::copy(&mut limited, &mut out).map_err(|e| ReleaseError::Io {
        path: dest.display().to_string(),
        source: e,
    })?;
    if copied > MAX_ASSET_BYTES {
        // The partial write is removed: a truncated binary under the name the
        // probe searches for is worse than nothing there at all.
        let _ = std::fs::remove_file(dest);
        return Err(ReleaseError::MemberTooLarge {
            archive: label.to_string(),
            member: member.to_string(),
            ceiling: MAX_ASSET_BYTES,
        });
    }
    Ok(())
}

fn extract_from_targz(
    archive: &[u8],
    member: &str,
    dest: &Path,
    label: &str,
) -> Result<bool, ReleaseError> {
    let mut tarball =
        tar::Archive::new(flate2::read::GzDecoder::new(std::io::Cursor::new(archive)));
    let entries = tarball.entries().map_err(|e| ReleaseError::Archive {
        archive: label.to_string(),
        detail: format!("cannot read tar entries: {e}"),
    })?;
    for entry in entries {
        let mut entry = entry.map_err(|e| ReleaseError::Archive {
            archive: label.to_string(),
            detail: format!("cannot read a tar entry: {e}"),
        })?;
        let path = entry.path().map_err(|e| ReleaseError::Archive {
            archive: label.to_string(),
            detail: format!("a tar entry has an unreadable path: {e}"),
        })?;
        if path.to_string_lossy() != member {
            continue;
        }
        copy_member_capped(&mut entry, dest, label, member)?;
        return Ok(true);
    }
    Ok(false)
}

fn extract_from_zip(
    archive: &[u8],
    member: &str,
    dest: &Path,
    label: &str,
) -> Result<bool, ReleaseError> {
    let mut zipfile =
        zip::ZipArchive::new(std::io::Cursor::new(archive)).map_err(|e| ReleaseError::Archive {
            archive: label.to_string(),
            detail: format!("cannot open zip: {e}"),
        })?;
    // The Windows asset holds `obscura.exe`, not `obscura`; accepting either
    // keeps the spec's `binary_in_archive` one string across all platforms.
    let wanted = [member.to_string(), format!("{member}.exe")];
    for i in 0..zipfile.len() {
        let mut entry = zipfile.by_index(i).map_err(|e| ReleaseError::Archive {
            archive: label.to_string(),
            detail: format!("cannot read zip entry {i}: {e}"),
        })?;
        let name = entry.name().to_string();
        if !wanted.iter().any(|w| w == &name) {
            continue;
        }
        copy_member_capped(&mut entry, dest, label, member)?;
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

/// The mirror the operator configured for `runtime`'s asset bytes, or
/// [`DEFAULT_DOWNLOAD_HOST`].
///
/// Mirrors `post_install::config_env`'s discipline: a config we could not read
/// is NOT a config with no mirror, so it warns and proceeds with the default.
///
/// **Per runtime, and never `[general.browser.runtime] download_host`.** That
/// key is `PLAYWRIGHT_DOWNLOAD_HOST` — an npmmirror-shaped Playwright CDN
/// mirror, which serves no GitHub release tree at all. Reading it here would
/// let a chromium mirror silently rewrite every obscura URL into a 404.
///
/// **A non-`https` mirror is rejected and the default used.** This is
/// **hygiene, not integrity**, and the distinction is the whole subject of this
/// module's trust-model doc: the checksum comes from GitHub's API regardless,
/// so a plaintext asset mirror could not substitute bytes undetected even if it
/// were honoured. What this prevents is an operator silently downgrading their
/// own transport with a typo — not an attack the digest would otherwise miss.
/// **Private, not CUT** (N3). At BASE `bootstrap.rs` called this directly; A(1)
/// replaced that call with [`ReleaseSource::for_runtime`], which calls it
/// internally — so `pub` became a visibility with no consumer outside this
/// module, which is G's own shape created by G's own round. It still has one
/// in-module caller, so the remedy is P5's "default to private", not a cut.
#[must_use]
fn configured_host(runtime: &str) -> String {
    let Some(key) = mirror_key_for(runtime) else {
        return DEFAULT_DOWNLOAD_HOST.to_string();
    };
    let configured = match crate::config::Config::load() {
        Ok(cfg) => (key.read)(&cfg),
        Err(e) => {
            warn!("cannot read config for the {runtime} release download host: {e}");
            None
        }
    };
    let Some(host) = configured else {
        return DEFAULT_DOWNLOAD_HOST.to_string();
    };
    if !host.starts_with("https://") {
        warn!(
            "{} is {host:?}, which is not https; ignoring it and using {DEFAULT_DOWNLOAD_HOST}",
            key.path
        );
        return DEFAULT_DOWNLOAD_HOST.to_string();
    }
    host
}

/// A runtime's download mirror: how to READ it, and what to CALL it.
///
/// Both halves in one value because they are one fact with two faces (判据 §9)
/// — the key `configured_host` consults and the key a refusal tells the
/// operator to check must never be different keys.
struct MirrorKey {
    /// The operator-facing spelling, exactly as it appears in `config.toml`.
    path: &'static str,
    /// How to read it out of the loaded config. A `fn` rather than a dotted
    /// string because the config is a typed tree and the accessor is what
    /// already applies the blank-is-unset rule.
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

/// What a mismatch between the bytes and the release document tells the
/// operator to look at first.
///
/// ⚠️ **This doc used to open "a hostile mirror never reaches this error at
/// all". That was true at BASE and A(1) repealed it**, and the sentence carried
/// its own refutation: it reasoned *"since the checksum always comes from
/// GitHub's API…"*, which is exactly the change that makes a substituting
/// mirror **reachable** here. Before A(1) the mirror served the bytes *and* the
/// digest, so the pair always matched and no mismatch could occur; now the
/// mirror cannot choose the hash, and this error is the **only** thing that
/// catches substituted bytes. The module header at the top of this file has
/// said so all along — a mirror "can … serve the wrong ones and be caught" —
/// so the file contradicted itself 490 lines apart (判据 §1, both halves in one
/// file). A premise consumed by the fix it justified.
///
/// So the with-mirror list is deliberately **not exhaustive**. An
/// "either out of date or not mirroring this release" pair reads as a closed
/// set of *benign* causes at the one error that can also mean substitution, and
/// a wrong label reads as a fact (判据 §17). From here the two are
/// indistinguishable, and saying so is the honest sentence.
///
/// * **with a mirror in effect** — most often stale or not carrying this
///   release; also what substituted bytes look like, and the message says both;
/// * **with none** — GitHub's API and its own asset host disagree, most often a
///   re-upload under the same tag. The version before this one emitted a
///   conditional about a `download_host` here, i.e. it went quiet exactly when
///   the operator had set nothing and most needed a next step.
fn digest_mismatch_advice(runtime: &str, mirror_configured: bool) -> String {
    if !mirror_configured {
        // "in effect", not "configured" (N4): `configured_host` rejects a
        // non-https mirror and returns the default, so an operator can reach
        // this arm with a `download_host` line sitting in their config file. A
        // `warn!` fires at rejection time; this sentence describes the
        // effective source, which is the fact that matters here.
        return "No mirror is in effect for this runtime, so this is GitHub's API and its own \
                release-asset host disagreeing about the same asset — most often because the \
                release was re-uploaded under the same tag. Retry; if it persists, the pinned tag \
                has to be re-checked against upstream."
            .to_string();
    }
    match mirror_key_for(runtime) {
        Some(key) => format!(
            "These bytes came from the {} mirror. Most often that means it is stale or does not \
             carry this release — but it is also what a mirror serving substituted bytes looks \
             like, and the two cannot be told apart from here. Clear the key to install from \
             GitHub directly, and treat the mirror as suspect until you know which it was.",
            key.path
        ),
        // N5: every sentence here describes WHERE the bytes came from and what
        // to do about it, and none of them reports the result of a comparison.
        // That is deliberate and load-bearing, because this string is reused by
        // `LongerThanDeclared`, which is reached BEFORE `verify_sha256` runs at
        // all. The previous version opened "…served bytes that do not match the
        // checksum GitHub's API publishes", which sat one sentence after "the
        // download was abandoned before it could be checked against the release
        // checksum" — the first saying the comparison never happened, the
        // second reporting its result. True by inference (a different-length
        // body cannot hash to the published digest) and false as a statement
        // about what this code did: a sentence stronger than the mechanism
        // under it, which is the whole subject of fix A. The measured mismatch
        // belongs to `DigestMismatch`'s own message, which is the only place
        // that ran the comparison.
        // Unreachable in production: a runtime with no mirror key can never
        // have `mirror_configured == true`, because `configured_host` returns
        // the default host for exactly those runtimes, and
        // `every_release_installed_runtime_has_a_mirror_key` keeps the set of
        // release-installed runtimes equal to the set of keyed ones. Written as
        // a stated answer rather than `unreachable!()` because those two
        // predicates agreeing is a property of this file, not of the types.
        None => "The configured download mirror is serving bytes that do not match the checksum \
                 GitHub's API publishes for this asset."
            .to_string(),
    }
}

/// Where the two halves of an install come from.
///
/// Two fields rather than one host, because they are not the same question.
/// The bytes may come from wherever the operator points them; **the checksum
/// may not**, or the mirror would be choosing both the archive and the hash it
/// is checked against, and the digest check would verify transport rather than
/// provenance.
#[derive(Debug, Clone)]
pub struct ReleaseSource {
    /// Where the release METADATA — and therefore the expected sha256 — is
    /// fetched from. [`Self::for_runtime`] fixes this at
    /// [`DEFAULT_DOWNLOAD_HOST`]; it is a field at all so a test fixture can
    /// answer the route.
    pub api_host: String,
    /// Where the asset BYTES are fetched from: the operator's mirror, or the
    /// default.
    pub asset_host: String,
}

impl ReleaseSource {
    /// The production constructor, and the single place that decides a mirror
    /// may not supply the checksum.
    ///
    /// `the_checksum_host_never_follows_the_mirror` is the falsifier: point
    /// `api_host` at [`configured_host`] and it goes red with a mirror set.
    #[must_use]
    pub fn for_runtime(runtime: &str) -> Self {
        Self {
            api_host: DEFAULT_DOWNLOAD_HOST.to_string(),
            asset_host: configured_host(runtime),
        }
    }

    /// Whether the operator pointed the asset bytes somewhere other than
    /// GitHub. The one input [`digest_mismatch_advice`] needs.
    #[must_use]
    fn mirror_configured(&self) -> bool {
        self.asset_host.trim_end_matches('/') != DEFAULT_DOWNLOAD_HOST
    }
}

/// Fetch, verify, extract. Returns the absolute path of the installed binary.
pub async fn install_release(
    runtime: &str,
    repo: &str,
    tag: &str,
    asset: &str,
    binary_in_archive: &str,
    source: &ReleaseSource,
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
            url: source.api_host.clone(),
            what: Fetch::Checksum,
            source: e,
        })?;

    // The checksum, from GitHub's API — never from the mirror. Every failure
    // here is fail-closed and says which fetch it was (判据 §8, §17).
    let meta_url = api_url(&source.api_host, repo, tag);
    let resp = client
        .get(&meta_url)
        .timeout(METADATA_TIMEOUT)
        .send()
        .await
        .map_err(|e| ReleaseError::Http {
            url: meta_url.clone(),
            what: Fetch::Checksum,
            source: e,
        })?;
    if !resp.status().is_success() {
        return Err(ReleaseError::Status {
            url: meta_url,
            what: Fetch::Checksum,
            status: resp.status().as_u16(),
        });
    }
    let body = resp.text().await.map_err(|e| ReleaseError::Http {
        url: meta_url.clone(),
        what: Fetch::Checksum,
        source: e,
    })?;
    let release: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| ReleaseError::Metadata {
            url: meta_url.clone(),
            detail: e.to_string(),
        })?;
    // Before the download, so a platform with no build — or an asset whose
    // declared size is absurd — fails in a second instead of after 90 MB.
    let meta = asset_meta(&release, asset)?;

    // One advice string for both ways the bytes can disagree with the release
    // document — a wrong hash, and a wrong length. They mean the same thing to
    // an operator and the second is the more likely one (N2).
    let advice = digest_mismatch_advice(runtime, source.mirror_configured());
    let dl_url = asset_url(&source.asset_host, repo, tag, asset);
    let bytes = download_capped(&client, &dl_url, asset, meta.size, &advice).await?;

    verify_sha256(&bytes, &meta.digest, &advice)?;

    // Extracted from the SAME buffer that was just hashed. No scratch file, so
    // nothing can change between the check and the use.
    let dest = dir.join(binary_in_archive);
    let staged = dir.join(format!("{binary_in_archive}.tmp"));
    let (staged_for_task, label) = (staged.clone(), asset.to_string());
    let member = binary_in_archive.to_string();
    // From the ASSET name. There is no scratch path to sniff any more, and
    // there must never be one again.
    let is_zip = asset
        .rsplit('.')
        .next()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let extracted = tokio::task::spawn_blocking(move || {
        extract_one(&bytes, &label, &member, &staged_for_task, is_zip)
    })
    .await;
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

/// Which bound a body crossed, and therefore what to call the refusal.
///
/// Two bounds apply at once and they are **different facts**: the release
/// document's `declared` size, and this installer's own [`MAX_ASSET_BYTES`].
/// Collapsing them into one `TooLarge { ceiling: declared.min(MAX) }` printed
/// "over this installer's 76038298-byte ceiling" for the real asset, when the
/// installer's ceiling is 536_870_912 — the number shown was the one that is
/// not the ceiling (判据 §17).
///
/// The `declared` breach also carries `advice`: it is a host and an API
/// disagreeing about the same asset, which is what [`ReleaseError::DigestMismatch`]
/// means, and it is reached **before** `verify_sha256` can say so. Without
/// that, the most likely stale-mirror shape — a body of a different length —
/// would surface with no mirror key and no next step.
fn over_bound(total: u64, declared: u64, asset: &str, advice: &str) -> ReleaseError {
    if total > MAX_ASSET_BYTES {
        ReleaseError::TooLarge {
            asset: asset.to_string(),
            declared: total,
            ceiling: MAX_ASSET_BYTES,
        }
    } else {
        ReleaseError::LongerThanDeclared {
            asset: asset.to_string(),
            declared,
            advice: advice.to_string(),
        }
    }
}

/// Download the asset into memory, refusing past `declared` bytes.
///
/// Streamed rather than `resp.bytes()`, and the buffer is **not** pre-allocated
/// from `declared`: the point of the bound is that a number in someone else's
/// document cannot make this process allocate. `Content-Length` is checked when
/// present and the running total is checked regardless, because a header that
/// can be absent can also lie.
async fn download_capped(
    client: &reqwest::Client,
    url: &str,
    asset: &str,
    declared: u64,
    advice: &str,
) -> Result<Vec<u8>, ReleaseError> {
    let cap = declared.min(MAX_ASSET_BYTES);
    let resp = client
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|e| ReleaseError::Http {
            url: url.to_string(),
            what: Fetch::Asset,
            source: e,
        })?;
    if !resp.status().is_success() {
        return Err(ReleaseError::Status {
            url: url.to_string(),
            what: Fetch::Asset,
            status: resp.status().as_u16(),
        });
    }
    if let Some(len) = resp.content_length() {
        if len > cap {
            return Err(over_bound(len, declared, asset, advice));
        }
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ReleaseError::Http {
            url: url.to_string(),
            what: Fetch::Asset,
            source: e,
        })?;
        let total = buf.len() as u64 + chunk.len() as u64;
        if total > cap {
            return Err(over_bound(total, declared, asset, advice));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    /// A real gzipped tar holding TWO members, in the order the upstream
    /// archive has them (`obscura` then `obscura-worker`, measured with
    /// `tar tzf`). A single-member fixture would let a "take the first entry"
    /// extractor pass.
    fn tiny_targz() -> Vec<u8> {
        let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
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
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn tiny_zip() -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().unix_permissions(0o755);
        w.start_file("obscura.exe", opts).unwrap();
        std::io::Write::write_all(&mut w, b"MZ fake windows binary\n").unwrap();
        w.start_file("obscura-worker.exe", opts).unwrap();
        std::io::Write::write_all(&mut w, b"worker\n").unwrap();
        w.finish().unwrap().into_inner()
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        hex::encode(h.finalize())
    }

    /// github.com keeps its API on a DIFFERENT host; a fixture serves both
    /// trees itself. One `format!` for both works against a fixture server and
    /// 404s against the real thing — a defect only a real run would find.
    #[test]
    fn urls_split_github_com_into_its_api_host_but_keep_a_fixture_whole() {
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

    /// **The checksum never follows the mirror.** With a mirror configured, the
    /// asset host moves and the API host does not — otherwise the mirror would
    /// choose both the bytes and the hash they are checked against, and the
    /// digest would verify transport rather than provenance.
    #[tokio::test]
    async fn the_checksum_host_never_follows_the_mirror() {
        let home = TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());
        std::fs::write(
            home.path().join("config.toml"),
            b"[general.browser.obscura]\ndownload_host = \"https://mirror.invalid/gh\"\n",
        )
        .expect("write config");
        // Precondition: the fixture is hostile only if a mirror really is set.
        let cfg = crate::config::Config::load().expect("the fixture config must parse");
        assert_eq!(
            cfg.general.browser.obscura.download_host(),
            Some("https://mirror.invalid/gh"),
            "precondition: a mirror must actually be configured"
        );

        let source = ReleaseSource::for_runtime("obscura");
        assert_eq!(source.asset_host, "https://mirror.invalid/gh");
        assert_eq!(
            source.api_host, DEFAULT_DOWNLOAD_HOST,
            "a mirror may supply the bytes; it may not supply the checksum"
        );
        assert!(source.mirror_configured());
        assert!(
            api_url(&source.api_host, "o/p", "v1").starts_with("https://api.github.com/"),
            "the metadata URL must resolve to GitHub's API"
        );
    }

    /// A non-https mirror is ignored. **Hygiene, not integrity**: once the
    /// checksum comes from the API regardless, a plaintext asset mirror could
    /// not substitute bytes undetected anyway. What this stops is an operator
    /// downgrading their own transport by typo.
    #[tokio::test]
    async fn a_non_https_mirror_is_rejected_in_favour_of_the_default() {
        let home = TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());
        std::fs::write(
            home.path().join("config.toml"),
            b"[general.browser.obscura]\ndownload_host = \"http://plaintext.invalid/gh\"\n",
        )
        .expect("write config");
        let cfg = crate::config::Config::load().expect("the fixture config must parse");
        assert_eq!(
            cfg.general.browser.obscura.download_host(),
            Some("http://plaintext.invalid/gh"),
            "precondition: the plaintext mirror must actually be set"
        );
        assert_eq!(configured_host("obscura"), DEFAULT_DOWNLOAD_HOST);
    }

    /// A release whose asset carries no usable digest must REFUSE. "The
    /// metadata did not say" is a form of "I do not know", and an unknown may
    /// never be spent as a go-ahead — least of all one that ends in `chmod 755`
    /// on 90 MB we then execute (判据 §8).
    #[test]
    fn a_missing_or_malformed_digest_is_a_refusal_not_a_default() {
        let release = serde_json::json!({"assets": [
            {"name": "a.tar.gz", "size": 10, "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"},
            {"name": "b.tar.gz"},
            {"name": "c.tar.gz", "digest": "md5:deadbeef"},
            {"name": "d.tar.gz", "digest": "sha256:nothex"}
        ]});
        let ok = asset_meta(&release, "a.tar.gz").unwrap();
        assert_eq!(
            ok.digest,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert_eq!(ok.size, 10);
        for (name, needle) in [
            ("b.tar.gz", "no digest"),
            ("c.tar.gz", "sha256:"),
            ("d.tar.gz", "64 hex"),
            ("nope.tar.gz", "not in the release"),
        ] {
            let err = asset_meta(&release, name).unwrap_err().to_string();
            assert!(err.contains(needle), "{name}: {err}");
            assert!(err.contains(name), "the refusal must name the asset: {err}");
        }
    }

    /// **A declared size this installer will not honour is refused, not
    /// obeyed.** `size` arrives in the same document as the digest, so it
    /// bounds memory and not authenticity; the ceiling is the number this code
    /// chose, and no response may raise it.
    #[test]
    fn a_declared_size_over_the_ceiling_is_refused_rather_than_honoured() {
        let release = serde_json::json!({"assets": [
            {"name": "huge.tar.gz", "size": 9_999_999_999u64,
             "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}
        ]});
        let err = asset_meta(&release, "huge.tar.gz").unwrap_err().to_string();
        assert!(err.contains("9999999999"), "names what was declared: {err}");
        assert!(
            err.contains(&MAX_ASSET_BYTES.to_string()),
            "names the ceiling it exceeded: {err}"
        );
    }

    /// A release document with no `size` still installs: the declared size is a
    /// tighter bound, not the only one, and losing it must not cost the
    /// integrity check.
    #[test]
    fn a_missing_size_falls_back_to_the_ceiling_rather_than_refusing() {
        let release = serde_json::json!({"assets": [
            {"name": "a.tar.gz",
             "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}
        ]});
        assert_eq!(
            asset_meta(&release, "a.tar.gz").unwrap().size,
            MAX_ASSET_BYTES
        );
    }

    #[test]
    fn verify_sha256_names_both_hashes_when_they_differ() {
        let bytes = b"hello";
        let good = sha256_hex(bytes);
        verify_sha256(bytes, &good, "advice").expect("a matching digest must verify");
        let bad = "f".repeat(64);
        let err = verify_sha256(bytes, &bad, "advice")
            .unwrap_err()
            .to_string();
        assert!(err.contains(&good), "must name what we measured: {err}");
        assert!(err.contains(&bad), "must name what was expected: {err}");
    }

    /// The two arms of the mismatch advice, asserted as **properties** rather
    /// than as pinned prose.
    ///
    /// The no-mirror arm is the one that used to go quiet: it emitted a
    /// conditional about a `download_host` to an operator who had set none.
    #[test]
    fn the_mismatch_advice_has_an_arm_for_having_no_mirror_at_all() {
        let with = digest_mismatch_advice("obscura", true);
        assert!(
            with.contains("[general.browser.obscura] download_host"),
            "with a mirror set, name the key that is most likely at fault: {with}"
        );
        assert!(
            !with.contains("[general.browser.runtime]"),
            "the Playwright CDN key cannot serve a GitHub release: {with}"
        );

        let without = digest_mismatch_advice("obscura", false);
        assert!(
            !without.contains('['),
            "with no mirror set, the operator must not be handed a config section they \
             never touched: {without}"
        );
        assert!(
            without.contains("tag"),
            "and must still be told what to do next: {without}"
        );
        // N4: `configured_host` rejects a non-https mirror and returns the
        // default, so an operator can reach this arm with a `download_host`
        // line sitting in their config. "in effect" is true then; "configured"
        // is not.
        assert!(
            !without.contains("configured"),
            "this arm is reached with a download_host line present but rejected, so it \
             must describe the effective source, not the config file: {without}"
        );
        assert_ne!(with, without, "the two cases are not the same sentence");
    }

    /// **N1.** Before A(1) the mirror supplied the bytes AND the digest, so the
    /// pair always matched and a substituting mirror could not produce a
    /// mismatch at all. A(1) pinned the checksum to GitHub's API, which makes
    /// this error the ONLY thing that catches substituted bytes — so an
    /// exhaustive-sounding pair of benign causes ("either stale or not
    /// mirroring this release") is a wrong label at exactly the wrong place
    /// (判据 §17).
    ///
    /// Asserted as the property that matters: the operator is told the causes
    /// are **not** exhaustive and that substitution is among them.
    #[test]
    fn the_with_mirror_advice_does_not_present_benign_causes_as_exhaustive() {
        let with = digest_mismatch_advice("obscura", true);
        assert!(
            with.contains("substituted"),
            "a mismatch under a mirror can mean substituted bytes, and this is the only \
             error that catches them: {with}"
        );
        assert!(
            !with.contains("Either it is out of date or it is not mirroring"),
            "the benign pair may not be presented as the whole set: {with}"
        );
        assert!(
            with.contains("suspect") || with.contains("until you know"),
            "and the operator needs a next step that does not assume the benign case: {with}"
        );
    }

    /// **N5.** The advice string is reused by two errors, and only one of them
    /// ran a comparison. `LongerThanDeclared` abandons the body at the first
    /// chunk past the cap, so `verify_sha256` never runs — and the advice that
    /// followed it said *"served bytes that do not match the checksum"*, one
    /// sentence after the error itself said *"abandoned before it could be
    /// checked against the release checksum"*. The first says the comparison
    /// never happened; the second reports its result. True by inference, false
    /// as a statement about what this code did — a sentence stronger than the
    /// mechanism under it, which is fix A's whole subject.
    ///
    /// Asserted as the property that keeps the split honest: **the advice
    /// reports no comparison result**, and the error that did run the
    /// comparison states it itself.
    #[test]
    fn the_shared_advice_reports_no_comparison_that_may_not_have_happened() {
        for (label, advice) in [
            ("with mirror", digest_mismatch_advice("obscura", true)),
            ("no mirror", digest_mismatch_advice("obscura", false)),
        ] {
            for claim in ["do not match", "does not match", "hash", "checksum"] {
                assert!(
                    !advice.contains(claim),
                    "{label} advice asserts {claim:?}, but it is also rendered by \
                     LongerThanDeclared, which never reached verify_sha256: {advice}"
                );
            }
        }

        // The error that DID measure it says so, in its own message.
        let measured = verify_sha256(
            b"x",
            &"f".repeat(64),
            &digest_mismatch_advice("obscura", true),
        )
        .unwrap_err()
        .to_string();
        assert!(
            measured.contains("hash to"),
            "the mismatch this code actually measured belongs here: {measured}"
        );

        // And the length error states what did NOT happen, without also
        // reporting what it would have found.
        let unmeasured =
            over_bound(50, 10, "a.tar.gz", &digest_mismatch_advice("obscura", true)).to_string();
        assert!(
            unmeasured.contains("before it could be checked"),
            "{unmeasured}"
        );
        assert!(
            !unmeasured.contains("do not match"),
            "this path abandoned the body; it may not report a comparison result: {unmeasured}"
        );
    }

    /// **`Fetch` is one fact with two faces, and nothing pinned either.** Swap
    /// `Fetch::Checksum` and `Fetch::Asset` at their construction sites and,
    /// before this test, nothing in the suite went red (判据 §9).
    ///
    /// The fail-closed behaviour was never in doubt — every metadata site is
    /// `?` or `return Err`. What was unverified is the 判据 §17 half: that the
    /// message says **which** fetch failed. This is ordinary operation, not an
    /// exotic arm: offline, a firewall, DNS, or `api.github.com`'s 60/hr
    /// unauthenticated rate limit — and A(1) makes it *more* reachable for
    /// exactly the mirror population, who set a mirror because they cannot
    /// reach github.com in the first place.
    #[tokio::test]
    async fn a_dead_checksum_host_says_it_was_the_checksum_that_could_not_be_reached() {
        let archive = tiny_targz();
        let digest = sha256_hex(&archive);
        let server =
            FixtureRelease::start("o/p", "v0.2.2", "a.tar.gz", archive, Some(digest)).await;

        let home = TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        // The asset host is live; only the checksum host is dead. Port 1 is
        // reserved and refuses immediately, so this does not wait on a timeout.
        let source = ReleaseSource {
            api_host: "http://127.0.0.1:1".to_string(),
            asset_host: server.host(),
        };

        let err = install_release("obscura", "o/p", "v0.2.2", "a.tar.gz", "obscura", &source)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("the release checksum"),
            "the refusal must name which of the two fetches failed: {err}"
        );
        assert!(
            err.contains("never from a download_host mirror"),
            "and say why a mirror cannot substitute for it, since the operators who hit \
             this are the ones who configured a mirror: {err}"
        );
        assert!(
            !err.contains("the release asset bytes"),
            "the asset host is live in this fixture; naming it would send the operator \
             at the wrong host: {err}"
        );
        assert_eq!(
            server.asset_requests(),
            0,
            "the checksum step failed closed, so nothing may have been downloaded"
        );
    }

    /// **N2, both arms rendered.** Two bounds apply at once and they are
    /// different facts. Collapsing them printed "over this installer's
    /// 76038298-byte ceiling" for the real asset, when the installer's ceiling
    /// is 536_870_912 — the number shown was the one that is not the ceiling.
    #[test]
    fn over_bound_calls_the_installers_own_ceiling_by_its_real_number() {
        let ceiling = over_bound(MAX_ASSET_BYTES + 1, 10, "a.tar.gz", "ADVICE").to_string();
        assert!(
            ceiling.contains(&MAX_ASSET_BYTES.to_string()),
            "the ceiling arm must print the installer's own ceiling: {ceiling}"
        );
        assert!(
            !ceiling.contains("10-byte"),
            "and must not print the per-asset declared size as if it were the ceiling: {ceiling}"
        );

        let declared = over_bound(50, 10, "a.tar.gz", "ADVICE").to_string();
        assert!(
            !declared.contains("ceiling"),
            "a body longer than the document declares is not a ceiling breach: {declared}"
        );
        assert!(
            declared.contains("10"),
            "it names what was declared: {declared}"
        );
        assert!(
            declared.contains("ADVICE"),
            "and carries the same advice a digest mismatch would, because it means the \
             same thing and is reached before verify_sha256 can say so: {declared}"
        );
    }

    /// The no-key arm exists because the function is total over `Option`, not
    /// because anything reaches it: `every_release_installed_runtime_has_a
    /// _mirror_key` (below) keeps the set of release-installed runtimes equal
    /// to the set of keyed ones, so `mirror_configured == true` with no key is
    /// unreachable in production.
    ///
    /// Asserted as a **property** — it names no config section it cannot
    /// justify — rather than by pinning wording whose rendering line cannot be
    /// pointed at (判据 §17).
    #[test]
    fn the_no_key_arm_names_no_section_it_cannot_justify() {
        let advice = digest_mismatch_advice("node", true);
        assert!(
            !advice.contains('['),
            "a runtime with no mirror key must not name one: {advice}"
        );
        assert!(!advice.is_empty());
    }

    #[test]
    fn extract_one_takes_the_named_member_and_only_that_one() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("obscura");
        extract_one(&tiny_targz(), "fixture.tar.gz", "obscura", &dest, false).unwrap();
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
        let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut builder = tar::Builder::new(enc);
        let body = &b"nested\n"[..];
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "bin/obscura", body)
            .unwrap();
        let archive = builder.into_inner().unwrap().finish().unwrap();

        let err = extract_one(
            &archive,
            "nested.tar.gz",
            "obscura",
            &dir.path().join("x"),
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("obscura"), "{err}");
        assert!(
            !dir.path().join("x").exists(),
            "a suffix match would have taken bin/obscura and written it here"
        );
    }

    /// The zip branch reaches the `.exe` sibling. Every other fixture in this
    /// module is a `.tar.gz`, so without this the Windows install ships green
    /// and fails on the first real machine.
    #[test]
    fn a_zip_member_is_extracted_through_its_exe_sibling() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("obscura");
        extract_one(
            &tiny_zip(),
            "obscura-x86_64-windows.zip",
            "obscura",
            &dest,
            true,
        )
        .unwrap();
        assert!(
            std::fs::read_to_string(&dest)
                .unwrap()
                .contains("fake windows binary"),
            "the zip branch must accept `obscura.exe` for `obscura`"
        );
    }

    #[test]
    fn extract_one_refuses_an_archive_that_does_not_hold_the_member() {
        let dir = TempDir::new().unwrap();
        let err = extract_one(
            &tiny_targz(),
            "fixture.tar.gz",
            "not-there",
            &dir.path().join("x"),
            false,
        )
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
    /// `objects.githubusercontent.com` has no door (判据 §14) — and the one door
    /// next to it, `[general.browser.runtime] download_host`, is a Playwright
    /// CDN mirror that would 404 every URL built here.
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

    /// The positive half of the mirror lookup: obscura's own key IS read, and a
    /// runtime with no mirror key gets the default rather than obscura's.
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

    /// The Playwright CDN mirror is not this module's mirror, asserted as an
    /// effect: a config that sets only `[general.browser.runtime]` leaves the
    /// obscura host at the default.
    #[tokio::test]
    async fn configured_host_ignores_the_playwright_cdn_mirror() {
        let home = TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());
        std::fs::write(
            home.path().join("config.toml"),
            b"[general.browser.runtime]\ndownload_host = \"https://npmmirror.invalid/playwright\"\n",
        )
        .expect("write config");
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

    /// End to end against a local fixture "GitHub". **The name says happy-path**
    /// on purpose: with a matching digest this test cannot tell verification
    /// from its absence (measured — it stayed green with `verify_sha256` mutated
    /// to always answer `Ok`). What it asserts is the EFFECT of a successful
    /// install: an executable at the tag-named path whose contents are the
    /// archive MEMBER's. `a_digest_mismatch_installs_nothing` is the only guard
    /// that verification happens at all.
    #[tokio::test]
    async fn install_release_lays_down_the_archive_member_on_the_happy_path() {
        let archive = tiny_targz();
        let digest = sha256_hex(&archive);
        let server =
            FixtureRelease::start("o/p", "v0.2.2", "a.tar.gz", archive, Some(digest)).await;

        let home = TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        let out = install_release(
            "obscura",
            "o/p",
            "v0.2.2",
            "a.tar.gz",
            "obscura",
            &server.source(),
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
        // There is no scratch download file any more — the archive is never
        // written to disk at all. Asserted as "the directory holds exactly the
        // binary" so that reintroducing one goes red here.
        let dir = install_dir("obscura", "v0.2.2").unwrap();
        let left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(left, vec!["obscura".to_string()], "{left:?}");
    }

    /// **The zip branch, reached through `install_release` rather than by
    /// handing `extract_one` an `is_zip` a caller chose.** The
    /// `extract_one`-level test passes `is_zip` itself, so it proves the
    /// extractor handles a zip and says nothing about the line that DECIDES.
    #[tokio::test]
    async fn install_release_chooses_the_zip_branch_from_the_asset_name() {
        let archive = tiny_zip();
        let digest = sha256_hex(&archive);
        let server = FixtureRelease::start(
            "o/p",
            "v0.2.2",
            "obscura-x86_64-windows.zip",
            archive,
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
            &server.source(),
        )
        .await
        .expect("a zip asset must install");
        assert!(
            std::fs::read_to_string(&out)
                .unwrap()
                .contains("fake windows binary"),
            "the zip branch must be chosen from the ASSET name"
        );
    }

    /// **The only guard that verification happens at all.** A byte-flipped
    /// asset installs nothing and the error names both hashes, so an operator
    /// can tell a stale mirror from a moved pin without re-running.
    #[tokio::test]
    async fn a_digest_mismatch_installs_nothing() {
        let mut archive = tiny_targz();
        let claimed = sha256_hex(&archive);
        archive.push(0x00); // one byte, added AFTER the digest was computed
        let server =
            FixtureRelease::start("o/p", "v0.2.2", "a.tar.gz", archive, Some(claimed.clone()))
                .await;

        let home = TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        let err = install_release(
            "obscura",
            "o/p",
            "v0.2.2",
            "a.tar.gz",
            "obscura",
            &server.source(),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains(&claimed), "names the expected digest: {err}");
        assert!(err.contains("sha256"), "{err}");
        let dir = install_dir("obscura", "v0.2.2").unwrap();
        assert!(!dir.join("obscura").exists(), "no binary may be laid down");
        assert!(
            !dir.join("obscura.tmp").exists(),
            "and no staging file may be left behind"
        );
    }

    /// **N2 end to end: the path a stale mirror is most likely to take.** A
    /// host serving a body of a different length than the release document
    /// declares never reaches `verify_sha256`, so before this it surfaced as a
    /// bare ceiling breach — no advice, no mirror key, no next step — and with
    /// the wrong number called "this installer's ceiling".
    #[tokio::test]
    async fn a_body_longer_than_the_document_declares_is_refused_with_the_mirror_advice() {
        let archive = tiny_targz();
        let digest = sha256_hex(&archive);
        let declared = archive.len() - 1;
        let server = FixtureRelease::start_declaring(
            "o/p",
            "v0.2.2",
            "a.tar.gz",
            archive,
            Some(digest),
            declared,
        )
        .await;

        let home = TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        let source = server.source();
        // Precondition: the fixture is only hostile if it counts as a mirror,
        // which is what selects the with-mirror advice arm.
        assert!(
            source.mirror_configured(),
            "precondition: a mirror is in effect"
        );

        let err = install_release("obscura", "o/p", "v0.2.2", "a.tar.gz", "obscura", &source)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(&declared.to_string()),
            "names what the document declared: {err}"
        );
        assert!(
            !err.contains("ceiling"),
            "this is a disagreement about length, not a breach of this installer's \
             ceiling — and the ceiling is not the number to print: {err}"
        );
        assert!(
            err.contains("[general.browser.obscura] download_host"),
            "and it carries the same advice a digest mismatch would, because it means the \
             same thing: {err}"
        );
        let dir = install_dir("obscura", "v0.2.2").unwrap();
        assert!(!dir.join("obscura").exists(), "no binary may be laid down");
    }

    /// An asset the release does not list must fail at the METADATA step,
    /// **before anything is downloaded** — asserted by counting the fixture's
    /// asset requests, not by looking for a scratch file that no longer exists.
    #[tokio::test]
    async fn an_unlisted_asset_fails_before_any_download() {
        let archive = tiny_targz();
        let digest = sha256_hex(&archive);
        let server =
            FixtureRelease::start("o/p", "v0.2.2", "a.tar.gz", archive, Some(digest)).await;

        let home = TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        let err = install_release(
            "obscura",
            "o/p",
            "v0.2.2",
            "other.tar.gz",
            "obscura",
            &server.source(),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("other.tar.gz"), "{err}");
        assert!(err.contains("not in the release"), "{err}");
        assert_eq!(
            server.asset_requests(),
            0,
            "the metadata step refused, so nothing may have been downloaded"
        );
    }

    /// A minimal HTTP server speaking the two routes `install_release` uses,
    /// and counting the asset route so a test can assert a download did NOT
    /// happen.
    struct FixtureRelease {
        port: u16,
        asset_hits: Arc<AtomicUsize>,
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
            let declared = body.len();
            Self::start_declaring(repo, tag, asset, body, digest, declared).await
        }

        /// A fixture whose release document declares a length **different from
        /// the body it serves** — the shape a stale or substituting mirror
        /// takes, and the one `install_release` refuses before
        /// `verify_sha256` can be reached.
        async fn start_declaring(
            repo: &str,
            tag: &str,
            asset: &str,
            body: Vec<u8>,
            digest: Option<String>,
            declared: usize,
        ) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let api_path = format!("/repos/{repo}/releases/tags/{tag}");
            let asset_path = format!("/{repo}/releases/download/{tag}/{asset}");
            let mut entry = serde_json::json!({ "name": asset, "size": declared });
            if let Some(d) = digest {
                entry["digest"] = serde_json::Value::String(format!("sha256:{d}"));
            }
            let api_body = serde_json::json!({ "assets": [entry] }).to_string();
            let asset_hits = Arc::new(AtomicUsize::new(0));
            let hits_for_task = asset_hits.clone();
            let task = tokio::spawn(async move {
                loop {
                    let Ok((mut sock, _)) = listener.accept().await else {
                        return;
                    };
                    let api_path = api_path.clone();
                    let asset_path = asset_path.clone();
                    let api_body = api_body.clone();
                    let body = body.clone();
                    let hits = hits_for_task.clone();
                    tokio::spawn(async move {
                        use tokio::io::{AsyncReadExt, AsyncWriteExt};
                        let mut buf = vec![0u8; 4096];
                        let n = sock.read(&mut buf).await.unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]).to_string();
                        let target = req.split_whitespace().nth(1).unwrap_or("").to_string();
                        let (status, payload): (&str, Vec<u8>) = if target == api_path {
                            ("200 OK", api_body.into_bytes())
                        } else if target == asset_path {
                            hits.fetch_add(1, Ordering::SeqCst);
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
            Self {
                port,
                asset_hits,
                _task: task,
            }
        }

        fn host(&self) -> String {
            format!("http://127.0.0.1:{}", self.port)
        }

        /// Both halves pointed at the fixture. Production never builds a
        /// `ReleaseSource` this way — [`ReleaseSource::for_runtime`] is the only
        /// production constructor and it fixes `api_host`.
        fn source(&self) -> ReleaseSource {
            ReleaseSource {
                api_host: self.host(),
                asset_host: self.host(),
            }
        }

        fn asset_requests(&self) -> usize {
            self.asset_hits.load(Ordering::SeqCst)
        }
    }
}
