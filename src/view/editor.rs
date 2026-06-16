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
// Our vendored, extended fork of iced's `text_editor` (see view::widget).
use crate::view::widget::text_editor::{self, Binding, KeyPress, Motion, TextEditor};
use iced::{Background, Border, Color, Element, Fill, Task};

use crate::app::{App, DocId, Message};
use crate::core::text::{self, Pos};

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
    let pos = doc.content.cursor().position;
    let cursor: Pos = (pos.line, pos.column);

    // Accent emphasis: while CTRL or SHIFT is held in the focused editor, paint
    // the text its BACKSPACE variant would delete — the previous word (CTRL) or
    // the current sentence (SHIFT) — so the writer sees the target first.
    let emphasis = if is_active && !selecting {
        let m = app.modifiers;
        if m.shift() {
            let lines = doc.lines();
            text::sentence_start_before(&lines, cursor).filter(|&s| s != cursor).map(|s| (s, cursor))
        } else if m.control() {
            let lines = doc.lines();
            text::word_before(&lines, cursor)
        } else {
            None
        }
    } else {
        None
    };

    // Phantom: the deleted-sentence ghost sits in the buffer just after the
    // cursor; dim it so it reads as a suggestion to type back or accept.
    let ghost = doc
        .phantom
        .as_ref()
        .map(|rem| (cursor, text::advance(cursor, rem)));

    let settings = DimSettings {
        // "Everything is active" — nothing gets dimmed.
        active: if is_active && app.config.focus_dimming && !selecting {
            app.active_paragraph()
        } else {
            (0, usize::MAX)
        },
        dim: theme.text_inactive,
        accent: theme.accent,
        generation: doc.generation,
        emphasis,
        ghost,
    };

    TextEditor::new(&doc.content)
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
        // Handled in `app.rs`: deletes the previous word, or trims the last
        // word off an active phantom.
        Key::Named(Named::Backspace) if m.control() => {
            Some(Binding::Custom(Message::DeleteWord))
        }
        // TAB accepts an active phantom; with none it inserts a tab (see
        // `app.rs`), so normal tabbing is unaffected.
        Key::Named(Named::Tab) if m.is_empty() => {
            Some(Binding::Custom(Message::PhantomAccept))
        }
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
    pub accent: Color,
    /// [`Document::generation`](crate::app::Document): changes on undo/redo
    /// (whole-content swaps) to force a re-highlight even when the active
    /// range happens to be identical.
    pub generation: usize,
    /// Span to paint in the accent color (the word/sentence a BACKSPACE would
    /// delete), as `(start, end)` `(line, byte_col)` positions.
    pub emphasis: Option<(Pos, Pos)>,
    /// Span of phantom (deleted-sentence) text to dim, same coordinates.
    pub ghost: Option<(Pos, Pos)>,
}

/// Clip a `(start, end)` position span to `line`, returning the covered
/// `[start_col, end_col)` byte range within a line of length `len`.
fn clip(span: (Pos, Pos), line: usize, len: usize) -> Option<(usize, usize)> {
    let ((sl, sc), (el, ec)) = span;
    if line < sl || line > el {
        return None;
    }
    let start = if line == sl { sc } else { 0 }.min(len);
    let end = if line == el { ec } else { len }.min(len);
    (start < end).then_some((start, end))
}

pub struct DimHighlighter {
    settings: DimSettings,
    line: usize,
}

impl iced::advanced::text::Highlighter for DimHighlighter {
    type Settings = DimSettings;
    /// `Some(color)` paints the range; `None` keeps the default text color.
    type Highlight = Option<Color>;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Self::Highlight)>;

    fn new(settings: &Self::Settings) -> Self {
        Self {
            settings: settings.clone(),
            line: 0,
        }
    }

    fn update(&mut self, new_settings: &Self::Settings) {
        self.settings = new_settings.clone();
        // Restart from the top: the active paragraph (or a span) changed.
        self.line = 0;
    }

    fn change_line(&mut self, line: usize) {
        self.line = line;
    }

    fn highlight_line(&mut self, text: &str) -> Self::Iterator<'_> {
        let s = &self.settings;
        let line = self.line;
        self.line += 1;
        let len = text.len();

        // Base color for the whole line: dimmed unless it is in the active
        // paragraph.
        let (active_start, active_end) = s.active;
        let base = (line < active_start || line > active_end).then_some(s.dim);

        // Override spans on this line, highest priority first.
        let emphasis = s.emphasis.and_then(|span| clip(span, line, len));
        let ghost = s.ghost.and_then(|span| clip(span, line, len));

        // No overrides → one segment for the whole line (the common case).
        if emphasis.is_none() && ghost.is_none() {
            return vec![(0..len, base)].into_iter();
        }

        // Split the line at every override boundary, then colour each segment
        // by the first override covering it (emphasis over ghost over base).
        let mut points = vec![0, len];
        for (a, b) in [emphasis, ghost].into_iter().flatten() {
            points.push(a);
            points.push(b);
        }
        points.sort_unstable();
        points.dedup();

        let covers = |span: Option<(usize, usize)>, at: usize| {
            span.is_some_and(|(a, b)| at >= a && at < b)
        };
        points
            .windows(2)
            .filter(|w| w[0] < w[1])
            .map(|w| {
                let color = if covers(emphasis, w[0]) {
                    Some(s.accent)
                } else if covers(ghost, w[0]) {
                    Some(s.dim)
                } else {
                    base
                };
                (w[0]..w[1], color)
            })
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn current_line(&self) -> usize {
        self.line
    }
}
