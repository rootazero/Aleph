fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        .map_err(|e| std::io::Error::other(format!("Failed to create i18n config: {e}")))?
        .add_locale("zh")
        .map_err(|e| std::io::Error::other(format!("Failed to add zh locale: {e}")))?
        .parse_options(leptos_i18n_build::ParseOptions::new().interpolate_display(true));

    let infos = leptos_i18n_build::TranslationsInfos::parse(cfg)
        .map_err(|e| std::io::Error::other(format!("Failed to parse i18n translations: {e}")))?;

    infos.rerun_if_locales_changed();

    let out_dir = std::env::var_os("OUT_DIR")
        .ok_or_else(|| std::io::Error::other("OUT_DIR environment variable is not set"))?;
    let out_dir = std::path::PathBuf::from(out_dir);
    infos
        .generate_i18n_module(out_dir.join("i18n"))
        .map_err(|e| std::io::Error::other(format!("Failed to generate i18n module: {e}")))?;

    Ok(())
}
