//! Modal dialogs: the unsaved-changes prompt and the compile-error overlay.
//!
//! Modals are the standard iced recipe: `stack![base, backdrop(dialog)]`
//! where the backdrop is an opaque, darkened mouse-catcher so the UI behind
//! is visible but inert.

use iced::widget::{
    button, center, column, container, mouse_area, opaque, row, scrollable, stack, text,
};
use iced::{Background, Border, Color, Element, Font, Shadow};

use crate::app::{App, Message, PendingAction};

/// Lay `dialog` over `base` with a darkened backdrop.
pub fn modal<'a>(
    base: Element<'a, Message>,
    dialog: Element<'a, Message>,
) -> Element<'a, Message> {
    let backdrop = center(opaque(dialog)).style(backdrop_style);
    stack![base, opaque(mouse_area(backdrop))].into()
}

/// Like [`modal`], but anchored near the top — where a command bar belongs.
pub fn modal_top<'a>(
    base: Element<'a, Message>,
    dialog: Element<'a, Message>,
) -> Element<'a, Message> {
    let backdrop = container(opaque(dialog))
        .width(iced::Fill)
        .height(iced::Fill)
        .align_x(iced::Center)
        .padding(iced::Padding::ZERO.top(60))
        .style(backdrop_style);
    stack![base, opaque(mouse_area(backdrop))].into()
}

fn backdrop_style(_: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color {
            a: 0.6,
            ..Color::BLACK
        })),
        ..container::Style::default()
    }
}

pub fn confirm(app: &App, pending: &PendingAction) -> Element<'static, Message> {
    let (title, detail) = match pending {
        PendingAction::CloseWindow => {
            let n = app.docs.values().filter(|d| d.modified).count();
            (
                "Save changes before closing?".to_string(),
                if n > 1 {
                    format!("{n} files have unsaved changes.")
                } else {
                    "There are unsaved changes.".to_string()
                },
            )
        }
        PendingAction::ClosePane(pane) => {
            let name = match app.panes.get(*pane) {
                Some(crate::app::PaneKind::Editor(id)) => app.docs[id].display_name(),
                _ => "this pane".to_string(),
            };
            (
                format!("Save changes to {name}?"),
                "Unsaved changes will be lost when the pane closes.".to_string(),
            )
        }
    };
    card(
        app,
        column![
            text(title).size(16),
            text(detail).size(13).color(app.theme.text_inactive),
            row![
                button(text("Save").size(14)).on_press(Message::ConfirmSave),
                button(text("Discard").size(14))
                    .on_press(Message::ConfirmDiscard)
                    .style(button::danger),
                button(text("Cancel").size(14))
                    .on_press(Message::ConfirmCancel)
                    .style(button::secondary),
            ]
            .spacing(10),
        ]
        .spacing(14)
        .into(),
    )
}

pub fn compile_error<'a>(app: &App, error: &'a str) -> Element<'a, Message> {
    card(
        app,
        column![
            text("compile error").size(16),
            scrollable(text(error).size(13).font(Font::MONOSPACE)).height(200),
            button(text("Dismiss").size(14)).on_press(Message::DismissError),
        ]
        .spacing(14)
        .into(),
    )
}

fn card<'a>(app: &App, body: Element<'a, Message>) -> Element<'a, Message> {
    let surface = app.theme.surface;
    let text_color = app.theme.text;
    container(body)
        .padding(20)
        .max_width(560)
        .style(move |_| container::Style {
            background: Some(Background::Color(surface)),
            text_color: Some(text_color),
            border: Border::default().rounded(8),
            shadow: Shadow {
                color: Color { a: 0.4, ..Color::BLACK },
                offset: iced::Vector::new(0.0, 4.0),
                blur_radius: 18.0,
            },
            ..container::Style::default()
        })
        .into()
}
