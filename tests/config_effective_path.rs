//! `--config <path>` must pin the config file for the whole process, not just
//! for the one loader that happened to be handed the flag.
//!
//! This is its own integration binary on purpose. The effective path is a
//! process-wide pin: whoever sets it changes what every later `Config::load()`
//! in that process sees. One pin per process means **one `#[test]` per file** —
//! a second test here would silently observe the first one's pin instead of
//! its own. Put any further pinning test in its own `tests/*.rs`.

use std::fs;

use alephcore::Config;

#[test]
fn the_pinned_config_path_is_the_one_the_process_reads() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Isolate from the developer's real ~/.aleph so `default_path()` is
    // deterministic and nothing here can read or write it.
    std::env::set_var("ALEPH_HOME", tmp.path());

    let pinned = tmp.path().join("deployment.toml");
    fs::write(&pinned, "default_hotkey = \"Ctrl+Alt+Pinned\"\n").expect("write pinned config");

    // Nothing pinned yet: an ordinary `aleph-server` with no `--config` must
    // keep resolving to the default location.
    assert_eq!(
        Config::effective_path(),
        Config::default_path(),
        "an unpinned process must still resolve to the default config file"
    );

    Config::set_effective_path(pinned.clone()).expect("the first pin must take");

    assert_eq!(Config::effective_path(), pinned);
    assert_ne!(
        Config::effective_path(),
        Config::default_path(),
        "the whole point of --config is that it is not the default file"
    );

    // The payoff. `--config` used to reach `FullGatewayConfig::load` and stop
    // there: `ConfigPatcher`, `AgentManager` and every `Config::load()` caller
    // went on reading ~/.aleph/config.toml, so the server ran on one file while
    // the Panel reported — and wrote — another.
    let loaded = Config::load().expect("load must follow the pin");
    assert_eq!(
        loaded.default_hotkey, "Ctrl+Alt+Pinned",
        "Config::load() read some file other than the pinned one"
    );

    // A late second pin must lose rather than silently win: by then the first
    // path has already been handed to consumers that keep their own copy of it.
    let too_late = tmp.path().join("too-late.toml");
    assert!(
        Config::set_effective_path(too_late).is_err(),
        "a second pin must be rejected, not applied"
    );
    assert_eq!(
        Config::effective_path(),
        pinned,
        "the rejected pin must leave the first one in place"
    );
}
