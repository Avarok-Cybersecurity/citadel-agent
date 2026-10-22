//! `ui`: host-rendered chrome. A choice comes back as `AppEvent::UiAction { id }`.

use crate::{call, Pending};
use citadel_app_sdk_types::calls::*;

pub fn set_toolbar(items: Vec<ToolbarItem>) -> Pending<()> {
    call(&SetToolbar { items })
}

pub fn open_menu(x: f32, y: f32, items: Vec<MenuItem>) -> Pending<()> {
    call(&OpenMenu { x, y, items })
}

pub fn dialog(title: &str, body_text: &str, actions: Vec<DialogAction>) -> Pending<()> {
    call(&ShowDialog {
        title: title.into(),
        body_text: body_text.into(),
        actions,
    })
}

pub fn toast(text: &str) -> Pending<()> {
    call(&ShowToast { text: text.into() })
}

/// The height the host granted (it clamps).
pub fn request_height(px: u32) -> Pending<u32> {
    call(&RequestHeight { px }).map(|h| h.px)
}
