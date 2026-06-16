//! The ESC menu: a halloy-style command bar.
//!
//! The root bar lists commands; picking one drills into a sub-bar (the theme
//! and compiler pickers) or a small panel (split slider, keybind help). All
//! state and actions live in `app.rs` ([`Menu`](crate::app::Menu) holds the
//! filter text and keyboard selection); this module renders it and exposes
//! the focus/scroll [`Task`]s tied to its widget ids.
//!
//! The bar is a plain `text_input` plus our own option list rather than
//! iced's `combo_box`: the combo box can't be focused programmatically
//! (it doesn't implement the focus operation), and an unfocused command bar
//! defeats the point. Arrow keys are delivered by the keyboard subscription
//! in `app.rs` — the input ignores them, so they reach it.

use iced::widget::scrollable::AbsoluteOffset;
use iced::widget::{
    button, column, container, row, scrollable, slider, text, text_input,
};
use iced::{Background, Border, Color, Element, Fill, Font, Task};

use crate::app::{
    compiler_options, filtered_commands, font_options, theme_options, App, Menu, Message,
    Picker,
};
use crate::core::theme::Theme as FlowTheme;
use crate::view::style;

const BAR_WIDTH: f32 = 520.0;
/// Height of one option row; [`scroll_to_selected`] relies on it.
const ROW_HEIGHT: f32 = 28.0;
/// Cap on the visible option list; longer lists (themes) scroll.
const LIST_HEIGHT: f32 = 308.0;

fn input_id() -> iced::widget::Id {
    iced::widget::Id::new("command-bar-input")
}

fn list_id() -> iced::widget::Id {
    iced::widget::Id::new("command-bar-list")
}

/// Focus the command bar's filter input (the bar just opened).
pub fn focus_input() -> Task<Message> {
    iced::widget::operation::focus(input_id())
}

/// Keep the keyboard selection visible in a scrolling option list.
pub fn scroll_to_selected(index: usize) -> Task<Message> {
    let y = (index as f32 + 0.5) * ROW_HEIGHT - LIST_HEIGHT / 2.0;
    iced::widget::operation::scroll_to(
        list_id(),
        AbsoluteOffset {
            x: 0.0,
            y: y.max(0.0),
        },
    )
}

pub fn view<'a>(app: &'a App, menu: &'a Menu) -> Element<'a, Message> {
    match menu {
        Menu::Commands(picker) => bar(
            app,
            picker,
            "Type a command...",
            filtered_commands(&picker.input)
                .into_iter()
                .map(|c| (c.to_string(), Message::CommandSelected(c)))
                .collect(),
        ),
        Menu::Theme(picker) => bar(
            app,
            picker,
            "Search themes...",
            theme_options(&picker.input)
                .into_iter()
                .map(|name| (name.clone(), Message::ThemeSelected(name)))
                .collect(),
        ),
        Menu::Font(picker) => bar(
            app,
            picker,
            "Search fonts...",
            font_options(&picker.input)
                .into_iter()
                .map(|name| (name.clone(), Message::FontSelected(name)))
                .collect(),
        ),
        Menu::Compiler(picker) => bar(
            app,
            picker,
            "Choose a LaTeX engine...",
            compiler_options(&picker.input)
                .into_iter()
                .map(|name| (name.clone(), Message::CompilerSelected(name)))
                .collect(),
        ),
        Menu::Split => split_panel(app),
        Menu::Help => help_panel(app),
    }
}

/// One command-bar level: the filter input on top of the (filtered) option
/// list, with the keyboard selection highlighted.
fn bar<'a>(
    app: &'a App,
    picker: &Picker,
    placeholder: &str,
    options: Vec<(String, Message)>,
) -> Element<'a, Message> {
    let theme = &app.theme;

    let input = text_input(placeholder, &picker.input)
        .id(input_id())
        .on_input(Message::CommandInput)
        .on_submit(Message::MenuSubmit)
        .size(14)
        .padding([8, 10])
        .style(input_style(theme));

    let selected = picker.selected;
    let rows = options
        .into_iter()
        .enumerate()
        .fold(column![], |col, (i, (label, message))| {
            col.push(
                button(text(label).size(13))
                    .width(Fill)
                    .height(ROW_HEIGHT)
                    .padding([5, 10])
                    .on_press(message)
                    .style(row_style(theme, i == selected)),
            )
        });

    let list = container(scrollable(rows).id(list_id()).width(Fill))
        .max_height(LIST_HEIGHT);

    container(column![input, list, hint(theme)].spacing(8))
        .width(BAR_WIDTH)
        .padding(8)
        .style(style::card(theme))
        .into()
}

/// The split-width panel: the one setting that needs a slider, not a list.
fn split_panel(app: &App) -> Element<'_, Message> {
    let theme = &app.theme;
    let ratio = app.config.split_ratio();

    let controls = row![
        slider(0.2..=0.8, ratio, Message::SplitRatioChanged)
            .on_release(Message::SplitRatioReleased)
            .step(0.05),
        text(format!("{:.0}%", ratio * 100.0))
            .size(12)
            .color(theme.text_inactive)
            .width(38),
    ]
    .spacing(10)
    .align_y(iced::Center);

    container(
        column![
            text("split width — editor share of the pane area").size(13),
            controls,
            hint(theme),
        ]
        .spacing(12),
    )
    .width(BAR_WIDTH)
    .padding(12)
    .style(style::card(theme))
    .into()
}

fn help_panel(app: &App) -> Element<'_, Message> {
    let theme = &app.theme;
    const BINDS: [(&str, &str); 18] = [
        ("CTRL+S", "save, then refresh/compile the preview"),
        ("CTRL+N", "new file"),
        ("CTRL+O", "open a file (system dialog)"),
        ("CTRL+F", "find in the focused pane"),
        ("CTRL+W", "close the focused pane"),
        ("CTRL+TAB", "focus the next pane"),
        ("CTRL+Q", "quit"),
        ("CTRL+Z", "undo"),
        ("CTRL+SHIFT+Z / CTRL+Y", "redo"),
        ("CTRL+BACKSPACE", "delete the word before the cursor"),
        ("SHIFT+BACKSPACE", "delete the current sentence"),
        ("ALT+N / ALT+SHIFT+N", "next / previous paragraph"),
        ("ALT+W / ALT+B", "next / previous word"),
        ("CTRL+arrows", "move by word"),
        ("CTRL+C / X / V", "copy / cut / paste"),
        ("ESC", "command bar · back · close"),
        ("drag a pane title bar", "swap panes"),
        ("drag the divider", "resize the split"),
    ];

    let rows = BINDS.iter().fold(column![].spacing(6), |col, (key, action)| {
        col.push(
            row![
                text(*key).size(12).font(Font::MONOSPACE).width(190),
                text(*action).size(13).color(theme.text_inactive),
            ]
            .spacing(10),
        )
    });

    container(
        column![text("keybindings").size(13), rows, hint(theme)].spacing(12),
    )
    .width(BAR_WIDTH)
    .padding(12)
    .style(style::card(theme))
    .into()
}

fn hint(theme: &FlowTheme) -> Element<'static, Message> {
    text("↑↓ select · ENTER confirm · ESC back")
        .size(11)
        .color(theme.text_inactive)
        .into()
}

/// The filter input, accent-bordered like halloy's command bar.
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

/// Option rows: bare, with the keyboard selection filled in the accent color
/// (halloy's selected menu entry).
fn row_style(
    theme: &FlowTheme,
    selected: bool,
) -> impl Fn(&iced::Theme, button::Status) -> button::Style + use<> {
    let text = theme.text;
    let dim = theme.text_inactive;
    let accent = theme.accent;
    move |_, status| button::Style {
        background: Some(Background::Color(if selected {
            Color { a: 0.35, ..accent }
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => {
                    Color { a: 0.06, ..Color::WHITE }
                }
                _ => Color::TRANSPARENT,
            }
        })),
        text_color: if selected { text } else { dim },
        border: Border::default().rounded(4),
        ..button::Style::default()
    }
}
