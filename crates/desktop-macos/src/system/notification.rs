//! Notifications via UNUserNotificationCenter with osascript fallback.

use aleph_desktop::Result;

pub async fn send_notification(_title: &str, _body: &str) -> Result<()> {
    todo!("notification::send_notification — implement with UNUserNotificationCenter")
}
