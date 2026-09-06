//! `Input.*` — synthetic mouse and keyboard events.

use serde_json::json;

use crate::connection::CdpConnection;
use crate::error::Result;
use crate::ids::SessionId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseType {
    Pressed,
    Released,
    Moved,
    Wheel,
}

impl MouseType {
    fn as_wire(self) -> &'static str {
        match self {
            MouseType::Pressed => "mousePressed",
            MouseType::Released => "mouseReleased",
            MouseType::Moved => "mouseMoved",
            MouseType::Wheel => "mouseWheel",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    None,
    Left,
    Right,
    Middle,
}

impl MouseButton {
    fn as_wire(self) -> &'static str {
        match self {
            MouseButton::None => "none",
            MouseButton::Left => "left",
            MouseButton::Right => "right",
            MouseButton::Middle => "middle",
        }
    }

    /// The bit this button contributes to `buttons`, CDP's held-down mask.
    fn mask(self) -> u32 {
        match self {
            MouseButton::None => 0,
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseEvent {
    pub r#type: MouseType,
    pub x: f64,
    pub y: f64,
    pub button: MouseButton,
    pub click_count: u32,
    pub modifiers: u32,
    pub delta_x: f64,
    pub delta_y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyType {
    KeyDown,
    KeyUp,
    RawKeyDown,
    Char,
}

impl KeyType {
    fn as_wire(self) -> &'static str {
        match self {
            KeyType::KeyDown => "keyDown",
            KeyType::KeyUp => "keyUp",
            KeyType::RawKeyDown => "rawKeyDown",
            KeyType::Char => "char",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyEvent {
    pub r#type: KeyType,
    pub key: String,
    pub code: String,
    pub text: Option<String>,
    pub windows_virtual_key_code: Option<u32>,
    pub modifiers: u32,
}

/// `buttons` is derived, not taken from the caller.
///
/// Chrome ignores a `mousePressed` whose `buttons` mask does not include the button being pressed,
/// and a drag's intermediate `mouseMoved` needs the mask too. Deriving it here is why a click that
/// looks correct on the wire actually lands.
pub async fn dispatch_mouse_event(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    ev: &MouseEvent,
) -> Result<()> {
    let buttons = match ev.r#type {
        MouseType::Released => 0,
        _ => ev.button.mask(),
    };
    let mut params = json!({
        "type": ev.r#type.as_wire(),
        "x": ev.x,
        "y": ev.y,
        "button": ev.button.as_wire(),
        "clickCount": ev.click_count,
        "modifiers": ev.modifiers,
        "buttons": buttons,
    });
    if matches!(ev.r#type, MouseType::Wheel) {
        // Deltas on anything but a wheel make Chrome reject the whole frame.
        params["deltaX"] = json!(ev.delta_x);
        params["deltaY"] = json!(ev.delta_y);
    }
    conn.call(session, "Input.dispatchMouseEvent", params)
        .await?;
    Ok(())
}

pub async fn dispatch_key_event(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    ev: &KeyEvent,
) -> Result<()> {
    let mut params = json!({
        "type": ev.r#type.as_wire(),
        "key": ev.key,
        "code": ev.code,
        "modifiers": ev.modifiers,
    });
    if let Some(text) = &ev.text {
        params["text"] = json!(text);
    }
    if let Some(vk) = ev.windows_virtual_key_code {
        params["windowsVirtualKeyCode"] = json!(vk);
        // Pages that read the legacy `keyCode` see nothing without the native mirror.
        params["nativeVirtualKeyCode"] = json!(vk);
    }
    conn.call(session, "Input.dispatchKeyEvent", params).await?;
    Ok(())
}

pub async fn insert_text(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    text: &str,
) -> Result<()> {
    conn.call(session, "Input.insertText", json!({ "text": text }))
        .await?;
    Ok(())
}
