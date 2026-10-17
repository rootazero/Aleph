use super::*;
use shared_ui_logic::transcript::ALL_SEMANTIC_COLORS;

fn custom(root: &Path, name: &str, json: &str) {
    std::fs::create_dir_all(root.join("themes")).unwrap();
    std::fs::write(root.join("themes").join(format!("{name}.json")), json).unwrap();
}

// These tests catch lost validation and inheritance, rather than checking source text.
#[test]
fn community_overrides_preserve_semantics_and_lightness() {
    let root = tempfile::tempdir().unwrap();
    custom(
        root.path(),
        "linen",
        r##"{"schemaVersion":1,"name":"linen","extends":"paper","colors":{"accent":"#123456","toolOk":42},"chrome":{"userBg":"terminal"}}"##,
    );
    let p = load_named(root.path(), "linen", true).unwrap();
    assert!(p.is_light());
    assert_eq!(p.preset, Preset::Custom);
    assert_eq!(p.color(SemanticColor::Accent), Color::Rgb(18, 52, 86));
    assert_eq!(p.color(SemanticColor::ToolOk), Color::Indexed(42));
    assert_eq!(p.rgb(SemanticColor::ToolOk), None);
    let t = Theme::from_palette(p.clone());
    assert_eq!(t.heading, Color::Rgb(18, 52, 86));
    assert_eq!(t.primary, Color::Rgb(51, 53, 47));
    assert_eq!(t.user_bg, Color::Reset);
    assert_eq!(t.status_bg, t.background);
    assert_eq!(t.tool_success, p.color(SemanticColor::ToolOk));
}

#[test]
fn approved_builtins_reach_semantic_and_chrome_colors() {
    let root = tempfile::tempdir().unwrap();
    for (name, fg, accent, surface, light) in [
        (
            "amber",
            (255, 240, 199),
            (255, 194, 71),
            (48, 32, 20),
            false,
        ),
        (
            "tide",
            (223, 232, 241),
            (135, 197, 239),
            (30, 42, 57),
            false,
        ),
        (
            "jade",
            (224, 233, 226),
            (151, 210, 184),
            (34, 48, 42),
            false,
        ),
        (
            "iris",
            (235, 229, 240),
            (198, 172, 240),
            (46, 41, 57),
            false,
        ),
        ("paper", (51, 53, 47), (139, 86, 44), (232, 228, 217), true),
    ] {
        let p = load_named(root.path(), name, true).unwrap();
        assert_eq!(p.rgb(SemanticColor::Fg), Some(fg));
        assert_eq!(p.rgb(SemanticColor::Accent), Some(accent));
        assert_eq!(p.is_light(), light);
        let t = Theme::from_palette(p.clone());
        assert_eq!(t.surface, Color::Rgb(surface.0, surface.1, surface.2));
        // Every theme leaves the screen background to the terminal so its
        // configured opacity applies — paper included.
        assert_eq!(t.background, Color::Reset);
        assert_eq!(t.heading, p.color(SemanticColor::Accent));
        assert_eq!(t.status_bg, t.background);
        for role in ALL_SEMANTIC_COLORS {
            assert_ne!(p.color(*role), Color::Reset, "{name}: {role:?}");
        }
    }
}

#[test]
fn amber_uses_vivid_readable_roles_and_surfaces() {
    let palette = Palette::for_preset(Preset::Amber, true);
    for (role, rgb) in [
        (SemanticColor::Fg, (255, 240, 199)),
        (SemanticColor::Dim, (215, 172, 98)),
        (SemanticColor::Accent, (255, 194, 71)),
        (SemanticColor::UserBar, (255, 173, 47)),
        (SemanticColor::ToolRunning, (255, 179, 71)),
        (SemanticColor::ToolOk, (99, 209, 138)),
        (SemanticColor::ToolErr, (255, 116, 110)),
    ] {
        assert_eq!(palette.rgb(role), Some(rgb), "{role:?}");
    }

    let theme = Theme::from_palette(palette.clone());
    assert_eq!(theme.surface, Color::Rgb(48, 32, 20));
    assert_eq!(theme.user_bg, Color::Rgb(69, 42, 14));
    assert_eq!(theme.assistant, palette.color(SemanticColor::Fg));
    assert_eq!(theme.primary, palette.color(SemanticColor::Fg));
    assert_eq!(theme.reasoning, palette.color(SemanticColor::Dim));
    assert_eq!(theme.status_fg, palette.color(SemanticColor::Dim));
    assert_eq!(
        theme.tool_running,
        palette.color(SemanticColor::ToolRunning)
    );
    assert_eq!(theme.tool_success, palette.color(SemanticColor::ToolOk));
    assert_eq!(theme.tool_failed, palette.color(SemanticColor::ToolErr));
}

#[test]
fn malformed_documents_are_rejected_without_installing() {
    let root = tempfile::tempdir().unwrap();
    for json in [
        "not JSON",
        r#"{"schemaVersion":2,"name":"broken"}"#,
        r#"{"schemaVersion":1,"name":"other"}"#,
        r#"{"schemaVersion":1,"name":"broken","extends":"other"}"#,
        r#"{"schemaVersion":1,"name":"broken","unexpected":1}"#,
        r##"{"schemaVersion":1,"name":"broken","colors":{"forground":"#123456"}}"##,
        r#"{"schemaVersion":1,"name":"broken","chrome":{"unknown":0}}"#,
        r#"{"schemaVersion":1,"name":"broken","colors":{"fg":256}}"#,
        r#"{"schemaVersion":1,"name":"broken","colors":{"fg":-1}}"#,
        r#"{"schemaVersion":1,"name":"broken","colors":{"fg":1.5}}"#,
        r#"{"schemaVersion":1,"name":"broken","colors":{"fg":null}}"#,
        r#"{"schemaVersion":1,"name":"broken","colors":null}"#,
        r#"{"schemaVersion":1,"name":"broken","light":null}"#,
        r##"{"schemaVersion":1,"name":"broken","colors":{"fg":"#abcdefzz"}}"##,
        r##"{"schemaVersion":1,"name":"broken","colors":{"fg":"#fff"}}"##,
        r#"{"schemaVersion":1,"name":"broken","colors":{"fg":"red"}}"#,
        r#"{"schemaVersion":1,"name":"broken","name":"broken"}"#,
    ] {
        custom(root.path(), "broken", json);
        let mut state = SessionTheme::new(Palette::for_preset(Preset::Jade, true));
        assert!(
            state.select(root.path(), "broken", true).is_err(),
            "accepted {json}"
        );
        assert_eq!(state.palette.name(), "jade");
        assert_eq!(
            state.palette.color(SemanticColor::Accent),
            Color::Rgb(151, 210, 184)
        );
    }
}

#[test]
fn names_and_size_limit_are_enforced_before_reading() {
    let root = tempfile::tempdir().unwrap();
    for name in [
        "",
        "../escape",
        "/absolute",
        "a/b",
        "a\\b",
        "A-theme",
        "-theme",
        "a.json",
        "list",
        "reload",
        &"a".repeat(65),
    ] {
        assert!(
            load_named(root.path(), name, true).is_err(),
            "accepted {name:?}"
        );
    }
    custom(root.path(), "large", &" ".repeat(128 * 1024 + 1));
    assert!(load_named(root.path(), "large", true)
        .unwrap_err()
        .contains("128 KiB"));
    custom(
        root.path(),
        "amber",
        r#"{"schemaVersion":1,"name":"amber","colors":{"fg":0}}"#,
    );
    assert_eq!(
        load_named(root.path(), "amber", true)
            .unwrap()
            .rgb(SemanticColor::Fg),
        Some((255, 240, 199))
    );
    let (names, warnings) = available_themes_at(root.path(), true);
    assert!(names.contains(&"amber".to_owned()));
    assert!(warnings.iter().any(|w| w.contains("reserved")));
    assert!(!names.contains(&"large".to_owned()));
}

#[test]
fn boot_falls_through_invalid_choices_and_collects_warnings() {
    let root = tempfile::tempdir().unwrap();
    custom(
        root.path(),
        "linen",
        r#"{"schemaVersion":1,"name":"linen","extends":"paper"}"#,
    );
    let state = boot_at(root.path(), Some("missing"), Some("linen"), true);
    assert_eq!(state.palette.name(), "linen");
    assert_eq!(state.warnings.len(), 1);
    let state = boot_at(root.path(), Some("tide"), Some("linen"), true);
    assert_eq!(state.palette.name(), "tide");
    assert!(state.warnings.is_empty());
    let state = boot_at(root.path(), Some("missing"), Some("../bad"), true);
    assert_eq!(state.palette.name(), "amber");
    assert_eq!(state.warnings.len(), 2);
}

#[test]
fn remembering_and_reloading_a_community_theme_only_touches_owned_root() {
    let root = tempfile::tempdir().unwrap();
    custom(
        root.path(),
        "linen",
        r#"{"schemaVersion":1,"name":"linen","extends":"paper"}"#,
    );
    let file = remember_theme_at(root.path(), "linen", true).unwrap();
    assert_eq!(file, root.path().join("tui-theme"));
    let stored = std::fs::read_to_string(file).unwrap();
    assert_eq!(
        boot_at(root.path(), None, Some(&stored), true)
            .palette
            .name(),
        "linen"
    );
    remember_theme_at(root.path(), "light", true).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("tui-theme")).unwrap(),
        "light"
    );
    assert!(remember_theme_at(root.path(), "../bad", true).is_err());
    assert_eq!(
        std::fs::read_to_string(root.path().join("tui-theme")).unwrap(),
        "light"
    );
}

#[test]
fn capability_signals_and_explicit_overrides_are_pure() {
    assert!(detect_truecolor(None, None, Some("session"), Some("dumb")));
    assert!(detect_truecolor(
        None,
        Some("truecolor"),
        None,
        Some("dumb")
    ));
    assert!(detect_truecolor(None, Some("24bit"), None, None));
    assert!(detect_truecolor(None, None, None, Some("xterm-direct")));
    assert!(detect_truecolor(None, None, None, Some("xterm-kitty")));
    assert!(!detect_truecolor(None, None, None, Some("dumb")));
    assert!(!detect_truecolor(None, None, None, Some("xterm-256color")));
    assert!(detect_truecolor(
        Some("truecolor"),
        None,
        None,
        Some("dumb")
    ));
    for mode in ["ansi", "256"] {
        assert!(!detect_truecolor(
            Some(mode),
            Some("truecolor"),
            Some("session"),
            Some("xterm-direct")
        ));
    }
}

#[test]
fn terminal_and_fallback_palettes_emit_no_rgb() {
    for preset in Preset::ALL {
        for truecolor in [false, true] {
            let p = Palette::for_preset(*preset, truecolor);
            if truecolor && *preset != Preset::Terminal {
                continue;
            }
            for role in ALL_SEMANTIC_COLORS {
                assert_eq!(p.rgb(*role), None);
                assert!(!matches!(p.color(*role), Color::Rgb(..)));
            }
            let t = Theme::from_palette(p);
            for color in [
                t.background,
                t.surface,
                t.selection_bg,
                t.user_bg,
                t.status_bg,
                t.border,
                t.border_focused,
            ] {
                assert!(!matches!(color, Color::Rgb(..)));
            }
        }
    }
    let amber = Palette::for_preset(Preset::Amber, false);
    let tide = Palette::for_preset(Preset::Tide, false);
    assert_ne!(
        amber.color(SemanticColor::Accent),
        tide.color(SemanticColor::Accent)
    );
}

#[test]
fn default_and_compatibility_names_are_stable() {
    assert_eq!(Preset::default(), Preset::Amber);
    for preset in Preset::ALL {
        assert_eq!(Preset::parse(preset.name()), Some(*preset));
    }
    assert_eq!(
        Palette::for_preset(Preset::Dark, true).rgb(SemanticColor::Fg),
        Some((255, 240, 199))
    );
    assert!(Palette::for_preset(Preset::Light, true).is_light());
}

#[test]
fn shipped_example_uses_the_production_loader() {
    let root = tempfile::tempdir().unwrap();
    custom(
        root.path(),
        "example",
        include_str!("../../../themes/example.json"),
    );
    let p = load_named(root.path(), "example", true).unwrap();
    assert!(p.is_light());
    let (names, warnings) = available_themes_at(root.path(), true);
    assert!(names.contains(&"example".to_owned()));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn spinner_frames_still_come_from_shared_roster() {
    for tick in 0..SPINNER_FRAMES.len() * 3 {
        assert_eq!(
            spinner_at(tick),
            shared_ui_logic::transcript::spinner_frame(
                tick as u64 * shared_ui_logic::transcript::SPINNER_PERIOD_MS
            )
        );
    }
}
