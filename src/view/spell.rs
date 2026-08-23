//! Keyboard-first spelling correction chooser (CTRL+.).

use iced::widget::{button, column, container, row, text, text_input};
use iced::{Element, Fill, Task};

use crate::app::{App, Message};
use crate::view::style;

fn input_id() -> iced::widget::Id {
    iced::widget::Id::new("spell-correction-input")
}

pub fn focus_input() -> Task<Message> {
    iced::widget::operation::focus(input_id())
}

pub fn bar(app: &App) -> Element<'_, Message> {
    let theme = &app.theme;
    let correction = app
        .spell_correction
        .as_ref()
        .expect("spell correction open");

    let input = text_input("replacement…", &correction.input)
        .id(input_id())
        .on_input(Message::SpellCorrectionInput)
        .on_submit(Message::SpellCorrectionSubmit)
        .size(13)
        .padding([7, 9])
        .width(Fill);

    let suggestions = if correction.loading {
        column![
            text("Finding suggestions…")
                .size(12)
                .color(theme.text_inactive)
        ]
    } else if correction.suggestions.is_empty() {
        column![
            text("No dictionary suggestions — type a replacement above.")
                .size(12)
                .color(theme.text_inactive)
        ]
    } else {
        correction.suggestions.iter().enumerate().fold(
            column![].spacing(1),
            |choices, (index, suggestion)| {
                let selected = index == correction.selected;
                choices.push(
                    button(text(suggestion).size(12))
                        .on_press(Message::SpellCorrectionApply(suggestion.clone()))
                        .padding([5, 8])
                        .width(Fill)
                        .style(move |iced_theme, status| {
                            if selected {
                                button::primary(iced_theme, status)
                            } else {
                                style::action_button(theme, false)(iced_theme, status)
                            }
                        }),
                )
            },
        )
    };

    let actions = row![
        button(text("Add to dictionary").size(12))
            .on_press(Message::AddSpellWord)
            .padding([5, 8])
            .style(style::bare_button(theme)),
        button(text("Ignore once").size(12))
            .on_press(Message::SpellCorrectionIgnore)
            .padding([5, 8])
            .style(style::bare_button(theme)),
        button(text("Cancel").size(12))
            .on_press(Message::CloseSpellCorrection)
            .padding([5, 8])
            .style(style::bare_button(theme)),
    ]
    .spacing(4);

    container(
        column![
            text(format!("Correct “{}”", correction.issue.word)).size(14),
            text("Type a replacement or choose a dictionary suggestion.")
                .size(12)
                .color(theme.text_inactive),
            input,
            suggestions,
            actions,
            text("↑↓ choose · ENTER replace · ESC close")
                .size(11)
                .color(theme.text_inactive),
        ]
        .spacing(9),
    )
    .width(360)
    .padding(10)
    .style(style::card(theme))
    .into()
}
