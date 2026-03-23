fn main() {
    let cfg = leptos_i18n_build::Config::new("en")
        .expect("Failed to create i18n config")
        .add_locale("zh")
        .expect("Failed to add zh locale");

    let infos = leptos_i18n_build::TranslationsInfos::parse(cfg)
        .expect("Failed to parse i18n translations");

    infos.rerun_if_locales_changed();

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    infos
        .generate_i18n_module(out_dir.join("i18n"))
        .expect("Failed to generate i18n module");
}
