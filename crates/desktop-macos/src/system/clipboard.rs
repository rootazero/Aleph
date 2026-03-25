//! Clipboard via NSPasteboard.

use aleph_desktop::system_types::ClipboardContent;
use aleph_desktop::Result;

pub fn read() -> Result<ClipboardContent> {
    todo!("clipboard::read — implement with NSPasteboard")
}

pub fn write(_text: &str) -> Result<()> {
    todo!("clipboard::write — implement with NSPasteboard")
}
