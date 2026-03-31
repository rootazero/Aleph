//! P5: Decay & Promotion
//!
//! 8 scenarios covering tiered decay curves, protected types,
//! access reinforcement (fresh vs stale, cap), and tier promotion logic.

use alephcore::memory::context::{FactType, MemoryFact, MemoryTier};
use alephcore::memory::decay::{
    effective_half_life, AccessReinforcementConfig, MemoryStrength, TieredDecayConfig,
};
use alephcore::memory::promotion::{check_promotion, PromotionCriteria};

use super::harness::DAY;

// ---------------------------------------------------------------------------
// Tiered decay
// ---------------------------------------------------------------------------

/// p5_01: At 7 days, ShortTerm strength < LongTerm < Core (tier ordering).
#[test]
fn p5_01_tiered_decay_short_term_fastest() {
    let config = TieredDecayConfig::default();
    let now = 7 * DAY;
    let ms = MemoryStrength {
        access_count: 0,
        last_accessed: 0,
        creation_time: 0,
    };

    let s_short =
        ms.calculate_strength_tiered(&config, &MemoryTier::ShortTerm, &FactType::Other, now);
    let s_long =
        ms.calculate_strength_tiered(&config, &MemoryTier::LongTerm, &FactType::Other, now);
    let s_core = ms.calculate_strength_tiered(&config, &MemoryTier::Core, &FactType::Other, now);

    assert!(
        s_short < s_long,
        "ShortTerm ({s_short}) should be weaker than LongTerm ({s_long}) at 7d"
    );
    assert!(
        s_long < s_core,
        "LongTerm ({s_long}) should be weaker than Core ({s_core}) at 7d"
    );
}

/// p5_02: ShortTerm half-life is 7 days, so strength at exactly 7d should be ~0.5.
#[test]
fn p5_02_tiered_decay_half_life_inflection() {
    let config = TieredDecayConfig::default();
    let now = 7 * DAY;
    let ms = MemoryStrength {
        access_count: 0,
        last_accessed: 0,
        creation_time: 0,
    };

    let strength =
        ms.calculate_strength_tiered(&config, &MemoryTier::ShortTerm, &FactType::Other, now);

    assert!(
        (strength - 0.5).abs() < 0.05,
        "ShortTerm at 7d should be ~0.5, got {strength}"
    );
}

/// p5_03: Personal (protected) fact type never decays, even after 365 days.
#[test]
fn p5_03_protected_type_never_decays() {
    let config = TieredDecayConfig::default();
    let now = 365 * DAY;
    let ms = MemoryStrength {
        access_count: 0,
        last_accessed: 0,
        creation_time: 0,
    };

    let strength =
        ms.calculate_strength_tiered(&config, &MemoryTier::ShortTerm, &FactType::Personal, now);

    assert!(
        (strength - 1.0).abs() < f32::EPSILON,
        "Protected type should remain 1.0 after 365d, got {strength}"
    );
}

// ---------------------------------------------------------------------------
// Access reinforcement
// ---------------------------------------------------------------------------

/// p5_04: 3 recent accesses extend effective half-life above base but below cap.
#[test]
fn p5_04_access_reinforcement_extends_half_life() {
    let config = AccessReinforcementConfig::default();
    let base = 7.0_f32;
    let eff = effective_half_life(base, 3, 1.0, &config);

    assert!(
        eff > base,
        "3 recent accesses should extend half-life: {eff} > {base}"
    );
    assert!(
        eff < base * config.max_multiplier,
        "Should stay below cap ({eff} < {})",
        base * config.max_multiplier
    );
}

/// p5_05: Same access count but stale (90 days) gives less extension than recent (1 day).
#[test]
fn p5_05_access_reinforcement_stale_access_fades() {
    let config = AccessReinforcementConfig::default();
    let base = 7.0_f32;

    let eff_stale = effective_half_life(base, 10, 90.0, &config);
    let eff_recent = effective_half_life(base, 10, 1.0, &config);

    assert!(
        eff_recent > eff_stale,
        "10 recent accesses ({eff_recent}) should extend more than 10 stale ones ({eff_stale})"
    );
}

/// p5_06: 1000 accesses hit the cap: effective_half_life = base * max_multiplier = 21.0.
#[test]
fn p5_06_access_reinforcement_capped_at_max_multiplier() {
    let config = AccessReinforcementConfig::default();
    let base = 7.0_f32;
    let cap = base * config.max_multiplier; // 21.0

    let eff = effective_half_life(base, 1000, 0.0, &config);

    assert!(
        (eff - cap).abs() < 0.01,
        "1000 accesses should hit cap {cap}, got {eff}"
    );
}

// ---------------------------------------------------------------------------
// Tier promotion
// ---------------------------------------------------------------------------

/// p5_07: ShortTerm → LongTerm promotion with 5 boundary combos.
#[test]
fn p5_07_promotion_short_to_long_all_criteria() {
    let criteria = PromotionCriteria::default();
    let now = 100 * DAY;

    // All criteria met: access=5, age=10d, strength=0.6 → LongTerm
    let fact_ok = MemoryFact::new("test".into(), FactType::Other, vec![])
        .with_tier(MemoryTier::ShortTerm)
        .with_access_count(5)
        .with_created_at(now - 10 * DAY);
    assert_eq!(
        check_promotion(&fact_ok, 0.6, now, &criteria),
        Some(MemoryTier::LongTerm),
        "All criteria met should promote to LongTerm"
    );

    // Access too low: access=1
    let fact_low_access = MemoryFact::new("test".into(), FactType::Other, vec![])
        .with_tier(MemoryTier::ShortTerm)
        .with_access_count(1)
        .with_created_at(now - 10 * DAY);
    assert_eq!(
        check_promotion(&fact_low_access, 0.6, now, &criteria),
        None,
        "Too few accesses should not promote"
    );

    // Too young: age=1d
    let fact_young = MemoryFact::new("test".into(), FactType::Other, vec![])
        .with_tier(MemoryTier::ShortTerm)
        .with_access_count(5)
        .with_created_at(now - 1 * DAY);
    assert_eq!(
        check_promotion(&fact_young, 0.6, now, &criteria),
        None,
        "Too young should not promote"
    );

    // Strength too low: strength=0.3
    let fact_weak = MemoryFact::new("test".into(), FactType::Other, vec![])
        .with_tier(MemoryTier::ShortTerm)
        .with_access_count(5)
        .with_created_at(now - 10 * DAY);
    assert_eq!(
        check_promotion(&fact_weak, 0.3, now, &criteria),
        None,
        "Low strength should not promote"
    );

    // Core tier never promotes
    let fact_core = MemoryFact::new("test".into(), FactType::Other, vec![])
        .with_tier(MemoryTier::Core)
        .with_access_count(100)
        .with_created_at(now - 365 * DAY);
    assert_eq!(
        check_promotion(&fact_core, 0.9, now, &criteria),
        None,
        "Core should never promote"
    );
}

/// p5_08: LongTerm → Core promotion threshold tests.
#[test]
fn p5_08_promotion_long_to_core_threshold() {
    let criteria = PromotionCriteria::default();
    let now = 100 * DAY;

    // All criteria met: access=15, age=60d, strength=0.8 → Core
    let fact_ok = MemoryFact::new("test".into(), FactType::Other, vec![])
        .with_tier(MemoryTier::LongTerm)
        .with_access_count(15)
        .with_created_at(now - 60 * DAY);
    assert_eq!(
        check_promotion(&fact_ok, 0.8, now, &criteria),
        Some(MemoryTier::Core),
        "LongTerm meeting all criteria should promote to Core"
    );

    // Access too low: access=5
    let fact_low = MemoryFact::new("test".into(), FactType::Other, vec![])
        .with_tier(MemoryTier::LongTerm)
        .with_access_count(5)
        .with_created_at(now - 60 * DAY);
    assert_eq!(
        check_promotion(&fact_low, 0.8, now, &criteria),
        None,
        "LongTerm with too few accesses should not promote"
    );
}
