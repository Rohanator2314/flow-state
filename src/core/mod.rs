//! UI-free application logic: text algorithms, undo history, the LaTeX
//! pipeline, configuration, and theme resolution.
//!
//! Nothing in here may depend on widgets or app state — these modules are
//! pure (or filesystem/subprocess at most) and carry the unit-test suite.
//! The one iced type allowed through is `iced::Color` (in [`theme`]), since
//! colors are what theme resolution produces.

pub mod config;
pub mod fonts;
pub mod latex;
pub mod text;
pub mod theme;
pub mod undo;

use std::path::Path;

/// What kind of preview (if any) a file gets, by extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Plain,
    Markdown,
    Latex,
}

pub fn file_kind(path: Option<&Path>) -> FileKind {
    match path.and_then(|p| p.extension()).and_then(|e| e.to_str()) {
        Some("md") => FileKind::Markdown,
        Some("tex") => FileKind::Latex,
        _ => FileKind::Plain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_file_kind() {
        assert_eq!(file_kind(Some(Path::new("a.md"))), FileKind::Markdown);
        assert_eq!(file_kind(Some(Path::new("a.tex"))), FileKind::Latex);
        assert_eq!(file_kind(Some(Path::new("a.txt"))), FileKind::Plain);
        assert_eq!(file_kind(None), FileKind::Plain);
    }
}
