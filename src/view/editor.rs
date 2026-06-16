//! The editor pane: iced's `text_editor` plus flow-state's keymap and the
//! paragraph-dimming highlighter.
//!
//! # Keymap
//!
//! [`key_binding`] is consulted for every key press; anything it doesn't
//! claim falls through to the widget's defaults (arrows, HOME/END,
//! CTRL+C/X/V, CTRL+arrows for words, …). Custom bindings produce
//! [`Message`]s handled in `app.rs`.
//!
//! # Focus dimming
//!
//! [`DimHighlighter`] paints every line *outside* the active paragraph in the
//! theme's inactive color. Its settings are the active line range, the dim
//! color, and the document generation; whenever any of them change (cursor in
//! a new paragraph, undo/redo swapping the content) the editor re-runs the
//! highlighter over the whole buffer. Dimming is suspended while text is
//! selected and when turned off in the config.

use std::ops::Range;

use iced::advanced::text::highlighter::Format;
use iced::keyboard::key::Named;
use iced::keyboard::Key;
// `text_editor` is both the module (types) and the helper function.
use iced::widget::text_editor;
use iced::widget::text_editor::{Binding, KeyPress, Motion};
use iced::{Background, Border, Color, Element, Fill, Task};

use crate::app::{App, DocId, Message};

fn editor_id(id: DocId) -> iced::widget::Id {
    iced::widget::Id::from(format!("editor-{id}"))
}

/// Refocus a document's editor — used when the command bar closes or focus
/// moves, so the writer can keep typing without reaching for the mouse.
pub fn focus(id: DocId) -> Task<Message> {
    iced::widget::operation::focus(editor_id(id))
}

/// Render the editor for document `id`. Only the focused document dims its
/// inactive paragraphs; other panes show their full text.
pub fn view(app: &App, id: DocId) -> Element<'_, Message> {
    let theme = &app.theme;
    let doc = &app.docs[&id];
    let is_active = id == app.active;
    // Dimming makes a selection that spans paragraphs hard to read, so it is
    // suspended while anything is selected (or turned off in the config), and
    // it only applies to the focused document.
    let selecting = doc.content.cursor().selection.is_some();
    let settings = DimSettings {
        // "Everything is active" — nothing gets dimmed.
        active: if is_active && app.config.focus_dimming && !selecting {
            app.active_paragraph()
        } else {
            (0, usize::MAX)
        },
        dim: theme.text_inactive,
        generation: doc.generation,
    };

    text_editor(&doc.content)
        .id(editor_id(id))
        .on_action(move |action| Message::Edit(id, action))
        .key_binding(key_binding)
        .highlight_with::<DimHighlighter>(settings, |highlight, _theme| Format {
            color: *highlight,
            font: None,
        })
        .font(app.editor_font)
        .size(16)
        .padding(24)
        .height(Fill)
        .style(|theme: &iced::Theme, _status| {
            let palette = theme.extended_palette();
            text_editor::Style {
                // Transparent: the pane card paints the (rounded) background.
                background: Background::Color(Color::TRANSPARENT),
                border: Border::default(),
                placeholder: palette.background.strong.color,
                value: palette.background.base.text,
                selection: Color {
                    a: 0.3,
                    ..palette.primary.base.color
                },
            }
        })
        .into()
}

/// flow-state's editor keymap; `None`-claimed keys use the widget defaults.
fn key_binding(press: KeyPress) -> Option<Binding<Message>> {
    custom_binding(&press).or_else(|| Binding::from_key_press(press))
}

fn custom_binding(press: &KeyPress) -> Option<Binding<Message>> {
    let m = press.modifiers;
    match press.key.as_ref() {
        Key::Character("s") if m.control() => Some(Binding::Custom(Message::Save)),
        Key::Character("z") if m.control() && m.shift() => {
            Some(Binding::Custom(Message::Redo))
        }
        Key::Character("z") if m.control() => Some(Binding::Custom(Message::Undo)),
        Key::Character("y") if m.control() => Some(Binding::Custom(Message::Redo)),
        Key::Character("n") if m.alt() && m.shift() => {
            Some(Binding::Custom(Message::PrevParagraph))
        }
        Key::Character("n") if m.alt() => Some(Binding::Custom(Message::NextParagraph)),
        Key::Character("w") if m.alt() => Some(Binding::Move(Motion::WordRight)),
        Key::Character("b") if m.alt() => Some(Binding::Move(Motion::WordLeft)),
        Key::Named(Named::Backspace) if m.shift() => {
            Some(Binding::Custom(Message::DeleteSentence))
        }
        Key::Named(Named::Backspace) if m.control() => Some(Binding::Sequence(vec![
            Binding::Select(Motion::WordLeft),
            Binding::Backspace,
        ])),
        // ESC is handled by the global `listen_with` subscription (see
        // `app::subscription`), which fires even though the editor would
        // otherwise capture the press for its default Unfocus.
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DimSettings {
    /// Inclusive line range of the active paragraph.
    pub active: (usize, usize),
    pub dim: Color,
    /// [`Document::generation`](crate::app::Document): changes on undo/redo
    /// (whole-content swaps) to force a re-highlight even when the active
    /// range happens to be identical.
    pub generation: usize,
}

pub struct DimHighlighter {
    settings: DimSettings,
    line: usize,
}

impl iced::advanced::text::Highlighter for DimHighlighter {
    type Settings = DimSettings;
    /// `Some(color)` dims the line; `None` keeps the default text color.
    type Highlight = Option<Color>;
    type Iterator<'a> = std::iter::Once<(Range<usize>, Self::Highlight)>;

    fn new(settings: &Self::Settings) -> Self {
        Self {
            settings: settings.clone(),
            line: 0,
        }
    }

    fn update(&mut self, new_settings: &Self::Settings) {
        self.settings = new_settings.clone();
        // Restart from the top: the active paragraph changed.
        self.line = 0;
    }

    fn change_line(&mut self, line: usize) {
        self.line = line;
    }

    fn highlight_line(&mut self, text: &str) -> Self::Iterator<'_> {
        let (start, end) = self.settings.active;
        let line = self.line;
        self.line += 1;
        let highlight = if line >= start && line <= end {
            None
        } else {
            Some(self.settings.dim)
        };
        std::iter::once((0..text.len(), highlight))
    }

    fn current_line(&self) -> usize {
        self.line
    }
}
