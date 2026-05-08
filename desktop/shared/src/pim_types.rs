//! Shared types for Personal Information Management (PIM) capabilities.
//!
//! Re-exported from `aleph_protocol::desktop_bridge::methods::pim` to provide
//! a stable public API surface while keeping the canonical definitions in the
//! protocol crate.

pub use aleph_protocol::desktop_bridge::methods::pim::{
    CalendarEvent,
    CalendarInfo,
    Contact,
    ContactDetail,
    ContactGroup,
    LabeledValue,
    MailAttachment,
    MailFolder,
    MailMessage,
    MailMessageDetail,
    NewCalendarEvent,
    NewReminder,
    NoteContent,
    NoteInfo,
    Reminder,
    ReminderList,
};
