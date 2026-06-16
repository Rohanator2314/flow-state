//! flow-state — a distraction-free writing app (iced GUI).
//!
//! Entry point: builds the iced application and hands everything to
//! [`app::App`]. See `ARCHITECTURE.md` for the module map.
//!
//! `exit_on_close_request(false)` lets [`App`] intercept the window close
//! and ask about unsaved changes before actually exiting.

mod app;
mod core;
mod view;

use crate::app::App;

fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .exit_on_close_request(false)
        .window_size((1200.0, 800.0))
        .run()
}
