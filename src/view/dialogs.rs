//! Modal dialogs: the unsaved-changes prompt and the compile-error overlay.
//!
//! Modals are the standard iced recipe — a darkened, opaque mouse-catcher with
//! the dialog centered on it, so the UI behind is visible but inert. These
//! return just the *overlay layer*; `view` stacks it over a base that always
//! stays at stack layer 0, so the editor's widget tree (and its focus state)
//! never shifts when a dialog opens or closes.

use iced::widget::{button, center, column, container, mouse_area, opaque, row, scrollable, text};
use iced::{Background, Border, Color, Element, Font, Shadow};

use crate::app::{App, Message, PendingAction};

/// The darkened, centered overlay layer carrying `dialog`.
pub fn modal_layer(dialog: Element<'_, Message>) -> Element<'_, Message> {
    let backdrop = center(opaque(dialog)).style(backdrop_style);
    opaque(mouse_area(backdrop))
}

/// Like [`modal_layer`], but anchored near the top — where a command bar belongs.
pub fn modal_top_layer(dialog: Element<'_, Message>) -> Element<'_, Message> {
    let backdrop = container(opaque(dialog))
        .width(iced::Fill)
        .height(iced::Fill)
        .align_x(iced::Center)
        .padding(iced::Padding::ZERO.top(60))
        .style(backdrop_style);
    opaque(mouse_area(backdrop))
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
            let n = app.workspace.documents.values().filter(|d| d.modified).count();
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
            let name = match app.workspace.panes.get(*pane) {
                Some(crate::app::PaneKind::Editor(id)) => app.workspace.documents[id].display_name(),
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

pub fn open_picker(app: &App) -> Element<'static, Message> {
    let file_choice = column![
        text("File").size(14).color(app.theme.text),
        text("Open a document in a pane")
            .size(12)
            .color(app.theme.text_inactive),
    ]
    .spacing(4);
    let folder_choice = column![
        text("Folder").size(14).color(app.theme.text),
        text("Use it as the sidebar directory")
            .size(12)
            .color(app.theme.text_inactive),
    ]
    .spacing(4);

    card(
        app,
        column![
            text("Open").size(16),
            text("Choose a document to edit or a folder to browse.")
                .size(13)
                .color(app.theme.text_inactive),
            row![
                button(file_choice)
                    .on_press(Message::ChooseFileToOpen)
                    .width(220)
                    .padding(14)
                    .style(crate::view::style::choice_button(&app.theme)),
                button(folder_choice)
                    .on_press(Message::ChooseFolderToOpen)
                    .width(220)
                    .padding(14)
                    .style(crate::view::style::choice_button(&app.theme)),
            ]
            .spacing(10),
            button(text("Cancel").size(13))
                .on_press(Message::CloseOpenPicker)
                .padding([5, 8])
                .style(crate::view::style::bare_button(&app.theme)),
        ]
        .spacing(12)
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
                color: Color {
                    a: 0.4,
                    ..Color::BLACK
                },
                offset: iced::Vector::new(0.0, 4.0),
                blur_radius: 18.0,
            },
            ..container::Style::default()
        })
        .into()
}
