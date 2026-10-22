//! `ui`: chrome the host renders from data, badged with the app's name; strings are text only.

use alloc::string::String;
use alloc::vec::Vec;

abi_types! {
    pub struct ToolbarItem {
        pub id: String,
        pub label: String,
        pub icon_name: Option<String>,
    }

    pub struct SetToolbar {
        pub items: Vec<ToolbarItem>,
    }

    pub struct MenuItem {
        pub id: String,
        pub label: String,
    }

    /// A menu at canvas coordinates; a choice arrives as `AppEvent::UiAction`.
    pub struct OpenMenu {
        pub x: f32,
        pub y: f32,
        pub items: Vec<MenuItem>,
    }

    pub struct DialogAction {
        pub id: String,
        pub label: String,
    }

    pub struct ShowDialog {
        pub title: String,
        pub body_text: String,
        pub actions: Vec<DialogAction>,
    }

    pub struct ShowToast {
        pub text: String,
    }

    /// An md zone asks for a height; the host clamps it.
    pub struct RequestHeight {
        pub px: u32,
    }

    pub struct HeightGranted {
        pub px: u32,
    }
}
