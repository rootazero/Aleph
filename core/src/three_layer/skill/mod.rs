//! Skill Layer - Middle layer with stable, testable DAG workflows

mod definition;

pub use definition::{
    AggregateStrategy, CostEstimate, RetryPolicy, SkillDefinition, SkillNode, SkillNodeType,
};
