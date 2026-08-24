//! Quiet, editor-local actions for the current text selection.

use iced::widget::{button, column, container, text};
use iced::{Element, Fill};

use crate::app::{App, Message, SelectionFormat};
use crate::view::style;

fn action<'a>(app: &'a App, label: &'a str, message: Option<Message>) -> Element<'a, Message> {
    button(text(label).size(12))
        .on_press_maybe(message)
        .padding([5, 8])
        .width(Fill)
        .style(style::action_button(&app.theme, false))
        .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let menu = app.selection_menu.as_ref().expect("selection menu open");
    let mut target = menu.selected.replace(['\n', '\r'], " ");
    if target.chars().count() > 28 {
        target = target.chars().take(27).collect::<String>() + "…";
    }
    let markdown = app.selection_is_markdown();

    container(
        column![
            text("SELECTION").size(10).color(app.theme.text_inactive),
            text(format!("“{target}”")).size(12).color(app.theme.text),
            action(app, "Copy", Some(Message::SelectionCopy)),
            action(app, "Cut", Some(Message::SelectionCut)),
            action(app, "Paste", Some(Message::SelectionPaste)),
            action(
                app,
                "Spell correct",
                app.selection_can_spell_correct()
                    .then_some(Message::SelectionSpellCorrect),
            ),
            text("MARKDOWN").size(10).color(app.theme.text_inactive),
            action(
                app,
                "Bold",
                markdown.then_some(Message::SelectionFormat(SelectionFormat::Bold)),
            ),
            action(
                app,
                "Italic",
                markdown.then_some(Message::SelectionFormat(SelectionFormat::Italic)),
            ),
            action(
                app,
                "Underline",
                markdown.then_some(Message::SelectionFormat(SelectionFormat::Underline)),
            ),
        ]
        .spacing(2),
    )
    .width(196)
    .padding(8)
    .style(style::card(&app.theme))
    .into()
}
