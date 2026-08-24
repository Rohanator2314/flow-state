//! Stable text-selection value objects shared by selection-driven commands.

use iced::Point;

use crate::app::DocId;
use crate::core::text::{self, Pos};
use crate::document::Document;
use crate::view::widget::text_editor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionFormat {
    Bold,
    Italic,
    Underline,
}

impl SelectionFormat {
    pub fn apply(self, selected: &str) -> String {
        match self {
            Self::Bold => format!("**{selected}**"),
            Self::Italic => format!("*{selected}*"),
            // CommonMark has no underline syntax; inline HTML is interoperable.
            Self::Underline => format!("<u>{selected}</u>"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSelection {
    document: DocId,
    start: Pos,
    end: Pos,
    snapshot: String,
}

impl TextSelection {
    pub fn capture(document: DocId, doc: &Document) -> Option<Self> {
        let snapshot = doc
            .content
            .selection()?
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        if snapshot.is_empty() {
            return None;
        }
        let (start, end) = selection_span(&doc.lines(), doc.content.cursor(), &snapshot)?;
        Some(Self {
            document,
            start,
            end,
            snapshot,
        })
    }

    pub fn document(&self) -> DocId {
        self.document
    }

    pub fn start(&self) -> Pos {
        self.start
    }

    pub fn end(&self) -> Pos {
        self.end
    }

    pub fn text(&self) -> &str {
        &self.snapshot
    }

    pub fn is_current(&self, doc: &Document) -> bool {
        text::slice(&doc.lines(), self.start, self.end) == self.snapshot
    }

    pub fn restore(&self, doc: &mut Document) -> bool {
        if !self.is_current(doc) {
            return false;
        }
        doc.content.move_to(text_editor::Cursor {
            position: text_editor::Position {
                line: self.end.0,
                column: self.end.1,
            },
            selection: Some(text_editor::Position {
                line: self.start.0,
                column: self.start.1,
            }),
        });
        true
    }
}

#[derive(Debug, Clone)]
pub struct SelectionMenu {
    pub target: TextSelection,
    pub position: Point,
}

fn selection_span(
    lines: &[String],
    cursor: text_editor::Cursor,
    selected: &str,
) -> Option<(Pos, Pos)> {
    let selected = selected.replace("\r\n", "\n").replace('\r', "\n");
    let joined = lines.join("\n");
    let absolute = |position: text_editor::Position| {
        lines.iter().take(position.line).map(|line| line.len() + 1).sum::<usize>()
            + position.column
    };
    let anchor = cursor.selection?;
    let low = absolute(cursor.position).min(absolute(anchor));
    let high = absolute(cursor.position).max(absolute(anchor));
    let (start_offset, _) = joined
        .match_indices(&selected)
        .find(|(start, value)| *start <= low && start + value.len() >= high)?;
    let end_offset = start_offset + selected.len();
    let position = |offset: usize| {
        let mut base = 0;
        for (line, value) in lines.iter().enumerate() {
            if offset <= base + value.len() {
                return (line, offset - base);
            }
            base += value.len() + 1;
        }
        let line = lines.len().saturating_sub(1);
        (line, lines.get(line).map_or(0, String::len))
    };
    Some((position(start_offset), position(end_offset)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_formats_wrap_without_changing_the_text() {
        assert_eq!(SelectionFormat::Bold.apply("quiet"), "**quiet**");
        assert_eq!(SelectionFormat::Italic.apply("quiet"), "*quiet*");
        assert_eq!(SelectionFormat::Underline.apply("quiet"), "<u>quiet</u>");
    }

    #[test]
    fn span_uses_the_word_containing_the_click_anchor() {
        let lines = vec!["quiet then quiet".to_string()];
        let cursor = text_editor::Cursor {
            position: text_editor::Position { line: 0, column: 13 },
            selection: Some(text_editor::Position { line: 0, column: 13 }),
        };
        assert_eq!(selection_span(&lines, cursor, "quiet"), Some(((0, 11), (0, 16))));
    }
}
