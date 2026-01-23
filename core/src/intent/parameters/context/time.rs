//! TimeContext - Temporal information for time-based routing rules

use chrono::{Datelike, Local, Timelike};
use serde::{Deserialize, Serialize};

/// Temporal information for time-based routing rules
///
/// Provides current time information that can be used to make
/// context-aware routing decisions based on time of day or day of week.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeContext {
    /// Hour of day (0-23)
    pub hour: u32,

    /// Minute (0-59)
    pub minute: u32,

    /// Day of week (0 = Sunday, 6 = Saturday)
    pub weekday: u32,

    /// Is weekend (Saturday or Sunday)
    pub is_weekend: bool,
}

impl TimeContext {
    /// Create time context for current time
    pub fn now() -> Self {
        let now = Local::now();

        Self {
            hour: now.hour(),
            minute: now.minute(),
            weekday: now.weekday().num_days_from_sunday(),
            is_weekend: matches!(now.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun),
        }
    }

    /// Create time context for a specific hour/minute (for testing)
    pub fn at(hour: u32, minute: u32) -> Self {
        let now = Local::now();
        Self {
            hour,
            minute,
            weekday: now.weekday().num_days_from_sunday(),
            is_weekend: matches!(now.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun),
        }
    }

    /// Check if current time is within a time range
    ///
    /// # Arguments
    ///
    /// * `start_hour` - Start hour (0-23)
    /// * `end_hour` - End hour (0-23), can be less than start for overnight ranges
    pub fn is_within_hours(&self, start_hour: u32, end_hour: u32) -> bool {
        if start_hour <= end_hour {
            // Normal range: e.g., 9-17
            self.hour >= start_hour && self.hour < end_hour
        } else {
            // Overnight range: e.g., 22-6
            self.hour >= start_hour || self.hour < end_hour
        }
    }

    /// Check if it's business hours (9am-6pm weekdays)
    pub fn is_business_hours(&self) -> bool {
        !self.is_weekend && self.is_within_hours(9, 18)
    }

    /// Check if it's morning (6am-12pm)
    pub fn is_morning(&self) -> bool {
        self.is_within_hours(6, 12)
    }

    /// Check if it's afternoon (12pm-6pm)
    pub fn is_afternoon(&self) -> bool {
        self.is_within_hours(12, 18)
    }

    /// Check if it's evening (6pm-10pm)
    pub fn is_evening(&self) -> bool {
        self.is_within_hours(18, 22)
    }

    /// Check if it's night (10pm-6am)
    pub fn is_night(&self) -> bool {
        self.is_within_hours(22, 6)
    }
}

impl Default for TimeContext {
    fn default() -> Self {
        Self::now()
    }
}
