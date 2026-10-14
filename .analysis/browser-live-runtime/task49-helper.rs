    /// The native fake encoder, compiled ONCE per test process. The source is
    /// `recording_test_encoder.rs` (std-only, include_str!-ed, not a module);
    /// `rustc` builds it shell-free into an owned temp dir and only the bytes
    /// are kept, so no `sh`/`cmd` and no pre-built artifact are involved.
    fn encoder_image() -> &'static [u8] {
        static IMAGE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        IMAGE.get_or_init(|| {
            let work = tempfile::tempdir().expect("encoder build dir");
            let src = work.path().join("recording_test_encoder.rs");
            let exe = work
                .path()
                .join(format!("encoder{}", std::env::consts::EXE_SUFFIX));
            assert!(src.starts_with(work.path()) && exe.starts_with(work.path()));
            std::fs::write(&src, include_str!("recording_test_encoder.rs")).expect("write src");
            let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
            let out = std::process::Command::new(rustc)
                // The manifest dir is only the toolchain anchor (rustup
                // proxies resolve overrides from the cwd); nothing is
                // written there — source and output are absolute temp paths.
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .args(["--edition", "2021", "-C", "debuginfo=0", "-o"])
                .arg(&exe)
                .arg(&src)
                .output()
                .expect("spawn rustc for the test encoder");
            assert!(
                out.status.success(),
                "compiling the test encoder failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
            std::fs::read(&exe).expect("read compiled encoder")
        })
    }

    /// Materialise the fake encoder for `mode` as `<dir>/<mode>[.exe]`; the
    /// encoder picks its behaviour from its own file stem (see
    /// `recording_test_encoder.rs`). Everything stays inside the owned dir.
    fn encoder_stub(dir: &tempfile::TempDir, mode: &str) -> PathBuf {
        const MODES: [&str; 5] = ["wedged", "slow", "failing", "short-lived", "no-touch"];
        assert!(MODES.contains(&mode), "unknown test encoder mode `{mode}`");
        let path = dir
            .path()
            .join(format!("{mode}{}", std::env::consts::EXE_SUFFIX));
        assert!(path.starts_with(dir.path()), "stub escapes its owned dir");
        std::fs::write(&path, encoder_image()).expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod stub");
        }
        path
    }

