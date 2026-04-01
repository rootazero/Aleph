//! EnhancedTokenUsage tests

use crate::components::session_compactor::*;

#[test]
fn test_enhanced_token_usage() {
    let usage = EnhancedTokenUsage {
        input: 1000,
        output: 500,
        reasoning: 200,
        cache_read: 300,
        cache_write: 100,
    };

    // Total for overflow check = input + cache_read + output
    assert_eq!(usage.total_for_overflow(), 1800);

    // Total billable (excluding cache reads which are cheaper)
    assert_eq!(usage.total_billable(), 1700);
}

#[test]
fn test_enhanced_token_usage_default() {
    let usage = EnhancedTokenUsage::default();

    assert_eq!(usage.input, 0);
    assert_eq!(usage.output, 0);
    assert_eq!(usage.reasoning, 0);
    assert_eq!(usage.cache_read, 0);
    assert_eq!(usage.cache_write, 0);
    assert!(usage.is_empty());
}

#[test]
fn test_enhanced_token_usage_new() {
    let usage = EnhancedTokenUsage::new(100, 200, 50, 75, 25);

    assert_eq!(usage.input, 100);
    assert_eq!(usage.output, 200);
    assert_eq!(usage.reasoning, 50);
    assert_eq!(usage.cache_read, 75);
    assert_eq!(usage.cache_write, 25);
}

#[test]
fn test_enhanced_token_usage_add() {
    let mut usage1 = EnhancedTokenUsage {
        input: 1000,
        output: 500,
        reasoning: 200,
        cache_read: 300,
        cache_write: 100,
    };

    let usage2 = EnhancedTokenUsage {
        input: 500,
        output: 250,
        reasoning: 100,
        cache_read: 150,
        cache_write: 50,
    };

    usage1.add(&usage2);

    assert_eq!(usage1.input, 1500);
    assert_eq!(usage1.output, 750);
    assert_eq!(usage1.reasoning, 300);
    assert_eq!(usage1.cache_read, 450);
    assert_eq!(usage1.cache_write, 150);
}

#[test]
fn test_enhanced_token_usage_add_to_empty() {
    let mut usage = EnhancedTokenUsage::default();

    let other = EnhancedTokenUsage {
        input: 100,
        output: 200,
        reasoning: 50,
        cache_read: 75,
        cache_write: 25,
    };

    usage.add(&other);

    assert_eq!(usage.input, 100);
    assert_eq!(usage.output, 200);
    assert_eq!(usage.reasoning, 50);
    assert_eq!(usage.cache_read, 75);
    assert_eq!(usage.cache_write, 25);
}

#[test]
fn test_enhanced_token_usage_is_empty() {
    let empty = EnhancedTokenUsage::default();
    assert!(empty.is_empty());

    let non_empty = EnhancedTokenUsage {
        input: 1,
        ..Default::default()
    };
    assert!(!non_empty.is_empty());
}

#[test]
fn test_enhanced_token_usage_total() {
    let usage = EnhancedTokenUsage {
        input: 1000,
        output: 500,
        reasoning: 200,
        cache_read: 300,
        cache_write: 100,
    };

    // Total = all fields combined
    assert_eq!(usage.total(), 2100);
}

#[test]
fn test_enhanced_token_usage_equality() {
    let usage1 = EnhancedTokenUsage::new(100, 200, 50, 75, 25);
    let usage2 = EnhancedTokenUsage::new(100, 200, 50, 75, 25);
    let usage3 = EnhancedTokenUsage::new(100, 200, 50, 75, 26);

    assert_eq!(usage1, usage2);
    assert_ne!(usage1, usage3);
}

#[test]
fn test_enhanced_token_usage_clone() {
    let usage = EnhancedTokenUsage::new(100, 200, 50, 75, 25);
    let cloned = usage.clone();

    assert_eq!(usage, cloned);
}
