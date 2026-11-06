//! Runtime constraints evaluated before and during capability activation.
//!
//! Constraints are deliberately small and context-driven. The caller supplies
//! the current hour, resource usage, and named conditions at the boundary; the
//! engine does not own a clock or a resource registry. This keeps the core
//! deterministic and avoids a second source of truth for runtime state.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Runtime facts used to evaluate capability constraints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstraintContext {
    current_hour: u8,
    active_resources: usize,
    conditions: BTreeMap<String, bool>,
}

impl ConstraintContext {
    /// Build a deterministic context for a UTC hour.
    pub fn at_hour(current_hour: u8) -> Result<Self, ConstraintViolation> {
        if current_hour > 23 {
            return Err(ConstraintViolation::InvalidContext { current_hour });
        }
        Ok(Self {
            current_hour,
            active_resources: 0,
            conditions: BTreeMap::new(),
        })
    }

    /// Build a context from the current UTC hour and resource count.
    pub fn now(active_resources: usize) -> Result<Self, ConstraintViolation> {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ConstraintViolation::ClockUnavailable)?;
        let current_hour = ((elapsed.as_secs() / 3_600) % 24) as u8;
        Ok(Self::at_hour(current_hour)?.with_active_resources(active_resources))
    }

    /// Set occupied slots excluding the activation/invocation being admitted.
    /// Runtime dispatch consumes a new invocation slot even when its controller
    /// is already Active; that lifecycle state is not itself an invocation slot.
    #[must_use]
    pub fn with_active_resources(mut self, active_resources: usize) -> Self {
        self.active_resources = active_resources;
        self
    }

    /// Set a named runtime condition.
    #[must_use]
    pub fn with_condition(mut self, name: impl Into<String>, value: bool) -> Self {
        self.conditions.insert(name.into(), value);
        self
    }

    #[must_use]
    pub const fn current_hour(&self) -> u8 {
        self.current_hour
    }

    #[must_use]
    pub const fn active_resources(&self) -> usize {
        self.active_resources
    }

    #[must_use]
    pub fn condition(&self, name: &str) -> Option<bool> {
        self.conditions.get(name).copied()
    }
}

/// A time-of-day activation window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeConstraint {
    start_hour: u8,
    end_hour: u8,
}

impl TimeConstraint {
    /// Create a half-open time window `[start_hour, end_hour)`.
    ///
    /// If `end_hour` is less than `start_hour`, the window crosses midnight.
    /// `24` is accepted as the end of a non-wrapping day-long window.
    pub fn new(start_hour: u8, end_hour: u8) -> Result<Self, ConstraintViolation> {
        if start_hour > 23 || end_hour > 24 || start_hour == end_hour {
            return Err(ConstraintViolation::InvalidTimeWindow {
                start_hour,
                end_hour,
            });
        }
        Ok(Self {
            start_hour,
            end_hour,
        })
    }

    #[must_use]
    pub const fn start_hour(&self) -> u8 {
        self.start_hour
    }

    #[must_use]
    pub const fn end_hour(&self) -> u8 {
        self.end_hour
    }

    /// Evaluate the window against the supplied runtime hour.
    pub fn evaluate(&self, context: &ConstraintContext) -> Result<(), ConstraintViolation> {
        let hour = context.current_hour;
        if hour > 23 {
            return Err(ConstraintViolation::InvalidContext { current_hour: hour });
        }

        let allowed = if self.start_hour < self.end_hour {
            hour >= self.start_hour && hour < self.end_hour
        } else {
            hour >= self.start_hour || hour < self.end_hour
        };
        if allowed {
            Ok(())
        } else {
            Err(ConstraintViolation::TimeWindowClosed {
                start_hour: self.start_hour,
                end_hour: self.end_hour,
                current_hour: hour,
            })
        }
    }
}

/// A limit on the number of resources already active when activation starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceConstraint {
    max_concurrent: usize,
}

impl ResourceConstraint {
    #[must_use]
    pub const fn max_concurrent(max_concurrent: usize) -> Self {
        Self { max_concurrent }
    }

    #[must_use]
    pub const fn limit(&self) -> usize {
        self.max_concurrent
    }

    /// Each candidate activation/invocation consumes one new capacity slot.
    /// The supplied count excludes that candidate, so it must be strictly
    /// below the configured maximum. Accounting/reservations belong to the
    /// invocation owner, not this evaluator.
    pub fn evaluate(&self, context: &ConstraintContext) -> Result<(), ConstraintViolation> {
        if context.active_resources < self.max_concurrent {
            Ok(())
        } else {
            Err(ConstraintViolation::ResourceLimitExceeded {
                max_concurrent: self.max_concurrent,
                active_resources: context.active_resources,
            })
        }
    }
}

/// A named boolean condition required by a capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalConstraint {
    name: String,
    expected: bool,
}

impl ConditionalConstraint {
    #[must_use]
    pub fn new(name: impl Into<String>, expected: bool) -> Self {
        Self {
            name: name.into(),
            expected,
        }
    }

    /// Require a named condition to be true.
    #[must_use]
    pub fn required(name: impl Into<String>) -> Self {
        Self::new(name, true)
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn expected(&self) -> bool {
        self.expected
    }

    pub fn evaluate(&self, context: &ConstraintContext) -> Result<(), ConstraintViolation> {
        if context.condition(&self.name) == Some(self.expected) {
            Ok(())
        } else {
            Err(ConstraintViolation::ConditionNotMet {
                name: self.name.clone(),
                expected: self.expected,
                actual: context.condition(&self.name),
            })
        }
    }
}

/// A single runtime constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constraint {
    Time(TimeConstraint),
    Resource(ResourceConstraint),
    Conditional(ConditionalConstraint),
}

impl Constraint {
    /// Create a time-window constraint.
    pub fn time_window(start_hour: u8, end_hour: u8) -> Result<Self, ConstraintViolation> {
        Ok(Self::Time(TimeConstraint::new(start_hour, end_hour)?))
    }

    /// Alias for [`Self::time_window`].
    pub fn time(start_hour: u8, end_hour: u8) -> Result<Self, ConstraintViolation> {
        Self::time_window(start_hour, end_hour)
    }

    #[must_use]
    pub const fn resource_limit(max_concurrent: usize) -> Self {
        Self::Resource(ResourceConstraint::max_concurrent(max_concurrent))
    }

    #[must_use]
    pub fn condition(name: impl Into<String>) -> Self {
        Self::Conditional(ConditionalConstraint::required(name))
    }

    #[must_use]
    pub fn conditional(name: impl Into<String>, expected: bool) -> Self {
        Self::Conditional(ConditionalConstraint::new(name, expected))
    }

    pub fn evaluate(&self, context: &ConstraintContext) -> Result<(), ConstraintViolation> {
        match self {
            Self::Time(constraint) => constraint.evaluate(context),
            Self::Resource(constraint) => constraint.evaluate(context),
            Self::Conditional(constraint) => constraint.evaluate(context),
        }
    }
}

/// An ordered collection of constraints. Evaluation stops at the first
/// violation, which makes the fail-closed behaviour explicit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConstraintSet {
    constraints: Vec<Constraint>,
}

impl ConstraintSet {
    #[must_use]
    pub fn new(constraints: Vec<Constraint>) -> Self {
        Self { constraints }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.constraints.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.constraints.len()
    }

    pub fn evaluate(&self, context: &ConstraintContext) -> Result<(), ConstraintViolation> {
        self.constraints
            .iter()
            .try_for_each(|constraint| constraint.evaluate(context))
    }
}

impl From<Vec<Constraint>> for ConstraintSet {
    fn from(constraints: Vec<Constraint>) -> Self {
        Self::new(constraints)
    }
}

/// Why a constraint prevented activation or runtime use.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConstraintViolation {
    #[error("invalid time window: {start_hour}:00 to {end_hour}:00")]
    InvalidTimeWindow { start_hour: u8, end_hour: u8 },
    #[error(
        "time window is closed: {start_hour}:00 to {end_hour}:00, current hour {current_hour}"
    )]
    TimeWindowClosed {
        start_hour: u8,
        end_hour: u8,
        current_hour: u8,
    },
    #[error("resource limit exceeded: {active_resources} active, maximum {max_concurrent}")]
    ResourceLimitExceeded {
        max_concurrent: usize,
        active_resources: usize,
    },
    #[error("condition `{name}` is not {expected}; actual value is {actual:?}")]
    ConditionNotMet {
        name: String,
        expected: bool,
        actual: Option<bool>,
    },
    #[error("runtime hour {current_hour} is outside 0..=23")]
    InvalidContext { current_hour: u8 },
    #[error("system clock is before the Unix epoch")]
    ClockUnavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_rejects_hours_outside_a_day() {
        assert!(matches!(
            ConstraintContext::at_hour(24),
            Err(ConstraintViolation::InvalidContext { current_hour: 24 })
        ));
    }

    #[test]
    fn time_constraint_allows_hour_inside_window() {
        let constraint = TimeConstraint::new(9, 17).unwrap();
        let context = ConstraintContext::at_hour(12).unwrap();
        assert!(constraint.evaluate(&context).is_ok());
    }

    #[test]
    fn time_constraint_rejects_hour_outside_window() {
        let constraint = TimeConstraint::new(9, 17).unwrap();
        let context = ConstraintContext::at_hour(18).unwrap();
        assert!(matches!(
            constraint.evaluate(&context),
            Err(ConstraintViolation::TimeWindowClosed { .. })
        ));
    }

    #[test]
    fn time_constraint_end_hour_is_exclusive() {
        let constraint = TimeConstraint::new(9, 17).unwrap();
        assert!(constraint
            .evaluate(&ConstraintContext::at_hour(17).unwrap())
            .is_err());
    }

    #[test]
    fn time_constraint_supports_overnight_windows() {
        let constraint = TimeConstraint::new(22, 6).unwrap();
        assert!(constraint
            .evaluate(&ConstraintContext::at_hour(23).unwrap())
            .is_ok());
        assert!(constraint
            .evaluate(&ConstraintContext::at_hour(5).unwrap())
            .is_ok());
        assert!(constraint
            .evaluate(&ConstraintContext::at_hour(12).unwrap())
            .is_err());
    }

    #[test]
    fn time_constraint_rejects_invalid_hour_bounds() {
        assert!(matches!(
            TimeConstraint::new(24, 1),
            Err(ConstraintViolation::InvalidTimeWindow { .. })
        ));
    }

    #[test]
    fn resource_constraint_allows_capacity_below_limit() {
        let constraint = ResourceConstraint::max_concurrent(3);
        let context = ConstraintContext::at_hour(12)
            .unwrap()
            .with_active_resources(2);
        assert!(constraint.evaluate(&context).is_ok());
    }

    #[test]
    fn resource_constraint_rejects_capacity_at_limit() {
        let constraint = ResourceConstraint::max_concurrent(3);
        let context = ConstraintContext::at_hour(12)
            .unwrap()
            .with_active_resources(3);
        assert!(matches!(
            constraint.evaluate(&context),
            Err(ConstraintViolation::ResourceLimitExceeded { .. })
        ));
    }

    #[test]
    fn resource_constraint_rejects_capacity_above_limit() {
        let constraint = ResourceConstraint::max_concurrent(3);
        let context = ConstraintContext::at_hour(12)
            .unwrap()
            .with_active_resources(4);
        assert!(constraint.evaluate(&context).is_err());
    }

    #[test]
    fn conditional_constraint_allows_matching_condition() {
        let constraint = ConditionalConstraint::required("network");
        let context = ConstraintContext::at_hour(12)
            .unwrap()
            .with_condition("network", true);
        assert!(constraint.evaluate(&context).is_ok());
    }

    #[test]
    fn conditional_constraint_rejects_missing_condition() {
        let constraint = ConditionalConstraint::required("network");
        let context = ConstraintContext::at_hour(12).unwrap();
        assert!(matches!(
            constraint.evaluate(&context),
            Err(ConstraintViolation::ConditionNotMet { .. })
        ));
    }

    #[test]
    fn conditional_constraint_rejects_false_condition() {
        let constraint = ConditionalConstraint::required("network");
        let context = ConstraintContext::at_hour(12)
            .unwrap()
            .with_condition("network", false);
        assert!(constraint.evaluate(&context).is_err());
    }

    #[test]
    fn conditional_constraint_can_require_false() {
        let constraint = ConditionalConstraint::new("maintenance", false);
        let context = ConstraintContext::at_hour(12)
            .unwrap()
            .with_condition("maintenance", false);
        assert!(constraint.evaluate(&context).is_ok());
    }

    #[test]
    fn constraint_set_evaluates_every_constraint() {
        let constraints = ConstraintSet::new(vec![
            Constraint::time_window(9, 17).unwrap(),
            Constraint::resource_limit(2),
            Constraint::condition("network"),
        ]);
        let context = ConstraintContext::at_hour(12)
            .unwrap()
            .with_active_resources(1)
            .with_condition("network", true);
        assert!(constraints.evaluate(&context).is_ok());
    }

    #[test]
    fn constraint_set_fails_closed_on_first_violation() {
        let constraints = ConstraintSet::new(vec![
            Constraint::time_window(9, 17).unwrap(),
            Constraint::resource_limit(2),
        ]);
        let context = ConstraintContext::at_hour(18)
            .unwrap()
            .with_active_resources(1);
        assert!(matches!(
            constraints.evaluate(&context),
            Err(ConstraintViolation::TimeWindowClosed { .. })
        ));
    }

    #[test]
    fn empty_constraint_set_allows_activation() {
        assert!(ConstraintSet::default()
            .evaluate(&ConstraintContext::at_hour(0).unwrap())
            .is_ok());
    }
}
