#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! flow-state — a distraction-free writing app (iced GUI).
//!
//! Entry point: builds the iced application and hands everything to
//! [`app::App`]. See `ARCHITECTURE.md` for the module map.
//!
//! `exit_on_close_request(false)` lets [`App`] intercept the window close
//! and ask about unsaved changes before actually exiting.

mod app;
mod core;
mod document;
mod selection;
mod ui_state;
mod view;
mod workspace;

use crate::app::App;

const WINDOW_ICON: &[u8] = include_bytes!("../wix/flow-state.ico");

fn main() -> iced::Result {
    #[cfg(target_os = "windows")]
    set_windows_app_user_model_id();

    iced::application(App::boot, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .window(iced::window::Settings {
            size: iced::Size::new(1200.0, 800.0),
            icon: Some(window_icon()),
            exit_on_close_request: false,
            ..iced::window::Settings::default()
        })
        .run()
}

fn window_icon() -> iced::window::Icon {
    iced::window::icon::from_file_data(WINDOW_ICON, None)
        .expect("bundled Flow State window icon is valid")
}

#[cfg(target_os = "windows")]
fn set_windows_app_user_model_id() {
    const APP_USER_MODEL_ID: &str = "Rohanator2314.FlowState";

    let app_id: Vec<u16> = APP_USER_MODEL_ID.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `app_id` is a valid, null-terminated UTF-16 string and remains
    // alive for the duration of the Windows API call.
    let _ = unsafe {
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr())
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_window_icon_decodes() {
        let (_, size) = window_icon().into_raw();
        assert_eq!(size, iced::Size::new(256, 256));
    }
}
