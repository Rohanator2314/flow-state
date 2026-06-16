//! The CTRL+F in-pane find bar.
//!
//! A small, non-modal bar anchored at the top-right of the window (it does not
//! darken or capture the editor behind it, so the highlighted match stays
//! visible). Typing re-runs the search over the focused document; ENTER, ALT+N
//! and the ‹/› buttons step between matches (ALT+SHIFT+N steps back); ESC, the
//! ✕ button, or a second CTRL+F closes it and returns focus to the editor. All
//! state lives in [`App::search`](crate::app::App); this module only renders it
//! and exposes the input's focus [`Task`]. CTRL+F and the ALT+N stepping are
//! global subscriptions in `app.rs`, since the find input — not the editor —
//! holds focus while the bar is open.

use iced::widget::{button, container, row, text, text_input};
use iced::{Background, Border, Color, Element, Task};

use crate::app::{App, Message};
use crate::core::theme::Theme as FlowTheme;
use crate::view::style;

fn input_id() -> iced::widget::Id {
    iced::widget::Id::new("search-input")
}

/// Focus the find input (the bar just opened).
pub fn focus_input() -> Task<Message> {
    iced::widget::operation::focus(input_id())
}

pub fn bar(app: &App) -> Element<'_, Message> {
    let theme = &app.theme;
    let search = app.search.as_ref().expect("search open");

    // Match counter: "3/12", "no matches", or blank for an empty query.
    let count = if search.query.is_empty() {
        String::new()
    } else if search.matches.is_empty() {
        "no matches".to_string()
    } else {
        let at = search.current.map_or(0, |i| i + 1);
        format!("{at}/{}", search.matches.len())
    };

    let input = text_input("find…", &search.query)
        .id(input_id())
        .on_input(Message::SearchInput)
        .on_submit(Message::SearchNext)
        .size(13)
        .padding([6, 8])
        .width(200)
        .style(input_style(theme));

    let bar = row![
        input,
        text(count).size(12).color(theme.text_inactive).width(72),
        nav_button("‹", Message::SearchPrev, theme),
        nav_button("›", Message::SearchNext, theme),
        nav_button("✕", Message::CloseSearch, theme),
    ]
    .spacing(6)
    .align_y(iced::Center);

    container(bar).padding(8).style(style::card(theme)).into()
}

fn nav_button<'a>(
    glyph: &'a str,
    message: Message,
    theme: &FlowTheme,
) -> iced::widget::Button<'a, Message> {
    button(text(glyph).size(14))
        .padding([2, 8])
        .on_press(message)
        .style(style::bare_button(theme))
}

fn input_style(
    theme: &FlowTheme,
) -> impl Fn(&iced::Theme, text_input::Status) -> text_input::Style + use<> {
    let background = theme.background;
    let value = theme.text;
    let dim = theme.text_inactive;
    let accent = theme.accent;
    move |_, _| text_input::Style {
        background: Background::Color(background),
        border: Border {
            color: accent,
            width: 1.0,
            radius: 4.0.into(),
        },
        icon: dim,
        placeholder: dim,
        value,
        selection: Color { a: 0.3, ..accent },
    }
}
