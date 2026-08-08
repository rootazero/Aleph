#[cfg(test)]
mod spec3_tests {
    use super::super::*;

    #[test]
    fn injection_mode_default_is_hybrid() {
        assert_eq!(MemoryInjectionMode::default(), MemoryInjectionMode::Hybrid);
    }

    #[test]
    fn injection_mode_round_trips_json() {
        for mode in [
            MemoryInjectionMode::Context,
            MemoryInjectionMode::Tools,
            MemoryInjectionMode::Hybrid,
        ] {
            let s = serde_json::to_string(&mode).unwrap();
            let back: MemoryInjectionMode = serde_json::from_str(&s).unwrap();
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn injection_mode_serialises_lowercase() {
        assert_eq!(
            serde_json::to_string(&MemoryInjectionMode::Context).unwrap(),
            "\"context\""
        );
        assert_eq!(
            serde_json::to_string(&MemoryInjectionMode::Tools).unwrap(),
            "\"tools\""
        );
        assert_eq!(
            serde_json::to_string(&MemoryInjectionMode::Hybrid).unwrap(),
            "\"hybrid\""
        );
    }

    #[test]
    fn memory_config_default_injection_mode_is_hybrid() {
        let cfg = MemoryConfig::default();
        assert_eq!(cfg.injection_mode, MemoryInjectionMode::Hybrid);
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn dreaming_config_defaults_include_new_fields() {
        let config = DreamingConfig::default();
        assert_eq!(config.drift_max_pairs_per_run, 20);
        assert_eq!(config.skill_distill_max_per_cycle, 3);
    }

    #[test]
    fn dreaming_config_skill_distill_max_overridable_via_toml() {
        let toml_src = r#"
            skill_distill_max_per_cycle = 7
        "#;
        let config: DreamingConfig = toml::from_str(toml_src).expect("parse");
        assert_eq!(config.skill_distill_max_per_cycle, 7);
    }

    #[test]
    fn assembler_config_defaults_sane() {
        let c = AssemblerConfig::default();
        assert!(c.enabled);
        assert_eq!(c.rerank_timeout_ms, 800);
        assert!(!c.force_fallback);
        assert_eq!(c.fallback_skeleton.relevant_notes_tokens, 5000);
        // Hot-surfacing + time-decay ranking are active by default ("automatic bubbling");
        // MMR and the external reranker stay opt-in.
        assert!(c.retrieval_scoring.is_active());
        assert!(c.retrieval_scoring.recency_enabled);
        assert!(c.retrieval_scoring.reinforcement_enabled);
        assert!(!c.retrieval_scoring.mmr_enabled);
        assert!(!c.rerank.enabled);
    }

    #[test]
    fn assembler_config_mirrors_top_level_retrieval_tuning() {
        // The proactive memory-context path reads `retrieval_scoring`/`rerank`
        // off the assembler config, so `assembler_config()` must fold the
        // top-level toggles in (mirroring the `project_scoped` convention).
        let mut cfg = MemoryConfig::default();
        cfg.retrieval_scoring.recency_enabled = true;
        cfg.retrieval_scoring.reinforcement_enabled = true;
        cfg.retrieval_scoring.mmr_enabled = true;
        cfg.rerank.enabled = true;
        cfg.expansion.max_seeds = 2;
        cfg.project_scoped = true;

        let assembler = cfg.assembler_config();
        assert!(assembler.retrieval_scoring.is_active());
        assert!(assembler.retrieval_scoring.recency_enabled);
        assert!(assembler.retrieval_scoring.reinforcement_enabled);
        assert!(assembler.retrieval_scoring.mmr_enabled);
        assert!(assembler.rerank.enabled);
        assert_eq!(assembler.expansion.max_seeds, 2);
        assert!(assembler.project_scoped);
    }

    #[test]
    fn assembler_config_default_folds_hot_surfacing() {
        // Unconfigured deployment: the folded assembler config carries the
        // default-on hot-surfacing + time-decay refinements; MMR and the
        // external reranker remain off.
        let assembler = MemoryConfig::default().assembler_config();
        assert!(assembler.retrieval_scoring.is_active());
        assert!(assembler.retrieval_scoring.recency_enabled);
        assert!(assembler.retrieval_scoring.reinforcement_enabled);
        assert!(!assembler.retrieval_scoring.mmr_enabled);
        assert!(!assembler.rerank.enabled);
    }

    #[test]
    fn assembler_partial_toml_falls_back_to_defaults() {
        let toml_src = r#"
            enabled = false
        "#;
        let c: AssemblerConfig = toml::from_str(toml_src).expect("parse");
        assert!(!c.enabled);
        assert_eq!(c.rerank_timeout_ms, 800);
        assert_eq!(c.fallback_skeleton.relevant_notes_tokens, 5000);
    }

    #[test]
    fn missing_curated_section_uses_defaults() {
        let cfg = MemoryConfig::default();
        assert_eq!(cfg.curated.memory_char_limit, 2_200);
        assert_eq!(cfg.curated.user_char_limit, 1_375);
        assert!((cfg.curated.legacy_warn_threshold - 0.95).abs() < 1e-6);
    }
}
