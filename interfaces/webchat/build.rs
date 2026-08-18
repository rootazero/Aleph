fn main() {
    // Expose the workspace version (root VERSION file) as ALEPH_VERSION so the
    // panic-recovery crash log can record which build crashed. Mirrors the root
    // build.rs version injection; CLAUDE.md forbids hardcoding version numbers.
    let version = std::fs::read_to_string("../../VERSION")
        .map_or_else(|_| "unknown".to_string(), |s| s.trim().to_string());
    println!("cargo:rustc-env=ALEPH_VERSION={version}");
    println!("cargo:rerun-if-changed=../../VERSION");

    // `interpolate_display` generates the `Display` impls that let `t_string!`
    // resolve a key carrying `{{ }}` placeholders. Without it the macro accepts
    // only plain strings, and a sentence with a number in the middle has to be
    // split around the number in Rust — which hard-codes English/Chinese word
    // order at the seam, the exact thing a locale file exists to avoid. It is a
    // build option rather than a cargo feature in leptos_i18n 0.6.
    let cfg = leptos_i18n_build::Config::new("en")
        .expect("Failed to create i18n config")
        .add_locale("zh")
        .expect("Failed to add zh locale")
        .parse_options(leptos_i18n_build::ParseOptions::new().interpolate_display(true));

    let infos = leptos_i18n_build::TranslationsInfos::parse(cfg)
        .expect("Failed to parse i18n translations");

    infos.rerun_if_locales_changed();

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    infos
        .generate_i18n_module(out_dir.join("i18n"))
        .expect("Failed to generate i18n module");
}
