//! Widget styles copied from halloy's appearance system
//! (GPL-3.0, <https://github.com/squidowl/halloy>,
//! `src/appearance/theme/button.rs` / `container.rs`), mapped onto our theme
//! colors: 4px radius, 1px borders, layered translucent backgrounds that
//! step up on hover/press, 0.2-alpha text when disabled.

use iced::widget::{button, container};
use iced::{Background, Border, Color, Shadow};

use crate::core::theme::Theme;

/// halloy's bare/transparent button.
pub fn bare_button(theme: &Theme) -> impl Fn(&iced::Theme, button::Status) -> button::Style + use<> {
    let text = theme.text;
    move |_, status| button::Style {
        background: Some(Background::Color(match status {
            button::Status::Hovered | button::Status::Pressed => {
                Color { a: 0.06, ..Color::WHITE }
            }
            _ => Color::TRANSPARENT,
        })),
        text_color: match status {
            button::Status::Disabled => Color { a: 0.2, ..text },
            _ => text,
        },
        border: Border::default().rounded(4),
        ..button::Style::default()
    }
}

/// halloy's floating card (`theme::container::tooltip` / dialog look):
/// surface background, 4px radius, 1px border, soft shadow.
pub fn card(theme: &Theme) -> impl Fn(&iced::Theme) -> container::Style + use<> {
    let surface = theme.surface;
    let text = theme.text;
    let border_color = theme.border;
    move |_| container::Style {
        background: Some(Background::Color(surface)),
        text_color: Some(text),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: 4.0.into(),
        },
        shadow: Shadow {
            color: Color { a: 0.4, ..Color::BLACK },
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 18.0,
        },
        ..container::Style::default()
    }
}
