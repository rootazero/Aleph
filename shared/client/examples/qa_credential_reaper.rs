//! QA-only exact credential reaper for the Task #56 real-OS-keyring QA run.
//!
//! The QA shell (`qa-owned-root` + `--qa-credential-backend os`) files its
//! paired-device credential in the REAL OS keyring under a random QA-only
//! service namespace, `aleph-qa.device-credential.<nonce>`. This tool is the
//! harness's exact precheck / cleanup / post-check for that ONE entry. It is
//! deliberately not a credential browser:
//!
//! * `--mode check-absent`  : succeeds only if `load_for_connect()` is `None`.
//! * `--mode delete-exact`  : `delete_for_connect()`, then `load_for_connect()`
//!   must be `None` and a second `delete_for_connect()` must report "nothing
//!   there". It never writes (no overwrite, no compensating secret).
//!
//! Inputs: `--owned-root <abs dir> --nonce <32..=128 lowercase hex>
//! --gateway <ws loopback url> --mode <check-absent|delete-exact>`; each exactly
//! once, space-separated form only, no environment / HOME / CWD fallback.
//!
//! Everything is proven BEFORE any keyring call:
//! * the root is a direct child of the canonical OS temp dir, named
//!   `aleph-shell-qa-*`, with no `.aleph` component, no reparse point / junction
//!   / alias anywhere on its path, and an exact `.qa-owned` file == nonce;
//! * the keyring service is derived ONLY from the nonce
//!   (`DeviceCredentialStore::qa_service_for_nonce`); there is no service flag,
//!   and the client re-validates the exact namespace, so the production service
//!   is unreachable from here;
//! * the account (normalised gateway endpoint) must equal the one derived from
//!   `<root>/.aleph/.desktop-shell-panel-target` (an `http(s)://host:port`
//!   origin, converted to `ws(s)`), and must be a `ws://` loopback endpoint.
//!   Any port (including the live Gateway's 18790) is fine: the random service
//!   is what isolates the entry.
//!
//! Output is fixed: success prints one JSON line on stdout
//! (`{"mode":"...","absent":true}`); failure prints `{"ok":false,"code":"E_..."}`
//! on stderr and exits non-zero. No credential, service, account, path or
//! provider error text is ever printed. Not built without `--features
//! qa-service` (then `main` refuses). Never run in production / CI by default.

#[cfg(not(feature = "qa-service"))]
fn main() -> std::process::ExitCode {
    eprintln!("{{\"ok\":false,\"code\":\"E_FEATURE\"}}");
    std::process::ExitCode::from(2)
}

#[cfg(feature = "qa-service")]
fn main() -> std::process::ExitCode {
    imp::run(std::env::args_os())
}

#[cfg(feature = "qa-service")]
mod imp {
    use std::ffi::OsString;
    use std::fs;
    use std::io::Read;
    use std::path::{Component, Path, PathBuf};
    use std::process::ExitCode;

    use aleph_client::DeviceCredentialStore;

    /// Required leaf-name prefix of the owned root (== the shell's `ROOT_PREFIX`).
    const ROOT_PREFIX: &str = "aleph-shell-qa-";
    /// Ownership marker inside the root; content == nonce.
    const NONCE_FILE: &str = ".qa-owned";
    const AUTH_DIR: &str = ".aleph";
    const TARGET_MARKER: &str = ".desktop-shell-panel-target";
    const NONCE_FILE_MAX_BYTES: u64 = 256;
    const MARKER_MAX_BYTES: u64 = 512;
    const NONCE_MIN_LEN: usize = 32;
    const NONCE_MAX_LEN: usize = 128;

    /// Fixed refusal codes. Nothing else is ever printed on failure.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Code {
        Usage,
        Nonce,
        TempUnavailable,
        RootShape,
        RootMissing,
        RootNotDir,
        RootLink,
        RootNotTemp,
        RootPrefix,
        OwnerMissing,
        OwnerInvalid,
        OwnerMismatch,
        MarkerInvalid,
        GatewayInvalid,
        GatewayNotLoopback,
        GatewayMismatch,
        Keyring,
        Present,
        NotDeleted,
    }

    impl Code {
        pub(super) fn as_str(self) -> &'static str {
            match self {
                Self::Usage => "E_USAGE",
                Self::Nonce => "E_NONCE",
                Self::TempUnavailable => "E_TEMP_UNAVAILABLE",
                Self::RootShape => "E_ROOT_SHAPE",
                Self::RootMissing => "E_ROOT_MISSING",
                Self::RootNotDir => "E_ROOT_NOT_DIR",
                Self::RootLink => "E_ROOT_LINK",
                Self::RootNotTemp => "E_ROOT_NOT_TEMP_CHILD",
                Self::RootPrefix => "E_ROOT_PREFIX",
                Self::OwnerMissing => "E_OWNER_MISSING",
                Self::OwnerInvalid => "E_OWNER_INVALID",
                Self::OwnerMismatch => "E_OWNER_MISMATCH",
                Self::MarkerInvalid => "E_TARGET_MARKER_INVALID",
                Self::GatewayInvalid => "E_GATEWAY_INVALID",
                Self::GatewayNotLoopback => "E_GATEWAY_NOT_LOOPBACK",
                Self::GatewayMismatch => "E_GATEWAY_MISMATCH",
                Self::Keyring => "E_KEYRING",
                Self::Present => "E_PRESENT",
                Self::NotDeleted => "E_NOT_DELETED",
            }
        }

        /// 1 = the keyring was reached and is not in the expected state;
        /// 2 = refused before any keyring call.
        fn exit(self) -> u8 {
            match self {
                Self::Keyring | Self::Present | Self::NotDeleted => 1,
                _ => 2,
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Mode {
        CheckAbsent,
        DeleteExact,
    }

    impl Mode {
        fn as_str(self) -> &'static str {
            match self {
                Self::CheckAbsent => "check-absent",
                Self::DeleteExact => "delete-exact",
            }
        }
    }

    #[derive(Debug)]
    pub(super) struct Args {
        pub(super) root: PathBuf,
        pub(super) nonce: String,
        pub(super) gateway: String,
        pub(super) mode: Mode,
    }

    pub(crate) fn run<I, S>(argv: I) -> ExitCode
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        match execute(argv) {
            Ok(mode) => {
                println!("{{\"mode\":\"{}\",\"absent\":true}}", mode.as_str());
                ExitCode::SUCCESS
            }
            Err(code) => {
                eprintln!("{{\"ok\":false,\"code\":\"{}\"}}", code.as_str());
                ExitCode::from(code.exit())
            }
        }
    }

    fn execute<I, S>(argv: I) -> Result<Mode, Code>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        // ---- pure + local-filesystem proofs; no keyring call until all pass ----
        let args = parse_args(argv)?;
        validate_nonce(&args.nonce)?;
        let service =
            DeviceCredentialStore::qa_service_for_nonce(&args.nonce).map_err(|_| Code::Nonce)?;
        let temp = fs::canonicalize(std::env::temp_dir()).map_err(|_| Code::TempUnavailable)?;
        let temp = strip_verbatim(&temp);
        let root = prove_root(&args.root, &args.nonce, &temp)?;
        let origin = read_target_marker(&root)?;
        let marker_ws = marker_origin_to_ws(&origin)?;
        let store = target_store(&args.gateway, &marker_ws, &service)?;

        // ---- keyring phase: exactly this (service, account) ----
        match args.mode {
            Mode::CheckAbsent => ensure_absent(&store)?,
            Mode::DeleteExact => {
                store.delete_for_connect().map_err(|_| Code::Keyring)?;
                ensure_absent(&store).map_err(|c| {
                    if c == Code::Present {
                        Code::NotDeleted
                    } else {
                        c
                    }
                })?;
                // Idempotence proof: a second delete must find nothing.
                match store.delete_for_connect() {
                    Ok(false) => {}
                    Ok(true) => return Err(Code::NotDeleted),
                    Err(_) => return Err(Code::Keyring),
                }
            }
        }
        Ok(args.mode)
    }

    /// `Ok` only when the keyring is healthy AND holds nothing. A present (even
    /// valid) credential is dropped unread; an unreadable one is a keyring error.
    fn ensure_absent(store: &DeviceCredentialStore) -> Result<(), Code> {
        match store.load_for_connect() {
            Ok(None) => Ok(()),
            Ok(Some(_)) => Err(Code::Present),
            Err(_) => Err(Code::Keyring),
        }
    }

    // ------------------------------------------------------------------
    // Pure validation (no filesystem / OS I/O)
    // ------------------------------------------------------------------

    /// All four flags exactly once; space-separated form only; anything else
    /// (unknown flag, `--flag=value`, missing/flag-looking value) is refused.
    pub(super) fn parse_args<I, S>(argv: I) -> Result<Args, Code>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut root: Option<PathBuf> = None;
        let mut nonce: Option<String> = None;
        let mut gateway: Option<String> = None;
        let mut mode: Option<Mode> = None;
        let mut it = argv.into_iter().map(Into::<OsString>::into).skip(1);
        while let Some(flag) = it.next() {
            let flag = flag.into_string().map_err(|_| Code::Usage)?;
            if !matches!(
                flag.as_str(),
                "--owned-root" | "--nonce" | "--gateway" | "--mode"
            ) {
                return Err(Code::Usage);
            }
            let value = it.next().ok_or(Code::Usage)?;
            if value.is_empty() || value.to_string_lossy().starts_with("--") {
                return Err(Code::Usage);
            }
            match flag.as_str() {
                "--owned-root" => {
                    if root.replace(PathBuf::from(value)).is_some() {
                        return Err(Code::Usage);
                    }
                }
                "--nonce" => {
                    let v = value.into_string().map_err(|_| Code::Usage)?;
                    if nonce.replace(v).is_some() {
                        return Err(Code::Usage);
                    }
                }
                "--gateway" => {
                    let v = value.into_string().map_err(|_| Code::Usage)?;
                    if gateway.replace(v).is_some() {
                        return Err(Code::Usage);
                    }
                }
                _ => {
                    let m = match value.to_str() {
                        Some("check-absent") => Mode::CheckAbsent,
                        Some("delete-exact") => Mode::DeleteExact,
                        _ => return Err(Code::Usage),
                    };
                    if mode.replace(m).is_some() {
                        return Err(Code::Usage);
                    }
                }
            }
        }
        Ok(Args {
            root: root.ok_or(Code::Usage)?,
            nonce: nonce.ok_or(Code::Usage)?,
            gateway: gateway.ok_or(Code::Usage)?,
            mode: mode.ok_or(Code::Usage)?,
        })
    }

    /// 32..=128 lowercase hex (the same rule the client enforces on the service).
    pub(super) fn validate_nonce(nonce: &str) -> Result<(), Code> {
        let ok = (NONCE_MIN_LEN..=NONCE_MAX_LEN).contains(&nonce.len())
            && nonce
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if ok {
            Ok(())
        } else {
            Err(Code::Nonce)
        }
    }

    /// Lexical root rules against the (already canonical) temp dir: absolute,
    /// normalised, no `.aleph` component, direct child of temp, `aleph-shell-qa-*`.
    pub(super) fn check_root_lexical(root: &Path, canonical_temp: &Path) -> Result<(), Code> {
        if !root.is_absolute() {
            return Err(Code::RootShape);
        }
        if root
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
        {
            return Err(Code::RootShape);
        }
        if has_aleph_component(root) {
            return Err(Code::RootShape);
        }
        match root.parent() {
            Some(p) if normalized(p) == normalized(canonical_temp) => {}
            _ => return Err(Code::RootNotTemp),
        }
        let leaf = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if leaf.starts_with(ROOT_PREFIX) && leaf.len() > ROOT_PREFIX.len() {
            Ok(())
        } else {
            Err(Code::RootPrefix)
        }
    }

    /// `http(s)://host[:port]` (no userinfo / path / query / fragment) ->
    /// `ws(s)://host[:port]`. Exact lowercase schemes only.
    pub(super) fn marker_origin_to_ws(origin: &str) -> Result<String, Code> {
        let (scheme, rest) = origin.split_once("://").ok_or(Code::MarkerInvalid)?;
        let ws = match scheme {
            "http" => "ws",
            "https" => "wss",
            _ => return Err(Code::MarkerInvalid),
        };
        if rest.is_empty()
            || rest
                .chars()
                .any(|c| matches!(c, '/' | '?' | '#' | '@' | '\\') || c.is_whitespace() || c.is_control())
        {
            return Err(Code::MarkerInvalid);
        }
        Ok(format!("{ws}://{rest}"))
    }

    /// Build the one store this tool may touch. Constructing it does no OS I/O.
    /// The gateway must be a `ws://` loopback endpoint (no userinfo) whose
    /// normalised account equals the one derived from the target marker.
    pub(super) fn target_store(
        gateway: &str,
        marker_ws: &str,
        service: &str,
    ) -> Result<DeviceCredentialStore, Code> {
        if gateway.contains('@') || !gateway.to_ascii_lowercase().starts_with("ws://") {
            return Err(Code::GatewayInvalid);
        }
        let store = DeviceCredentialStore::for_gateway_in_service(gateway, service)
            .map_err(|_| Code::GatewayInvalid)?;
        let from_marker = DeviceCredentialStore::for_gateway_in_service(marker_ws, service)
            .map_err(|_| Code::MarkerInvalid)?;
        let host = store
            .account()
            .strip_prefix("ws://")
            .and_then(|hp| hp.rsplit_once(':'))
            .map(|(h, _)| h)
            .ok_or(Code::GatewayInvalid)?;
        if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
            return Err(Code::GatewayNotLoopback);
        }
        if store.account() != from_marker.account() {
            return Err(Code::GatewayMismatch);
        }
        Ok(store)
    }

    fn has_aleph_component(p: &Path) -> bool {
        p.components().any(|c| match c {
            Component::Normal(s) => s.to_string_lossy().eq_ignore_ascii_case(".aleph"),
            _ => false,
        })
    }

    /// Component list for equality; case-insensitive on Windows.
    fn normalized(p: &Path) -> Vec<String> {
        p.components()
            .map(|c| {
                let s = c.as_os_str().to_string_lossy().into_owned();
                if cfg!(windows) {
                    s.to_ascii_lowercase()
                } else {
                    s
                }
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // Local filesystem proofs (read-only; this tool never writes a file)
    // ------------------------------------------------------------------

    fn is_link_or_reparse(md: &fs::Metadata) -> bool {
        if md.file_type().is_symlink() {
            return true;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
            if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return true;
            }
        }
        false
    }

    /// `\\?\C:\x` -> `C:\x` (drive-letter verbatim only; anything else is left
    /// alone and then fails the temp-dir comparison, i.e. fails closed).
    fn strip_verbatim(p: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            let s = p.to_string_lossy();
            if let Some(rest) = s.strip_prefix(r"\\?\") {
                let b = rest.as_bytes();
                if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                    return PathBuf::from(rest);
                }
            }
        }
        p.to_path_buf()
    }

    /// Prove `root` is the harness-owned directory for `nonce`; returns its
    /// canonical form.
    fn prove_root(root: &Path, nonce: &str, canonical_temp: &Path) -> Result<PathBuf, Code> {
        let root = strip_verbatim(root);
        check_root_lexical(&root, canonical_temp)?;

        let md = fs::symlink_metadata(&root).map_err(|_| Code::RootMissing)?;
        if is_link_or_reparse(&md) {
            return Err(Code::RootLink);
        }
        if !md.is_dir() {
            return Err(Code::RootNotDir);
        }
        // Any symlink/junction/8.3 alias in the chain makes canonical != lexical.
        let canon = strip_verbatim(&fs::canonicalize(&root).map_err(|_| Code::RootMissing)?);
        if normalized(&canon) != normalized(&root) {
            return Err(Code::RootLink);
        }
        check_root_lexical(&canon, canonical_temp)?;
        // Belt and braces: no ancestor (up to the drive/filesystem root) is a link.
        for anc in canon.ancestors() {
            let md = fs::symlink_metadata(anc).map_err(|_| Code::RootLink)?;
            if is_link_or_reparse(&md) {
                return Err(Code::RootLink);
            }
        }

        // Exact ownership file == nonce.
        let owner = canon.join(NONCE_FILE);
        let md = fs::symlink_metadata(&owner).map_err(|_| Code::OwnerMissing)?;
        if is_link_or_reparse(&md) || !md.is_file() || md.len() > NONCE_FILE_MAX_BYTES {
            return Err(Code::OwnerInvalid);
        }
        let mut buf = String::new();
        fs::File::open(&owner)
            .and_then(|f| f.take(NONCE_FILE_MAX_BYTES).read_to_string(&mut buf))
            .map_err(|_| Code::OwnerInvalid)?;
        if buf.trim_end_matches(['\r', '\n']) != nonce {
            return Err(Code::OwnerMismatch);
        }
        Ok(canon)
    }

    /// `<root>/.aleph/.desktop-shell-panel-target` content (the panel origin),
    /// after proving `.aleph` and the marker are plain, small, non-link entries
    /// of the proven root.
    fn read_target_marker(root: &Path) -> Result<String, Code> {
        let dir = root.join(AUTH_DIR);
        let md = fs::symlink_metadata(&dir).map_err(|_| Code::MarkerInvalid)?;
        if is_link_or_reparse(&md) || !md.is_dir() {
            return Err(Code::MarkerInvalid);
        }
        let file = dir.join(TARGET_MARKER);
        let md = fs::symlink_metadata(&file).map_err(|_| Code::MarkerInvalid)?;
        if is_link_or_reparse(&md) || !md.is_file() || md.len() > MARKER_MAX_BYTES {
            return Err(Code::MarkerInvalid);
        }
        let mut buf = String::new();
        fs::File::open(&file)
            .and_then(|f| f.take(MARKER_MAX_BYTES).read_to_string(&mut buf))
            .map_err(|_| Code::MarkerInvalid)?;
        Ok(buf.trim_end_matches(['\r', '\n']).to_owned())
    }

    // ------------------------------------------------------------------
    // Tests: pure only (no filesystem, no keyring, no process)
    // ------------------------------------------------------------------

    #[cfg(test)]
    mod tests {
        use super::*;

        const NONCE: &str = "0123456789abcdef0123456789abcdef";

        fn fake_temp() -> PathBuf {
            if cfg!(windows) {
                PathBuf::from(r"C:\Fake\Temp")
            } else {
                PathBuf::from("/fake/tmp")
            }
        }

        fn argv(rest: &[&str]) -> Vec<OsString> {
            std::iter::once("reaper")
                .chain(rest.iter().copied())
                .map(OsString::from)
                .collect()
        }

        fn good(root: &str) -> Vec<&str> {
            vec![
                "--owned-root",
                root,
                "--nonce",
                NONCE,
                "--gateway",
                "ws://127.0.0.1:18790",
                "--mode",
                "check-absent",
            ]
        }

        #[test]
        fn args_require_each_flag_exactly_once() {
            let a = parse_args(argv(&good("X"))).unwrap();
            assert_eq!(a.mode, Mode::CheckAbsent);
            assert_eq!(a.nonce, NONCE);
            let mut dele = good("X");
            dele[7] = "delete-exact";
            assert_eq!(parse_args(argv(&dele)).unwrap().mode, Mode::DeleteExact);

            // Missing each flag.
            for skip in 0..4 {
                let g = good("X");
                let mut v = Vec::new();
                for (i, pair) in g.chunks(2).enumerate() {
                    if i != skip {
                        v.extend_from_slice(pair);
                    }
                }
                assert_eq!(parse_args(argv(&v)).err(), Some(Code::Usage), "skip {skip}");
            }
            // Duplicate, unknown, `=` form, bad mode, flag-looking value, no service flag.
            let mut dup = good("X");
            dup.extend(["--nonce", NONCE]);
            let mut svc = good("X");
            svc.extend(["--service", "x"]);
            let mut badmode = good("X");
            badmode[7] = "delete";
            let mut eq = good("X");
            eq[2] = "--nonce=abc";
            let mut flaglike = good("X");
            flaglike[1] = "--nonce";
            for bad in [dup, svc, badmode, eq, flaglike] {
                assert_eq!(parse_args(argv(&bad)).err(), Some(Code::Usage), "{bad:?}");
            }
        }

        #[test]
        fn nonce_must_be_32_to_128_lowercase_hex() {
            for n in [32, 64, 128] {
                assert!(validate_nonce(&"a".repeat(n)).is_ok());
            }
            let bad: Vec<String> = vec![
                String::new(),
                "abc".to_owned(),
                "a".repeat(31),
                "a".repeat(129),
                "A".repeat(32),
                "g".repeat(32),
                format!("{}\n", "a".repeat(32)),
            ];
            for n in &bad {
                assert_eq!(validate_nonce(n), Err(Code::Nonce), "{n:?}");
            }
        }

        #[test]
        fn root_lexical_rules() {
            let t = fake_temp();
            assert_eq!(check_root_lexical(&t.join("aleph-shell-qa-x1"), &t), Ok(()));
            // Not a direct child of temp.
            assert_eq!(
                check_root_lexical(&t.join("sub").join("aleph-shell-qa-x1"), &t),
                Err(Code::RootNotTemp)
            );
            let other = if cfg!(windows) { PathBuf::from(r"C:\Other") } else { PathBuf::from("/other") };
            assert_eq!(
                check_root_lexical(&other.join("aleph-shell-qa-x1"), &t),
                Err(Code::RootNotTemp)
            );
            // Wrong / production-looking / empty prefix.
            for leaf in ["aleph-shell-qa-", "aleph-shell-qb-x", "aleph-qa.device-credential.ab", ".aleph", "x"] {
                let r = check_root_lexical(&t.join(leaf), &t);
                assert!(r.is_err(), "{leaf}");
            }
            // `.aleph` component anywhere, `.`/`..`, relative.
            assert_eq!(
                check_root_lexical(&t.join(".aleph").join("aleph-shell-qa-x1"), &t),
                Err(Code::RootShape)
            );
            assert_eq!(
                check_root_lexical(&t.join("..").join("aleph-shell-qa-x1"), &t),
                Err(Code::RootShape)
            );
            assert_eq!(
                check_root_lexical(Path::new("aleph-shell-qa-x1"), &t),
                Err(Code::RootShape)
            );
        }

        #[test]
        fn marker_origin_converts_http_to_ws_only() {
            assert_eq!(marker_origin_to_ws("http://127.0.0.1:18790").as_deref(), Ok("ws://127.0.0.1:18790"));
            assert_eq!(marker_origin_to_ws("https://gw.example.com:8443").as_deref(), Ok("wss://gw.example.com:8443"));
            for bad in [
                "",
                "ws://127.0.0.1:1",
                "ftp://h",
                "http://",
                "http://h/path",
                "http://u@h",
                "http://h?x",
                "http://h#x",
                "http://h h",
                "http://h\\x",
                "HTTP://h",
                "127.0.0.1:1",
            ] {
                assert_eq!(marker_origin_to_ws(bad), Err(Code::MarkerInvalid), "{bad:?}");
            }
        }

        fn service() -> String {
            DeviceCredentialStore::qa_service_for_nonce(NONCE).unwrap()
        }

        #[test]
        fn service_is_derived_from_the_nonce_only() {
            assert_eq!(service(), format!("aleph-qa.device-credential.{NONCE}"));
            assert!(DeviceCredentialStore::qa_service_for_nonce("abc").is_err());
        }

        #[test]
        fn gateway_must_match_marker_and_be_ws_loopback() {
            let svc = service();
            // Any port incl. the live Gateway's 18790; path/case/default normalised away.
            for (gw, marker) in [
                ("ws://127.0.0.1:18790", "ws://127.0.0.1:18790"),
                ("ws://127.0.0.1:18790/ws", "ws://127.0.0.1:18790"),
                ("WS://LOCALHOST:5000", "ws://localhost:5000"),
                ("ws://[::1]:7", "ws://[::1]:7"),
            ] {
                let s = target_store(gw, marker, &svc).unwrap_or_else(|e| panic!("{gw}: {e:?}"));
                assert_eq!(s.service(), svc);
            }
            // Port / host mismatch with the marker.
            assert_eq!(
                target_store("ws://127.0.0.1:18790", "ws://127.0.0.1:18791", &svc).err(),
                Some(Code::GatewayMismatch)
            );
            assert_eq!(
                target_store("ws://127.0.0.1:18790", "ws://localhost:18790", &svc).err(),
                Some(Code::GatewayMismatch)
            );
            // wss / http / remote / userinfo / garbage gateways.
            assert_eq!(target_store("wss://127.0.0.1:1", "wss://127.0.0.1:1", &svc).err(), Some(Code::GatewayInvalid));
            assert_eq!(target_store("http://127.0.0.1:1", "ws://127.0.0.1:1", &svc).err(), Some(Code::GatewayInvalid));
            assert_eq!(target_store("ws://u@127.0.0.1:1", "ws://127.0.0.1:1", &svc).err(), Some(Code::GatewayInvalid));
            assert_eq!(target_store("nonsense", "ws://127.0.0.1:1", &svc).err(), Some(Code::GatewayInvalid));
            assert_eq!(
                target_store("ws://gw.example.com:80", "ws://gw.example.com:80", &svc).err(),
                Some(Code::GatewayNotLoopback)
            );
            // The production service can never reach a store.
            assert_eq!(
                target_store("ws://127.0.0.1:1", "ws://127.0.0.1:1", "aleph-client.device-credential").err(),
                Some(Code::GatewayInvalid)
            );
        }

        #[test]
        fn refusal_output_is_a_fixed_code_set() {
            for c in [
                Code::Usage,
                Code::Nonce,
                Code::RootShape,
                Code::OwnerMismatch,
                Code::GatewayMismatch,
                Code::Keyring,
                Code::Present,
                Code::NotDeleted,
            ] {
                let s = c.as_str();
                assert!(s.starts_with("E_") && s.bytes().all(|b| b.is_ascii_uppercase() || b == b'_'));
            }
        }
    }
}
