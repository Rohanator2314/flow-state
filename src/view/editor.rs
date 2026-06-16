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

use iced::advanced::graphics::text::cosmic_text;
use iced::advanced::text::highlighter::Format;
use iced::keyboard::key::Named;
use iced::keyboard::Key;
// Our vendored, extended fork of iced's `text_editor` (see view::widget).
use crate::view::widget::text_editor::{
    self, Binding, DecorationQuad, KeyPress, Motion, TextEditor,
};
use iced::{Background, Border, Color, Element, Fill, Rectangle, Task};

use crate::app::{App, DocId, Message};
use crate::core::text::{self, Pos};
use crate::core::center;
use crate::view::decoration;

/// Thickness of the accent emphasis underline, in pixels.
const UNDERLINE_THICKNESS: f32 = 1.5;

/// Padding (and corner radius) of the active-paragraph glow, in pixels.
const GLOW_PADDING: f32 = 8.0;

/// Alpha of the (static) active-paragraph glow over the accent color.
const GLOW_ALPHA: f32 = 0.10;

/// Most visual lines a single centering animation frame scrolls (the eased
/// step is capped here so big jumps still take a few frames).
const MAX_CENTER_STEP: i32 = 4;

/// One eased scroll step (in visual lines) toward vertically centering the
/// active paragraph `para` (inclusive logical line range) in the viewport.
/// Reads the editor's current layout from its cosmic buffer. Returns `None`
/// when the paragraph is not laid out (off-screen) or already centered — both
/// signals for the caller to stop animating.
pub fn center_step(buffer: &cosmic_text::Buffer, para: (usize, usize)) -> Option<i32> {
    let vh = buffer.size().1?;
    let line_height = buffer.metrics().line_height;
    let (p0, p1) = para;

    // Pixel extent of the paragraph's visible visual lines (viewport-local).
    let mut top = f32::INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    for run in buffer.layout_runs() {
        if run.line_i >= p0 && run.line_i <= p1 {
            top = top.min(run.line_top);
            bottom = bottom.max(run.line_top + run.line_height);
        }
    }
    if !top.is_finite() {
        return None; // paragraph not currently laid out
    }
    let mid = (top + bottom) / 2.0;
    let delta = center::delta_lines(mid - vh / 2.0, line_height);
    let step = center::ease_step(delta, MAX_CENTER_STEP);
    (step != 0).then_some(step)
}

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

    // Accent emphasis: while CTRL or SHIFT is held in the focused editor,
    // underline the text its BACKSPACE variant would delete, so the writer sees
    // the target. With a phantom active, the BACKSPACE variants act on the ghost
    // (which sits just after the cursor): SHIFT → the whole phantom, CTRL → its
    // first word (closest to the cursor). Otherwise: SHIFT → the current
    // sentence, CTRL → the previous word.
    let emphasis = if is_active && !selecting {
        let m = app.modifiers;
        match (&doc.phantom, m.shift(), m.control()) {
            (Some(rem), true, _) => Some((cursor, text::advance(cursor, rem))),
            (Some(rem), _, true) => {
                let word = rem[..text::first_word_end(rem)].trim_end();
                Some((cursor, text::advance(cursor, word)))
            }
            (None, true, _) => {
                let lines = doc.lines();
                text::sentence_start_before(&lines, cursor)
                    .filter(|&s| s != cursor)
                    .map(|s| (s, cursor))
            }
            (None, _, true) => {
                let lines = doc.lines();
                text::word_before(&lines, cursor)
            }
            _ => None,
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
        generation: doc.generation,
        ghost,
    };

    // Decorations are a quad pass (the color highlighter can recolor text but
    // not underline or glow it). Geometry comes from the editor's current
    // layout via the cosmic buffer.
    //   - the emphasis underline (CTRL/SHIFT), over the glyphs;
    //   - the active-paragraph glow (opt-in), behind them.
    let glow = (is_active && app.config.paragraph_glow).then(|| app.active_paragraph());
    let decorations = if emphasis.is_some() || glow.is_some() {
        doc.content.with_buffer(|buffer| {
            let mut quads = Vec::new();
            if let Some((start, end)) = emphasis {
                quads.extend(underline_quads(buffer, start, end, theme.accent));
            }
            if let Some(para) = glow {
                let color = Color { a: GLOW_ALPHA, ..theme.accent };
                quads.extend(glow_quad(buffer, para, color));
            }
            quads
        })
    } else {
        Vec::new()
    };

    TextEditor::new(&doc.content)
        .id(editor_id(id))
        .on_action(move |action| Message::Edit(id, action))
        .key_binding(key_binding)
        .decorations(decorations)
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
    // Only the focused editor acts on a key press. Every open editor pane sees
    // the event (iced's default `from_key_press` already gates on focus), so
    // without this an unfocused pane would also fire these messages — e.g. a
    // second `DeleteSentence` that discards the phantom the focused pane just
    // created, or a double Undo.
    if !matches!(press.status, text_editor::Status::Focused { .. }) {
        return None;
    }
    let m = press.modifiers;
    match press.key.as_ref() {
        // Quality-of-life global binds (CTRL): new/open/find/close/quit and
        // CTRL+TAB to cycle panes. These fire from the focused editor, which is
        // where the writer almost always is.
        Key::Character("n") if m.control() => Some(Binding::Custom(Message::NewFile)),
        Key::Character("o") if m.control() => Some(Binding::Custom(Message::OpenFilePicker)),
        // CTRL+F (toggle find) is handled by a global subscription in `app.rs`,
        // so it works to *close* the bar too — at which point the editor is
        // unfocused and this keymap wouldn't run.
        Key::Character("w") if m.control() => {
            Some(Binding::Custom(Message::CloseActivePane))
        }
        Key::Character("q") if m.control() => {
            Some(Binding::Custom(Message::CloseRequested))
        }
        Key::Named(Named::Tab) if m.control() && m.shift() => {
            Some(Binding::Custom(Message::PrevPane))
        }
        Key::Named(Named::Tab) if m.control() => Some(Binding::Custom(Message::NextPane)),
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

/// Accent underlines for the [`emphasis`](DimSettings) span — one thin quad
/// along the baseline of every visual line the span covers.
fn underline_quads(
    buffer: &cosmic_text::Buffer,
    start: Pos,
    end: Pos,
    color: Color,
) -> Vec<DecorationQuad> {
    decoration::span_rects(buffer, start, end)
        .into_iter()
        .map(|r| DecorationQuad {
            bounds: Rectangle {
                x: r.x,
                y: r.y + r.height - UNDERLINE_THICKNESS - 1.0,
                width: r.width,
                height: UNDERLINE_THICKNESS,
            },
            color,
            radius: 0.0,
            behind: false,
        })
        .collect()
}

/// A soft (static, low-alpha, rounded) glow behind the active paragraph's
/// visible visual lines. `None` when the paragraph is not laid out.
fn glow_quad(buffer: &cosmic_text::Buffer, para: (usize, usize), color: Color) -> Option<DecorationQuad> {
    let (p0, p1) = para;
    let (mut top, mut bottom, mut left, mut right) =
        (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
    for run in buffer.layout_runs() {
        if run.line_i >= p0 && run.line_i <= p1 {
            top = top.min(run.line_top);
            bottom = bottom.max(run.line_top + run.line_height);
            for g in run.glyphs {
                left = left.min(g.x);
                right = right.max(g.x + g.w);
            }
        }
    }
    if !top.is_finite() || !left.is_finite() {
        return None;
    }
    let pad = GLOW_PADDING;
    Some(DecorationQuad {
        bounds: Rectangle {
            x: left - pad,
            y: top - pad,
            width: (right - left) + 2.0 * pad,
            height: (bottom - top) + 2.0 * pad,
        },
        color,
        radius: pad,
        behind: true,
    })
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
    /// Span of phantom (deleted-sentence) text to dim, as `(start, end)`
    /// `(line, byte_col)` positions.
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

        // The only sub-line override is the phantom ghost, dimmed even though it
        // sits inside the active paragraph. (Emphasis is an underline decoration
        // now, not a recolor.)
        let Some((g0, g1)) = s.ghost.and_then(|span| clip(span, line, len)) else {
            return vec![(0..len, base)].into_iter();
        };
        let mut out = Vec::with_capacity(3);
        if g0 > 0 {
            out.push((0..g0, base));
        }
        out.push((g0..g1, Some(s.dim)));
        if g1 < len {
            out.push((g1..len, base));
        }
        out.into_iter()
    }

    fn current_line(&self) -> usize {
        self.line
    }
}
