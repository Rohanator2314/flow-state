//! Document aggregate: editable content and its persistence, history, preview,
//! phantom-text, and spelling state.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use iced::widget::{image, markdown};

use crate::core::spell::SpellIssue;
use crate::core::undo::{History, Snapshot};
use crate::core::{FileKind, file_kind, text};
use crate::view::widget::text_editor;

pub trait DocumentStorage {
    fn read(&self, path: &std::path::Path) -> Result<String, String>;
    fn write(&self, path: &std::path::Path, contents: &str) -> Result<(), String>;
}

pub struct FileSystemStorage;

impl DocumentStorage for FileSystemStorage {
    fn read(&self, path: &std::path::Path) -> Result<String, String> {
        std::fs::read_to_string(path).map_err(|error| error.to_string())
    }

    fn write(&self, path: &std::path::Path, contents: &str) -> Result<(), String> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(path, contents).map_err(|error| error.to_string())
    }
}

/// The open file: its text (owned by iced's editor), undo history, path, and
/// its own preview/compile state (so each document keeps its rendered preview
/// even while another is focused).
pub struct Document {
    pub path: Option<PathBuf>,
    pub content: text_editor::Content,
    pub history: History,
    pub modified: bool,
    /// Bumped whenever `content` is replaced wholesale (undo/redo), so the
    /// dimming highlighter knows its cached lines are stale.
    pub generation: usize,
    /// This document's rendered preview (markdown / PDF pages), shown when it
    /// is the focused document.
    pub preview: Preview,
    /// A LaTeX compile is running for this document.
    pub compiling: bool,
    /// The last compile error for this document, shown as a modal while it is
    /// focused.
    pub compile_error: Option<String>,
    /// A "phantom" of a just-deleted sentence: the deleted text, kept dimmed in
    /// the buffer immediately after the cursor. The writer can type it back
    /// (matching chars fill in, others push it along), TAB to accept it, or
    /// SHIFT/CTRL+BACKSPACE to discard it (whole, or the last word). `None`
    /// when no phantom is active. Stripped from the buffer on save.
    pub phantom: Option<String>,
    /// Derived spelling diagnostics for the current real document text.
    pub spell_issues: Vec<SpellIssue>,
    /// Bumped for every text mutation; gates asynchronous scan/suggestion
    /// results so byte spans from an older buffer are never applied.
    spell_revision: u64,
    /// When this document last became eligible for a debounced scan.
    spell_dirty_since: Option<Instant>,
    /// A scan is currently running for some revision of this document.
    spell_in_flight: bool,
    /// The buffer text as last saved or loaded — the baseline `modified` is
    /// measured against, so reverting edits (or undoing to the saved state)
    /// clears the unsaved marker. See [`Document::refresh_modified`].
    saved_text: String,
}

impl Document {
    pub(crate) fn untitled() -> Self {
        Self {
            path: None,
            content: text_editor::Content::new(),
            history: History::default(),
            modified: false,
            generation: 0,
            preview: Preview::None,
            compiling: false,
            compile_error: None,
            phantom: None,
            spell_issues: Vec::new(),
            spell_revision: 0,
            spell_dirty_since: None,
            spell_in_flight: false,
            saved_text: String::new(),
        }
    }

    pub(crate) fn open(path: PathBuf) -> Self {
        Self::open_with(path, &FileSystemStorage)
    }

    pub(crate) fn open_with(path: PathBuf, storage: &impl DocumentStorage) -> Self {
        let content = match storage.read(&path) {
            Ok(text) => text_editor::Content::with_text(&text),
            Err(_) => text_editor::Content::new(),
        };
        let saved_text = content.text();
        let mut doc = Self {
            path: Some(path),
            content,
            history: History::default(),
            modified: false,
            generation: 0,
            preview: Preview::None,
            compiling: false,
            compile_error: None,
            phantom: None,
            spell_issues: Vec::new(),
            spell_revision: 0,
            spell_dirty_since: None,
            spell_in_flight: false,
            saved_text,
        };
        // `with_text` leaves the cursor — and the viewport — at the end of the
        // text; jump to the very start so the document opens showing its
        // beginning. DocumentStart scrolls the view to the top too, which a
        // bare cursor move would not.
        doc.content.perform(text_editor::Action::Move(
            text_editor::Motion::DocumentStart,
        ));
        doc
    }

    pub fn kind(&self) -> FileKind {
        file_kind(self.path.as_deref())
    }

    /// A fresh, never-edited [untitled] scratch buffer — safe to replace when
    /// opening a file, since it holds nothing the user typed.
    pub(crate) fn is_pristine(&self) -> bool {
        self.path.is_none() && !self.modified && self.content.text().trim().is_empty()
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "[untitled]".to_string())
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        let pos = self.content.cursor().position;
        Snapshot {
            text: self.content.text(),
            cursor: (pos.line, pos.column),
        }
    }

    pub(crate) fn restore(&mut self, snapshot: Snapshot) {
        // The content is rebuilt wholesale, so any phantom's positions are
        // void — drop it (the snapshot text already holds the solid sentence).
        self.phantom = None;
        self.content = text_editor::Content::with_text(&snapshot.text);
        self.move_to(snapshot.cursor);
        self.modified = true;
        self.generation += 1;
    }

    pub(crate) fn move_to(&mut self, (line, column): text::Pos) {
        self.content.move_to(text_editor::Cursor {
            position: text_editor::Position { line, column },
            selection: None,
        });
    }

    /// The cursor as a `(line, byte_col)` position.
    pub(crate) fn cursor_pos(&self) -> text::Pos {
        let p = self.content.cursor().position;
        (p.line, p.column)
    }

    /// Select `[a, b)` and delete it, leaving the cursor at `a`.
    pub(crate) fn delete_span(&mut self, a: text::Pos, b: text::Pos) {
        self.content.move_to(text_editor::Cursor {
            position: text_editor::Position {
                line: a.0,
                column: a.1,
            },
            selection: Some(text_editor::Position {
                line: b.0,
                column: b.1,
            }),
        });
        self.content
            .perform(text_editor::Action::Edit(text_editor::Edit::Backspace));
    }

    /// Replace an exact byte span as one undoable edit.
    pub(crate) fn replace_span(&mut self, start: text::Pos, end: text::Pos, replacement: String) {
        self.history.record(self.snapshot(), false);
        self.content.move_to(text_editor::Cursor {
            position: text_editor::Position {
                line: start.0,
                column: start.1,
            },
            selection: Some(text_editor::Position {
                line: end.0,
                column: end.1,
            }),
        });
        self.content
            .perform(text_editor::Action::Edit(text_editor::Edit::Paste(
                Arc::new(replacement),
            )));
        self.modified = true;
    }

    /// Apply one editor action while maintaining the document-local history
    /// and phantom invariants.
    pub(crate) fn apply_action(&mut self, action: text_editor::Action) {
        if self.phantom.is_some() {
            match &action {
                text_editor::Action::Edit(text_editor::Edit::Insert(c)) => {
                    let c = *c;
                    let rem = self.phantom.as_deref().unwrap_or_default();
                    if rem.starts_with(c) {
                        let rest = rem[c.len_utf8()..].to_string();
                        self.content
                            .perform(text_editor::Action::Move(text_editor::Motion::Right));
                        self.phantom = (!rest.is_empty()).then_some(rest);
                        self.modified = true;
                    } else {
                        self.history.record(self.snapshot(), !c.is_whitespace());
                        self.modified = true;
                        self.content.perform(action);
                    }
                    return;
                }
                text_editor::Action::Edit(text_editor::Edit::Backspace) => {
                    self.history.record(self.snapshot(), false);
                    self.modified = true;
                    self.content.perform(action);
                    return;
                }
                text_editor::Action::Edit(_) | text_editor::Action::Move(_) => {
                    self.history.break_run();
                    self.phantom_discard();
                    self.content.perform(action);
                    return;
                }
                _ => self.phantom_discard(),
            }
        }

        match &action {
            text_editor::Action::Edit(edit) => {
                let coalesce = matches!(
                    edit,
                    text_editor::Edit::Insert(c) if !c.is_whitespace()
                );
                self.history.record(self.snapshot(), coalesce);
                self.modified = true;
            }
            text_editor::Action::Move(_)
            | text_editor::Action::Select(_)
            | text_editor::Action::Click(_)
            | text_editor::Action::Drag(_) => self.history.break_run(),
            _ => {}
        }
        self.content.perform(action);
    }

    /// Discard an active phantom by removing its remaining ghost text from the
    /// buffer — the deleted sentence stays gone.
    pub(crate) fn phantom_discard(&mut self) {
        if let Some(rem) = self.phantom.take() {
            let cur = self.cursor_pos();
            self.delete_span(cur, text::advance(cur, &rem));
            self.modified = true;
        }
    }

    /// Accept an active phantom: keep its text as real content, moving the
    /// cursor to its end.
    pub(crate) fn phantom_accept(&mut self) {
        if let Some(rem) = self.phantom.take() {
            let cur = self.cursor_pos();
            self.move_to(text::advance(cur, &rem));
            self.modified = true;
        }
    }

    /// Trim the first word off an active phantom (CTRL+BACKSPACE) — the word
    /// closest to the cursor, since the ghost sits just after it. Removes that
    /// word (and its trailing space) from the buffer and keeps the tail.
    pub(crate) fn phantom_trim_word(&mut self) {
        let Some(rem) = self.phantom.take() else {
            return;
        };
        let cur = self.cursor_pos();
        let end = text::first_word_end(&rem);
        // The first word sits flush after the cursor; delete it from the front,
        // which leaves the cursor exactly where it was, ahead of the tail.
        self.delete_span(cur, text::advance(cur, &rem[..end]));
        self.modified = true;
        let tail = &rem[end..];
        if !tail.trim().is_empty() {
            self.phantom = Some(tail.to_string());
        }
    }

    /// The buffer's lines as owned strings, for the `core::text` algorithms.
    pub fn lines(&self) -> Vec<String> {
        (0..self.content.line_count())
            .map(|i| {
                self.content
                    .line(i)
                    .map(|l| l.text.to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    pub(crate) fn save(&mut self) -> Result<PathBuf, String> {
        self.save_with(&FileSystemStorage)
    }

    pub(crate) fn save_with(
        &mut self,
        storage: &impl DocumentStorage,
    ) -> Result<PathBuf, String> {
        // A pending phantom is ghost text, not document content — drop it so it
        // never reaches disk.
        self.phantom_discard();
        let path = self
            .path
            .clone()
            .ok_or("no filename — create one in the sidebar")?;
        let text = self.content.text();
        let mut file_text = text.clone();
        if !file_text.ends_with('\n') {
            file_text.push('\n');
        }
        storage.write(&path, &file_text)?;
        // Baseline is the buffer text (without the newline we add only on disk),
        // so a freshly-saved document reads as unmodified.
        self.saved_text = text;
        self.modified = false;
        Ok(path)
    }

    /// Recompute the unsaved-changes flag by comparing the buffer to the last
    /// saved/loaded text — so making an edit and then reverting it (by hand or
    /// via undo back to the saved state) clears the marker. A pending phantom
    /// always counts as modified: it is a deletion the next save will commit.
    pub(crate) fn refresh_modified(&mut self) {
        self.modified = self.phantom.is_some() || self.content.text() != self.saved_text;
    }

    pub(crate) fn invalidate_spelling(&mut self, enabled: bool) {
        self.spell_revision = self.spell_revision.wrapping_add(1);
        self.spell_issues.clear();
        self.spell_dirty_since = (enabled && self.phantom.is_none()).then(Instant::now);
    }

    pub fn spell_issues(&self) -> &[SpellIssue] {
        &self.spell_issues
    }

    pub(crate) fn spell_revision(&self) -> u64 {
        self.spell_revision
    }

    pub(crate) fn spelling_pending(&self) -> bool {
        self.spell_dirty_since.is_some()
    }

    pub(crate) fn clear_spelling(&mut self) {
        self.spell_revision = self.spell_revision.wrapping_add(1);
        self.spell_issues.clear();
        self.spell_dirty_since = None;
        self.spell_in_flight = false;
    }

    pub(crate) fn begin_spell_scan(&mut self, debounce: std::time::Duration) -> Option<u64> {
        if self.spell_dirty_since.is_some_and(|since| since.elapsed() >= debounce)
            && !self.spell_in_flight
        {
            self.spell_dirty_since = None;
            self.spell_in_flight = true;
            Some(self.spell_revision)
        } else {
            None
        }
    }

    pub(crate) fn complete_spell_scan(
        &mut self,
        revision: u64,
        enabled: bool,
        issues: Vec<SpellIssue>,
    ) {
        self.spell_in_flight = false;
        if enabled && self.spell_revision == revision && self.phantom.is_none() {
            self.spell_issues = issues;
        }
    }

    pub(crate) fn ignore_spell_issue(&mut self, issue: &SpellIssue) {
        self.spell_issues.retain(|candidate| candidate != issue);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct MemoryStorage(RefCell<String>);

    impl DocumentStorage for MemoryStorage {
        fn read(&self, _path: &std::path::Path) -> Result<String, String> {
            Ok(self.0.borrow().clone())
        }

        fn write(&self, _path: &std::path::Path, contents: &str) -> Result<(), String> {
            self.0.replace(contents.to_string());
            Ok(())
        }
    }

    #[test]
    fn persistence_is_replaceable_without_touching_document_behavior() {
        let storage = MemoryStorage(RefCell::new("draft".to_string()));
        let path = PathBuf::from("note.md");
        let mut doc = Document::open_with(path, &storage);
        doc.content
            .perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
        doc.content.perform(text_editor::Action::Edit(text_editor::Edit::Insert('!')));
        doc.save_with(&storage).unwrap();
        assert_eq!(&*storage.0.borrow(), "draft!\n");
    }

    #[test]
    fn spelling_correction_is_one_undoable_edit() {
        let mut doc = Document::untitled();
        doc.content = text_editor::Content::with_text("hello wurld");
        doc.replace_span((0, 6), (0, 11), "world".to_string());
        assert_eq!(doc.content.text(), "hello world");

        let current = doc.snapshot();
        let previous = doc.history.undo(current).unwrap();
        doc.restore(previous);
        assert_eq!(doc.content.text(), "hello wurld");
    }

    #[test]
    fn applying_an_edit_records_the_pre_edit_snapshot() {
        let mut doc = Document::untitled();
        doc.apply_action(text_editor::Action::Edit(text_editor::Edit::Insert('a')));
        assert_eq!(doc.content.text(), "a");

        let previous = doc.history.undo(doc.snapshot()).unwrap();
        assert_eq!(previous.text, "");
    }

    #[test]
    fn spelling_invalidation_clears_stale_spans_and_waits_for_phantoms() {
        let mut doc = Document::untitled();
        doc.spell_issues.push(SpellIssue {
            start: (0, 0),
            end: (0, 5),
            word: "wurld".to_string(),
        });
        let revision = doc.spell_revision;
        doc.invalidate_spelling(true);
        assert!(doc.spell_issues.is_empty());
        assert_ne!(doc.spell_revision, revision);
        assert!(doc.spell_dirty_since.is_some());

        doc.phantom = Some("ghost".to_string());
        doc.invalidate_spelling(true);
        assert!(doc.spell_dirty_since.is_none());
    }
}

/// One rasterized PDF page: its image plus aspect ratio (height / width), so
/// the preview can scale every page to the pane width and keep proportions.
#[derive(Debug, Clone)]
pub struct PdfPage {
    pub handle: image::Handle,
    pub aspect: f32,
}

/// The right pane's content.
pub enum Preview {
    None,
    Markdown(markdown::Content),
    /// All pages, stacked top-to-bottom in a scrollable.
    Pdf(Vec<PdfPage>),
}
