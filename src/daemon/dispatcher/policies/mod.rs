//! Policy Implementations
//!
//! MVP policy rules for the Dispatcher system.

pub mod battery;
pub mod cpu;
pub mod focus;
pub mod idle;
pub mod meeting;

pub use battery::LowBatteryPolicy;
pub use cpu::HighCpuAlertPolicy;
pub use focus::FocusModePolicy;
pub use idle::IdleCleanupPolicy;
pub use meeting::MeetingMutePolicy;
